# Vendored: DBX Plugin SDK for Rust

本目录是**官方** DBX Plugin SDK for Rust 的**字节级复制**。**禁止修改**——任何本地改动都可能悄悄偏离宿主的协议预期，而这类偏离的表现是「宿主静默丢弃侧车」，UI 上只看到"什么都不工作"，极难排查。

## 来源

`@dbx-app/plugin-cli` npm 包内：

```
sdk-root/plugins/sdk/rust/dbx-plugin-sdk/
```

本机绝对路径随 npm 全局目录而定，**不要照抄别人的路径**，自己查：

```powershell
Join-Path (npm root -g) '@dbx-app/plugin-cli/sdk-root/plugins/sdk/rust/dbx-plugin-sdk'
```

上游仓库位置：`t8y2/dbx` → `plugins/sdk/rust/dbx-plugin-sdk`。

## 校验和（复制时核算，升级后必须更新此处）

| 文件 | SHA-256 |
|---|---|
| `Cargo.toml` | `9EA0FBBEE3D9F4278D60500FCA422467A82DB6276790117B71F31A34CC6E6722` |
| `src/lib.rs` | `6F5146A9892C8850BA593BD034591CCFC76C528FFDBDA5984EF700584A0D14C7` |

## 为什么 vendor 而不是照抄模板的写法

`dbx-plugin-sdk` **没有发布到 crates.io**（查 `dbx-plugin-sdk` / `dbx_plugin_sdk` / `dbx-plugin` / `dbx-plugin-host` 全 404）。官方 `dbx-plugin create --template rust` 生成的 `Cargo.toml` 写的是

```toml
dbx-plugin-sdk = { version = "0.1.0" }
```

这条依赖**只有在 `dbx-plugin package` 的调用链里才能解析** —— packager 会给 cargo 追加

```
--config 'patch.crates-io.dbx-plugin-sdk.path="<SDK_ROOT>/plugins/sdk/rust/dbx-plugin-sdk"'
```

于是裸 cargo 调用全线失败：

```
error: no matching package named `dbx-plugin-sdk` found
location searched: crates.io index
```

`cargo test` 跑不了 = 单元测试与 e2e 都无法在本机或 CI 建立，这是硬阻塞。

**处理方式**：保留模板那行 `version = "0.1.0"`（不违反上游约定、也不影响 packager），另用 `.cargo/config.toml` 的 `[patch.crates-io]` 指向本目录来补裸 cargo 调用。命令行 `--config` 优先级高于配置文件（已实测：追加 packager 那条 --config 后解析到 CLI 自带 SDK，而非本 vendor），所以**打包路径仍然完全由上游 SDK 决定**，vendor 只服务于测试/构建。

与 Go 插件的 vendor 做法同源（翻译插件 `backend/dbxsdk/` 同样字节级复制 + VENDOR.md，理由之一是上游未发布到 module proxy）。

## 升级方式

重装/升级 `@dbx-app/plugin-cli` 后整体覆盖复制，并核对与上游一致：

```powershell
# SDK 路径机器相关：用 npm root -g 取自己机器上的位置
$sdk = Join-Path (npm root -g) "@dbx-app/plugin-cli/sdk-root/plugins/sdk/rust/dbx-plugin-sdk"
fc.exe /b "$sdk\Cargo.toml" "backend\vendor\dbx-plugin-sdk\Cargo.toml"
fc.exe /b "$sdk\src\lib.rs"  "backend\vendor\dbx-plugin-sdk\src\lib.rs"
```

有差异时**整体覆盖**并更新上面的 SHA-256。**不要手工合并。**

## 许可

Apache-2.0（见上游 `Cargo.toml`）。本插件同样 Apache-2.0，兼容。
