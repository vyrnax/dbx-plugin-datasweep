# DataSweep（DBX 扫雷插件）—— 实施蓝图

> 面向执行者（人或 agent）的**决策完备**文档：照本文件实施时不需要再做任何方案性决策。
> 三份文档分工不重叠：面向用户的说明（能力、截图、安装、隐私、已知限制）在 [`README.md`](README.md)；开发规范、架构契约、RPC 参考表、构建发布流程与踩坑清单在 [`AGENTS.md`](AGENTS.md)；本文件只写"怎么建、建成什么样、怎么验"的蓝图与阶段任务。
> 状态：**文档先行，代码尚未开始。** 下一动作是阶段 0。

---

## 一、摘要

| 项 | 决定 |
|---|---|
| 目标 | 在 DBX 宿主内提供一个可玩的扫雷工作台，能自动多平台打包并上架官方商店 |
| 后端 | **Rust** 原生 sidecar，JSON-RPC 2.0 over stdio（JSON Lines），官方 `dbx-plugin-sdk` 0.1.0 |
| 算法 | 直接依赖 crates.io 的 **`ms_toollib` 1.5.x**（MIT，元扫雷的算法工具箱），不自写判雷/3BV/埋雷 |
| 前端 | **原生 HTML + CSS + JS（ES5 风格 IIFE）+ Canvas**，零构建、零 npm 依赖（沿用翻译插件路线） |
| 局面归属 | **侧车持有唯一权威局面**（真实雷位不出侧车；flood-fill 交给 `refresh_board`） |
| 打包 | **官方 `dbx-plugin package`**，不自写打包器 |
| 平台 | 5 个 target：`windows-x64`、`darwin-arm64`、`darwin-x64`、`linux-x64`、`linux-arm64`，全部由 CI 真机产出 |
| 发布 | 推 tag → CI 构建 → 自动 Release + 资产上传 → 手工提 `t8y2/dbx-store` 候选 PR |
| v0.1.0 范围 | 经典全套玩法（三档 + 自定义尺寸、首点保护、旗/问号、压枪、秒表、3BV、本地最佳成绩） |
| 许可证 | Apache-2.0 |
| 插件身份 | `com.vyrnax.datasweep` / 显示名「扫雷」/ publisher `vyrnax` |
| **明确排除** | 录像播放（`.avf`/`.evf`/`.rmv`）、OBR 截屏识别、机扫/自动求解、在线排行榜、云同步 |

---

## 二、已探明事实（计划的事实基础，不可推翻）

以下每一条都在真实环境核对过（官方文档、CLI 二进制、上游 schema/workflow、crate 源码），实施时不要再重复验证、也不要凭直觉偏离。

### 2.1 插件框架与打包工具链

1. **官方 CLI 有 `rust` 模板**：`dbx-plugin create [dir] --template rust`（`--backend` 是别名；`--language` 是兼容别名）。CLI 为 `@dbx-app/plugin-cli`，本机 0.1.9，npm registry latest 亦为 0.1.9。命令：`create` / `package` / `dev` / `keygen` / `version`。
2. **`dbx-plugin package` 对 Rust 项目自己跑 cargo**。从 CLI 二进制提取出的实际调用形态：
   ```
   cargo build --release --manifest-path <backend>/Cargo.toml [--locked]   # Cargo.lock 存在时加 --locked
   --config 'patch.crates-io.dbx-plugin-sdk.path="<SDK_ROOT>/plugins/sdk/rust/dbx-plugin-sdk"'
   env CARGO_TARGET_DIR=<...>
   ```
   错误串可佐证：`Rust backend is missing`、`Cannot read backend Cargo.toml`、`Rust plugin SDK was not found at`、`Rust SDK root is invalid`。
3. **`dbx-plugin-sdk`（Rust）没有发布到 crates.io**（crates.io 查 `dbx-plugin-sdk`/`dbx_plugin_sdk`/`dbx-plugin`/`dbx-plugin-host` 全 404）。它只存在于 CLI 自带的 `sdk-root/plugins/sdk/rust/dbx-plugin-sdk`（本机位置随 npm 全局目录而定，用 `npm root -g` 查自己机器上的路径），由上面那条 `patch.crates-io` 注入。**⇒ 模板生成的 SDK 依赖写法禁止手改**，也不要试图"规范化"成版本依赖。
4. **官方 packager 拒绝非本机 target**：
   `Native plugin target 'X' does not match build host 'Y'; run this package command on the target platform`
   help 文本亦写明 "Rust and Go projects build a native sidecar for the current host target"。**⇒ 本地只能产出 `windows-x64`；其余平台必须 CI 真机。** Go 插件那套 `CGO_ENABLED=0` 一次交叉编译 6 平台的经验在这里不可复用，不要照抄。
5. **官方 reusable workflow 原生支持 Rust**：`t8y2/dbx/.github/workflows/plugin-release-reusable.yml`（tag 引用 `@plugin-sdk-v1`）
   - 入参：`release-tag`(必填) / `package-command`(必填) / `working-directory`(默认 `.`) / `package-path`(默认 `dist/*.dbxp`) / `metadata-path`(默认 `dist/*.artifact.json`) / `node-version`(默认 22) / `go-version`(默认 `1.22.x`，**无 Go 后端时传 `""` 关掉安装**) / `rust-toolchain`(默认 `stable`，**发布要 pin 精确版本**) / `sdk-ref`(默认 `plugin-sdk-v1`) / `plugin-cli-version`(默认 `0.1.2`) / `install-plugin-cli`(默认 true) / `install-plugin-cli-from-source`(默认 false) / `build-matrix`
   - Rust 工具链步骤：`dtolnay/rust-toolchain@stable`，条件 `inputs.rust-toolchain != '' || (install-plugin-cli && from-source)`，`toolchain: ${{ inputs.rust-toolchain || 'stable' }}`
   - **默认 `build-matrix` 正好是本次要的 5 个 target/runner** ⇒ 不需要自定义：
     `ubuntu-24.04`→`linux-x64`、`ubuntu-24.04-arm`→`linux-arm64`、`windows-2022`→`windows-x64`、`macos-15-intel`→`darwin-x64`、`macos-15`→`darwin-arm64`
   - `DBX_PLUGIN_TARGET` 以环境变量注入 `package-command`（`shell: bash`，Windows runner 上是 git-bash）
   - `publish` job 校验 sha256/size 与 `.dbxp` 字节一致、拒绝含 `signature.json` 的包、校验各 target 的 manifest 身份一致、汇总 `release-candidates.json`，并 `gh release upload --clobber`
   - **`publish` 不上传各 target 的 `.artifact.json`** ⇒ 需要自己补一个 upload-metadata job（翻译插件已踩过：从 run artifact `dbx-plugin-<target>` 取，不要从 Release 下载通配符）
6. **manifest v1 的 `entrypoints.backend` 只需 `executable`**（schema 允许 `protocol_versions`/`transport`，但官方 rust 模板只写 `executable`，Rust SDK 默认 JSON Lines，与宿主一致）。只有 `stdio-framed` 才需要 `transport` + `host.binary` 权限，本插件用不到。
7. **`executable` 不带 `.exe`**：官方 `executable_name()` 只在 Windows 目标补 `.exe`。`manifest.json` 写 `bin/dbx-plugin-datasweep`。
8. **schema 严格**：`manifest.schema.json` 顶层 `additionalProperties: false`；`menusContribution` 只允许 `type`/`id`/`items`（**加 `label` 会被拒**，报 `unknown field 'label', expected 'id' or 'items'`）；`permissions` 是固定枚举 + `host.network:https://…` 正则；`identifier` 形如 `^[a-z0-9][a-z0-9._-]*$`（≤128）。
9. **侧车身份必须与 manifest 逐字一致**：`PluginMetadata::new(id, version)` 的 id/version 不等于 `manifest.json` 时，宿主在 `plugin/initialize` 阶段拒绝后端，**UI 表现只是"什么都不工作"且宿主不报错**。
10. **SDK API 面**（432 行，仅依赖 `serde` + `serde_json`，无 tokio/async）：
    - `PluginMetadata::new(id, version)`、`.with_capability(…)`
    - `trait PluginHandler: Send + Sync + 'static { fn handle(&self, RequestContext, &str, Value, &PluginEmitter) -> Result<Value, PluginError>; fn handle_binary(…) }`
    - `PluginEmitter::event(method, params)`、`.binary(channel, data)`（后者要求 Framed）
    - `PluginServer::new(metadata, handler).transport(PluginTransport::JsonLines).worker_threads(n).work_queue_capacity(n).serve()`
    - `PluginError::new(code, message)` / `::method_not_found(method)`（`-32601`）
    - 常量：`PROTOCOL_VERSION = 1`、`MAX_JSON_BYTES = 8 MiB`、`MAX_BINARY_BYTES = 64 MiB`、默认 worker 2–16（按 available parallelism）、队列 256
    - 进程规则：**stdout 只留给协议，诊断写 stderr**；插件自有会话放在 handler 或进程级注册表；connect/disconnect 幂等；不要在事件或 workbench context 里传连接密钥
    - ⚠ **worker 里没有 `catch_unwind`**（`lib.rs` 全文搜不到）。模板注释写着 `panic stays "unwind" so the sidecar protocol can report panics as errors`，但那是**落空的约定**：handler 一 panic，那条响应永远写不出去，宿主的表现是"侧车莫名重启 + 本局凭空作废 + 前端干等到超时"。⇒ 本插件在 `Sweeper::handle` 里**自己包 `catch_unwind`** 把 panic 转成 `-32603`，不指望上游兜底
11. **UI↔侧车 API**：`dbxPlugin.invoke(method, params, {timeoutMs})` 调侧车、`dbxPlugin.request(method, params)` 调宿主、`onEvent`/`onContext`/`onInit` 订阅、`copy(text)` 写剪贴板（无需权限）。`timeoutMs` 被宿主夹到 **1–120000，默认 30000**。API 对象永远是 `window.dbxPlugin` 本身，**不要把 `ready` 的结果当宿主对象用**。
12. **沙箱 iframe 是 opaque origin**：`localStorage` 一碰就抛 `SecurityError`；不能覆盖宿主 DOM；无编辑器选区 API；无 keybinding 贡献点。
13. **侧车数据目录（三级回退，翻译插件实测）**：
    `DBX_PLUGIN_DATA_DIR` → `DBX_PLUGIN_SPACE` → 平台配置目录 `/dbx/plugins/<id>`（Windows `%APPDATA%`、macOS `~/Library/Application Support`、Linux `~/.config`）→ `./data`
    Rust 标准库没有 `UserConfigDir` ⇒ **用 `std::env::var` 手工拼，不引入 `dirs`/`directories` crate**（少一个依赖就少一个跨平台构建面）。
14. **商店流程**：候选 PR 提到 **`t8y2/dbx-store:main`**（不是 `t8y2/dbx`）；`dbx-store` 的 discover 脚本只认 **非 draft 且非 prerelease** 的 Release；`.dbx-store.json` 的 `name`/`description`/`permissions` 必须与 `manifest.json` 逐字一致；根目录必须有 `LICENSE` 全文。
15. **Rust 侧车没有内建 VCS 戳**（Go 有 `go version -m` 可见 `vcs.revision`/`vcs.modified`）。⇒ provenance 自证改为：CI 从 tag 检出 + `Cargo.lock` 提交（packager 自动 `--locked`）+ `rust-toolchain` pin 精确版本 + summary 打印各 `.artifact.json` 的 target/size/sha256。**本地构建后手工上传 = provenance 硬伤**（翻译插件 v0.2.1 因此被打回）。
16. **开发宿主**：`dbx-plugin dev --path . --port 5190`（需 Node 22+），会构建后端并加载 UI；`.dbx-dev/` 存本地开发数据，**不得提交也不得打包**。开发宿主不模拟桌面窗口管理能力，"跟随宿主"类功能只能在真实 DBX 桌面端验收。
17. **官方 rust 模板的 `[profile.release]`**：`strip = true`、`lto = true`、`codegen-units = 1`，且**刻意保留 `panic = "unwind"`**（模板注释：让侧车协议能把 panic 作为错误上报，而不是直接 abort 掉整个 sidecar）。⇒ 不要改成 `panic = "abort"`。

### 2.2 算法层 `ms_toollib`

18. **crates.io 有 `ms_toollib`**，最新稳定 **1.5.16**，**MIT**，描述 "Algorithms for Minesweeper"，同源发布到 pypi/npm。元扫雷 README 亦确认 `ms_toollib` 为 MIT。
19. **feature 结构（从发布的 crate 源码核实）**：
    ```toml
    [features]
    default = ["rs"]
    rs = ["rand", "tract-onnx", "web-time"]
    js = ["web-time"]
    py = ["rand", "tract-onnx", "web-time"]
    ```
    - `tract-onnx` / `web-time` / `rand` 是 **optional 依赖**
    - `mod obr`、`ImageBoard`、`SafeBoard`、`SafeBoardRow`、`laymine_solvable_thread`、`sample_bbbvs_exp`、`obr_board` 全部在 `#[cfg(any(feature="py", feature="rs"))]` 下
    - **但 `web_time::Instant` 在非 gated 的 `miscellaneous.rs` 与 `videos/base_video*.rs` 里被无条件 `use`**，`rand` 也有非 gated 使用点 ⇒ `default-features = false` 且只开 `rand` 极可能编译失败
    - 顺带：`getrandom` 依赖带 `features = ["js"]`，纯 Rust，无 C 链接面
    - **【阶段 0 实测 → 阶段 1 推翻 → 终判 = B】**（本节唯一被修订的事实，务必读完再动 `Cargo.toml`）：
      - 阶段 0 时 `default-features = false, features = ["rand", "web-time"]` 在 `cargo check` 上**通过**（解析到 1.5.16），依赖树仅 34 个 package，当时据此记为结果 A。
      - **阶段 1 证明该结论是错的**：`get_board_1d` 的洗牌写在 `#[cfg(any(feature = "py", feature = "rs"))]` 之下（源码 `utils.rs:456-470`）。关掉 `rs` 后**洗牌静默消失**，埋雷退化成「所有雷按构造顺序堆在一维数组末尾」，二维化后连成一整行/整列 ⇒ 首点一次 flood 就开完全盘、对局立刻结束。**能编译 ≠ 行为正确**，这是本项目到目前为止最贵的一条教训。
      - ⇒ **终判改回 B**：`ms_toollib = { version = "1.5.16", features = ["rs"] }`（即默认 feature 那一档）。实测代价：依赖树从 34 涨到 **126 个 package**（带来 `tract-*` 一族、`ndarray`、`liquid`、`prost`、`regex` 等），本机 release 构建 **2m19s**，侧车产物 **512 KB**。按 4.1 的依赖纪律，仍**不**走 vendor 精简 fork 那条路。
      - 回归闸门：`backend/src/msbridge.rs` 的 `tests` 里有两条测试专门守这个坑（布局随机性、零格占比），**任何人想再动这条 feature 都必须先看它们**。
20. **已确认的函数签名**（`Vec<Vec<i32>>`，行优先，`board[0][0]` 左上角）：
    | 用途 | 签名 |
    |---|---|
    | 标准埋雷（首点非雷） | `laymine(row: usize, column: usize, minenum: usize, x0: usize, y0: usize) -> Vec<Vec<i32>>` |
    | Win7 埋雷（首点开空 + 邻域无雷） | `laymine_op(row, column, minenum, x0, y0) -> Vec<Vec<i32>>` |
    | 无猜埋雷（0.2.0） | `laymine_solvable(row, column, minenum, x0, y0)`、`laymine_solvable_adjust(…)` |
    | 翻格 / flood-fill / 失败标注 | `refresh_board<T>(board: &T, board_of_game: &mut Vec<Vec<i32>>, clicked_poses: Vec<(usize, usize)>)` |
    | 由真实局面算数字（**不幂等**） | `cal_board_numbers(board: &mut Vec<Vec<i32>>)` |
    | 3BV | `cal_bbbv(board: &T) -> usize`；缓存版 `Board::new(b).get_bbbv()` |
    | 好的压枪判定 | `is_good_chording(board_of_game: &Vec<Vec<i32>>, pos: (usize, usize)) -> bool` |
    | 其他指标 | `cal_op`(空/0 的 8 连通域数)、`cal_isl`(岛数)、`cal_cell_nums`、`cal_zini/cal_rzini` |
    | 判雷引擎（0.2.0） | `solve_direct`、`solve_minus`、`solve_enumerate`、`mark_board`、`is_able_to_solve`、`is_solvable`、`try_solve`、`get_all_not_and_is_mine_on_board` |
    | 概率（0.2.0） | `cal_probability_onboard`、`cal_probability_enum`、`cal_probability_csp`、`cal_probability_cells_is_op`、`cal_probability_cells_not_mine` |
21. **局面编码约定**（写进 `msbridge.rs` 的常量，全项目只用这套）：
    - 真实局面 `board`：`0` 空、`1..8` 数字、`-1` 雷
    - 游戏局面 `board_of_game`：`10` 未打开、`11` 雷（算法判定为雷 **或** 玩家插旗，二者不共存）、`12` 已判定非雷但未点、`0..8` 已开、`14` 失败后显示标错的雷、`15` 踩中的红雷、`16` 失败后显示的其他雷、`18` 压枪高亮造成的"看起来像 0"
    - **ms_toollib 没有"问号"编码** ⇒ 问号标记由本插件的 `Game` 结构自己维护（`marks: Vec<(usize, usize)>`），下发为独立字段，**不自造数字码**去污染算法语义
22. **`GameBoard` 类型不是点击状态机**（`board.rs` 实测：`set_game_board`/`get_poss`/`get_basic_is_mine`/`get_enum_*` 等），是给判雷与概率引擎用的分析包装。⇒ **`Game` 结构自己持有 `board` + `board_of_game` + `flags` + 计时**，翻格动作转成 `refresh_board(真实局面, &mut game_board, 坐标列表)`。
23. **合规红线：Metasweeper 本体是 GPL-3.0 + 附加商业限制与贡献者收益分配条款。**
    - 禁止搬运其任何 Python 源码、`uiFiles`/`media` 素材、图标、字体、音效、截图、`.avf` 样例、README 文案
    - 只把 crates.io 上 MIT 发布的 `ms_toollib` 作为依赖（`Cargo.lock` 与 README 注明来源与许可）
    - 游戏机制本身（翻格/压枪/首点保护/3BV 定义）属公开玩法概念与算法，实现全部新写
24. **`ms_toollib` 官方立场**：不提供机扫工具，明确反对用机扫模拟人类操作攻击排行榜。⇒ 本插件**不做自动点击、不做整局自动求解**；0.2.0 的"无猜模式"仅指*生成 100% 可判雷的局面*（`laymine_solvable*`）与*逐格概率提示*，属于玩法模式，与机扫无关。

### 2.3 本机环境（已实测）

| 工具 | 状态 |
|---|---|
| `cargo` / `rustc` / `rustup` | **【阶段 0 已安装】** `winget install --id Rustlang.Rustup` → rustup **1.29.1**，默认工具链 `stable-x86_64-pc-windows-msvc`，**rustc 1.99.1 (58cf8f9c3 2026-10-13)** / **cargo 1.99.1 (5f94df478 2026-08-27)** ✅。**精确版本串 `1.99.1` 即 `ci.yml` 与 `release.yml` 的 `rust-toolchain` 值**。<br>注意：安装器写入的用户 PATH 需新开 shell 才生效；本会话内命令均前置 `$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"` |
| winget | v1.29.380 ✅（`winget install --id Rustlang.Rustup`） |
| Visual Studio | Community 2026，`C:\Program Files\Microsoft Visual Studio\18\Community`，MSVC 工具集 **14.51.36231**（`bin/Hostx64/x64/cl.exe` 存在）、Windows SDK **10.0.26100.0** ✅ ⇒ `stable-msvc` 可直接用 |
| `link.exe` on PATH | 不在（正常，Rust 自行定位 MSVC，无需 Developer Prompt） |
| Node.js | v24.18.0 ✅（`dbx-plugin dev` 要求 22+） |
| Python | 3.14.6 ✅（仅前端静态闸门） |
| `@dbx-app/plugin-cli` | 0.1.9 已全局安装 ✅，`sdk-root` 含 Rust/Go SDK；CLI 原生二进制在其 `node_modules/@dbx-app/plugin-cli-win32-x64/bin/dbx-plugin.exe` |
| zig / scoop / choco / cmake / ninja | 均不存在（不需要） |
| 参考实现 | 已上架的 Go 插件 `dbx-plugin-translation`（github.com/vyrnax）、`Metasweeper`（GPL 本体 + MIT 算法库来源），均放在本机工作目录，路径不入库 |
| 仓库 | 本仓库，remote `https://github.com/vyrnax/dbx-plugin-datasweep.git`，分支 `main` |

### 2.4 阶段 0 实测补充（比事实 19–22 更细，且有两处纠正原假设）

以下几条是阶段 0 在真实编译与真实源码上测出来的，实施时直接照做。

**A. 本地所有裸 cargo 命令都必须显式注入 SDK patch。**
`dbx-plugin-sdk` 不在 crates.io（事实 3），而 `--config patch.crates-io…` 只有 `dbx-plugin package` 内部会自动加。所以本机跑 `cargo check/test/clippy/build` 必须自己带：

```powershell
# SDK 路径机器相关，用 `npm root -g` 取自己的
$sdk = Join-Path (npm root -g) '@dbx-app/plugin-cli/sdk-root/plugins/sdk/rust/dbx-plugin-sdk'
cargo check --manifest-path backend\Cargo.toml --config "patch.crates-io.dbx-plugin-sdk.path=`"$sdk`""
```

否则解析 `dbx-plugin-sdk = "0.1.0"` 直接失败。**不要把这条 patch 写进仓库里的 `.cargo/config.toml`**（路径是机器相关的），也不要写进全局 `~/.cargo/config.toml`（会与 packager 自己注入的那条冲突）。CI 侧的解法：先 `npm i -g @dbx-app/plugin-cli@0.1.9`，再用 `npm root -g` 拼出 sdk-root 传给同一个 `--config`（阶段 4 落 `ci.yml` 时照此写）。

**A2. `dbx-plugin package` 不会在 `backend/target/release/` 留下产物。** packager 自带 `CARGO_TARGET_DIR`（实测出包后 `backend/target/` 下只有 `debug/` 与 `tmp/`）。⇒ `_e2e.mjs` 要驱动二进制，必须先自己 `cargo build --release --manifest-path backend\Cargo.toml --config <同上 patch>`，产物才落在 `backend/target/release/dbx-plugin-datasweep[.exe]`。`ci.yml` 的 test job 已经显式做了 `cargo build --release`，所以不受影响。

**B. `refresh_board` 的真实语义（源码 `utils.rs:604-646`）——纠正原假设。**
只有两个副作用，**其余一概不做**：

1. 对 `clicked_poses` 逐个出栈：真值 `>0` 写数字；真值 `==0` 写 `0` 并**把 8 邻域里值为 `10` 或 `12` 的格压回栈**（⇒ flood-fill 只在真值为 0 的格上扩散，因此绝不会翻开雷；**值为 `11`（旗）的格不会被 flood 打开**）；真值 `==-1` 写 `15` 并置 `loss_flag`。
2. `loss_flag` 时，把所有「游戏局面 `==11` 但真值 `!=-1`」的格改写为 `14`（标错的雷）。

⇒ **`16`（失败后显示的其他雷）`ms_toollib` 从来不写，必须本插件自己在判定 `lost` 后刷一遍**：遍历真实局面，把「真值是雷**且当前还盖着**（`10`/`12`）」的格置 `16`。**注意别把 `15` 覆盖掉**——阶段 1 第一版就是按"只要不是 `11` 就置 `16`"写的，结果踩中的红雷被刷成白雷，红雷计数变成 0（已由 `snapshot` 的单测拦截）。`14` 也由 `refresh_board` 自己写，我们不动。这是 4.4 节胜负判定里需要补的一步。
⇒ **`18`（压枪高亮）同理不由侧车下发**，属前端渲染态。
⇒ 直接揭示一个 `11`（旗）格会把旗覆盖成真实数字，所以 `rpc.rs` 的 `game/reveal` 必须自己拦掉已插旗的格（算法不拦）。

**B2. `laymine` / `laymine_op` 返回的盘面**已经**算好数字 —— 这条纠正 5.3 的流程。**
源码 `utils.rs:473-501`（`trans_board_1d_2d_op`）与 `506-534`（`laymine`）在铺下每个 `-1` 时就对邻域做 `+= 1`，所以**返回的盘面自带数字**。而 `cal_board_numbers` 不幂等（`utils.rs:1607` 对邻域内 `>=0` 的格继续 `+= 1`），在其后再调一次会**让每个数字翻倍**——阶段 1 实测到的症状正是"数字恰好等于真实邻雷数的 2 倍"（`value=2` 而邻域只有 1 颗雷）。
⇒ **`msbridge::lay` 里已不再调用 `cal_board_numbers`**，5.3 流程中的那一行作废。它只在**手工搭盘**时才需要（0.2.0 的 `board/from-array` 届时会再加回来）。
⇒ 回归闸门：`msbridge` 的 `埋雷返回的数字必须等于实际邻雷数`。
⇒ 阶段 0 记下的 3BV / flood 基线**不受影响**：那次探针用的是手工搭的 `0/-1` 盘，只调了一次 `cal_board_numbers`，是正确用法。

**C. 其余 API 实测细节。**

| 事项 | 实测 |
|---|---|
| `cal_board_numbers` 非幂等 | 源码 `utils.rs:1607` 是对邻域内 `>=0` 的格做 `+= 1`，**无 `-1` 跳过保护** ⇒ 调第二次数字翻倍。`laymine*` 返回时已含数字，**埋雷路径上不调用它**（见 5.3 与 2.4） |
| `Board::new` | 签名是 `pub fn new(board: Vec<Vec<i32>>) -> Board`，**按值接收**（会 move）⇒ 对照校验要传 `board.clone()`，不要传 `&board` |
| `cal_bbbv` 与 `Board::get_bbbv` | 同一局面两者返回值 **相同**（已实测一致） |
| `laymine_op(row,column,minenum,x0,y0)` | 200 次采样：雷数准确等于 `minenum`、首点及其邻域必非雷 ✅（`laymine` 同样保证首点非雷） |
| 张量命名 | `x0` 是**行**、`y0` 是**列**（与 5.2 一致，已在实现里按此映射） |

**C2. 打包器会改写**包内**那份 manifest 的 `entrypoints.backend.executable`。**
实测：源 `manifest.json` 写 `bin/dbx-plugin-datasweep`（不带 `.exe`，符合事实 7），
而解包 `.dbxp` 后看到的是 `bin/windows-x64/dbx-plugin-datasweep.exe`——打包器自己补上了
**目标子目录 + 平台扩展名**。⇒ "executable 不带 `.exe`" 这条只对**源 manifest** 成立，
两个口径不一样，`_verify.mjs` 里因此是分开查的（源：禁止 `.exe`；包内：必须存在且以源名为后缀）。

**D. 阶段 1 单测基线（阶段 0 探针实测，逐格手工核对无误）。**

固定局面 5×5，雷在 `(1,1)`、`(1,4)`、`(3,3)`（`board` 其余为 `0`），`cal_board_numbers` 后：

```
 1  1  1  1  1
 1 -1  1  1 -1
 1  1  2  2  2
 0  0  1 -1  1
 0  0  1  1  1
```

- `cal_bbbv` = `Board::new(board.clone()).get_bbbv()` = **14**
  （手工核对：空区 `{(3,0),(3,1),(4,0),(4,1)}` 算 1 次；25 格 − 3 雷 = 22 个可开格，扣掉这次连通的 9 格，余 13 ⇒ 1 + 13 = 14 ✅）
- `refresh_board(&board, &mut game, vec![(3,0)])` 打开**恰好 9 格**，结果：

```
10 10 10 10 10
10 10 10 10 10
 1  1  2 10 10
 0  0  1 10 10
 0  0  1 10 10
```

- `vec![(0,3)]`（真值 1）只开 `(0,3)` 一格；`vec![(2,2)]`（真值 2）只开 `(2,2)` 一格 ⇒ 数字格不扩散
- `vec![(1,1)]`（踩雷）只把该格写成 `15`，**没有 14/16**（此时还没有错旗、且见上文 B）

---

### 2.5 vendor 与裸 cargo（阶段 1 补的硬事实）

25. **裸 `cargo test` / `clippy` / `build` 默认解析不到 `dbx-plugin-sdk`**：它不在 crates.io 上，而 packager 是靠命令行 `--config` 注入 path 的。直接失败的信息是
    `error: no matching package named dbx-plugin-sdk found` + `location searched: crates.io index`。
    ⇒ 单测与 e2e 全部建立不起来，属硬阻塞。
26. **处理**：`backend/vendor/dbx-plugin-sdk/`（上游**字节级复制**，`fc /b` 核对无差异，校验和记在 `backend/vendor/VENDOR.md`）+ 一段 `[patch.crates-io]` 配置。`Cargo.toml` 里那行模板生成的 `version = "0.1.0"` **保持不变**，不去改成 path 依赖——避免与上游约定分叉。
27. **`.cargo/config.toml` 必须放两份**（仓库根 **和** `backend/`）。cargo 的配置按**当前工作目录**向上查找，与 `--manifest-path` 指向无关。三种调用形式都已实测：
    - `cd backend && cargo …` → 由 `backend/.cargo/config.toml` 生效
    - 仓库根 `cargo … --manifest-path backend/Cargo.toml` → 由仓库根那份生效（**只放一份时这一种必然失败**，而 CI 用的正是这种）
    - packager 追加 `--config 'patch…path="<SDK_ROOT>/plugins/sdk/rust/dbx-plugin-sdk"'` → 命令行优先级高于配置文件，**实际链接的是上游那份 SDK 而不是 vendor**（`cargo tree` 显示解析到 `AppData\Roaming\npm\node_modules\@dbx-app\plugin-cli\sdk-root\…`）。所以 vendor 只服务于测试/构建，不改变发布产物的来源
28. **`dbx-plugin package` 不会重写 `Cargo.lock`**（打包前后 SHA-256 实测一致）⇒ CI 用 `--locked` 安全，也意味着 `ci.yml` 里既不必装 plugin-cli、也不必往 cargo 命令拼 `--config`（已据此简化）。
29. **`cargo fmt` 不会碰 vendor**（它是独立 crate，不在本 crate 的 `src/` 下），跑完 `fc /b` 仍无差异。`.gitattributes` 已加 `backend/vendor/** -text`，防止 Windows checkout 把上游 LF 转成 CRLF 而破坏逐字节一致。
30. **SDK 的 worker 没有 `catch_unwind`**（见 2.1 事实 10 的 ⚠ 条）。模板注释许下的"panic 会被上报成错误"是落空的，必须自己在 `Sweeper::handle` 兜住。
31. **`ms_toollib` 的 `get_board_1d` 会做 `area - minenum`（usize）**，雷数超过 `area` 就是下溢 panic / 天量分配；`laymine` 还会按 `x0 + y0*row` 索引一维数组，首点越界即 index panic。⇒ `msbridge::lay` 自己复刻 `area` 计算（`FirstSafe = 行列积-1`；`Win7` 按首点位置扣 角 4 / 边 6 / 内部 9）并校验首点坐标，作为不依赖 rpc 层的第二道防线。

---

## 三、功能范围

### 3.1 纳入（v0.1.0）

- 三档预设难度 + 自定义尺寸/雷数
- 首点保护两种规则（Win7 `laymine_op` 默认 / 仅首点非雷 `laymine`）
- 左键翻格、右键旗/问号/空循环、压枪、拖拽批量插旗、`Ctrl` 盲开
- 键盘操作（方向键 + Enter/Space/F2/Ctrl+R/Esc）
- 秒表（侧车权威计时）、剩余雷数、3BV、3BV/s、已开格数
- 分维度本地最佳成绩 Top 10 + 清空
- 偏好持久化（难度、规则、是否显示问号、面板开关）
- 主题与语言跟随宿主
- CI：`ci.yml`（fmt/clippy/test/e2e/本机打包校验）+ `release.yml`（tag 触发官方矩阵）
- 商店：`.dbx-store.json` + `RELEASE_NOTES.md` + 候选文件产出

### 3.2 纳入（0.2.0，接口在本蓝图里已预留）

- 纯无猜模式：`laymine_solvable` / `laymine_solvable_adjust` 生成 100% 可判雷局面
- 按 `Space` 显示逐格雷概率着色：`cal_probability_onboard`（配合 `mark_board` 预处理）
- 局面导入/导出（文本矩阵），用于研究

### 3.3 明确排除

录像播放（`.avf`/`.evf`/`.rmv`/`.mvf`）、OBR 截屏识别、机扫/自动求解、在线排行榜、云同步、zh/en 之外的多语言、触屏适配、逐像素透明。

---

## 四、技术架构

### 4.1 分层与依赖

```
ui/  ── 零构建原生 JS + Canvas
  │  invoke / request / onEvent
backend/ (独立 crate，自带 [workspace])
  ├─ dbx-plugin-sdk   0.1.0  Apache-2.0  ← packager 注入 patch.crates-io；裸 cargo 走 vendor（见 2.5）
  ├─ ms_toollib       1.5.16 MIT         ← 必须带 rs（见 2.4 终判 B）
  ├─ serde / serde_json  1
  └─ build.rs（可选，见 4.4）
```

**依赖纪律**：除上表外不加任何依赖。特别地——
- **不加 `rusqlite`/`sqlite`**：偏好用单个 JSON 文件足够（成绩最多几十条），而 `rusqlite` 的 `bundled` 会引入 C 编译面，把 5 个平台的构建风险乘上去。
- **不加 `tokio`/`anyhow`/`thiserror`/`log`**：SDK 是同步 worker pool；错误直接用 `PluginError::new(code, msg)`；诊断写 stderr（宿主收），不引日志框架。
- **不加 `rand`**（RNG 由 `ms_toollib` 内部使用）。
- **不加 `dirs`/`directories`**：数据目录用 `std::env` 手工回退（见 5.5）。

### 4.2 权限与贡献点

- `permissions: []` —— 空数组。游戏不需要任何宿主能力：主题/locale 不需要权限，持久化走侧车数据目录，不读连接、不读 SQL、不读剪贴板、不联网。**每一项权限都是一条审核问答，且可能抬高最低宿主版本。**
- 0.2.0 若概率计算要走流式事件，再加 `host.events` 并**同步更新 `.dbx-store.json`**（两处不一致商店审核会拦）。
- 贡献点三件套（照翻译插件已上架形态）：
  1. `workbench`：`com.vyrnax.datasweep.main`，label「扫雷」，icon `assets/plugin.svg`
  2. `command`：`com.vyrnax.datasweep.cmd.main`，`action = { type: "open-workbench", workbench: "….main", presentation: "tab" }`
  3. `menus`：`items` = `appToolbar`（`default_visible: true`）+ `commandPalette`；**`menus` 贡献点对象本身不得带 `label`**（事实 8）
- `localizations.en` 覆盖 `name`/`description` 与两个贡献点的 label/description。

### 4.3 后端模块划分

| 文件 | 职责 |
|---|---|
| `backend/src/main.rs` | 装配 `PluginMetadata`（常量 fallback + 从包内 `manifest.json` 向上找 6 层反读 version，仅当 `id` 匹配才采用）；`PluginServer::new(...).serve()`；启动/异常诊断写 stderr |
| `backend/src/rpc.rs` | `impl PluginHandler for Sweeper`：方法名 → 参数解析 → 调 `game`/`store` → 快照序列化；集中错误码表；`match` 兜底 `method_not_found` |
| `backend/src/game.rs` | `Game` 结构（`board`/`board_of_game`/`flags`/`marks`/`opened`/`status`/`started_at`/`elapsed_ms`/`bbbv`）+ 进程级 `Mutex<HashMap<u64, Game>>` 会话注册表 + 翻格/压枪/胜负判定 |
| `backend/src/msbridge.rs` | `ms_toollib` 唯一出口：埋雷（`laymine` / `laymine_op`，返回时已含数字）、`refresh_board` 翻格与 flood-fill、`cal_bbbv`、（0.2.0）判雷与概率；局面编码常量；入参的第二道防线（防 `get_board_1d` 的 usize 下溢，见 2.5 事实 31）；**绝不把真实雷位发给 UI** |
| `backend/src/snapshot.rs` | 快照 DTO（`Serialize`）：`cells`/`marks`/`status`/`bbbv`/`elapsed_ms`/…；`ready` 时 `bbbv: null` |
| `backend/src/store.rs` | `datasweep.json` 读写：`load_or_default` / `save`（temp → rename 原子写）/ prefs 局部更新 / records 插入并裁剪 Top 10 |
| `backend/src/paths.rs` | 数据目录三级回退（事实 13） |
| `backend/src/build_info.rs` | 可选：`option_env!("DATASWEEP_BUILD_REV")` 暴露构建 commit |

**会话生命周期**：只保留**一个**活动对局（`gameId` 单调递增，新 `game/new` 覆盖旧的）。`won`/`lost` 后 `Game` 保留到下一次 `game/new`（供 UI 回看与成绩展示）；侧车被宿主回收 ⇒ 本局作废，UI 收到 `-32010` 后提示并重置。**不做跨进程局面恢复**（复杂度和收益不成比例，README 已列为已知限制）。

### 4.4 计时、胜负与成绩

- **权威计时在侧车**：`game/new` 记录 `ready_at`；**第一次 `game/reveal` 才启动 `started_at`**（首点前不计时，符合扫雷惯例）；每次动作后按 `Instant::now() - started_at` 更新 `elapsed_ms`；终局冻结。UI 每 100 ms 本地 tick 只做显示，终局显示以侧车返回的 `elapsed_ms` 为准。
  理由：成绩若由前端上报就能被任意篡改。既然局面已在侧车，用时也必须在侧车。
- **胜负**：已开的非雷格数达到 `rows*cols - mineNum` ⇒ `won`；`refresh_board` 内部已判定踩雷并写 `14/15/16` ⇒ 依据返回的 `board_of_game` 中是否出现 `15` 判定 `lost`。
- **成绩键**：`{rows}x{cols}-{mineNum}-{rule}`；每个键最多 10 条，按 `elapsedMs` 升序，用 `finishedAt`（RFC3339）去重。自定义尺寸与预设共用同一套键，不为预设单开表。
- **可选构建戳**：`backend/build.rs` 读 `DATASWEEP_BUILD_REV` 环境变量（CI 里由 `package-command` 注入 `$GITHUB_SHA`）并 `cargo:rustc-env=` 写入；无该变量时为 `unknown`。**不引入 `vergen` 之类构建依赖，也不在 build.rs 里调 `git`**（保证从源码 tarball 也能构建）。

### 4.5 前端文件划分（零构建）

| 文件 | 内容 |
|---|---|
| `ui/index.html` | 单一工作台页；顶部只放品牌图标 + 「扫雷」+ 笑脸重开按钮；右侧栏顺序为 状态提示 → 设置（难度，选自定义才展开行/列/雷数与区间提示）→ 统计格 → 成绩 |
| `ui/bridge.js` | `window.dbxPlugin` 封装：`call(method, params, timeout)`（超时常量必须落在 1–120000）、`ready` 不阻塞启动（宿主 init 可能晚于脚本执行）、`onContext` 订阅 theme/locale、错误文案归一 |
| `ui/game.js` | 前端状态（最近快照、焦点格、队列）；**`game/*` 单在途请求 + FIFO 队列**（防乱序回退）；`-32010` → 重置提示 |
| `ui/canvas.js` | DPR 适配、格子边长自适应（下限 12px，超出容器时按容器缩放）、`cells`+`marks`→绘制（未开/数字 1–8 配色/旗/问号/雷/红雷/标错雷/高亮）、脏区重绘 |
| `ui/input.js` | `pointerdown`/`pointerup`/`pointermove`/`contextmenu`(preventDefault)；压枪判据 **`e.buttons === 3`**；`button===2` 进入旗状态机；拖拽批量插旗记录 `dragBaseline` 只在跨过新格时切一次；键盘映射 |
| `ui/panels.js` | 秒表 tick、剩余雷数、3BV 与 3BV/s、成绩表、设置面板（难度/自定义尺寸/问号开关）；`readForm` 按后端口径夹取输入并回写，`syncSizeInputs` 用后端 snapshot 的真实尺寸回填输入框，`restart` 用合并语义（在途期间的改动不丢） |
| `ui/i18n.js` | zh/en 字典，按 `dbxPlugin.locale` 切换，未知 locale 退英文 |
| `ui/styles.css` | 宿主主题 CSS 变量（明暗跟随 `onContext`），不自建配色体系 |

**渲染选型**：Canvas 而非 DOM——最大 2500 格，DOM 方案在连点 + 拖拽插旗下必然掉帧，这是本项目选 Canvas 的唯一理由（不是性能崇拜）。

---

## 五、详细设计

### 5.1 `dbx-plugin.toml`（定稿）

```toml
schema_version = 1

[backend]
language = "rust"
directory = "backend"
binary = "dbx-plugin-datasweep"

[package]
include = ["assets", "ui"]
```

`.dbx-dev/`、`backend/target/`、`dist/` 进 `.gitignore`；**`backend/Cargo.lock` 必须提交**（事实 15：packager 见 lock 即 `--locked`，锁死依赖树才谈得上可复现）。

### 5.2 RPC 全表

语义、入参、返回、错误码见 [`AGENTS.md` 第 4 节](AGENTS.md)（**唯一来源，不在此重复**）。此处只记实现约束：

- 所有 `game/*` **返回全量快照**，不做增量 diff（避免"两份实现语义不一致"这一类最难查的 bug）
- 坐标一律 `r`（行，0-based）/ `c`（列，0-based）；`ms_toollib` 内部 `x` = 行、`y` = 列，`msbridge.rs` 做映射并**在函数名/注释里写死这个对应关系**（`laymine(row, column, minenum, x0, y0)` 里 `x0` 是行）
- 参数**夹取**在 `rpc.rs` 一处完成：`rows/cols` 夹进 `[5,50]`，`mineNum` 夹进 `[ceil(格数×10%), floor(格数×35%)]`（`game::clamp_size` / `game::mine_range`）。越界**不再报 `-32600`**，只有类型不对才由 `read_usize` 报错。坐标 `r/c` 仍按越界报 `-32600`（那是 UI 状态陈旧，不是可以替玩家决定的输入）
- 区间口径经 `system/info` 暴露（`minSide` / `maxSide` / `minePercentMin` / `minePercentMax`），前端 `game.js::mineRange` 用同一算法实时提示；**前端不自己抄常量**
- `prefs/set` 是**局部更新**（只覆盖传入的键），不是整体替换
- 未实现的能力（0.2.0 预留名）返回 `-32601`，不要返回"成功但空结果"

### 5.3 首点埋雷（核心行为，必须照做）

```
game/new          → status = ready，board_of_game 全 10，bbbv = null，真实局面尚未生成
game/reveal(r,c) 首次 →
    board = laymine_op(rows, cols, mineNum, r, c)   // rule=win7；仅首点非雷时用 laymine
                                                    // 返回时**已含数字**，不要再算一遍
    bbbv = cal_bbbv(&board)                         // 3BV 从此固定
    refresh_board(&board, &mut board_of_game, vec![(r, c)])
    started_at = Instant::now()
    status = playing
```

- 只有 `ready` 状态才允许埋雷；`playing` 之后的 `reveal` 不再触碰真实局面
- `rule` 变更或 `game/new` 重开 ⇒ 丢弃旧真实局面，回到 `ready`

### 5.4 压枪（chord）

```
game/chord(r, c):
  cells[r][c] ∈ 1..8            否则 -32011
  周围 8 格中旗数(=11) == cells[r][c]   否则 -32011（不合法压枪不响应，与扫雷一致）
  targets = 周围未打开(=10) 且未插旗的格
  targets 非空 → refresh_board(&board, &mut board_of_game, targets)
```

`is_good_chording(...)` 只用于 0.2.0 的提示，**不作为压枪前置校验**（玩家有权做局部最差压枪）。

### 5.5 存储（`datasweep.json`）

```json
{
  "version": 1,
  "prefs": { "preset": "expert", "custom": {"rows":16,"cols":30,"mineNum":99},
             "rule": "win7", "useMarks": true, "panel": {"records": true} },
  "records": { "16x30-99-win7": [
    {"elapsedMs":213456,"bbbv":152,"finishedAt":"2026-10-30T14:00:00+08:00"}
  ] }
}
```

- 路径：`<dataDir>/datasweep.json`；`dataDir` 按事实 13 三级回退
- **启动时必须先读**（翻译插件踩过的坑：只在运行时写、从不启动读，导致跨进程重启后回到空值/兜底位置）
- 写：**temp → rename** 原子替换；解析失败 ⇒ 备份成 `datasweep.json.corrupt-<ts>` 并用默认值重建，不静默丢、也不硬失败
- 文件明文、无凭据；卸载插件不自动删除（README 已声明，商店问答口径一致）

### 5.6 闸门脚本（只做校验，不做打包）

因为打包交给官方 CLI（事实 2），本项目**不需要**翻译插件那种自写 `_buildpkg.js`。三个校验脚本：

| 脚本 | 检查项 |
|---|---|
| `_verify.mjs` | 解包 `.dbxp`：manifest 通过 schema 字段白名单（一次性报完所有问题，宿主只报第一个）；`entrypoints.backend.executable` 不带 `.exe` 但包内 `bin/` 有对应可执行文件（Windows 目标带 `.exe`）；`manifest.version`/`id` 与侧车自报一致；包内 `.json/.js/.css/.html/.svg/.md/.toml` **无 `\r\n`**；资源文件名全小写；`.dbxp` 与 `.artifact.json` 的 sha256/size 对得上；UI 源码（剥注释后）不含 `localStorage`；根目录存在 `LICENSE` |
| `_e2e.mjs` | spawn 已构建侧车，走 JSON Lines：`plugin/initialize` 协商 → `game/new`(ready) → 首点 `game/reveal`（断言首点邻域无雷、`bbbv` 非 null）→ 连点 → `game/chord` → `game/flag` 循环 → 终局 → `records/list` 有新纪录 → **重启进程** → `records/list` 仍在 → `-32601`/`-32600`/`-32011` 三类错误 → 并发 32 请求不串响应 |
| `_domcheck.py` | `index.html` 里被 JS 引用的每个 `id` 都存在；`ui/*.js` 不使用 `localStorage`/`sessionStorage`/`indexedDB`（扫描前先剥注释，否则注释里解释"为什么不用"会被自己的闸门判违规）；文件均 UTF-8 无 BOM |

**判据一致性要求**：`_verify.mjs` 与 `_domcheck.py` 的"剥注释 + 禁用 API"规则必须完全一致，否则出现"本地过、CI 红"。

---

## 六、开发演进路线

### 阶段 0：地基（0.5–1 天）—— 全部是去风险动作，顺序不可调

1. **装 Rust**：`winget install --id Rustlang.Rustup` → `rustup default stable-msvc` → `cargo --version` 记录**精确版本号**（这个字符串要写进 `release.yml` 的 `rust-toolchain`，事实 15）
2. **生成官方骨架**：`dbx-plugin create .\tmp-scaffold --template rust --yes --id com.vyrnax.datasweep --name "扫雷" --publisher vyrnax --version 0.1.0`，把生成的文件搬进仓库根（仓库已有 `.git`，`create` 未必接受就地生成），完成后删除临时目录
   - 逐项**读**生成的 `backend/Cargo.toml`、`backend/src/main.rs`、`dbx-plugin.toml`、`.github/workflows/*.yml`：SDK 依赖写法、`[profile.release]`、workflow 传参一律以生成物为准（事实 1、2、17）
3. **`ms_toollib` feature 探针**（决定包体与 CI 时长，必须留档）：
   ```powershell
   cargo add ms_toollib --no-default-features --features rand,web-time --manifest-path backend\Cargo.toml
   cargo check --manifest-path backend\Cargo.toml
   ```
   - 通过 ⇒ 结果 **A**，保留该写法
   - 失败 ⇒ 改 `cargo add ms_toollib`（默认 `rs`，含 `tract-onnx`），结果 **B**，并在本节末记录"包体/构建时间已接受"
   - 无论 A/B，再写一段临时冒烟代码调用 `laymine_op` + `cal_board_numbers` + `cal_bbbv` + `refresh_board`，**把 3BV 与手工核对结果回写进本文件**（阶段 1 的断言基线），然后删掉临时冒烟代码
   - 顺带 `cargo tree` 记录依赖数，作为日后是否值得 vendor 精简的判断依据（当前决定：不 vendor）
4. **本机出包 + 开发宿主**：`cargo clippy --manifest-path backend\Cargo.toml -- -D warnings`、`dbx-plugin package .` 出 `windows-x64`、`dbx-plugin dev --path . --port 5190` 里把模板 UI 打开看一眼
5. **真机冒烟**：把 `dist/*.dbxp` 装进本机 DBX 桌面端，确认侧车被拉起（`system/info` 返回）——**这一步验证的是"宿主接受我们的 manifest 与身份"，越早越好**
6. **`.gitattributes` 强制 LF**：`*.json *.js *.css *.html *.svg *.md *.toml text eol=lf`；`*.dbxp -text`
7. **提交初始 commit**（空仓库首个提交直接进 `main`；后续特性分支用 `codex/` 前缀）

> **出口判据**：`cargo test` 能跑、`dbx-plugin package .` 出包、本机 DBX 里能看到模板工作台且侧车应答。任一不成立**不要进阶段 1**。

### 阶段 1：侧车内核（3–5 天）→ 可被 e2e 驱动的完整游戏逻辑

顺序：`paths.rs` → `store.rs` → `msbridge.rs` → `game.rs` → `snapshot.rs` → `rpc.rs` → `main.rs`（身份反读放最后，因为它依赖前面都能跑）。
交付判据：`cargo test` 全绿（含第七节单测）、`_e2e.mjs` 全绿、`dbx-plugin dev` 里能用控制台命令玩通一局。

### 阶段 2：Canvas 与交互手感（5–8 天）—— **全项目最大风险，优先打磨**

`canvas.js` 渲染（含全部编码状态与 DPR）→ `input.js`（连点、压枪、拖拽插旗、`contextmenu`）→ `game.js` 串行队列 → `panels.js`。
交付判据：高速连点不丢不重；压枪不误判为双击；拖拽插旗不重复切换同一格；50×50 不掉帧。

### 阶段 3：偏好、成绩与外观收尾（2–3 天）

设置面板 + `prefs/*`、成绩面板 + `records/*`、主题与 i18n、`assets/plugin.svg`（自绘：雷 + 旗 + 网格意象，**禁止使用元扫雷素材**）。

### 阶段 4：闸门、CI/CD 与上架（2–3 天）

`_e2e.mjs` / `_verify.mjs` / `_domcheck.py` → `ci.yml` → `release.yml` → `LICENSE`（Apache-2.0 全文）→ `RELEASE_NOTES.md`（只写本版）→ `.dbx-store.json` → 打 `v0.1.0` → CI 出 5 target → 提 `t8y2/dbx-store` 候选 PR。

**`ci.yml` 结构**（两个 job；Rust 无 Go 那类 `-race` 需要 cgo 的特例）：

| job | steps |
|---|---|
| `test`（ubuntu-24.04） | 装精确 `rust-toolchain`（与 release.yml **同一字符串**）→ `cargo fmt --check` → `cargo clippy --all-targets -- -D warnings` → `cargo test` → `cargo build --release` → `node _e2e.mjs ./backend/target/release/dbx-plugin-datasweep` → `python _domcheck.py` |
| `package-windows`（windows-2022） | `npm i -g @dbx-app/plugin-cli@0.1.9` → 装同一 `rust-toolchain` → `dbx-plugin package .` → `node _verify.mjs dist/*.dbxp` → `upload-artifact` 收 `dist/*.dbxp` + `dist/*.artifact.json` |

**`release.yml` 结构**（照翻译插件骨架，删掉 Go/cgo/交叉编译相关注释与 `go-version`，改为官方矩阵）：

1. `prepare`：解析并校验 tag；校验 `tag == "v" + manifest.version`；创建/转正 Release（**非 draft、非 prerelease**）；`workflow_dispatch` 的 `release-tag` 输入必须经 env 传递并走白名单正则 `^v[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9.-]+)?$`（防表达式注入）
2. `build`：`uses: t8y2/dbx/.github/workflows/plugin-release-reusable.yml@plugin-sdk-v1`，
   `plugin-cli-version: "0.1.9"`、`rust-toolchain: "<阶段 0 记录的精确版本>"`、`go-version: ""`、`install-plugin-cli: true`、`package-command: dbx-plugin package .`、`build-matrix` 用官方默认（5 target）；`concurrency.cancel-in-progress: false`
3. `upload-metadata`：补传各 target `.artifact.json`，数据源用 run artifact `dbx-plugin-*`（不要从 Release 下载通配符——publish job 不上传它们）
4. `summary`：打印 target/size/sha256、`release-candidates.json`、CI run 链接与商店 PR 指引

> reusable workflow 的引用优先用 tag `@plugin-sdk-v1`；若只能用 `main` 这类浮动引用，则**pin 到 commit sha**，否则上游改动会让同一 tag 重建出不同字节。

### 阶段 5：0.2.0（无猜 + 概率，1–1.5 周）

`laymine_solvable*` 生成无猜局面（量级参考：元扫雷 3.3.2 声称 16×30/200 雷调整法埋雷最长可压到 200 ms 以下）→ `Space` 概率着色（`mark_board` + `cal_probability_onboard`）→ 若单次计算 >1 s 则加 `host.events` 流式进度并**同步 `.dbx-store.json`**。

---

## 七、测试计划

### 7.1 Rust 单元测试（`#[cfg(test)]` 内联）

- **首点保护**：`rule=win7` 下 1000 次采样，断言首点及其 8 邻域均非 `-1`；`rule=first-safe` 下断言首点非 `-1`
- **flood-fill**：角/边/整片开空三类，断言 `board_of_game` 打开集合与手工基线一致（基线取自阶段 0 冒烟记录）
- **压枪**：合法压枪打开周围；旗数≠数字时状态不变；周围全已开时返回 `-32011`
- **胜负**：全部非雷格已开 ⇒ `won`；踩雷 ⇒ `lost` 且 `15` 恰有一处、`14`/`16` 符合预期
- **3BV**：`game.rs` 结果与 `Board::new(b).get_bbbv()` 一致；且整局生命周期内不随操作变化
- **校验**：`rows/cols/mineNum` 越界、坐标越界 ⇒ `-32600`
- **存储**：prefs 局部更新不覆盖其他键；records 裁剪到 10 且升序；temp→rename 后旧文件不留；损坏 JSON ⇒ 备份 + 重建
- **泄露红线**：所有下发 `cells` 的值必须 ∈ `{0..8,10,11,12,14,15,16,18}`——**永远不出现 `-1`**（真实雷位不出侧车）

### 7.2 协议 e2e（`_e2e.mjs`，驱动真实二进制）

流程见 5.6。关键增量断言：`plugin/initialize` 返回的 `plugin.id`/`version` 与 `manifest.json` 逐字相等；`-32010` 语义正确（新进程查旧 gameId）。

### 7.3 包校验（`_verify.mjs`）

对 `dbx-plugin package` 产物做结构与字节校验（5.6）。**不重复实现打包逻辑。**

### 7.4 前端静态闸门（`_domcheck.py`）

DOM id 存在性、禁用 API、UTF-8 无 BOM、注释剥离后再扫描。

### 7.5 手工验收清单

| 场景 | 判据 |
|---|---|
| 真实 DBX 桌面端安装 `.dbxp` | 工作台打开、侧车应答、一局完整通关 |
| 明暗主题切换 | Canvas 与面板颜色跟随，无白底白字 |
| 宿主语言切英文 | UI 文案切 `en` |
| 连点 20 次 | 无动作丢失、无快照回退 |
| 拖拽插旗 30 格 | 每格恰好切一次 |
| 关窗后重开工作台 | prefs/records 仍在；本局按 `-32010` 提示作废 |
| 拖动分隔条 resize | Canvas 重算边长，不出现半个格子 |

---

## 八、风险与对策

| # | 风险 | 对策 |
|---|---|---|
| 1 | ~~本机无 Rust 工具链~~ → 已装 rustc/cargo **1.99.1**（stable-msvc），与 CI 的 `rust-toolchain` 同值 | 剩余注意：新装的用户 PATH 要新开 shell 才生效 |
| 2 | 本地出不了全平台包（与 Go 经验不同，极易误判成 bug） | 事实 4 写死：本地只 `windows-x64`，其余靠 CI 默认矩阵；AGENTS.md 5.4 同样交代 |
| 3 | `ms_toollib` 关不掉 `tract-onnx`（`web_time` 有非 gated 引用） | 阶段 0 探针 A/B 分支已定；B 情况下接受体积与时间，**不**转 vendor fork |
| 4 | 手改 SDK 依赖写法导致打包失败 | 事实 3 + 4.1 纪律：保持模板生成写法；`Cargo.lock` 必须提交 |
| 5 | 高频点击的 IPC 竞态 / 事件丢失 | 前端 `game/*` 单在途 + FIFO 队列（4.5）；压枪用 `e.buttons===3` 不用 `dblclick`；`contextmenu` preventDefault |
| 6 | 侧车被宿主回收导致本局丢失 | 只保证 prefs/records 落盘；`-32010` 明确提示"侧车已重启，本局作废"，不假装能恢复 |
| 7 | GPL-3.0 + 商业限制污染 | 事实 23 红线：零搬运 Metasweeper 源码/素材/文案；只用 MIT 的 `ms_toollib`；README「许可」节与 AGENTS.md 6.4 公开声明无衍生关系 |
| 8 | manifest 字段被宿主拒绝（且只报第一个错） | `_verify.mjs` 内置 schema 字段白名单闸门，打包前一次报完；`menus` 不带 `label` |
| 9 | 包内 CRLF / 大写资源名 / 缺 `LICENSE` 被商店拦 | 事实 14 + `.gitattributes` + `_verify.mjs` 闸门；`RELEASE_NOTES.md` 只写本版且首行版本号必须等于 `manifest.version` |
| 10 | Rust 无内建 VCS 戳，provenance 被审核质疑 | 事实 15 的四条自证 + summary 打印 + 可选 `DATASWEEP_BUILD_REV`（4.4），候选 PR 附 CI run 链接 |
| 11 | `darwin-x64`（`macos-15-intel`）runner 下线 | 失败时临时降为 4 target 并在候选说明注明，**不**擅自改用交叉编译路线 |
| 12 | iframe 内键盘与宿主快捷键冲突 / 焦点丢失 | 快捷键只在工作台获焦时生效；焦点态由 `game.js` 状态机持有，不挂全局 `document` 监听 |
| 13 | 显示名「扫雷」与商店已有条目撞名 | 提 PR 前检索 `t8y2/dbx-store`；冲突则改「DataSweep 扫雷」，但 `manifest.id` 不动（改动 ID 会被宿主视为新入口，排序记录丢失） |
| 14 | 大尺寸自定义局概率计算慢（0.2.0） | 概率功能限死在预设三档 + 雷数上限；超出则禁用按钮并说明，而不是让它卡住 |

---

## 九、假设与默认值（用户未明说时按此执行，不再询问）

| 项 | 默认 |
|---|---|
| 插件身份 | `com.vyrnax.datasweep` / 「扫雷」/ `publisher: vyrnax` / Apache-2.0 / 起始 `0.1.0` |
| `engines.dbx` | `">=0.6.21"`（沿用已上架插件的实测门槛）；`host_api: "1"` |
| `permissions` | 空数组 `[]` |
| 预设难度 | 初 9×10/10、中 16×16/40、高 16×30/99；默认档位取**中级**（宿主是办公工具，高级开局对摸鱼场景过重；如后续要改只动 `prefs` 默认值一处） |
| 自定义尺寸边界 | `rows/cols ∈ [5,50]`，`mineNum ∈ [ceil(格数×10%), floor(格数×35%)]`，越界自动夹取 |
| 首点规则 | 默认 `win7`（`laymine_op`） |
| 成绩 | 每维度键 Top 10，按用时升序 |
| 存储 | 单文件 `<dataDir>/datasweep.json`，temp→rename 原子写，明文无凭据，卸载不自动清除 |
| 计时 | 首点启动、侧车权威、终局冻结 |
| Rust | `stable-msvc`；`edition = "2021"`（与 SDK/模板一致）；`[profile.release]` 保持模板值（`strip`/`lto`/`codegen-units=1`，**不改 panic**） |
| CI | `plugin-cli-version: "0.1.9"`；`rust-toolchain` pin 阶段 0 实际精确版本；`go-version: ""`；矩阵用官方默认 5 target |
| 分支 | 首个提交直接进 `main`（空仓库无对比基线）；后续特性分支用 `codex/` 前缀 |
| 代码风格 | 交给 IDE 自动格式化，不在文档里规定大括号/缩进 |

---

## 十、当前进度与下一动作

阶段 0–4 已落地：侧车 7 个模块、前端 8 个文件、四个闸门脚本、`ci.yml` / `release.yml`。

### 已验证（本机全绿）

| 项 | 结果 |
|---|---|
| `cargo test` | **61 passed / 0 failed**，且连跑 40 轮无随机翻车 |
| `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` | 通过 / 零警告 |
| `node _e2e.mjs` | 58/58 |
| `python _domcheck.py` | 通过 |
| `python _releasecheck.py` | 通过，且 16 个反向用例全部能被抓到 |
| `dbx-plugin package .` + `node _verify.mjs` | 通过，包内 14 文件 |
| `node _storecandidate.mjs` | CI 形态（`--from release-candidates.json`）与本地形态都验过 |
| **真机（dev 宿主 + CDP）** | 明暗跟随生效、option 深色可读、LED 不被拉伸、笑脸居中、图标不再裂、点一格后 3BV/计时正常 |

### 本轮抓到并修掉的缺陷（都补了回归守卫）

1. **旗格左键报"侧车内部错误"** → 改无反应；插旗后计时被重置 → 新增 `Game::tick()`。
2. **业务错误码全被吞成 -32603** → 宿主对失败是**正常返回 `{error}`** 而非 reject，`bridge.js` 补解包。
3. **宿主超时也显示"侧车内部错误"** → 超时 reject 的是不带 code 的纯 Error，归一到 `-32001` 让 `errTimeout` 真正生效。
4. **主题/语言根本不跟随宿主**（静默失效）→ 原先从 `context.theme` 读，而 dev-runtime 的 context 里**没有 theme 字段**；改读 `dbxPlugin.theme` + `dbx-plugin-env`（window/document 双注册 + capture）+ `onInit` + 1.5s 轮询兜底。
5. **图标在宿主里全是裂图** → 宿主 CSP 是 `img-src data: blob:` 且**不内联 `<img src>`**；改内联 SVG（与已上架的翻译插件一致）。
6. **LED 被 grid 的 `1fr` 拉伸成 531px 黑带** → `justify-self` + `width: max-content`。
7. **打 tag 不会触发 release** → `on.push.tags` 写成了正则，而 GitHub 这里用 **glob**。
8. **reusable workflow 的 `@plugin-sdk-v1` 已从上游消失** → 改 pin commit sha（与翻译插件同一个）。
9. **`.artifact.json` 用 bash glob 查找会匹配不到** → merge-multiple 平铺 + 未开 globstar；改 `find`。
10. **不稳定测试**：`完整一局能通关并写出成绩` 的断言既过时（`mineNum==1`，实际夹成 3）又被 `if status == "won"` 包着、平时根本不执行 → 拆成"夹取口径 + 记账不变量 + game.rs 里用真实雷位做确定性通关"三条。

### 剩下的

1. **桌面端真机验收**：本轮只覆盖了 `dbx-plugin dev`。字体跟随（`--font-*`）与浮动小组件等只能在装了 DBX 桌面端的机器上验（开发宿主实测只投递 15 个 token：13 个 `--color-*` + 2 个 `--radius-*`，**不含 `--font-*`**）。
2. **提交并推 `v0.1.0`**：CI 用官方矩阵真机产出其余 4 个 target；确认 Release 是非 draft 非 prerelease 且 `--latest`。
3. **提商店候选 PR** 到 `t8y2/dbx-store:main`：只需 `candidates/com.vyrnax.datasweep.json`（取自 CI 的 `dbx-store-candidate` artifact）；`publishers/vyrnax.json` **已存在**，不用重复提交。提 PR 前先检索「扫雷」是否撞名。
4. 0.2.0：无猜模式 + 空格概率着色（接口名已预留）。
