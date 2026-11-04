//! 侧车数据目录的三级回退（PLAN 事实 13）。
//!
//! Rust 标准库没有 `UserConfigDir`，而 PLAN 4.1 明确不引入 `dirs` / `directories`
//! crate——少一个依赖就少一个跨平台构建面，所以这里用 `std::env` 手工拼。

use std::path::PathBuf;

/// 三级回退，依次取第一个非空的：
///
/// 1. `DBX_PLUGIN_DATA_DIR`
/// 2. `DBX_PLUGIN_SPACE`
/// 3. 平台配置目录下的 `dbx/plugins/<plugin_id>`
/// 4. `./data`
///
/// 第 4 级是兜底而不是常态：开发宿主会给出前两级之一。
pub fn data_dir(plugin_id: &str) -> PathBuf {
    for key in ["DBX_PLUGIN_DATA_DIR", "DBX_PLUGIN_SPACE"] {
        if let Some(dir) = env_or_blank(key) {
            return PathBuf::from(dir);
        }
    }
    if let Some(config) = config_dir() {
        return config.join("dbx").join("plugins").join(plugin_id);
    }
    PathBuf::from("./data")
}

/// 平台配置目录：Windows `%APPDATA%`、macOS `~/Library/Application Support`、
/// Linux `$XDG_CONFIG_HOME`（未设则 `~/.config`）。
fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return env_or_blank("APPDATA").map(PathBuf::from);
    }
    if cfg!(target_os = "macos") {
        return home().map(|home| home.join("Library").join("Application Support"));
    }
    if let Some(xdg) = env_or_blank("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg));
    }
    home().map(|home| home.join(".config"))
}

fn home() -> Option<PathBuf> {
    env_or_blank("HOME")
        .or_else(|| env_or_blank("USERPROFILE"))
        .map(PathBuf::from)
}

/// 空串视作未设置——宿主把变量设成空是很常见的，若据此建目录会落到cwd下。
fn env_or_blank(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// 本组测试会改**进程级**环境变量，并行跑必然互相踩（实测：A 设的
    /// `DBX_PLUGIN_DATA_DIR` 会被 B 看到，断言随机翻车）。CI 靠
    /// `--test-threads=1` 侥幸掩盖，本机 `cargo test` 一并行就红 ——
    /// 所以这里自己上锁串行，并在用例结束后复原现场。
    fn lock_env() -> MutexGuard<'static, ()> {
        // Mutex::new 不是 const fn，而 const_new 仍未稳定，
        // 所以静态量只能借 OnceLock 懒初始化。
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let mutex = ENV_LOCK.get_or_init(|| Mutex::new(()));
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 临时设置环境变量，返回旧值以便复原；`None` 表示原本未设置。
    fn set_temp(key: &str, value: &str) -> Option<String> {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        previous
    }

    fn restore(key: &str, previous: Option<String>) {
        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn 第一级优先于第二级() {
        let _guard = lock_env();
        let first = set_temp("DBX_PLUGIN_DATA_DIR", "C:/tmp/one");
        let second = set_temp("DBX_PLUGIN_SPACE", "C:/tmp/two");
        assert_eq!(
            data_dir("com.vyrnax.datasweep"),
            PathBuf::from("C:/tmp/one")
        );
        restore("DBX_PLUGIN_DATA_DIR", first);
        restore("DBX_PLUGIN_SPACE", second);
    }

    #[test]
    fn 第一级为空串时落到第二级() {
        let _guard = lock_env();
        let first = set_temp("DBX_PLUGIN_DATA_DIR", "   ");
        let second = set_temp("DBX_PLUGIN_SPACE", "C:/tmp/two");
        assert_eq!(
            data_dir("com.vyrnax.datasweep"),
            PathBuf::from("C:/tmp/two")
        );
        restore("DBX_PLUGIN_DATA_DIR", first);
        restore("DBX_PLUGIN_SPACE", second);
    }

    #[test]
    fn 前两级都为空时落到平台配置目录或当前目录() {
        let _guard = lock_env();
        let first = set_temp("DBX_PLUGIN_DATA_DIR", "");
        let second = set_temp("DBX_PLUGIN_SPACE", "");
        let dir = data_dir("com.vyrnax.datasweep");
        if cfg!(windows) && std::env::var("APPDATA").is_ok() {
            assert!(dir.ends_with("dbx/plugins/com.vyrnax.datasweep"));
        } else {
            assert_eq!(dir, PathBuf::from("./data"));
        }
        restore("DBX_PLUGIN_DATA_DIR", first);
        restore("DBX_PLUGIN_SPACE", second);
    }
}
