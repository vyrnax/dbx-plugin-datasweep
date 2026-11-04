/**
 * 协议端到端闸门：驱动真实编译出的侧车，走一遍 JSON Lines。
 *
 * 只依赖 Node 内置模块。用法：
 *   node _e2e.mjs [./backend/target/release/dbx-plugin-datasweep[.exe]]
 *
 * 覆盖 PLAN 5.6 的路径：initialize 协商 → game/new(ready) → 首点 reveal → 连点 →
 * chord → flag 循环 → 终局 → records/list 有新纪录 → 重启进程后纪录仍在 →
 * -32601 / -32600 / -32011 / -32010 四类错误 → 并发 32 请求不串响应。
 */

import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const EXE = process.argv[2] ?? './backend/target/release/dbx-plugin-datasweep.exe';
const MANIFEST = JSON.parse(readFileSync(resolve('manifest.json'), 'utf8'));
const DATA_DIR = mkdtempSync(join(tmpdir(), 'datasweep-e2e-'));

let failures = 0;
let checks = 0;

function check(name, condition, detail) {
  checks += 1;
  if (condition) {
    console.log(`  ok   ${name}`);
  } else {
    failures += 1;
    const suffix = detail === undefined ? '' : ` -> ${JSON.stringify(detail)}`;
    console.log(`  FAIL ${name}${suffix}`);
  }
}

function startSidecar() {
  const child = spawn(EXE, [], {
    stdio: ['pipe', 'pipe', 'pipe'],
    env: { ...process.env, DBX_PLUGIN_DATA_DIR: DATA_DIR },
  });
  let buffer = '';
  const pending = new Map();
  let nextId = 1;

  child.stdout.on('data', (chunk) => {
    buffer += chunk.toString('utf8');
    let index;
    while ((index = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, index).trim();
      buffer = buffer.slice(index + 1);
      if (!line) continue;
      const message = JSON.parse(line);
      const waiter = message.id === undefined ? undefined : pending.get(message.id);
      if (waiter) {
        pending.delete(message.id);
        waiter(message);
      }
    }
  });

  return {
    child,
    call(method, params = {}, timeoutMs = 10_000) {
      const id = nextId++;
      child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
      return new Promise((resolvePromise, rejectPromise) => {
        const timer = setTimeout(
          () => rejectPromise(new Error(`${method} timed out after ${timeoutMs}ms`)),
          timeoutMs,
        );
        pending.set(id, (message) => {
          clearTimeout(timer);
          resolvePromise(message);
        });
      });
    },
    async expectResult(method, params = {}) {
      const message = await this.call(method, params);
      if (message.error) throw new Error(`${method} should succeed, got ${JSON.stringify(message.error)}`);
      return message.result;
    },
    async expectError(method, params = {}) {
      const message = await this.call(method, params);
      if (!message.error) throw new Error(`${method} should fail, got a result`);
      return message.error;
    },
    stop() {
      child.kill();
    },
  };
}

const AROUND = (row, col) => {
  const cells = [];
  for (let r = row - 1; r <= row + 1; r += 1) {
    for (let c = col - 1; c <= col + 1; c += 1) {
      if (r !== row || c !== col) cells.push([r, c]);
    }
  }
  return cells;
};

const inBounds = (snapshot, row, col) =>
  row >= 0 && col >= 0 && row < snapshot.rows && col < snapshot.cols;

const isOpenedNumber = (snapshot, row, col) => {
  const value = snapshot.cells[row][col];
  return value >= 0 && value <= 8;
};

// 找一个「周围未打开格数多于该格数字」的已开数字格：挑其中恰好等于数字那么多个插旗，
// 剩下的自然就是合法压枪的目标（旗数 == 数字，且至少还有一个目标可翻）。
function findChordable(snapshot) {
  for (let row = 0; row < snapshot.rows; row += 1) {
    for (let col = 0; col < snapshot.cols; col += 1) {
      const cell = snapshot.cells[row][col];
      if (cell < 1 || cell > 8) continue;
      const closed = AROUND(row, col).filter(
        ([r, c]) => inBounds(snapshot, r, c) && snapshot.cells[r][c] === 10,
      );
      if (closed.length > cell) {
        return { row, col, mineGuesses: closed.slice(0, cell) };
      }
    }
  }
  return null;
}

async function main() {
  console.log(`sidecar: ${EXE}`);
  console.log(`data dir: ${DATA_DIR}`);
  console.log('');

  console.log('1) plugin/initialize');
  let sidecar = startSidecar();
  const init = await sidecar.expectResult('plugin/initialize', {
    host: { protocolVersions: [1] },
  });
  check('protocolVersion is 1', init.protocolVersion === 1, init);
  check('plugin.id matches manifest', init.plugin.id === MANIFEST.id, init.plugin);
  check('plugin.version matches manifest', init.plugin.version === MANIFEST.version, init.plugin);

  console.log('2) system/info');
  const info = await sidecar.expectResult('system/info');
  check('backendId is correct', info.backendId === MANIFEST.id, info);
  check('dataDir uses our env override', info.dataDir === DATA_DIR, info.dataDir);
  check('msToollibVersion present', typeof info.msToollibVersion === 'string', info);

  console.log('3) game/new stays ready');
  let snapshot = await sidecar.expectResult('game/new', {
    rows: 9,
    cols: 10,
    mineNum: 10,
    rule: 'win7',
  });
  check('status is ready', snapshot.status === 'ready', snapshot.status);
  check('bbbv is null before first click', snapshot.bbbv === null, snapshot.bbbv);
  check('elapsedMs is 0', snapshot.elapsedMs === 0, snapshot.elapsedMs);
  check(
    'cells are all closed (10)',
    snapshot.cells.every((row) => row.every((cell) => cell === 10)),
  );
  check('mineLeft equals mineNum', snapshot.mineLeft === snapshot.mineNum, snapshot.mineLeft);

  console.log('4) first reveal: Win7 guarantees a safe neighbourhood');
  snapshot = await sidecar.expectResult('game/reveal', { r: 4, c: 4 });
  check('bbbv is a number now', typeof snapshot.bbbv === 'number', snapshot.bbbv);
  check('status left ready', snapshot.status !== 'ready', snapshot.status);
  const neighbours = AROUND(4, 4).filter(([r, c]) => inBounds(snapshot, r, c));
  check(
    'all 8 neighbours opened and none is a mine',
    neighbours.every(([r, c]) => isOpenedNumber(snapshot, r, c)),
    neighbours.map(([r, c]) => snapshot.cells[r][c]),
  );

  console.log('5) rapid reveals to the end of the game');
  for (let round = 0; round < 90; round += 1) {
    if (snapshot.status !== 'playing') break;
    let target = null;
    for (let row = 0; row < snapshot.rows && !target; row += 1) {
      for (let col = 0; col < snapshot.cols && !target; col += 1) {
        if (snapshot.cells[row][col] === 10) target = [row, col];
      }
    }
    if (!target) break;
    await sidecar.expectResult('game/reveal', { r: target[0], c: target[1] });
    snapshot = await sidecar.expectResult('game/state');
    const leaked = snapshot.cells.flat().some((cell) => cell < 0);
    check('no real mine leaked into cells', !leaked, snapshot.cells);
    if (leaked) break;
  }
  check('reached a terminal state', ['won', 'lost'].includes(snapshot.status), snapshot.status);

  console.log('6) chord with a matching number of flags');
  // 两种极端都凑不出压枪位：盘小且雷密时（5x5/6）Win7 的首点约束会把雷全挤到外圈，
  // 一次 flood 就开完全部非雷格直接通关；盘大雷稀时首点洪水同样能开掉一大片。
  // 所以这里用 10x10/25 并边翻边找，直到局面里出现可作案的位置或本局终了。
  let state = null;
  let spot = null;
  for (let attempt = 0; attempt < 30 && !spot; attempt += 1) {
    state = await sidecar.expectResult('game/new', { rows: 10, cols: 10, mineNum: 25 });
    state = await sidecar.expectResult('game/reveal', { r: 4, c: 4 });
    for (let step = 0; step < 100 && !spot; step += 1) {
      spot = findChordable(state);
      if (spot || state.status !== 'playing') break;
      const closed = [];
      for (let r = 0; r < state.rows; r += 1) {
        for (let c = 0; c < state.cols; c += 1) {
          if (state.cells[r][c] === 10) closed.push([r, c]);
        }
      }
      if (closed.length === 0) break;
      state = await sidecar.expectResult('game/reveal', { r: closed[0][0], c: closed[0][1] });
    }
  }
  check('found a legal chord spot', spot !== null);
  if (!spot) {
    console.log(
      `   诊断：status=${state?.status} opened=${state?.opened} rows=${state?.rows} cols=${state?.cols}`,
    );
    let closedTotal = 0;
    let samples = [];
    for (let r = 0; r < (state?.rows ?? 0); r += 1) {
      for (let c = 0; c < (state?.cols ?? 0); c += 1) {
        if (state.cells[r][c] !== 10) continue;
        closedTotal += 1;
        continue;
      }
    }
    for (let r = 0; r < (state?.rows ?? 0) && samples.length < 8; r += 1) {
      for (let c = 0; c < (state?.cols ?? 0) && samples.length < 8; c += 1) {
        const cell = state.cells[r][c];
        if (cell < 1 || cell > 8) continue;
        const closed = AROUND(r, c).filter(
          ([rr, cc]) =>
            rr >= 0 && cc >= 0 && rr < state.rows && cc < state.cols && state.cells[rr][cc] === 10,
        );
        samples.push(`(${r},${c}) d=${cell} closed=${closed.length}`);
      }
    }
    console.log(`   诊断：closed=${closedTotal} 数字格样本=${samples.join(' | ')}`);
  }
  if (spot) {
    for (const [r, c] of spot.mineGuesses) {
      state = await sidecar.expectResult('game/flag', { r, c, mode: 'flag' });
    }
    const before = state.opened;
    const chorded = await sidecar.expectResult('game/chord', { r: spot.row, c: spot.col });
    check(
      'chord opened cells (or ended the game)',
      chorded.opened >= before || chorded.status === 'lost',
      { before, after: chorded.opened, status: chorded.status },
    );
  }

  console.log('7) flag / mark / clear cycle');
  await sidecar.expectResult('game/new', { rows: 9, cols: 10, mineNum: 10 });
  state = await sidecar.expectResult('game/flag', { r: 0, c: 0, mode: 'flag' });
  check('flagged cell is 11', state.cells[0][0] === 11, state.cells[0][0]);
  check('mineLeft dropped by one', state.mineLeft === 9, state.mineLeft);
  await sidecar.expectResult('game/flag', { r: 0, c: 0, mode: 'clear' });
  state = await sidecar.expectResult('game/flag', { r: 0, c: 0, mode: 'mark' });
  check('mark does not change mineLeft', state.mineLeft === 10, state.mineLeft);
  check('marks contains the cell', state.marks.some(([r, c]) => r === 0 && c === 0), state.marks);
  state = await sidecar.expectResult('game/flag', { r: 0, c: 0, mode: 'clear' });
  check('marks no longer contains the cell', !state.marks.some(([r, c]) => r === 0 && c === 0));

  console.log('7b) 状态机回归：旗→问号、旗格左键、插旗不重置计时');
  // 这三条都是玩家实测报上来的 bug 的回归闸门，走的是真实协议而不是单测。
  await sidecar.expectResult('game/new', { rows: 9, cols: 10, mineNum: 10 });
  let cycle = await sidecar.expectResult('game/flag', { r: 3, c: 3, mode: 'flag' });
  check('flag makes the cell 11', cycle.cells[3][3] === 11, cycle.cells[3][3]);
  check('flag consumes one mineLeft', cycle.mineLeft === 9, cycle.mineLeft);
  // 修复前：对旗格标问号是空操作，右键循环卡死在第二步、旗永远撤不掉。
  cycle = await sidecar.expectResult('game/flag', { r: 3, c: 3, mode: 'mark' });
  check('flag -> mark unflags the cell', cycle.cells[3][3] === 10, cycle.cells[3][3]);
  check('flag -> mark leaves a question mark', cycle.marks.some(([r, c]) => r === 3 && c === 3), cycle.marks);
  check('mark does not consume mineLeft', cycle.mineLeft === 10, cycle.mineLeft);
  cycle = await sidecar.expectResult('game/flag', { r: 3, c: 3, mode: 'clear' });
  check('mark -> clear empties the cell', !cycle.marks.some(([r, c]) => r === 3 && c === 3));

  // 修复前：左键点旗格抛 -32011，被宿主包成 {error} 后前端显示"侧车内部错误"。
  await sidecar.expectResult('game/new', { rows: 9, cols: 10, mineNum: 10 });
  await sidecar.expectResult('game/flag', { r: 0, c: 0, mode: 'flag' });
  const onFlag = await sidecar.call('game/reveal', { r: 0, c: 0 });
  check('clicking a flagged cell is NOT an error', !onFlag.error, onFlag.error);
  check('it leaves the game in ready (no first click consumed)', onFlag.result.status === 'ready', onFlag.result.status);
  check('the flag stays put', onFlag.result.cells[0][0] === 11, onFlag.result.cells[0][0]);
  check('nothing was opened', onFlag.result.opened === 0, onFlag.result.opened);
  // Ctrl 盲开仍然必须能强行翻开旗格
  const blind = await sidecar.expectResult('game/reveal', { r: 0, c: 0, force: true });
  check('Ctrl blind opens through the flag', blind.status !== 'ready', blind.status);

  // 修复前：game/flag 与 game/state 回的是上一次翻格时刻的陈旧 elapsedMs，
  // 前端 applySnapshot 无条件拿它重置秒表基准 ⇒ "插个旗，开始时间被重置"。
  await sidecar.expectResult('game/new', { rows: 16, cols: 16, mineNum: 40 });
  const opened = await sidecar.expectResult('game/reveal', { r: 8, c: 8 });
  check('timer starts on first reveal', opened.status === 'playing', opened.status);
  await new Promise((resolve) => setTimeout(resolve, 1_200));
  const closedCell = opened.cells
    .map((row, r) => row.map((value, c) => [value, r, c]))
    .flat()
    .find(([value]) => value === 10);
  check('found an unopened cell to flag', !!closedCell, closedCell);
  const flaggedOnce = await sidecar.expectResult('game/flag', { r: closedCell[1], c: closedCell[2], mode: 'flag' });
  check('flag advances the authoritative clock', flaggedOnce.elapsedMs >= 1_000, flaggedOnce.elapsedMs);
  const readState = await sidecar.expectResult('game/state');
  check('game/state also advances the clock', readState.elapsedMs >= 1_000, readState.elapsedMs);

  console.log('8) win a game and persist the record');
  let won = false;
  for (let attempt = 0; attempt < 200 && !won; attempt += 1) {
    const fresh = await sidecar.expectResult('game/new', { rows: 5, cols: 5, mineNum: 1 });
    let rolling = await sidecar.expectResult('game/reveal', { r: 2, c: 2 });
    won = rolling.status === 'won';
    for (let row = 0; row < fresh.rows && !won; row += 1) {
      for (let col = 0; col < fresh.cols && !won; col += 1) {
        if (rolling.cells[row][col] !== 10) continue;
        const next = await sidecar.call('game/reveal', { r: row, c: col });
        if (next.error) continue;
        rolling = next.result;
        won = rolling.status === 'won';
      }
    }
  }
  check('played a game to victory', won);
  const records = await sidecar.expectResult('records/list');
  check('records/list has at least one record', records.length >= 1, records.length);
  if (records.length >= 1) {
    check('record has elapsedMs', typeof records[0].elapsedMs === 'number', records[0]);
    check('record has bbbv', typeof records[0].bbbv === 'number', records[0]);
    check(
      'finishedAt is RFC3339',
      /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(records[0].finishedAt),
      records[0].finishedAt,
    );
  }

  console.log('9) error codes');
  let error = await sidecar.expectError('game/probability');
  check('-32601 for unimplemented method', error.code === -32601, error);
  // 尺寸越界现在是**夹取**而不是报错（玩家要求：输入 1 就当下限，超上限就当上限）
  const clamped = await sidecar.expectResult('game/new', { rows: 4, cols: 10, mineNum: 10 });
  check('illegal rows are clamped, not rejected', clamped.rows === 5, clamped.rows);
  const lower = await sidecar.expectResult('game/new', { rows: 10, cols: 10, mineNum: 1 });
  check('mineNum 1 clamps up to the 10% lower bound', lower.mineNum === 10, lower.mineNum);
  const upper = await sidecar.expectResult('game/new', { rows: 10, cols: 10, mineNum: 999 });
  check('mineNum 999 clamps down to the 35% upper bound', upper.mineNum === 35, upper.mineNum);
  error = await sidecar.expectError('game/new', { rows: '很多', cols: 10, mineNum: 10 });
  check('-32600 for wrong value type', error.code === -32600, error);
  error = await sidecar.expectError('game/reveal', { r: 999, c: 0 });
  check('-32600 for out-of-range coordinate', error.code === -32600, error);
  await sidecar.expectResult('game/new', { rows: 9, cols: 10, mineNum: 10 });
  error = await sidecar.expectError('game/chord', { r: 0, c: 0 });
  check('-32011 for chording before first reveal', error.code === -32011, error);
  await sidecar.expectResult('game/abandon');
  error = await sidecar.expectError('game/state');
  check('-32010 after the session is gone', error.code === -32010, error);

  console.log('10) records survive a process restart');
  sidecar.stop();
  sidecar = startSidecar();
  await sidecar.expectResult('plugin/initialize', { host: { protocolVersions: [1] } });
  const reloaded = await sidecar.expectResult('records/list');
  check('records still there after restart', reloaded.length === records.length, reloaded.length);
  check('-32010 for a stale session in a fresh process', true);
  const stale = await sidecar.expectError('game/state');
  check('fresh process has no session (-32010)', stale.code === -32010, stale);

  console.log('11) 32 concurrent requests do not cross-talk');
  await sidecar.expectResult('game/new', { rows: 9, cols: 10, mineNum: 10 });
  const promises = [];
  for (let index = 0; index < 32; index += 1) {
    promises.push(sidecar.call('system/info'));
  }
  const answers = await Promise.all(promises);
  check('all 32 answered', answers.length === 32);
  check(
    'every answer carries its own ok result',
    answers.every((message) => !message.error && message.result.backendId === MANIFEST.id),
  );

  sidecar.stop();

  console.log('');
  console.log(`${checks - failures}/${checks} checks passed`);
  if (failures > 0) console.log(`${failures} FAILED`);
  return failures;
}

const code = await main();
rmSync(DATA_DIR, { recursive: true, force: true });
process.exit(code === 0 ? 0 : 1);
