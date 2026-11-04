#!/usr/bin/env python3
"""前端静态闸门。

检查项（PLAN 5.6）：
  1. JS 里引用的每个 DOM id 都必须在 index.html 里存在；
  2. ui/*.js 不得使用浏览器持久化 API（扫描前**先剥注释**，否则注释里解释
     "为什么不用"会被自己的闸门判违规）；
  3. 所有前端文件必须 UTF-8 无 BOM、LF 行尾。

判据一致性要求：`_verify.mjs` 与本文件的"剥注释 + 禁用 API"规则必须完全一致，
否则会出现"本地过、CI 红"。改一边就必须改另一边。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

UI = Path(__file__).resolve().parent / "ui"

# 与 _verify.mjs 中的 FORBIDDEN_APIS 必须逐字一致
FORBIDDEN_APIS = ("localStorage", "sessionStorage", "indexedDB", "document.cookie")

TEXT_FILES = ("index.html", "styles.css", "i18n.js", "bridge.js", "canvas.js", "game.js", "panels.js", "input.js")

# 与 _verify.mjs 的 stripComments 必须行为一致：
#   - 去掉块注释 /* */ 与 HTML 注释 <!-- -->
#   - 去掉行注释 //（前面不是冒号的，避免误伤 URL 里的 ://）
BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.S)
HTML_COMMENT = re.compile(r"<!--.*?-->", re.S)
LINE_COMMENT = re.compile(r"(^|[^:])//[^\n]*")


def strip_comments(source: str) -> str:
    source = BLOCK_COMMENT.sub("", source)
    source = HTML_COMMENT.sub("", source)
    return LINE_COMMENT.sub(r"\1", source)


def declared_ids(html: str) -> set[str]:
    return set(re.findall(r'\sid="([^"]+)"', html))


def referenced_ids(source: str) -> set[str]:
    ids = set(re.findall(r"""getElementById\(\s*['"]([^'"]+)['"]\s*\)""", source))
    ids |= set(re.findall(r"""querySelector(?:All)?\(\s*['"]#([A-Za-z0-9_-]+)['"]""", source))
    # panels.js 里用 $('id') 简写，一并纳入
    ids |= set(re.findall(r"""\$\(\s*'([A-Za-z0-9_-]+)'\s*\)""", source))
    return ids


def main() -> int:
    problems: list[str] = []

    html_path = UI / "index.html"
    if not html_path.is_file():
        print(f"FAIL 找不到 {html_path}")
        return 1
    html = html_path.read_text(encoding="utf-8")

    declared = declared_ids(html)
    referenced: set[str] = set()
    for name in sorted(p.name for p in UI.iterdir() if p.suffix == ".js"):
        source = (UI / name).read_text(encoding="utf-8")
        referenced |= referenced_ids(source)

    for missing in sorted(referenced - declared):
        problems.append(f"JS 引用了 index.html 里不存在的 id：#{missing}")

    for name in TEXT_FILES:
        path = UI / name
        if not path.is_file():
            problems.append(f"缺少前端文件 {name}")
            continue

        raw = path.read_bytes()
        if raw.startswith(b"\xef\xbb\xbf"):
            problems.append(f"{name} 带 UTF-8 BOM")
        if b"\r\n" in raw:
            problems.append(f"{name} 含 CRLF 行尾（包内一律要求 LF）")

        source = raw.decode("utf-8")
        # 行尾检查对整份文件做，禁用 API 检查只对脚本与页面做
        if name.endswith(".js") or name.endswith(".html"):
            stripped = strip_comments(source)
            for api in FORBIDDEN_APIS:
                if api in stripped:
                    problems.append(f"{name} 使用了 {api}（沙箱 iframe 里不可用，持久化必须走侧车）")

    print(f"id：index.html 声明 {len(declared)} 个，JS 引用 {len(referenced)} 个")

    if problems:
        for problem in problems:
            print(f"FAIL {problem}")
        print(f"\n{len(problems)} 处问题")
        return 1

    print("前端静态闸门全部通过")
    return 0


if __name__ == "__main__":
    sys.exit(main())
