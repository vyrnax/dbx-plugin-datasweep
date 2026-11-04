//! 侧车入口：装配身份并起服务。
//!
//! 进程规则（PLAN 事实 10）：stdout 只留给协议，诊断一律写 stderr。

mod game;
mod msbridge;
mod paths;
mod rpc;
mod snapshot;
mod store;

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use dbx_plugin_sdk::{PluginMetadata, PluginServer};
use serde_json::Value;

use crate::rpc::{Sweeper, PLUGIN_ID};

/// 自可执行文件路径向上最多找 6 层，从包内的 `manifest.json` 反读版本号。
///
/// 侧车自报的 id/version 必须与 `manifest.json` **逐字一致**，否则宿主在
/// `plugin/initialize` 阶段静默拒绝后端（UI 上只表现为"什么都不工作"，宿主不报错）。
/// 只有 crate 内的版本号参与编译，包清单是运行时唯一真相，所以这里以包清单为准。
fn resolve_manifest_version() -> String {
    let fallback = env!("CARGO_PKG_VERSION").to_string();
    let Ok(exe) = env::current_exe() else {
        eprintln!("[datasweep] 定位不到可执行文件路径，回退到编译期版本 {fallback}");
        return fallback;
    };
    let Some(mut dir) = exe.parent().map(Path::to_path_buf) else {
        return fallback;
    };
    for _ in 0..6 {
        if let Some(version) = version_in(&dir.join("manifest.json")) {
            return version;
        }
        if !dir.pop() {
            break;
        }
    }
    eprintln!("[datasweep] 向上 6 层内没找到本插件的 manifest.json，回退到编译期版本 {fallback}");
    fallback
}

/// 只有 `id` 对得上才采用该清单，避免误读父目录里别的插件。
fn version_in(file: &Path) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let manifest: Value = serde_json::from_str(&text).ok()?;
    if manifest.get("id").and_then(Value::as_str) != Some(PLUGIN_ID) {
        return None;
    }
    let version = manifest.get("version").and_then(Value::as_str)?;
    if version.is_empty() {
        return None;
    }
    Some(version.to_string())
}

fn main() -> io::Result<()> {
    let version = resolve_manifest_version();
    let data_dir: PathBuf = paths::data_dir(PLUGIN_ID);
    eprintln!(
        "[datasweep] 启动：id={PLUGIN_ID} version={version} dataDir={}",
        data_dir.display()
    );
    let metadata = PluginMetadata::new(PLUGIN_ID, version);
    PluginServer::new(metadata, Sweeper::new(data_dir)).serve()
}
