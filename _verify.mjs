#!/usr/bin/env node
/**
 * 包结构闸门：解包 `.dbxp` 做结构与字节校验。
 *
 * **只做校验，不做打包** —— 打包是官方 `dbx-plugin package` 的事（PLAN 5.6），
 * 这里绝不重复实现打包逻辑。只用 Node 内置模块（自带一个最小 zip 读取器）。
 *
 * 用法：
 *   node _verify.mjs "dist/*.dbxp"
 *
 * 一次性报完所有问题：宿主校验 manifest 时只报第一个错，逐个试错非常费时间（PLAN 风险 8）。
 */

import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, statSync, mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { inflateRawSync } from 'node:zlib';
import { basename, dirname, join, resolve } from 'node:path';

// —— 与 _domcheck.py 必须保持一致的规则 ——
// 判据一致性要求（PLAN 5.6）：两个脚本的"剥注释 + 禁用 API"规则必须完全一样，
// 否则会出现"本地过、CI 红"。
const FORBIDDEN_APIS = ['localStorage', 'sessionStorage', 'indexedDB', 'document.cookie'];

function stripComments(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/(^|[^:])\/\/[^\n]*/g, '$1');
}

const TEXT_EXTENSIONS = ['.json', '.js', '.css', '.html', '.svg', '.md', '.toml'];

// —— manifest schema 字段白名单 ——
const TOP_LEVEL = [
  '$schema', 'manifest_version', 'id', 'name', 'version', 'publisher', 'description',
  'icon', 'source', 'homepage', 'engines', 'permissions', 'entrypoints', 'contributions',
  'localizations'
];
const ENGINES_KEYS = ['dbx', 'host_api'];
const ENTRYPOINT_KEYS = ['ui', 'backend'];
const UI_ENTRY_KEYS = ['root', 'entry'];
const BACKEND_ENTRY_KEYS = ['executable', 'protocol_versions', 'transport'];
const CONTRIBUTION_COMMON = ['type', 'id', 'label', 'description', 'icon'];
const CONTRIBUTION_EXTRA = {
  workbench: [],
  command: ['action'],
  menus: ['items'],
  'connection-provider': ['database_type', 'fields', 'workbench', 'capabilities']
};
const ACTION_KEYS = ['type', 'workbench', 'presentation'];
const LOCALIZATION_KEYS = ['name', 'description', 'contributions'];
const IDENTIFIER = /^[a-z0-9][a-z0-9._-]*$/;

const problems = [];
const notes = [];
const fail = (message) => problems.push(message);
const note = (message) => notes.push(message);

function checkUnknownKeys(allowed, value, path) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    if (value !== null && typeof value !== 'object') {
      fail(`${path} 应是对象，实际是 ${typeof value}`);
    }
    return;
  }
  const allowedSet = new Set(allowed);
  for (const key of Object.keys(value)) {
    if (!allowedSet.has(key)) {
      fail(`${path} 出现未知字段 "${key}"（允许的字段：${allowed.join(', ')}）`);
    }
  }
}

// —— 最小 zip 读取器（只处理 store 方法，够 dbxp 用）——
function readZip(file) {
  const buffer = readFileSync(file);
  let eocd = -1;
  for (let index = buffer.length - 22; index >= 0; index -= 1) {
    if (buffer.readUInt32LE(index) === 0x06054b50) {
      eocd = index;
      break;
    }
  }
  if (eocd < 0) {
    throw new Error('不是有效的 zip：找不到 EOCD 记录');
  }
  const count = buffer.readUInt16LE(eocd + 10);
  let offset = buffer.readUInt32LE(eocd + 16);
  const files = new Map();
  for (let index = 0; index < count; index += 1) {
    if (buffer.readUInt32LE(offset) !== 0x02014b50) {
      throw new Error('zip 中央目录记录损坏');
    }
    const method = buffer.readUInt16LE(offset + 10);
    const compressedSize = buffer.readUInt32LE(offset + 20);
    const nameLength = buffer.readUInt16LE(offset + 28);
    const extraLength = buffer.readUInt16LE(offset + 30);
    const commentLength = buffer.readUInt16LE(offset + 32);
    const localOffset = buffer.readUInt32LE(offset + 42);
    const name = buffer.slice(offset + 46, offset + 46 + nameLength).toString('utf8');
    offset += 46 + nameLength + extraLength + commentLength;

    const localNameLength = buffer.readUInt16LE(localOffset + 26);
    const localExtraLength = buffer.readUInt16LE(localOffset + 28);
    const start = localOffset + 30 + localNameLength + localExtraLength;
    const raw = buffer.slice(start, start + compressedSize);
    files.set(name, method === 0 ? raw : inflateRawSync(raw));
  }
  return files;
}

function expandArg(argument) {
  if (argument && !argument.includes('*')) {
    return [argument];
  }
  const pattern = argument || 'dist/*.dbxp';
  const folder = dirname(pattern) || '.';
  const suffix = basename(pattern).replace('*', '');
  if (!isDirectory(folder)) {
    return [];
  }
  return readdirSync(folder)
    .filter((name) => name.endsWith(suffix))
    .map((name) => join(folder, name));
}

function isDirectory(path) {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
}

/** 侧车自报的 id / version 必须与 manifest 逐字一致（宿主不一致时会静默拒绝后端）。 */
function askIdentity(binary) {
  return new Promise((done) => {
    let child;
    try {
      child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'] });
    } catch {
      done(null);
      return;
    }
    let buffer = '';
    let settled = false;
    const finish = (value) => {
      if (settled) {
        return;
      }
      settled = true;
      // Windows 上正在运行的 exe 会被锁住，必须等进程真正退出再删临时文件
      let exited = false;
      const proceed = () => {
        if (exited) {
          return;
        }
        exited = true;
        done(value);
      };
      child.on('exit', proceed);
      try {
        child.kill();
      } catch {
        proceed();
      }
      setTimeout(proceed, 3000);
    };
    child.stdout.on('data', (chunk) => {
      buffer += chunk.toString('utf8');
      const index = buffer.indexOf('\n');
      if (index < 0) {
        return;
      }
      const line = buffer.slice(0, index).trim();
      try {
        const message = JSON.parse(line);
        finish(message.result && message.result.plugin ? message.result.plugin : null);
      } catch {
        finish(null);
      }
    });
    child.on('error', () => finish(null));
    child.stderr.on('data', () => undefined);
    child.stdin.write(
      `${JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'plugin/initialize', params: { host: { protocolVersions: [1] } } })}\n`
    );
    setTimeout(() => finish(null), 8000);
  });
}

function hostTarget() {
  const platform = { win32: 'windows', darwin: 'darwin', linux: 'linux' }[process.platform];
  const arch = { x64: 'x64', arm64: 'arm64', ia32: 'x86' }[process.arch];
  return platform && arch ? `${platform}-${arch}` : null;
}

async function main() {
  const targets = expandArg(process.argv[2] ?? 'dist/*.dbxp');
  if (targets.length === 0) {
    console.error('没有找到 .dbxp，先跑 dbx-plugin package .');
    process.exit(1);
  }

  // 仓库根目录的 LICENSE：商店审核看的是仓库根目录（AGENTS.md 6.3）。
  // 注意 dbx-plugin.toml 的 include 只有 assets / ui，所以包里本来就不带 LICENSE，
  // 这条按"仓库根目录"来检查。
  const repoLicense = resolve('LICENSE');
  if (!isFile(repoLicense) || readFileSync(repoLicense).length < 1000) {
    fail('仓库根目录缺少 LICENSE 全文（Apache-2.0）');
  }

  // 源 manifest.json：打包器会改写包内那份，所以有两条规则只能在源头上查
  let sourceManifest = null;
  const sourceManifestPath = resolve('manifest.json');
  if (isFile(sourceManifestPath)) {
    sourceManifest = JSON.parse(readFileSync(sourceManifestPath, 'utf8'));
  } else {
    fail('仓库根目录缺少 manifest.json');
  }

  // —— 仓库一致性（AGENTS.md 3.3：版本号必须五处一致，tag 由 CI 的 prepare job 校验）——
  if (sourceManifest) {
    const version = sourceManifest.version;

    const cargoPath = resolve('backend/Cargo.toml');
    if (isFile(cargoPath)) {
      const matched = /^version = "([^"]+)"$/m.exec(readFileSync(cargoPath, 'utf8'));
      if (!matched) {
        fail('backend/Cargo.toml 里找不到 [package] 的 version');
      } else if (matched[1] !== version) {
        fail(`manifest.version(${version}) 与 backend/Cargo.toml(${matched[1]}) 不一致`);
      }
    } else {
      fail('缺少 backend/Cargo.toml');
    }

    const notesPath = resolve('RELEASE_NOTES.md');
    if (!isFile(notesPath)) {
      fail('缺少 RELEASE_NOTES.md');
    } else {
      const firstLine = readFileSync(notesPath, 'utf8').split('\n')[0].trim();
      if (!firstLine.includes(version)) {
        fail(`RELEASE_NOTES.md 首行应含版本号 ${version}，实际是「${firstLine}」`);
      }
    }

    // 商店元数据：name / description / permissions 必须与 manifest 逐字一致（PLAN 事实 14）
    const storePath = resolve('.dbx-store.json');
    if (!isFile(storePath)) {
      fail('缺少 .dbx-store.json');
    } else {
      const store = JSON.parse(readFileSync(storePath, 'utf8'));
      for (const field of ['name', 'description']) {
        if (store[field] !== sourceManifest[field]) {
          fail(`.dbx-store.json 的 ${field} 与 manifest.json 不一致：${JSON.stringify(store[field])} vs ${JSON.stringify(sourceManifest[field])}`);
        }
      }
      if (JSON.stringify(store.permissions) !== JSON.stringify(sourceManifest.permissions)) {
        fail('.dbx-store.json 的 permissions 与 manifest.json 不一致');
      }
      const storeNotes = String(store.releaseNotes || '').split('\n')[0].trim();
      if (!storeNotes.includes(version)) {
        fail(`.dbx-store.json 的 releaseNotes 首行应含版本号 ${version}，实际是「${storeNotes}」`);
      }
    }
  }

  for (const packagePath of targets) {
    console.log(`\n=== ${packagePath} ===`);
    let files;
    try {
      files = readZip(packagePath);
    } catch (error) {
      fail(`${packagePath} 解包失败：${error.message}`);
      continue;
    }
    const names = [...files.keys()].filter((name) => !name.endsWith('/'));

    // 1) sha256 / size 与 .artifact.json 对齐
    const artifactPath = packagePath.replace(/\.dbxp$/, '.artifact.json');
    if (isFile(artifactPath)) {
      const artifact = JSON.parse(readFileSync(artifactPath, 'utf8'));
      const bytes = readFileSync(packagePath);
      const sha256 = createHash('sha256').update(bytes).digest('hex');
      if (artifact.sha256 !== sha256) {
        fail(`sha256 不符：包内实际 ${sha256}，artifact 记录 ${artifact.sha256}`);
      }
      if (artifact.size !== bytes.length) {
        fail(`size 不符：包内实际 ${bytes.length}，artifact 记录 ${artifact.size}`);
      }
      // url 是 Release 里的资产文件名，不是包内路径，所以只比对文件名
      if (artifact.url !== basename(packagePath)) {
        fail(`artifact.url 是 ${artifact.url}，与包名 ${basename(packagePath)} 不一致`);
      }
    } else {
      note(`没有找到 ${artifactPath}，跳过 sha256 / size 校验`);
    }

    // 2) manifest 字段白名单（一次性报完）
    const manifestName = names.includes('manifest.json') ? 'manifest.json' : null;
    if (!manifestName) {
      fail('包内缺少 manifest.json');
      continue;
    }
    const manifest = JSON.parse(files.get(manifestName).toString('utf8'));
    checkUnknownKeys(TOP_LEVEL, manifest, 'manifest');
    checkUnknownKeys(ENGINES_KEYS, manifest.engines, 'manifest.engines');
    checkUnknownKeys(ENTRYPOINT_KEYS, manifest.entrypoints, 'manifest.entrypoints');
    checkUnknownKeys(UI_ENTRY_KEYS, manifest.entrypoints?.ui, 'manifest.entrypoints.ui');
    checkUnknownKeys(BACKEND_ENTRY_KEYS, manifest.entrypoints?.backend, 'manifest.entrypoints.backend');

    if (typeof manifest.id !== 'string' || !IDENTIFIER.test(manifest.id) || manifest.id.length > 128) {
      fail(`manifest.id 不合法：${JSON.stringify(manifest.id)}`);
    }
    if (!Array.isArray(manifest.permissions)) {
      fail('manifest.permissions 必须是数组');
    }

    // 3) 可执行文件
    // 实测：打包器会把**包内**这份 manifest 的 executable 改写成
    // `bin/<target>/<binary>`（补上目标子目录，Windows 再补 .exe），
    // 而**源** manifest.json 必须保持不带 .exe（PLAN 事实 7）。两边口径不同，分开查。
    const executable = manifest.entrypoints?.backend?.executable;
    if (typeof executable !== 'string' || executable.length === 0) {
      fail('manifest.entrypoints.backend.executable 缺失');
    } else if (!names.includes(executable)) {
      fail(`包内找不到 executable 指向的 ${executable}`);
    } else if (!executable.startsWith('bin/')) {
      fail(`executable 应该在 bin/ 下：${executable}`);
    }
    if (sourceManifest) {
      const sourceExecutable = sourceManifest.entrypoints?.backend?.executable;
      if (typeof sourceExecutable !== 'string' || sourceExecutable.length === 0) {
        fail('源 manifest.json 的 entrypoints.backend.executable 缺失');
      } else if (sourceExecutable.endsWith('.exe')) {
        fail(`源 manifest.json 的 executable 不应带 .exe（打包器按目标自己补）：${sourceExecutable}`);
      } else if (basename(executable).replace(/\.exe$/, '') !== basename(sourceExecutable)) {
        // 包内那份会加上目标子目录与平台扩展名，去掉再比
        fail(`包内 executable ${executable} 与源 manifest 的 ${sourceExecutable} 对不上`);
      }
      if (sourceManifest.version !== manifest.version || sourceManifest.id !== manifest.id) {
        fail('源 manifest.json 与包内 manifest.json 的 id / version 不一致');
      }
    }

    // 4) 贡献点
    if (!Array.isArray(manifest.contributions)) {
      fail('manifest.contributions 必须是数组');
    } else {
      for (const [index, contribution] of manifest.contributions.entries()) {
        const path = `manifest.contributions[${index}]`;
        if (!contribution || typeof contribution.type !== 'string') {
          fail(`${path} 缺少 type`);
          continue;
        }
        const extra = CONTRIBUTION_EXTRA[contribution.type];
        if (!extra) {
          fail(`${path} 的 type "${contribution.type}" 不在已知类型里（${Object.keys(CONTRIBUTION_EXTRA).join(', ')}）`);
          continue;
        }
        checkUnknownKeys(CONTRIBUTION_COMMON.concat(extra), contribution, path);
        // 事实 8：menus 贡献点带 label 会被宿主拒
        if (contribution.type === 'menus' && Object.prototype.hasOwnProperty.call(contribution, 'label')) {
          fail(`${path} 是 menus，不能带 label（宿主会报 unknown field 'label'）`);
        }
        if (contribution.action) {
          checkUnknownKeys(ACTION_KEYS, contribution.action, `${path}.action`);
        }
      }
    }

    // 5) localizations
    if (manifest.localizations) {
      for (const locale of Object.keys(manifest.localizations)) {
        checkUnknownKeys(LOCALIZATION_KEYS, manifest.localizations[locale], `manifest.localizations.${locale}`);
      }
    }

    // 6) 文本资源不得含 CRLF（包内字节可重现的前提）
    for (const name of names) {
      if (!TEXT_EXTENSIONS.some((extension) => name.endsWith(extension))) {
        continue;
      }
      const text = files.get(name).toString('utf8');
      if (text.includes('\r\n')) {
        fail(`${name} 含 CRLF 行尾，包内一律要求 LF`);
      }
      if (text.charCodeAt(0) === 0xfeff) {
        fail(`${name} 带 UTF-8 BOM`);
      }
    }

    // 7) 资源文件名全小写（只管 ui/ 与 assets/；根目录的 LICENSE / README 惯例是大写）
    for (const name of names) {
      if (!name.startsWith('ui/') && !name.startsWith('assets/')) {
        continue;
      }
      const segments = name.split('/');
      const last = segments[segments.length - 1];
      if (last !== last.toLowerCase()) {
        fail(`资源名含大写：${name}`);
      }
    }

    // 8) UI 源码不得碰浏览器持久化 API（沙箱 iframe 是 opaque origin，一碰就抛）
    for (const name of names) {
      if (!name.startsWith('ui/')) {
        continue;
      }
      if (!name.endsWith('.js') && !name.endsWith('.html')) {
        continue;
      }
      const stripped = stripComments(files.get(name).toString('utf8'));
      for (const api of FORBIDDEN_APIS) {
        if (stripped.includes(api)) {
          fail(`${name} 使用了 ${api}（沙箱里不可用，持久化必须走侧车）`);
        }
      }
    }

    // 9) 侧车自报身份与 manifest 逐字一致（只在包的目标与本机一致时才能跑）
    const target = hostTarget();
    const exeNames = names.filter((name) => name.startsWith('bin/'));
    const runnable = target
      ? exeNames.find((name) => name.startsWith(`bin/${target}/`) || name.startsWith(`bin/${target}\\`))
      : null;
    if (!runnable) {
      note(`包的目标不是 ${target || process.platform}，跳过侧车身份校验`);
    } else {
      const stage = mkdtempSync(join(tmpdir(), 'datasweep-verify-'));
      const binary = join(stage, basename(runnable) + (process.platform === 'win32' ? '.exe' : ''));
      mkdirSync(dirname(binary), { recursive: true });
      writeFileSync(binary, files.get(runnable));
      const identity = await askIdentity(binary);
      removeQuietly(stage);
      if (!identity) {
        note('侧车没能在 8 秒内应答 plugin/initialize，跳过身份校验');
      } else {
        if (identity.id !== manifest.id) {
          fail(`侧车自报 id "${identity.id}" 与 manifest 的 "${manifest.id}" 不一致`);
        }
        if (identity.version !== manifest.version) {
          fail(`侧车自报 version "${identity.version}" 与 manifest 的 "${manifest.version}" 不一致`);
        }
      }
    }

    console.log(`  包内 ${names.length} 个文件，manifest id=${manifest.id} version=${manifest.version}`);
  }

  for (const line of notes) {
    console.log(`  note ${line}`);
  }
  console.log('');
  if (problems.length > 0) {
    for (const line of problems) {
      console.log(`  FAIL ${line}`);
    }
    console.log(`\n${problems.length} 处问题`);
    process.exit(1);
  }
  console.log('包结构校验全部通过');
}

/** 临时目录清理：Windows 上文件可能还被占着，退让几次再放弃。 */
function removeQuietly(path) {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      rmSync(path, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 });
      return;
    } catch {
      Atomics.wait(new Int32Array(1), 0, 0, 200);
    }
  }
  note(`临时目录没能清掉：${path}`);
}

function isFile(path) {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

await main();
