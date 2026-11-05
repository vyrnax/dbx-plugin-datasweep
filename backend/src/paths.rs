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
/// 第 4 级只是兜底：实测 `dbx-plugin dev` 下取到的是第 3 级
/// （`%APPDATA%\dbx\plugins\<id>`），而 dev-runtime 全文并没有设置
/// `DBX_PLUGIN_DATA_DIR` / `DBX_PLUGIN_SPACE`。所以真实宿主到底走第几级
/// **不要臆断**，前两级留空是常态路径之一。
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

    /// `config_dir()` / `home()` 会读到的全部变量。
    /// 测试前后都要完整保存并复原，否则会污染同进程里其他用例。
    const CONFIG_KEYS: [&str; 4] = ["APPDATA", "XDG_CONFIG_HOME", "HOME", "USERPROFILE"];

    /// 本平台 `config_dir()` 实际会读的那个变量，以及一个假的配置根目录。
    ///
    /// 关键：**显式给出**，而不是"看看环境里恰好有没有"。原先的用例是
    /// `if cfg!(windows) && APPDATA 存在 {…} else { 断言 ./data }`，
    /// 而 Linux runner 上 `HOME=/home/runner` 必然存在 ⇒ 实际落到第 3 级，
    /// 那句 `assert_eq!(dir, "./data")` 就红了（CI 实测，本机 Windows 复现不了）。
    /// 也就是说那个用例从来没验到第 3 级与第 4 级的分界。
    fn platform_config_source() -> (&'static str, std::path::PathBuf) {
        if cfg!(windows) {
            ("APPDATA", PathBuf::from("C:/fake-roaming"))
        } else if cfg!(target_os = "macos") {
            // macOS 分支走 home()，读 HOME / USERPROFILE
            ("HOME", PathBuf::from("/fake-home"))
        } else {
            ("XDG_CONFIG_HOME", PathBuf::from("/fake-xdg"))
        }
    }

    fn hold(keys: &[&'static str]) -> Vec<(&'static str, Option<String>)> {
        keys.iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect()
    }

    fn release(saved: Vec<(&'static str, Option<String>)>) {
        for (key, value) in saved {
            match value {
                Some(text) => std::env::set_var(key, text),
                None => std::env::remove_var(key),
            }
        }
    }

    /// 清空所有配置目录来源，让 `config_dir()` 必然返回 None ⇒ 只剩第 4 级。
    fn blind_config_dirs() -> Vec<(&'static str, Option<String>)> {
        let saved = hold(&CONFIG_KEYS);
        for key in CONFIG_KEYS {
            std::env::remove_var(key);
        }
        saved
    }

    #[test]
    fn 前两级为空时落到第3级平台配置目录() {
        let _guard = lock_env();
        let saved = hold(&CONFIG_KEYS);
        let first = set_temp("DBX_PLUGIN_DATA_DIR", "");
        let second = set_temp("DBX_PLUGIN_SPACE", "");

        let (key, root) = platform_config_source();
        std::env::set_var(key, &root);

        // 三个平台断言同一条：落在 <平台配置目录>/dbx/plugins/<id>
        let want = if cfg!(target_os = "macos") {
            root.join("Library")
                .join("Application Support")
                .join("dbx")
                .join("plugins")
                .join("com.vyrnax.datasweep")
        } else {
            root.join("dbx")
                .join("plugins")
                .join("com.vyrnax.datasweep")
        };
        assert_eq!(data_dir("com.vyrnax.datasweep"), want);

        restore("DBX_PLUGIN_DATA_DIR", first);
        restore("DBX_PLUGIN_SPACE", second);
        release(saved);
    }

    #[test]
    fn 连平台配置目录都拿不到时才兜底到_data() {
        let _guard = lock_env();
        let saved = blind_config_dirs();
        let first = set_temp("DBX_PLUGIN_DATA_DIR", "");
        let second = set_temp("DBX_PLUGIN_SPACE", "");

        assert_eq!(data_dir("com.vyrnax.datasweep"), PathBuf::from("./data"));

        restore("DBX_PLUGIN_DATA_DIR", first);
        restore("DBX_PLUGIN_SPACE", second);
        release(saved);
    }

    #[test]
    fn 第一级优先于平台配置目录() {
        // 显式给出第 1 级时，无论环境里有没有 APPDATA/HOME 都必须用它 ——
        // 这条以前是隐式覆盖的，现在钉死，免得回退顺序被改错而无人发现。
        let _guard = lock_env();
        let saved = blind_config_dirs();
        let first = set_temp("DBX_PLUGIN_DATA_DIR", "/fake/explicit-dir");
        assert_eq!(
            data_dir("com.vyrnax.datasweep"),
            PathBuf::from("/fake/explicit-dir")
        );
        restore("DBX_PLUGIN_DATA_DIR", first);
        release(saved);
    }
}
