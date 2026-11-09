# AGENTS.md —— 扫雷 DataSweep 开发规范手册

> 本手册是 `dbx-plugin-datasweep` 的**强制开发规范**：架构、契约、构建、发布、踩坑清单。
> 面向用户的说明（能力、安装、隐私、已知限制）在 [`README.md`](./README.md)，
> 实施蓝图与逐阶段任务在 [`PLAN.md`](./PLAN.md)。三者分工不重叠。
>
> 规范来源：DBX 官方文档 <https://dbxio.com/cn/docs/plugin-development>、`@dbx-app/plugin-cli` 0.1.9 的
> 二进制与 dev-runtime 源码、`t8y2/dbx` 的 manifest schema 与 reusable workflow、`ms_toollib` 1.5.16 crate 源码，
> 以及已上架的参考实现 `dbx-plugin-translation`（Go）。
>
> **本文档里的事实都是实测核实过的，不是设想。** 每条"不要这样做"后面都跟着真实的失败症状。

---

## 0. 铁律

违反下列任意一条，插件会表现为「装上了但什么都不工作 / 静默劣化」，而**宿主不报错**。这类问题极难排查，因此铁律不可协商。

| # | 铁律 | 违反后的真实症状 |
|---|---|---|
| 1 | 侧车自报的 `id` / `version` 必须与 `manifest.json` **逐字相等** | 宿主在 `plugin/initialize` 阶段直接拒绝后端；UI 上只是"存不上、点不动"，无任何错误 |
| 2 | 主题与语言走 **`dbxPlugin.theme` + `dbx-plugin-env`**，不走 `onContext` | `context` 里**没有 theme 字段**，于是永远退到 `prefers-color-scheme`：DBX 切深色，插件纹丝不动 |
| 3 | `dbx-plugin-env` 必须 **window + document 双注册且 `capture: true`** | 新版宿主把事件 target 设在 document 上**且不冒泡**，只挂 window 冒泡阶段的监听器永远收不到 |
| 4 | 图标必须**内联 SVG**，不能 `<img src="…">` | 宿主沙箱 CSP 是 `img-src data: blob:` 且**不内联 `<img>`** ⇒ 真机上全是裂图 |
| 5 | 宿主的业务失败是**正常返回 `{error}`**，不是 reject | 不解包 ⇒ `-32011` / `-32600` 全部显示成「侧车内部错误」，玩家报上来的 bug 单彻底误导 |
| 6 | `ms_toollib` 必须带默认 feature（`rs`） | 洗牌代码在 `#[cfg(any(feature="py",feature="rs"))]` 下，关掉后**静默**退化成"雷全部堆在一起"，首点一次开完整盘 |
| 7 | `laymine` / `laymine_op` 的返回**已含数字**，不可再调 `cal_board_numbers` | 该函数不幂等，再调一次每个数字翻倍 |
| 8 | 源 `manifest.json` 的 `executable` **不带 `.exe`**，也不写 `transport` | 写死 `.exe` 会让非 Windows 包指向不存在的文件 |
| 9 | 前端**不碰** `localStorage` / `sessionStorage` / `indexedDB` / `document.cookie` | 沙箱 iframe 是 opaque origin，一碰就抛 `SecurityError`；且绝不静默退化到 localStorage——那会让用户以为存上了 |
| 10 | 所有源文件 **UTF-8 without BOM + LF** | 包内字节不可重现；中文在 Windows 上乱码 |
| 11 | handler 必须自己包 `catch_unwind` | 官方 SDK 的 worker **没有**包（模板注释是落空的约定），一次 panic 就是进程消失 + 本局凭空作废 |
| 12 | **不要本地构建后手工上传 Release** | 候选文件的 `sha256` 与审核者下载的资产对不上 ⇒ provenance 硬伤（翻译插件 v0.2.1 正是因此被打回） |

---

## 1. 架构

```
┌────────────────────────── DBX 宿主 ──────────────────────────┐
│  ui/  沙箱 iframe（opaque origin，无 localStorage / 无磁盘）  │
│    index.html  bridge.js  game.js  canvas.js  input.js        │
│    panels.js   i18n.js  styles.css                            │
│         │ window.dbxPlugin.invoke(method, params, {timeoutMs})│
│         │ window.dbxPlugin.request(...)  ← 宿主能力            │
│         ▼                                                     │
│  ┌──────────── JSON-RPC 2.0 over stdio（JSON Lines）───────┐  │
│  │ backend/  Rust 侧车                                      │  │
│  │   main.rs      SDK 装配 + manifest 身份反读              │  │
│  │   rpc.rs       方法分发 + 参数校验 + panic 兜住           │  │
│  │   game.rs      会话注册表（真实局面/游戏局面/旗/计时）    │  │
│  │   msbridge.rs  ms_toollib 薄封装 + 局面↔JSON 编码         │  │
│  │   snapshot.rs  快照编码（越界返回 Option 而非 assert!）   │  │
│  │   store.rs     datasweep.json（偏好 + 最佳成绩）          │  │
│  │   paths.rs     数据目录三级回退                            │  │
│  │        ▼ ms_toollib 1.5.x（MIT，必须带 rs feature）       │  │
│  │   laymine / laymine_op 埋雷（返回时已含数字）              │  │
│  │   refresh_board 翻格与 flood-fill    cal_bbbv 3BV          │  │
│  └───────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

**为什么局面在 Rust 侧而不是前端**（按重要性排序）：

1. flood-fill、踩雷后 `14`/`15`/`16` 三种标记、3BV 全都来自 `ms_toollib` 对**游戏局面编码**的既定语义。放前端就得用 JS 再实现一遍，两份实现迟早不一致。
2. 真实雷位永不出侧车，只下发玩家本就该看得见的游戏局面 ⇒ 盲扫、翻格统计、成绩结算在侧车内部闭环，前端改不了成绩。
3. `refresh_board` 一次接受多个坐标，压枪天然是"把周围未开非旗格一次性交给算法"。

代价说清楚：每次点击是一次本地 stdio 往返（子毫秒级，扫雷点击频率远达不到瓶颈），且 `game/*` 返回**全量快照**——最大 50×50 = 2500 格约 7.5 KB，离 SDK 上限很远。

**轻量逻辑放前端、复杂逻辑放侧车**这条常规建议在本项目里不适用：翻格必须走 `refresh_board`，否则两份 flood-fill 语义迟早分叉。

---

## 2. 目录结构

```
dbx-plugin-datasweep/
├── README.md                  # 面向用户：能力、截图、安装、隐私、已知限制
├── AGENTS.md                  # 本文件：开发规范、契约、发布、踩坑清单
├── PLAN.md                    # 实施蓝图（已探明事实、详设、阶段任务、测试计划）
├── RELEASE_NOTES.md           # 【只写当前版本】的变更说明（作为 Release body）
├── LICENSE                    # Apache-2.0 全文（商店首次上架强制要求）
├── manifest.json              # 插件清单（运行时契约）
├── dbx-plugin.toml            # 打包配置：backend.language = "rust"
├── .dbx-store.json            # 商店元数据（name/permissions 必须与 manifest 逐字一致）
├── .gitattributes             # 强制 LF（包内字节可重现的前提）
├── .gitignore                 # 含 _tmp_* ——临时脚本与浏览器 profile 绝不入库
├── .github/workflows/
│   ├── ci.yml                 # fmt + clippy + cargo test + 协议 e2e + 本机打包校验
│   └── release.yml            # 推 tag 触发，复用官方 reusable workflow 真机矩阵构建
├── assets/plugin.svg          # 插件图标（自绘；禁止使用元扫雷的任何素材）
├── docs/screenshots/          # README 用的真实截图
├── ui/                        # 前端，无构建步骤，产物即源码
│   ├── index.html
│   ├── bridge.js              # dbxPlugin 封装：解包、超时归一、theme/env 通道
│   ├── game.js                # 前端状态机 + 串行请求队列
│   ├── canvas.js              # Canvas 渲染（DPR 适配、边长自适应）
│   ├── input.js               # pointer 事件、压枪、拖拽插旗
│   ├── panels.js              # 秒表 / 雷数 / 3BV / 成绩 / 设置 / applyEnv
│   ├── i18n.js                # zh / en（t(key, params) 支持 {name} 占位符）
│   ├── styles.css             # --ds-* 色板 + color-scheme + 宿主 token 兜底引用
│   └── icons/                 # smilingface.svg（笑脸）、cryingface.svg（哭脸，踩雷时显示）
│                              # 必须放 ui/ 内：沙箱 iframe 里引用 ../assets/ 会跨出 ui.root
│                              # 顶部品牌行已移除 ⇒ 没有 brand.svg；插件图标只由宿主侧栏/商店用
├── .cargo/config.toml         # [patch.crates-io] 指向 vendor（理由见 5.1）
├── backend/                   # 独立 Rust crate（自带 [workspace]）
│   ├── Cargo.toml
│   ├── Cargo.lock             # 必须提交：packager 见 lock 即加 --locked；升版本时本包条目要同步（六处之一）
│   ├── .cargo/config.toml     # 同上，覆盖 `cd backend && cargo …` 的用法
│   ├── vendor/dbx-plugin-sdk/ # 上游 Rust SDK 的字节级复制（见 vendor/VENDOR.md）
│   └── src/{main,rpc,game,msbridge,snapshot,store,paths}.rs
├── _e2e.mjs                   # 驱动已编译侧车跑完整协议流程（仅用 Node 内置模块）
├── _verify.mjs                # 包结构闸门：解包核对 manifest/bin/ui、CRLF、大小写
├── _storecandidate.mjs        # 生成 dbx-store 扁平候选文件（schema 自检 + provenance 闸门）
├── _domcheck.py               # 前端静态闸门：DOM id 引用、禁用 API 扫描、编码行尾
├── _releasecheck.py           # 发布链路静态闸门 + 宿主主题/图标通道守卫
└── dist/                      # 构建输出，不提交
```

---

## 3. manifest 与打包契约

### 3.1 `dbx-plugin.toml`

```toml
schema_version = 1

[backend]
language = "rust"
directory = "backend"
binary = "dbx-plugin-datasweep"

[package]
include = ["assets", "ui"]
```

`binary` 是可执行文件名，**不参与**版本一致性校验。

### 3.2 `manifest.json` 的关键约定

```json
{
  "manifest_version": 1,
  "id": "com.vyrnax.datasweep",
  "name": "扫雷",
  "version": "0.1.1",
  "publisher": "vyrnax",
  "icon": "assets/plugin.svg",
  "engines": { "dbx": ">=0.6.21", "host_api": "1" },
  "permissions": [],
  "entrypoints": {
    "ui": { "root": "ui", "entry": "ui/index.html" },
    "backend": { "executable": "bin/dbx-plugin-datasweep" }
  },
  "contributions": [ { "type": "workbench", "...": "..." } ],
  "localizations": { "en": { "name": "DataSweep" } }
}
```

三条会咬人的硬规则见第 0 节铁律 1 / 8。补充：

- `permissions: []` 是**有意的**。游戏不需要宿主能力，少一项权限就少一项审核问答，
  也不把最低宿主版本抬高。
- **主题与语言不依赖 `host.events`**——但这条的证据边界要说清楚：
  dev-runtime 里受 `host.events` 门控的只有**侧车事件转发**（`runtime.mjs:2843`
  `if (manifest.permissions?.includes("host.events")) broadcast("event", …)`），
  而宿主自己的 `init` / `env` 消息是无条件投递的（dev 宿主的主题开关对所有 frame
  `postMessage({type:"env",theme})`）。**真实桌面端是否同样不门控，本仓库尚未验证**
  （本机没装 DBX 桌面端）。
  正因为这个不确定性，`startEnvWatch()` 除了事件监听还留了 **1.5 秒轮询 `dbxPlugin.theme` getter**
  的兜底：即便 `env` 事件被权限拦掉，外观与语言仍然会在一两秒内跟上。
  桌面端验收时请确认这一点（见 PLAN 第十节剩下的第 1 项）。
- `assets/plugin.svg` 是插件图标的**唯一**来源（宿主侧栏、工作台标签、商店页都用它）。
  UI 内部不再引用它：顶部品牌行已移除，计数器条里的两张脸是 `ui/icons/` 下的独立 SVG。
- **表情靠 `data-face` 在两张内联 SVG 之间切显隐**，不是换图片地址（铁律 4）。
  笑脸与哭脸**共用同一个 `viewBox="0 0 1024 1024"`**，所以 `_releasecheck.py` 的一致性
  守卫必须按 class（`face-plain` / `face-cry`）定位 —— 用 viewBox 匹配会永远只比到第一张脸，
  "改了哭脸没改笑脸"就查不出来。CSS 侧同理：显隐规则的特异性必须高于
  `.icon-button svg` 那条尺寸规则，否则隐藏的那张脸会被重新显示。

### 3.3 版本号必须一致的地方

`manifest.json` 的 `version` = `backend/Cargo.toml` 的 `version` = **`backend/Cargo.lock` 里 `name = "dbx-plugin-datasweep"` 条目的 `version`** = tag 去掉 `v` = `.dbx-store.json` 的 `releaseNotes` 首行版本 = `RELEASE_NOTES.md` 首行版本。

> `Cargo.lock` 是最容易漏的第六处：packager 见 lock 就自动 `--locked`，lock 里的本包版本停在旧值时
cargo 拒绝改写 lock，**release 矩阵在打包阶段直接 101**（`cannot update the lock file ... because --locked`）。
而 CI 的裸 `cargo test/build` 不带 `--locked`，只会静默改写工作区副本——**main 全绿照样踩**。v0.1.3 首发即栽在这。
`_verify.mjs` 与 `_releasecheck.py` 已把这条锁成闸门。

`_verify.mjs` 与 `release.yml` 的 prepare job 都会校验；`_releasecheck.py` 在本机也查一遍。

---

## 4. 侧车 RPC 参考

调用方式：`window.dbxPlugin.invoke(method, params, { timeoutMs })`。宿主把 `timeoutMs` 夹到 1–120000（默认 30000），前端超时必须落在这个区间内。

**尺寸与雷数采用「夹取」而不是「拒绝」**：行列夹进 5–50，雷数夹进格数的 10%–35%（下限 `div_ceil` 向上、上限整除向下）。输入 1 自动等于下限，超过 35% 自动等于上限——只有**类型**不对（如 `rows` 传字符串）才返回 `-32600`。区间口径的唯一真相在后端 `game::mine_range`，经 `system/info` 的 `minSide` / `maxSide` / `minePercentMin` / `minePercentMax` 暴露给前端实时显示，避免两处各写一份而漂移。

### 4.1 方法表（v0.1.x）

| 方法 | 入参 | 返回 | 说明 |
|---|---|---|---|
| `system/info` | `{}` | `{ pluginVersion, backendId, targets, dataDir, msToollibVersion, minSide, maxSide, minePercentMin, minePercentMax }` | 自检与排障用 |
| `game/new` | `{ rows, cols, mineNum, rule }` | 快照（`status:"ready"`） | **不埋雷**，见 4.2 |
| `game/reveal` | `{ r, c, force? }` | 快照 | 翻一格；首次调用时才真正生成局面。**旗格上是无反应**（不报错、不开局），`force: true` 用于 `Ctrl` 盲开 |
| `game/chord` | `{ r, c }` | 快照 | 对已开数字格压枪 |
| `game/flag` | `{ r, c, mode }` | 快照 | `mode` = `flag` / `mark` / `clear`，构成右键循环 空→旗→问号→空（`mark` 会先把旗撤下） |
| `game/state` | `{}` | 快照 | UI 重挂载 / 侧车重启后恢复 |
| `game/abandon` | `{}` | `{ ok: true }` | 认输并释放会话，不计成绩 |
| `prefs/get` | `{}` | `{ preset, custom:{rows,cols,mineNum}, rule, useMarks, panel:{records} }` | |
| `prefs/set` | `{ … }` | 最新 prefs | 局部更新；`custom` 可整体给，也可只给其中一个字段。越界会被夹取后落盘 |
| `records/list` | `{}` | `[{ rows, cols, mineNum, rule, elapsedMs, bbbv, finishedAt }]` | 每个维度键 Top 10 |
| `records/clear` | `{ key? }` | `{ removed: n }` | 不传 `key` 清全部 |

预留（0.2.0，当前返回 `-32601`）：`game/solve-next`、`game/probability`、`board/from-array`、`board/export`。

### 4.2 首点埋雷（关键行为）

`game/new` **不生成雷位**，局面处于 `ready`；第一次 `game/reveal` 携带坐标时，侧车才用该坐标调用 `laymine_op(rows, cols, mineNum, x0, y0)`（或 `laymine`），并用 `cal_bbbv` 把 3BV 定死。

这既实现"首点必然安全"，也让 3BV 在开局就固定——`bbbv` 在 `ready` 阶段是 `null`，第一次翻格后变成整数，UI 按可空处理。

> **不要再调 `cal_board_numbers`**（铁律 7）。阶段 1 实测到的症状正是"数字恰好是真实邻雷数的 2 倍"。

`ms_toollib` 的 `get_board_1d` 内部直接做 `vec![0; area - minenum]`（usize），其中
`laymine` 的 `area = rows*cols - 1`，`laymine_op` 的 `area = rows*cols - area_op`（首点 3×3 邻域裁边后：角 4 / 边 6 / 内部 9）。
`minenum` 一旦超过 `area` 就是**usize 下溢**——轻则 panic，重则申请天量内存直接 abort。
`msbridge::lay` 复刻这套 area 计算作为第二道自我防护，不依赖调用方。

### 4.3 快照结构

```json
{
  "gameId": 1,
  "rows": 16, "cols": 16, "mineNum": 40,
  "rule": "win7",
  "status": "ready | playing | won | lost",
  "cells": [[10, 10, 11, "…"], "…"],
  "marks": [[3, 4], [9, 2]],
  "opened": 0, "mineLeft": 40,
  "bbbv": null, "elapsedMs": 0
}
```

游戏局面编码（`backend/src/msbridge.rs` 与前端 `canvas.js` 逐条对应）：

| 码 | 含义 | 码 | 含义 |
|---|---|---|---|
| `0`–`8` | 已开且周围雷数 | `12` | `KNOWN_SAFE` 判定安全 |
| `10` | `OPEN` 未翻开 | `14` | `WRONG_FLAG` 标错的旗 |
| `11` | `FLAG` 旗 | `15` | `HIT_MINE` 踩中的雷 |
| `16` | `OTHER_MINE` 其余雷 | `18` | `CHORD_HIGHLIGHT` 压枪高亮 |

- `cells` 是**游戏局面**，不含真实雷位；`lost` 时由 `refresh_board` 补上 `14`/`15`/`16`
- `marks` 单独列**问号**——ms_toollib 的编码里没有问号这一档，不自造数字码，避免和算法语义打架
- `elapsedMs` 是侧车权威用时；UI 显示的跳动只是本地 tick，终局以本字段为准

### 4.4 错误码

| 码 | 含义 | 触发场景 |
|---|---|---|
| `-32601` | 方法不存在 | SDK 内置；也用于未实现的能力（如 0.2.0 的 `game/probability`） |
| `-32600` | 参数不合法 | 字段**类型**不对、坐标越界。尺寸与雷数**越界不报错**，一律夹取 |
| `-32603` | 内部错误 | 序列化失败；或 handler panic 被我们自己的 `catch_unwind` 兜住（见铁律 11） |
| `-32001` | 超时 | **不是侧车发的**，是 `bridge.js` 给"宿主超时"归出来的本地码：宿主超时 reject 的是**不带 code 的纯 Error**，不专门认一下就兜到 `-32603`，把"等太久"显示成"侧车内部错误" |
| `-32010` | 会话不存在 | 侧车被宿主回收后 UI 仍在操作 |
| `-32011` | 状态不允许 | `won`/`lost` 后继续动作、对未开格压枪、旗数与数字不符、给已开格插旗 |
| `-32013` | 算法失败 | 埋雷入参越界（雷数超出该首点位置可容纳的上限） |
| `-32014` | 存储失败 | `datasweep.json` 读写失败 |

盘上的 prefs 若被手工改坏（非法难度档位 / 规则 / 尺寸），**不会**走到错误分支：`store::normalize()` 在加载时就把它夹回默认值并往 stderr 写一行诊断。否则一个坏字符串会让每次开局都报 `-32603`，而 UI 只显示"侧车内部错误"——那是最难自查的坑。

> 注意 `store.rs` 的消毒语义与 `game.rs` 的夹取语义**故意不同**：
> 输入框里的越界值是"玩家想要但超出范围" ⇒ 夹到边界；
> 磁盘上的坏值是"数据已损坏" ⇒ 整体回退默认。别把它们统一。

---

## 5. 构建与本地开发

### 5.1 为什么仓库里要 vendor 一份 SDK（以及两份 `.cargo/config.toml`）

官方 `dbx-plugin-sdk`（Rust）**没有发布到 crates.io**。`dbx-plugin create --template rust` 生成的那行 `dbx-plugin-sdk = { version = "0.1.0" }` 只有经 `dbx-plugin package` 才解析得通——packager 会追加：

```
--config 'patch.crates-io.dbx-plugin-sdk.path="<SDK_ROOT>/plugins/sdk/rust/dbx-plugin-sdk"'
```

裸 `cargo test` / `cargo clippy` 拿不到这条 patch，直接 `error: no matching package named dbx-plugin-sdk found`。跑不了单测 = 本机与 CI 都无法建立任何验证，这是硬阻塞。处理方式：

- `backend/vendor/dbx-plugin-sdk/`：上游 SDK 的**字节级复制**，禁止修改；校验和与升级方法见 `backend/vendor/VENDOR.md`
- `.cargo/config.toml`（仓库根）**与** `backend/.cargo/config.toml`：两份都必须存在。cargo 的配置按**当前工作目录**向上查找，与 `--manifest-path` 指向哪里无关——只放一份的话，`cd backend && cargo test` 与 `cargo test --manifest-path backend/Cargo.toml` 必有一种解析不到
- **打包路径不受影响**：命令行 `--config` 优先级高于配置文件（已实测），`.dbxp` 里的侧车仍然链接**上游那份** SDK 而不是 vendor。已验证 `dbx-plugin package` **不会重写 `Cargo.lock`**，所以 CI 的 `--locked` 安全

### 5.2 前置

| 工具 | 要求 | 说明 |
|---|---|---|
| Rust | `stable-msvc`（rustup） | 本机与 CI/release 都是 **1.99.1**。`ci.yml` 的 `RUST_TOOLCHAIN` 与 `release.yml` 的 `rust-toolchain` 必须写这**同一个精确版本**——浮动值会让同一 tag 重建出不同字节 |
| MSVC 工具集 | `link.exe` + Windows SDK | Rust 能自行定位，无需 Developer Prompt |
| Node.js | 22+（`dbx-plugin dev` 要求） | CI 用 22 |
| `@dbx-app/plugin-cli` | **0.1.9** | 含 Rust SDK 与 Go SDK 模板；`plugin-cli-version` 要与本地一致，别留 reusable workflow 的默认值 |
| Python | 3.x | 仅跑两个静态闸门 |

依赖树：`cargo tree -e normal` 去重 **117 个 package**，release 构建约 5 分钟（`ms_toollib` 的 `rs` feature 会把 `tract-onnx` 一族带进来，代价已接受，见铁律 6）。

### 5.3 日常命令

```bash
cargo fmt --check --manifest-path backend/Cargo.toml
cargo clippy --manifest-path backend/Cargo.toml --all-targets -- -D warnings
cargo test   --manifest-path backend/Cargo.toml
cargo build  --release --manifest-path backend/Cargo.toml

node _e2e.mjs ./backend/target/release/dbx-plugin-datasweep.exe
python _domcheck.py
python _releasecheck.py

dbx-plugin package .                                  # 本机只能出 windows-x64
node _verify.mjs "dist/*.dbxp"
node _storecandidate.mjs --allow-unverified           # 本地自测，产物不可提交商店

dbx-plugin dev --path . --port 5190                   # 开发宿主，打开 http://127.0.0.1:5190
```

`dbx-plugin package` 内部执行 `cargo build --release --manifest-path backend/Cargo.toml`，发现 `Cargo.lock` 时自动追加 `--locked`。

**> SDK 依赖写法不要手改**（见 5.1）。手写 `dbx-plugin-sdk = "0.1"` 或改成 git 依赖都会破坏打包。

### 5.4 关于跨平台（与 Go 经验不同，极易误判成 bug）

翻译插件用 Go，`CGO_ENABLED=0` 之后**在 Windows 上能一次交叉编译出全部平台的合法包**。

**Rust 不行，官方 packager 也不行**：`dbx-plugin package` 明确"为当前宿主平台构建 sidecar"，并拒绝为非本机 target 打包：

```
Native plugin target 'X' does not match build host 'Y';
run this package command on the target platform
```

所以本地开发期只会得到 `windows-x64.dbxp`，**这是预期行为，不是 bug**。其余 4 个平台由 CI 在对应平台 runner 上真机产出——官方 reusable workflow 的默认矩阵恰好就是这 5 个 target，不需要自定义 `build-matrix`。

---

## 6. 发布与上架

### 6.1 流程（推 tag 即全自动）

```bash
# 1) 改版本号，六处一致（见 3.3，Cargo.lock 最易漏）
#    RELEASE_NOTES.md 整体重写为「本版增量」，不要在旧内容后面追加
# 2) 本地自检
cargo test --manifest-path backend/Cargo.toml
node _e2e.mjs ./backend/target/release/dbx-plugin-datasweep.exe
dbx-plugin package . && node _verify.mjs "dist/*.dbxp"
python _releasecheck.py
# 3) 提交并推 tag（tag 必须是 v + manifest.version）
# tag 名现场从 manifest 取，绝不手写死版本号（升版后复制粘贴就会打错 tag）
V=$(node -p "require('./manifest.json').version")
git commit -am "release: v$V" && git tag -a "v$V" -m "v$V" && git push origin main && git push origin "v$V"
```

`release.yml` 由 tag 触发：先校验 tag 与 `manifest.version`、`Cargo.toml` 一致并创建/转正 Release（**非 draft、非 prerelease、`--latest`**——商店的 discover 脚本只认这个，prerelease 会被直接跳过），再调用官方 reusable workflow 按平台矩阵构建，最后把 5 个 `.dbxp` + 5 个 `.artifact.json` + `release-candidates.json` 上传到该 Release，并由 summary job 产出 dbx-store 扁平候选文件。

reusable workflow 的传参（Rust 插件专用）。**ref 必须 pin 到 commit sha**：

```yaml
uses: t8y2/dbx/.github/workflows/plugin-release-reusable.yml@374d60573d3a6ae2bbc3035d4d71e63f4fe546b1
permissions:
  contents: write        # reusable 内部要 checkout 本仓库并 gh release upload
with:
  release-tag: ${{ needs.prepare.outputs.tag }}
  package-command: dbx-plugin package .
  package-path: dist/*.dbxp
  metadata-path: dist/*.artifact.json
  plugin-cli-version: "0.1.9"      # 与本地一致，别留默认 0.1.2/0.1.10
  rust-toolchain: "1.99.1"         # 必须与 ci.yml 的 RUST_TOOLCHAIN 同一个字符串
  go-version: ""                   # 关掉 Go 安装，否则每个 target 白装一遍工具链
  install-plugin-cli: true
```

> `@plugin-sdk-v1` **不可用**：该 tag 已从 `t8y2/dbx` 上消失（`git ls-remote --tags` 只剩 `plugin-cli-v0.1.x`，
> API 查 `refs/tags/plugin-sdk-v1` 返回 404），用它会让整个 run 在解析 `uses:` 时直接失败。
> `@main` 这类浮动引用更不行——上游一改行为，同一 tag 重建出的字节就可能变化，审核者无法复现哈希。
> 要升级就显式换 sha 并在提交信息里说明。

`on.push.tags` 也必须写对：GitHub 这里是 **glob 不是正则**，`"v[0-9]+.[0-9]+.[0-9]+*"` 等于要求 tag 字面长成那样 ⇒ v0.1.0 一个都匹配不上，**打 tag 后 workflow 静默不触发且无报错**。用 `"v*.*.*"`，形态校验交给 prepare 里的白名单正则。

### 6.2 Rust 的 provenance 自证方式

Go 侧车能把 `vcs.revision` / `vcs.modified` 内嵌进二进制，审核者 `go version -m` 一眼对上；**Rust 二进制没有这个内建戳**。因此可复现性靠这四件事自证，release job 的 summary 会把它们全部打印出来：

1. CI 从 tag 检出源码构建（本地手传产物会被商店判 provenance 硬伤——翻译插件 v0.2.1 就是这么被打回的）
2. `Cargo.lock` 提交进仓库，packager 自动 `--locked`
3. `rust-toolchain` pin 到**精确版本号**（写 `stable` 会随 runner 镜像漂移）
4. 每个 `.artifact.json` 的 `target` / `size` / `sha256` 汇总（reusable workflow 本来就会逐包校验 sha256 与 `.dbxp` 字节一致）

### 6.3 提交商店

候选 PR 提到 **`t8y2/dbx-store:main`**（不是 `t8y2/dbx`）。审核通过后由商店签名，用户实际安装的是商店目录里那份签名包，而不是本仓库的 `.dbxp`。

**首次上架走手工 PR**：fork `t8y2/dbx-store`，提一个 PR。要交几个文件取决于 `publishers/vyrnax.json` 在不在——**本仓库不需要再交它**：

| 文件 | 本仓库要不要交 |
|---|---|
| `publishers/vyrnax.json` | **不用**。它已随「Translation 即时翻译」上架时提交进商店（提 PR 前用商店仓库的 API 或搜索再确认一次）。重复提交会被判成 publisher 已存在 |
| `candidates/com.vyrnax.datasweep.json` | **要**，且内容必须取自 CI 的 **`dbx-store-candidate`** artifact（`release.yml` 的 summary job 产出），本地生成的那份不可提交 |

> 文件名**必须**等于 `<plugin-id>.json`，商店的 `validate.mjs` 会核对文件名与 `id` 一致。
> `plugins/*.json` 与 `catalog/index.json` **永不手工编辑**，由签名 Workflow 生成。
> 提交后 PR 的 CI 会一直红（`open candidate(s) awaiting DBX Store signing`），这是设计如此：
> 维护者审核 → 跑受保护的签名 Workflow → 回写目录 → CI 变绿 → 合并。

**后续版本**可走自动同步：仓库在 `t8y2/dbx-store` 的 `automation/plugin-sources.json` 注册且 `autoUpdate: true` 时，商店的同步 Workflow 会定期扫公开 Release，自动创建/更新同一个候选 PR。

两条路径吃的**格式不同**，别弄混：

| 文件 | schema | 用途 |
|---|---|---|
| `release-candidates.json`（随 Release 发布，由官方 reusable workflow 汇总） | `{plugin, artifacts}` | **自动同步**路径消费 |
| `dist/store-candidate/<id>.json`（由 `_storecandidate.mjs` 生成） | 扁平 `schemaVersion`/`targets[]` | **手工提 PR** 时用 |

把前者当后者提交，`validate.mjs` 会直接报 `unknown field(s): plugin, artifacts`。

`_storecandidate.mjs` 内置两道保险：

1. **provenance 闸门**（默认强制）：CI 里核对 `GITHUB_REF` 就是 `refs/tags/v<version>`、`GITHUB_SHA` 等于 HEAD、工作树干净；本地则核对工作树干净、tag 存在且指向 HEAD、HEAD 已推送。任一不满足即 `[FATAL]` 退出。`--allow-unverified` 仅供本地自测，产物**不可提交商店**。
2. **schema 自检**：把 `validate.mjs` 的静态规则（键白名单、semver / `sha256` / `target` 正则、icon 只能是 SVG/PNG、`targets[].url` 必须 HTTPS 且不能指向 dbx-store 的 release）在本地先跑一遍。

审核必答口径（本插件的披露内容，**README「数据与隐私」节是唯一来源**，商店表单照抄）：不联网、无权限声明、只写 `datasweep.json`、明文无凭据、卸载不自动删除、无自动点击 / 机器求解、5 个 target 均为未签名候选（darwin 未做 notarization）。
---

## 7. 前端约束与手感要点

- **串行请求队列**：`game/*` 同一时刻只允许一个在途请求，后续动作排 FIFO。否则极速连点会收到乱序快照，画面出现"回退"。队列的作用不是限流（IPC 是毫秒级），而是保证快照按动作顺序到达。
- **压枪判定用 `e.buttons === 3`**，不要自己维护"左右键都按下了"的状态位，也不要用 `dblclick`——扫雷的高频连点下 `dblclick` 会误判和丢事件。
- **`contextmenu` 必须 `preventDefault()`**，否则宿主弹浏览器原生右键菜单。
- **左键在 `pointerup` 才生效**，允许拖出取消（与经典扫雷一致）；按住右键拖动批量插旗时，整段动作由第一格决定，每格只在"跨进新格"时切一次，绝不重复切换同一格。
- **旗格上的左键 = 无反应**（`Ok(())`），不是抛错。旗的语义是"我认为这里是雷"，玩家紧接着左键点它本就是矛盾操作；弹一条 `-32011` 远不如静默忽略。真要强行翻开用 `force`。
- **`flag(mark)` 作用在旗格上要先撤旗**，否则右键循环卡死在同一格。
- **`elapsedMs` 每次出快照前必须 `tick()` 重算**：它原先只在 `settle()` 里更新，于是 `game/flag` / `game/state` 返回的是陈旧值，症状就是"插个旗，开始时间被重置"。终局后 `tick()` 冻结。
- **Canvas 按 `devicePixelRatio` 缩放**；格子边长由容器尺寸自适应，下限 12px、上限 96px，撑不下时容器滚动而不是无限缩小。只有变化过的格子重绘（flood-fill 一次开一片，脏区重绘收益明显）。
- **`relayout` 必须用 `clientWidth` / `clientHeight`**（不含 border 与滚动条）。用 `getBoundingClientRect()` 会把 1px 边框算进可用宽度 ⇒ 算出的 canvas 比内容区宽 2px ⇒ 出滚动条 ⇒ 可用宽度又变小 ⇒ ResizeObserver 再触发……尺寸在两个值之间来回跳。
- **棋盘用 `margin: auto` 居中**，不要用 `align/justify-items: center`——后者在内容溢出时会把左上边裁到滚动区外（滚不回去），大尺寸棋盘就废了。
- **LED 计数器在 grid 里要显式 `justify-self` + `width: max-content`**：`.counter-bar` 用 `1fr auto 1fr` 保证笑脸真正居中，而 grid item 默认 `stretch` 会把 LED 拉成半条计数器那么宽的黑带（实测 1440 视口下 w=531，`min-width` 形同虚设）。
- **面板标题行要让标题不收缩**（`flex: 0 0 auto`）：侧栏只有 232px，而维度串很长，默认 flex 会把「最佳成绩」挤成「最佳成/绩」两行。
- **restart 用合并语义**：在途期间来的新请求只打一个 pending 标记，本轮结束后用**最新表单值**再开一轮。原先"在途就直接丢弃"在只有笑脸按钮触发时无所谓，加了"改行列/雷数自动换盘"后，玩家连改两项会静默丢掉第二次，棋盘停在旧尺寸（成绩键也跟着错）。
- **自定义改动即时换盘用 `change` 而不是 `input`**：number 控件的 `change` 在失焦或点上下箭头时才触发，输入"50"的过程中不会刚打到"5"就换盘。
- **尺寸输入框的回写取后端 snapshot**（`syncSizeInputs`）：前端也夹一次只是为了默认值好看，后端才是唯一真相；正在被打字的输入框要跳过。
- **键盘只在工作台获焦时生效**。DBX 没有 keybinding 贡献点，宿主全局快捷键抢不走也不需要抢。

### 7.1 跟随宿主的矩阵与验证边界

本插件**不提供**任何主题或语言的手动设置项，一律实时跟随宿主。

| 跟随项 | 宿主投递形式 | 本插件的消费方式 | 开发宿主可验？ |
|---|---|---|---|
| 浅色 / 深色 | `theme.appearance` | `panels.js` 的 `applyAppearance()` 设 `[data-theme]`，CSS 里两套完整 `--ds-*` 色板 | **可以**（已实测） |
| 调色板 | `--color-*` tokens | 仅 `body` 的底色与文字色走 `var(--color-background/foreground, 兜底)`；棋盘内部不盖 | **可以** |
| 圆角 | `--radius-*` tokens | `--ds-radius: var(--radius-lg, 8px)`、`--ds-radius-sm: var(--radius-md, 6px)` | **可以** |
| UI 字体 | `--font-sans` token | `--ds-font-ui: var(--font-sans, var(--font-family, 兜底栈))` | **不行**，见下 |
| 等宽字体 | `--font-mono` + `theme.editor.fontFamily` | `--ds-font-mono`，Canvas 的数字字体也从这里读 | **不行**，见下 |
| 界面语言 | `dbxPlugin.locale` | `i18n.js` 归一化，只支持 zh / en，其余退英文 | 可以（调试页切语言） |

**开发宿主的局限**：`dbx-plugin dev` 只投递 **15 个 token**（实测 13 个 `--color-*` + 2 个 `--radius-*`），**不含 `--font-*`**。所以字体跟随**只能在装了 DBX 桌面端的机器上验证**，开发宿主里永远走兜底字体栈——在开发宿主里看到"字体没变"**不是 bug**，别去改代码。

**配色为什么不能逐条盖**：本项目的 `--ds-*` 是经典扫雷色板（凸起灰面 + 1..8 数字色），必须**整套成对切换**、只由 `data-theme` 驱动。只盖一半会得到"浅底浅字"这种没法玩的颜色。`body` 的底色/文字色是例外，它们不涉及棋盘内部对比度。

**为什么还要一层轮询兜底**：事件通道没有版本保证，而"主题不跟随"的失败方式是**静默**的——界面看着完全正常，用户只会以为插件不支持深色。`startEnvWatch()` 同时挂 `dbx-plugin-env`（window + document + capture）、`onInit`，再加 1.5 秒的「locale + appearance + token 数量」指纹轮询，三路都收敛到同一个幂等的 `applyEnv()`。

---

## 8. 闸门脚本

| 脚本 | 查什么 | 为什么必须有 |
|---|---|---|
| `_e2e.mjs` | 驱动**真实编译产物**跑完整协议：初始化、三档预设、翻格/插旗/压枪、越界夹取、`-32601`/`-32600`/`-32011`/`-32010`、重启后成绩仍在、32 并发不串响应 | 单测只证明函数逻辑；协议层与 stdio 编解码只有真进程能验 |
| `_verify.mjs` | 解包核对 manifest/bin/ui 路径、`executable` 两端口径、版本号六处一致、包内无 CRLF/BOM、资源文件名全小写、仓库根有 `LICENSE`、真跑一次侧车探针 | 包结构错了，装上去就是"一片空白" |
| `_domcheck.py` | JS 引用的每个 DOM id 必须存在于 `index.html`；扫描前**先剥注释**再查禁用 API；编码与行尾 | 漏一个 id 就是运行时 `null`；不剥注释会把"解释为什么不用 localStorage"判成违规 |
| `_releasecheck.py` | 发布链路静态闸门（见下） | `release.yml` 只有打 tag 才跑，错误要等到发版那一刻才暴露，代价是白等一轮 + 一个废 tag |
| `_storecandidate.mjs` | 生成 dbx-store 扁平候选文件 + provenance 闸门 + schema 自检 | 格式错了商店 PR 直接红 |

`_releasecheck.py` 覆盖：tag 触发过滤器必须是 glob 且能匹配当前版本、reusable 引用必须 pin 到 commit sha、`.artifact.json` 查找不能用 bash `**/`、reusable job 要有 `secrets: inherit` 与 `contents: write`、`gh` 必须带 `--repo`、Release 转正要 `--latest`、`concurrency.group` 必须含 `github.ref`、`GITHUB_REF` 守卫、`go-version` 必须显式关掉、`ci.yml` 与 `release.yml` 的工具链与 plugin-cli 版本必须一致且 pin 精确值、版本号六处一致、**宿主主题通道**（`onEnv` 存在、双注册 + capture、`themeObject`、`index.html` 脚本里不得自己写 `data-theme`）、**图标通道**（不得出现 `<img src>`；笑脸与哭脸两份内联 SVG **各自**与源文件一致，按 class 定位而非 viewBox）。

**每条守卫都必须反向验证**——把问题塞回去看它红不红。本会话两次抓到"永远通过"的假守卫：shell block 正则里 `.` 只匹配一个字符（整条检查静默失效）、panels.js 的判据用词表匹配（被注释里的字样骗过）。只会被动通过的闸门等于没有闸门。

判据一致性要求：`_verify.mjs`、`_domcheck.py`、`_releasecheck.py` 三者的"剥注释 + 禁用 API"规则必须完全一致，否则会出现"本地过、CI 红"。改一边就必须改另一边。

---

## 9. 踩坑清单（全部真实发生并已修复）

| # | 症状 | 根因 | 修法 |
|---|---|---|---|
| 1 | 旗格左键报「侧车内部错误」，且开始时间被重置 | `game/flag` 作用在旗格上抛 `-32011`；`elapsedMs` 只在 `settle()` 重算，出快照走的是陈旧值 | 旗格左键改无反应；新增 `Game::tick()`，出快照前刷新，终局冻结 |
| 2 | 所有错误码都显示成「侧车内部错误」 | 真宿主对业务失败**不 reject**，而是正常返回 `{error}` 包装；`bridge.js` 没解包 ⇒ 全落兜底分支 | 加 `unwrap()` / `fromRaw()` |
| 3 | 超时也说「侧车内部错误」 | 宿主超时 reject 的是**不带 code 的纯 Error**，兜到 `-32603` | 按文案识别归到 `-32001`，让 `errTimeout` 真正生效 |
| 4 | 一次 panic 掀掉整个侧车 | 官方 SDK 的 worker **没有** `catch_unwind`（模板注释是落空的约定） | `Sweeper::handle` 自行包裹；`snapshot::leaked_cell()` 返回 `Option` 而非 `assert!` |
| 5 | `ms_toollib` 内部 usize 下溢 panic | `get_board_1d` 做 `area - minenum`，`minenum > area` 时下溢 | `msbridge::lay` 复刻 area 计算（FirstSafe = 格数−1；Win7 按首点位置扣 角4/边6/内部9）作第二道防线 |
| 6 | DBX 切深色，插件纹丝不动 | 从 `context.theme` 读，而 dev-runtime 的 `context` 里**没有 theme 字段** ⇒ 永远退到 `prefers-color-scheme` | 读 `dbxPlugin.theme` + `dbx-plugin-env`（双注册 + capture）+ `onInit` + 轮询兜底 |
| 7 | 深色下 `<select>` 的选项是黑字看不见 | 未声明 `color-scheme`，原生下拉弹层由 OS 绘制 | `[data-theme]` 驱动 `color-scheme` + 给 `option` 显式配色 |
| 8 | `hidden` 属性完全失效 | `.custom{display:flex}` 是作者样式，盖掉 UA 的 `[hidden]{display:none}` | 全局 `[hidden]{display:none !important}` |
| 9 | 难度下拉进来显示「初级」而非默认中级 | 缺 `selected`，JS 回填前浏览器显示第一项 | `<option value="intermediate" selected>` |
| 10 | 图标在宿主里全是裂图 | 宿主 CSP `img-src data: blob:` 且**不内联 `<img src>`** | 改内联 SVG（已上架的翻译插件同样是这个做法） |
| 11 | 打 tag 后 CI 完全没跑，且无报错 | `on.push.tags` 写成正则，GitHub 这里用 **glob** | 改 `"v*.*.*"`，形态校验交给 prepare 的白名单正则 |
| 12 | release run 在解析 `uses:` 时直接失败 | `@plugin-sdk-v1` 已从上游消失 | pin commit sha |
| 13 | Release 缺 `.artifact.json` | `merge-multiple` 平铺 + 未开 globstar ⇒ bash `**/` 匹配不到 | 改 `find … -print0` |
| 14 | `cargo test` 偶发 1 项失败（25 轮红 1 轮） | 断言 `mineNum == 1` 是"越界报错"时代的残留（25 格下限是 3）；且整段被 `if status == "won"` 包着，压枪常踩雷 ⇒ 绝大多数运行**根本不执行断言** | 拆成三条：夹取口径、记账不变量、game.rs 里用真实雷位做确定性通关。连跑 40 轮全绿 |
| 15 | `paths.rs` 三处环境变量测试随机翻车 | 测试改**进程级**环境变量，并行必然互踩 | `OnceLock<Mutex>` 串行 + 用例结束复原现场（不需要 `--test-threads=1`） |

---

## 10. 编码与格式约定

- 所有源文件 **UTF-8 without BOM**、**LF**（`.gitattributes` 强制；`_verify.mjs` 有闸门：包内任何 `.json/.js/.css/.html/.svg/.md/.toml` 含 `\r\n` 直接拒绝）
- `backend/vendor/**` 标 `-text`：上游是 LF，而 Windows 默认 checkout 会转成 CRLF，一旦转换 `VENDOR.md` 里记录的 SHA-256 与 `fc /b` 核对必然失败
- 注释、文档、错误提示一律**简体中文**；标识符英文
- 代码格式交给 `cargo fmt` / IDE 自动格式化，本手册不规定大括号风格
- **临时脚本（`.ps1` / `.py` / `.mjs`）与浏览器 profile 用完即清**；`.gitignore` 已忽略 `_tmp_*` 作双保险
- PowerShell 内嵌中文会被 GBK 破坏 ⇒ 需要跑含中文的脚本时写成 `.py` 文件并设 `PYTHONIOENCODING=utf-8`

---

## 11. 排查

| 症状 | 先看这里 |
|---|---|
| 装上但一片空白 | 开发宿主看 `dbxPlugin` 是否注入；`system/info` 能否返回 |
| 点格子没反应、成绩存不上 | 九成是侧车自报 id/version 与 manifest 不一致，宿主初始化即拒（宿主不报错）→ `node _verify.mjs` 解包核对 |
| 只有 `windows-x64` 的包 | 预期行为，见 5.4 |
| 中文乱码 | 源文件必须 UTF-8 **without BOM**、LF；见 10 节 |
| 宿主切深色，插件不变 | 主题读的是 `context.theme`（该字段不存在）→ 见 7.1 与踩坑 6 |
| 深色下下拉选项看不见 | 未声明 `color-scheme` → 见踩坑 7 |
| 图标在宿主里是裂图 | 用了 `<img src>` → 见踩坑 10 |
| 插旗后计时被重置 | `elapsedMs` 没在出快照前 `tick()` → 见踩坑 1 |
| 打 tag 后 CI 完全没跑 | `on.push.tags` 写成了正则 → `python _releasecheck.py` 会抓 |
| release run 秒失败、日志说解析不到 workflow | reusable 的 `@ref` 失效 → 见 6.1 |
| `cargo test` 偶发红 | 跑 `_releasecheck.py` 与 40 轮循环；见踩坑 14 / 15 |
