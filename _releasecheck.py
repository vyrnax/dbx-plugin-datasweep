#!/usr/bin/env python3
"""发布链路静态闸门。

为什么需要它：`release.yml` 只在**打 tag 时**才执行，所以它的任何错误都要等到真正
发版那一刻才暴露 —— 而那一刻的代价是"白等一轮 CI + 一个已经推出去的废 tag"。
本次实测就抓到三个这种问题：

  1. `on.push.tags` 写成 `"v[0-9]+.[0-9]+.[0-9]+*"`。GitHub 的 tags 过滤器是
     **glob 不是正则**，`[0-9]` 与 `+` 在这里没有正则含义 ⇒ v0.1.0 一个都匹配不上，
     打 tag 后 workflow **静默不触发**，没有任何报错。
  2. reusable workflow 引用 `@plugin-sdk-v1`，而 t8y2/dbx 上**已经没有这个 ref**
     （`git ls-remote --tags` 只有 plugin-cli-v0.1.x）⇒ 整个 run 在解析 `uses:` 时失败。
  3. `.artifact.json` 用 bash glob `artifacts/**/*.artifact.json` 查找；未开 globstar 的
     bash 里它等价于 `artifacts/*/*.artifact.json`（一层子目录），而 download-artifact
     开了 merge-multiple 会把内容**平铺**到根目录 ⇒ 匹配不到 ⇒ 发布缺元数据。

这三类都不是"跑不过"，而是"跑过了但没干活"或"根本没开始跑"，比编译错误更难发现。
本文件把它们变成纯静态检查，分支 CI 就能挡住。

检查项分三组：
  A. release.yml 的触发与引用（1/2/3 的直接回归）
  B. 工具链与版本口径（ci.yml ↔ release.yml ↔ manifest ↔ Cargo.toml ↔ RELEASE_NOTES）
  C. 宿主主题通道（防止又退回"只从 context 读 theme"这个静默失效）
"""

from __future__ import annotations

import ast
import io
import json
import re
import sys
import textwrap
from pathlib import Path

# heredoc 标记：`<<'PY'` / `<<PY`，后面可以跟着 shell 重定向
# （release.yml 实际写的是 `python3 - <<'PY' >> "$GITHUB_STEP_SUMMARY"`，
#   所以这里必须是 [^\n]* 而不是 [^>\n]* —— 后者会卡在 `>>` 的 `>` 上，
#   结果是 0 匹配，而 0 匹配会被误读成"没有 heredoc"。）
HEREDOC_PY = re.compile(r"<<'?PY'?[^\n]*\n(.*?)\n[ ]*PY\b", re.S)

ROOT = Path(__file__).resolve().parent
# 宿主沙箱 CSP 是 img-src data: blob:，且宿主不内联 <img src> ⇒ 相对路径图片必裂图
IMG_SRC_RE = re.compile(r'''(?i)<img\b[^>]*\bsrc\s*=\s*["'](?!(?:data|blob):)''')

RELEASE = ROOT / ".github" / "workflows" / "release.yml"
CI = ROOT / ".github" / "workflows" / "ci.yml"
MANIFEST = ROOT / "manifest.json"
CARGO = ROOT / "backend" / "Cargo.toml"
STORE_META = ROOT / ".dbx-store.json"
NOTES = ROOT / "RELEASE_NOTES.md"
BRIDGE = ROOT / "ui" / "bridge.js"
HTML = ROOT / "ui" / "index.html"


def read(path: Path) -> str:
    return io.open(path, encoding="utf-8").read()


def strip_yaml_comments(text: str) -> str:
    """剥掉 YAML 的整行注释与行尾注释。

    必须剥：release.yml 里有大量「为什么不能用 X 写法」的说明性注释，
    不剥的话检查器会把注释里的反例当成违规本身（本次实测就误报了三次）。
    只剥 `#` 前是空白或行首的，避免误伤 `url#fragment` 这类值。
    """
    out = []
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("#"):
            continue
        out.append(re.sub(r"(?<=\s)#(?!\s*\{).*?$", "", line))
    return "\n".join(out)


def strip_js_comments(text: str) -> str:
    """剥 JS/HTML 注释，判据与 _domcheck.py / _verify.mjs 保持一致。"""
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    text = re.sub(r"<!--.*?-->", "", text, flags=re.S)
    return re.sub(r"(^|[^:])//[^\n]*", r"\1", text)


def main() -> int:
    problems: list[str] = []
    release = strip_yaml_comments(read(RELEASE))
    ci = strip_yaml_comments(read(CI))
    manifest = json.loads(read(MANIFEST))
    version = manifest["version"]

    # ── A. 触发与引用 ────────────────────────────────────────────────
    tags_block = re.search(r"^  push:\n(?:.*\n)*?      - .*?$", release, re.M)
    if not tags_block:
        problems.append("release.yml 里找不到 on.push.tags 段")
    else:
        patterns = re.findall(r'-\s*"([^"]+)"', tags_block.group(0))
        if not patterns:
            problems.append("on.push.tags 没有任何模式")
        for pat in patterns:
            # glob 语法里出现正则专用字符 = 作者以为是正则，实际会当字面量匹配
            if re.search(r"\[|\]|\+|\\d|\(\?", pat):
                problems.append(
                    f"tags 过滤器 {pat!r} 含正则写法：GitHub 这里用的是 **glob**，"
                    "[0-9] 与 + 没有正则含义 ⇒ 真实 tag 匹配不上、workflow 静默不触发"
                )
            # 模式必须能匹配上当前版本的 tag，否则等于没配
            if not _glob_matches(pat, "v" + version):
                problems.append(f"tags 模式 {pat!r} 匹配不到本次要发的 tag v{version}")

    # reusable workflow 的 ref：必须是不可变的 commit sha
    uses = re.findall(r"uses:\s*(t8y2/dbx[^\s]*)", release)
    if not uses:
        problems.append("release.yml 没有引用官方 reusable workflow")
    for ref in uses:
        if "@" not in ref:
            problems.append(f"reusable 引用缺少 @ref：{ref}")
            continue
        path_part, _, rev = ref.rpartition("@")
        if re.fullmatch(r"[0-9a-f]{40}", rev):
            continue  # 完整 sha：不可变，可复现
        if rev == "main":
            problems.append(f"reusable 引用 {path_part}@main 是浮动引用：上游一改行为，同一 tag 重建出的字节就可能变化")
        else:
            problems.append(
                f"reusable 引用 {path_part}@{rev} 不是 commit sha：tag/branch 名可能被上游删掉或移动"
                "（本次实测 @plugin-sdk-v1 就已从 t8y2/dbx 消失，导致整个 run 解析失败）"
            )

    # .artifact.json 的查找方式：merge-multiple 平铺后 bash glob 不匹配。
    # 只查 shell 上下文 —— summary job 里的 python `glob.glob(..., recursive=True)`
    # 是合法的（Python 的 ** 确实递归），不能一并算违规。
    shell_blobs = re.findall(r"run:\s*\|\n((?:[ ]{10,}.*\n?)+)", read(RELEASE))
    for blob in shell_blobs:
        code = "\n".join(l for l in blob.splitlines() if not l.strip().startswith("#"))
        if re.search(r"\S\*\*/\*\.artifact\.json", code):
            problems.append(
                "用 bash glob `dir/**/*.artifact.json` 查找元数据：download-artifact 开了 merge-multiple，"
                "内容被平铺到根目录，而未开 globstar 的 bash 里 `**/` 等价于 `*/` ⇒ 匹配不到 ⇒ 发布缺元数据"
            )
    if "artifact.json" in release and "find " not in release:
        problems.append("release.yml 里有 artifact.json 相关处理但没有 find 命令，检查查找方式")

    # reusable job 必须拿到写权限与 secrets
    reusable_job = re.search(r"^  build:\n(?:.*\n)*?^  [a-z]", release, re.M)
    body = reusable_job.group(0) if reusable_job else ""
    if "secrets: inherit" not in body:
        problems.append("调用 reusable workflow 的 job 缺少 `secrets: inherit`")
    if "contents: write" not in body:
        problems.append("调用 reusable workflow 的 job 缺少 `permissions: contents: write`（内部要 checkout + 上传 Release）")

    # gh 必须显式 --repo：workflow 里 GH_TOKEN 是 GITHUB_TOKEN，
    # 不带 --repo 时 gh 依赖仓库上下文，跨 job / fork 场景容易报 "no repository context"
    for line_no, line in enumerate(release.splitlines(), 1):
        stripped = line.strip()
        if stripped.startswith("gh release ") and "--repo" not in stripped and "$GITHUB_REPOSITORY" not in stripped:
            # 允许同一行续行里带（用 \ 折行的情况）
            tail = release.splitlines()[line_no - 1 : line_no + 2]
            if not any("--repo" in t for t in tail):
                problems.append(f"release.yml:{line_no} gh 命令未带 --repo \"$GITHUB_REPOSITORY\"：{stripped[:60]}")

    # 商店 discover 只认非 draft 非 prerelease，且同 tag 重跑要能转正为 latest
    if "--draft=false" not in release or "--prerelease=false" not in release:
        problems.append("release.yml 必须显式 --draft=false --prerelease=false（商店 discover 只认正式发布）")
    if "--latest" not in release:
        problems.append("已存在 Release 的分支要加 --latest，否则重跑 tag 时 discover 可能扫不到这一版")

    # concurrency 必须按 ref 分组：按 workflow 名分组会让不同 tag 互相取消
    group = re.search(r"^\s*group:\s*(.+?)\s*$", release, re.M)
    if not group or "github.ref" not in group.group(1):
        problems.append("concurrency.group 必须包含 github.ref，否则不同 tag 的发布会互相取消")

    # workflow_dispatch 也必须校验 ref：否则从 main 手动触发会构建与目标 tag 无关的字节
    if 'GITHUB_REF" != "refs/tags/' not in release.replace("$GITHUB_REF", 'GITHUB_REF" != "refs/tags/'):
        if "refs/tags/" not in release:
            problems.append("prepare 缺少 GITHUB_REF 守卫：workflow_dispatch 从别的 ref 触发会构建出与 tag 无关的字节")

    # ── B. 版本与工具链口径 ──────────────────────────────────────────
    cargo_version = re.search(r'^version = "([^"]+)"', read(CARGO), re.M)
    if not cargo_version or cargo_version.group(1) != version:
        problems.append(f"backend/Cargo.toml 的版本 {cargo_version and cargo_version.group(1)} 与 manifest.version {version} 不一致")

    notes_head = read(NOTES).lstrip().splitlines()[0] if read(NOTES).strip() else ""
    if not notes_head.lstrip("# ").startswith(version):
        problems.append(f"RELEASE_NOTES.md 首行 {notes_head!r} 应以版本号 {version} 开头")

    store = json.loads(read(STORE_META))
    first_line = (store.get("releaseNotes") or "").splitlines()[0] if store.get("releaseNotes") else ""
    if not first_line.startswith(version):
        problems.append(f".dbx-store.json 的 releaseNotes 首行 {first_line!r} 应以版本号 {version} 开头")

    toolchains = set(re.findall(r'RUST_TOOLCHAIN:\s*"([^"]+)"', ci))
    toolchains |= set(re.findall(r'rust-toolchain:\s*"([^"]+)"', release))
    if len(toolchains) != 1:
        problems.append(f"ci.yml 与 release.yml 的 Rust 工具链不一致：{sorted(toolchains)}")
    pinned = toolchains.pop() if toolchains else ""
    if pinned and not re.fullmatch(r"\d+\.\d+\.\d+", pinned):
        problems.append(f"Rust 工具链 {pinned!r} 不是精确版本号（写 stable/1.99 这类浮动值会让同一 tag 构建出不同字节）")

    cli_versions = set(re.findall(r'PLUGIN_CLI_VERSION:\s*"([^"]+)"', ci))
    cli_versions |= set(re.findall(r'plugin-cli-version:\s*"([^"]+)"', release))
    if len(cli_versions) != 1:
        problems.append(f"ci.yml 与 release.yml 的 plugin-cli 版本不一致：{sorted(cli_versions)}")

    # go-version 必须显式关掉：留默认 "1.22.x" 会让每个 target 白装一次 Go
    if 'go-version: ""' not in release:
        problems.append('release.yml 未把 go-version 设为 ""（纯 Rust 侧车不需要 Go，默认值会白装一遍工具链）')

    # ── C. 宿主主题通道 ──────────────────────────────────────────────
    bridge = read(BRIDGE)
    html = strip_js_comments(read(HTML))
    if "function onEnv" not in bridge:
        problems.append("bridge.js 缺少 onEnv：主题/语言变化走 dbx-plugin-env，不是 onContext")
    if "dbx-plugin-env" not in bridge:
        problems.append("bridge.js 未监听 dbx-plugin-env 事件")
    if "addEventListener('dbx-plugin-env', callback, true)" not in bridge:
        problems.append("bridge.js 的 dbx-plugin-env 监听必须 capture=true（新版宿主把事件 target 在 document 且不冒泡）")
    if "document.addEventListener('dbx-plugin-env'" not in bridge:
        problems.append("bridge.js 要同时在 document 上注册 dbx-plugin-env（window + document 双注册才兼容新旧宿主）")
    if "function themeObject" not in bridge:
        problems.append("bridge.js 缺少 themeObject：宿主投递的是 {appearance, tokens, editor} 对象，不是字符串")

    # 回归守卫用**不变式**而不是字面量：原先查 `context\.theme`，
    # 把形参改名成 `c` 就绕过了，等于没有守卫。
    #
    # 事实依据（本地 @dbx-app/plugin-cli 0.1.9 的 dev-runtime 源码，逐行核过）：
    #   runtime.mjs:2536  onContext: (fn) => listen("context", fn)   ← 只投 context
    #   runtime.mjs:2561  listeners.context 收到的是 m.context（工作台上下文）
    #   runtime.mjs:2504  applyTheme(v){ root.dataset.dbxTheme=v.appearance; … }
    #   runtime.mjs:2555/2567  只有 init / env 消息带 theme
    # ⇒ context 里**没有 theme 字段**。谁在 index.html 里自己拍 data-theme，
    #   谁就会静默退回 prefers-color-scheme，症状是「DBX 切了深色，插件纹丝不动」。
    #
    # 所以这里要求：data-theme 只能由 panels.js 的 applyAppearance 从 dbxPlugin.theme 写。
    # <html data-theme="light"> 是合理的静态初值（首次 applyAppearance 前避免闪色），
    # 要禁的是 <script> 里自己拍主题 —— 所以只看脚本区。
    scripts = "\n".join(re.findall(r"<script[^>]*>(.*?)</script>", html, re.S))
    if "data-theme" in scripts:
        problems.append(
            "index.html 的脚本里写了 data-theme：主题必须只由 panels.js 的 applyAppearance "
            "从 dbxPlugin.theme 读取（context 里没有 theme 字段，自己拍会静默退回 prefers-color-scheme）"
        )
    # 判据是那句**赋值本身**，不是"文件里有没有 data-theme 这个词" ——
    # applyAppearance 的注释里就写着 data-theme，用词表判断会把删掉的实现也判成通过。
    panels_code = strip_js_comments(read(ROOT / "ui" / "panels.js"))
    if "setAttribute('data-theme'" not in panels_code:
        problems.append("panels.js 不再写 data-theme：applyAppearance 丢了，明暗主题不会跟随宿主")
    if "DS.panels.startEnvWatch()" not in html:
        problems.append("index.html 未启动环境监听（事件 + onInit + 轮询兜底）")
    if "DS.panels.applyAppearance()" not in html:
        problems.append("index.html 未在首屏落一次 data-theme")

    # ── D. 宿主沙箱的资源规则 ────────────────────────────────────────
    # 依据：dev-runtime/runtime.mjs 的 sandboxDocument
    #   CSP = default-src 'none'; img-src data: blob:; …
    #   只内联 <script src> 与 <link rel=stylesheet href>，**不处理 <img src>**
    # ⇒ 相对路径的 <img> 在宿主里必然裂图（真机截图实证）。
    # 扫的是**剥注释后**的文本：注释里"不能写 <img src=...>"这种反例
    # 会被自己的闸门判成违规（与 _domcheck.py / _verify.mjs 同一套判据）。
    # 行号按剥注释后的文本计，够定位即可。
    html_code = strip_js_comments(read(HTML))
    html_raw = read(HTML)
    for line_no, line in enumerate(html_code.splitlines(), 1):
        if IMG_SRC_RE.search(line):
            problems.append(
                f"index.html:{line_no} 用了 <img src>：宿主沙箱 CSP 是 img-src data: blob:，"
                "且宿主不内联 <img> ⇒ 真机上是裂图。图标必须内联 SVG"
            )

    # 内联 SVG 与源文件的一致性（防漂移）：源文件仍是唯一真相
    for inline_id, source in [("brand-icon", ROOT / "ui" / "icons" / "brand.svg"),
                              ("btnApply", ROOT / "ui" / "icons" / "smilingface.svg")]:
        if not source.is_file():
            problems.append(f"图标源文件缺失：{source.name}")
            continue
        src_body = _svg_inner(read(source))
        if inline_id == "brand-icon":
            m = re.search(r'<svg class="brand-icon"[^>]*>(.*?)</svg>', html_raw, re.S)
        else:
            m = re.search(r'<svg viewBox="0 0 1024 1024"[^>]*>(.*?)</svg>', html_raw, re.S)
        # 用原文而非剥注释后的文本：剥注释会顺手改动空白，比对不稳定
        if not m:
            problems.append(f"index.html 里找不到 {inline_id} 的内联 SVG")
            continue
        if _svg_inner("<svg>" + m.group(1) + "</svg>") != src_body:
            problems.append(
                f"{inline_id} 的内联 SVG 与源文件 {source.name} 不一致 —— "
                "改了源文件要同步改内联，否则界面上是旧图、assets/ 里是新图"
            )

    # 同一个图形有三份消费方：manifest/store 用 assets/plugin.svg，
    # UI 内联 SVG 的源文件是 ui/icons/brand.svg。两者只改一个不会报错，
    # 症状是"宿主侧栏图标与插件界面图标不一样"，所以钉死必须逐字节相同。
    assets_icon = ROOT / "assets" / "plugin.svg"
    brand_icon = ROOT / "ui" / "icons" / "brand.svg"
    if assets_icon.is_file() and brand_icon.is_file():
        if assets_icon.read_bytes() != brand_icon.read_bytes():
            problems.append(
                "assets/plugin.svg 与 ui/icons/brand.svg 内容不一致 —— 前者是宿主侧栏/工作台标签/"
                "商店页的图标，后者是 UI 内联 SVG 的源文件，不同步就会出现两处图标长得不一样"
            )

    # ── E. workflow 里内嵌 Python 的编译级检查 ───────────────────────
    # 见文件头注释：这类错误 YAML / bash / 本机三层都查不出来，只在发版那一刻炸。
    for wf, expect_min in [(RELEASE, 1), (CI, 0)]:
        text = read(wf)
        blocks = [m.group(1) for m in HEREDOC_PY.finditer(text)]
        if len(blocks) < expect_min:
            problems.append(
                f"{wf.name} 里没抠到 heredoc Python（找到 {len(blocks)} 块，期望 ≥{expect_min}）"
                " —— 检查器自身失效了，比没有检查更危险"
            )
        for idx, code in enumerate(blocks, 1):
            # YAML 的 `run: |` 会剥离公共缩进，必须 dedent 后再 parse，
            # 否则拿带缩进的原文去编译会误报 unexpected indent
            try:
                ast.parse(textwrap.dedent(code))
            except SyntaxError as exc:
                snippet = textwrap.dedent(code).splitlines()
                where = snippet[exc.lineno - 1][:90] if exc.lineno and exc.lineno <= len(snippet) else "?"
                problems.append(
                    f"{wf.name} 第 {idx} 段 heredoc Python 第 {exc.lineno} 行语法错误："
                    f"{exc.msg}；该行：{where}"
                )

    # ── 输出 ─────────────────────────────────────────────────────────
    if problems:
        print("FAIL 发布链路静态闸门发现 %d 个问题：" % len(problems))
        for item in problems:
            print("  - " + item)
        return 1
    print(f"发布链路静态闸门全部通过（tag v{version}，工具链 {pinned}，主题通道已接对）")
    return 0


def _svg_inner(text: str) -> str:
    """取 <svg> 的内部内容并归一化（去注释、压空行、去每行首尾空白）。

    内联版不带源文件里的注释，所以比对前两边都要剥注释 + 规范化空白，
    否则会因为格式差异误报漂移。
    """
    m = re.search(r"<svg[^>]*>(.*)</svg>", text, re.S)
    body = m.group(1) if m else text
    body = re.sub(r"<!--.*?-->", "", body, flags=re.S)
    lines = [l.strip() for l in body.splitlines() if l.strip()]
    return "\n".join(lines)


def _glob_matches(pattern: str, text: str) -> bool:
    """把 GitHub 的 tags glob 翻译成正则来验一遍。

    GitHub 这里只支持 `*`（任意字符，含 `/` 在 tags 场景下不参与，够用即可）、
    `?`（单字符）与 `[...]` 字符类。我们只关心"真实 tag 能不能匹配上"，
    所以按最宽松的方式实现：`*` → `.*`，`?` → `.`，`[...]` 原样保留。
    """
    out = []
    i = 0
    while i < len(pattern):
        ch = pattern[i]
        if ch == "[":
            j = pattern.find("]", i + 1)
            if j < 0:
                out.append(re.escape(ch))
            else:
                out.append(pattern[i : j + 1])
                i = j + 1
                continue
        elif ch == "*":
            out.append(".*")
        elif ch == "?":
            out.append(".")
        else:
            out.append(re.escape(ch))
        i += 1
    return re.fullmatch("".join(out), text) is not None


if __name__ == "__main__":
    sys.exit(main())
