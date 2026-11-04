//! JSON-RPC 方法分发、参数校验与错误码。
//!
//! RPC 语义与返回结构以 `README.md` 第 5 节为准；这里只负责把它们接进来。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use dbx_plugin_sdk::{PluginEmitter, PluginError, PluginHandler, RequestContext};
use serde_json::{json, to_value, Value};

use crate::game::{self, FlagMode, Rule, Sessions};
use crate::snapshot;
use crate::store::{self, PanelPrefs, Size, Store};

pub const PLUGIN_ID: &str = "com.vyrnax.datasweep";
/// `Cargo.toml` 里锁定的算法库版本，随依赖升级手工同步
pub const MS_TOOLLIB_VERSION: &str = "1.5.16";

const ERR_INVALID: i32 = -32600;
const ERR_INTERNAL: i32 = -32603;
const ERR_SESSION: i32 = -32010;
const ERR_STATE: i32 = -32011;
const ERR_ALGORITHM: i32 = -32013;
const ERR_STORAGE: i32 = -32014;

pub struct Sweeper {
    sessions: Arc<Mutex<Sessions>>,
    store: Arc<Mutex<Store>>,
    data_dir: PathBuf,
}

impl Sweeper {
    /// 启动时就读盘（翻译插件踩过：只写不读 ⇒ 跨进程重启后回到兜底值）。
    /// 读盘失败不硬失败——带着内存里的默认值起来，诊断写 stderr。
    pub fn new(data_dir: PathBuf) -> Self {
        let store = store::load(&data_dir).unwrap_or_else(|error| {
            eprintln!(
                "[datasweep] 读取 {} 失败（{}），本次使用默认值",
                data_dir.display(),
                error.message()
            );
            Store::default()
        });
        Self {
            sessions: Arc::new(Mutex::new(Sessions::new())),
            store: Arc::new(Mutex::new(store)),
            data_dir,
        }
    }

    fn sessions(&self) -> MutexGuard<'_, Sessions> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn persist(&self) -> Result<(), PluginError> {
        let store = self.store();
        store::save(&self.data_dir, &store).map_err(|error| storage_error(error.message()))
    }

    fn game_error(error: game::Error) -> PluginError {
        let (code, message) = match error {
            game::Error::Session => (ERR_SESSION, error.message()),
            game::Error::Invalid(message) => (ERR_INVALID, message),
            game::Error::State(message) => (ERR_STATE, message),
            game::Error::Algorithm(message) => (ERR_ALGORITHM, message),
        };
        PluginError::new(code, message)
    }

    fn prefs_value(&self) -> Result<Value, PluginError> {
        to_value(self.store().prefs.clone()).map_err(internal)
    }

    fn snapshot_value(&self) -> Result<Value, PluginError> {
        let mut sessions = self.sessions();
        let game = sessions.current().map_err(Self::game_error)?;
        // 出快照前先刷新权威用时：`game/flag` 与 `game/state` 都走这条路径，
        // 不 tick() 就会把上一次翻格时刻的陈旧用时发出去（前端表现为"秒表被重置"）。
        game.tick();
        let snapshot = snapshot::from_game(game);
        drop(sessions);
        // 泄露红线在每次下发前都过一遍，成本远低于上线后把答案发给前端。
        // 报错而不是 panic：panic 会让侧车进程直接消失，连错误都发不出去。
        if let Some(value) = snapshot::leaked_cell(&snapshot) {
            eprintln!("[datasweep] 泄露红线：快照含非法格值 {value}，已拒绝下发");
            return Err(PluginError::new(
                ERR_INTERNAL,
                format!("内部错误：快照含非法格值 {value}"),
            ));
        }
        to_value(snapshot).map_err(internal)
    }

    /// 翻格 / 压枪之后：把可能产生的胜利成绩写盘，再返回全量快照。
    fn settle(&self) -> Result<Value, PluginError> {
        let claimed = {
            let mut sessions = self.sessions();
            match sessions.current() {
                Ok(game) => game
                    .claim_win()
                    .map(|(elapsed_ms, bbbv)| (game.record_key(), elapsed_ms, bbbv)),
                Err(_) => None,
            }
        };
        if let Some((key, elapsed_ms, bbbv)) = claimed {
            let finished_at =
                store::now_rfc3339().map_err(|error| storage_error(error.message()))?;
            let mut store = self.store();
            store::push_record(
                &mut store,
                &key,
                store::Record {
                    elapsed_ms,
                    bbbv,
                    finished_at,
                },
            );
            drop(store);
            self.persist()?;
        }
        self.snapshot_value()
    }

    fn new_game(&self, params: &Value) -> Result<Value, PluginError> {
        let prefs = self.store().prefs.clone();
        let base = game::resolve_size(&prefs.preset, prefs.custom).map_err(Self::game_error)?;
        let rows = read_usize(params, "rows")?.unwrap_or(base.rows);
        let cols = read_usize(params, "cols")?.unwrap_or(base.cols);
        let mine_num = read_usize(params, "mineNum")?.unwrap_or(base.mine_num);
        // 尺寸一律夹取（行列 5..50、雷数 10%..35%），不再因越界报 -32600。
        let size = game::clamp_size(Size::new(rows, cols, mine_num));

        let rule = match read_str(params, "rule")? {
            Some(value) => Rule::parse(&value)
                .ok_or_else(|| PluginError::new(ERR_INVALID, format!("未知首点规则：{value}")))?,
            None => Rule::parse(&prefs.rule).ok_or_else(|| {
                PluginError::new(ERR_INTERNAL, format!("偏好里的规则不合法：{}", prefs.rule))
            })?,
        };

        self.sessions().create(size, rule);
        self.snapshot_value()
    }

    fn reveal(&self, params: &Value) -> Result<Value, PluginError> {
        let (Some(row), Some(col)) = (read_usize(params, "r")?, read_usize(params, "c")?) else {
            return Err(PluginError::new(ERR_INVALID, "缺少 r / c"));
        };
        let force = read_bool(params, "force")?.unwrap_or(false);
        self.sessions()
            .current()
            .map_err(Self::game_error)?
            .reveal(row, col, force)
            .map_err(Self::game_error)?;
        self.settle()
    }

    fn chord(&self, params: &Value) -> Result<Value, PluginError> {
        let (Some(row), Some(col)) = (read_usize(params, "r")?, read_usize(params, "c")?) else {
            return Err(PluginError::new(ERR_INVALID, "缺少 r / c"));
        };
        self.sessions()
            .current()
            .map_err(Self::game_error)?
            .chord(row, col)
            .map_err(Self::game_error)?;
        self.settle()
    }

    fn flag(&self, params: &Value) -> Result<Value, PluginError> {
        let (Some(row), Some(col)) = (read_usize(params, "r")?, read_usize(params, "c")?) else {
            return Err(PluginError::new(ERR_INVALID, "缺少 r / c"));
        };
        let mode =
            FlagMode::parse(&read_str(params, "mode")?.unwrap_or_else(|| "clear".to_string()))
                .ok_or_else(|| PluginError::new(ERR_INVALID, "mode 只支持 flag / mark / clear"))?;
        self.sessions()
            .current()
            .map_err(Self::game_error)?
            .flag(row, col, mode)
            .map_err(Self::game_error)?;
        self.snapshot_value()
    }

    fn set_prefs(&self, params: &Value) -> Result<Value, PluginError> {
        let mut prefs = self.store().prefs.clone();
        if let Some(preset) = read_str(params, "preset")? {
            if preset != "custom" && game::resolve_size(&preset, prefs.custom).is_err() {
                return Err(PluginError::new(
                    ERR_INVALID,
                    format!("未知难度档位：{preset}"),
                ));
            }
            prefs.preset = preset;
        }
        if let Some(rule) = read_str(params, "rule")? {
            Rule::parse(&rule)
                .ok_or_else(|| PluginError::new(ERR_INVALID, format!("未知首点规则：{rule}")))?;
            prefs.rule = rule;
        }
        if let Some(use_marks) = read_bool(params, "useMarks")? {
            prefs.use_marks = use_marks;
        }

        // 自定义尺寸支持逐个字段局部更新：既接受顶层写法，也接受 custom 子对象。
        let custom_source = params
            .get("custom")
            .cloned()
            .unwrap_or_else(|| params.clone());
        let mut custom = prefs.custom;
        if let Some(rows) = read_usize(&custom_source, "rows")? {
            custom.rows = rows;
        }
        if let Some(cols) = read_usize(&custom_source, "cols")? {
            custom.cols = cols;
        }
        if let Some(mine_num) = read_usize(&custom_source, "mineNum")? {
            custom.mine_num = mine_num;
        }
        prefs.custom = game::clamp_size(custom);

        if let Some(panel) = params.get("panel") {
            if let Some(records) = read_bool(panel, "records")? {
                prefs.panel = PanelPrefs { records };
            }
        }
        self.store().prefs = prefs;
        drop(self.store());
        self.persist()?;
        self.prefs_value()
    }

    fn list_records(&self) -> Result<Value, PluginError> {
        to_value(store::flatten(&self.store())).map_err(internal)
    }

    fn clear_records(&self, params: &Value) -> Result<Value, PluginError> {
        let key = read_str(params, "key")?;
        let mut store = self.store();
        let removed = store::clear(&mut store, key.as_deref());
        drop(store);
        self.persist()?;
        Ok(json!({ "removed": removed }))
    }

    fn info(&self) -> Result<Value, PluginError> {
        Ok(json!({
            "pluginVersion": env!("CARGO_PKG_VERSION"),
            "backendId": PLUGIN_ID,
            "targets": [format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)],
            "dataDir": self.data_dir.display().to_string(),
            "msToollibVersion": MS_TOOLLIB_VERSION,
            // **区间口径的唯一真相在这边。** 前端要靠它实时算"当前行列允许的雷数区间"
            // 并设置输入的 min/max；如果前端自己抄一份 10/35/5/50，改后端时必然漂移。
            "minSide": game::MIN_SIDE,
            "maxSide": game::MAX_SIDE,
            "minePercentMin": game::MINE_PERCENT_MIN,
            "minePercentMax": game::MINE_PERCENT_MAX,
        }))
    }

    /// 真正的分发。`PluginEmitter` 的字段是私有的，无法在单测里构造，
    /// 所以把可测的那一层单独拎出来，handler 只做转发。
    fn dispatch(&self, method: &str, params: &Value) -> Result<Value, PluginError> {
        match method {
            "system/info" => self.info(),
            "game/new" => self.new_game(params),
            "game/reveal" => self.reveal(params),
            "game/chord" => self.chord(params),
            "game/flag" => self.flag(params),
            "game/state" => self.snapshot_value(),
            "game/abandon" => {
                self.sessions().abandon();
                Ok(json!({ "ok": true }))
            }
            "prefs/get" => self.prefs_value(),
            "prefs/set" => self.set_prefs(params),
            "records/list" => self.list_records(),
            "records/clear" => self.clear_records(params),
            // 0.2.0 预留名也走这里，返回 -32601 而不是"成功但空结果"
            _ => Err(PluginError::method_not_found(method)),
        }
    }
}

impl PluginHandler for Sweeper {
    fn handle(
        &self,
        _context: RequestContext,
        method: &str,
        params: Value,
        _emitter: &PluginEmitter,
    ) -> Result<Value, PluginError> {
        // **为什么要自己包 catch_unwind。** 官方 rust 模板的注释写着
        // `panic stays "unwind" so the sidecar protocol can report panics as errors`，
        // 但 SDK 的 worker 里**根本没有 catch_unwind**（lib.rs 全文搜不到）。
        // worker 线程 panic 的后果是那条响应永远写不出去：宿主只看到侧车莫名重启、
        // 本局凭空作废、前端干等到超时 —— 正是最难排查的一类现象。
        // 模板许下的约定只能由我们自己兑现。
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.dispatch(method, &params)
        }));
        match outcome {
            Ok(result) => result,
            Err(payload) => {
                let detail = panic_detail(&payload);
                eprintln!("[datasweep] handler panic：method={method} {detail}");
                Err(PluginError::new(
                    ERR_INTERNAL,
                    format!("侧车内部错误：{detail}"),
                ))
            }
        }
    }
}

/// 从 panic payload 里尽量取一句人话（`String` / `&str` 两种形态）。
fn panic_detail(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_string();
    }
    "未知 panic".to_string()
}

fn internal(error: serde_json::Error) -> PluginError {
    PluginError::new(ERR_INTERNAL, error.to_string())
}

fn storage_error(message: &str) -> PluginError {
    PluginError::new(ERR_STORAGE, message)
}

fn read_usize(params: &Value, key: &str) -> Result<Option<usize>, PluginError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(|number| number as usize)
            .map(Some)
            .ok_or_else(|| PluginError::new(ERR_INVALID, format!("{key} 必须是非负整数"))),
    }
}

fn read_bool(params: &Value, key: &str) -> Result<Option<bool>, PluginError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| PluginError::new(ERR_INVALID, format!("{key} 必须是布尔值"))),
    }
}

fn read_str(params: &Value, key: &str) -> Result<Option<String>, PluginError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|text| text.to_string())
            .map(Some)
            .ok_or_else(|| PluginError::new(ERR_INVALID, format!("{key} 必须是字符串"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_sweeper(name: &str) -> Sweeper {
        let dir = std::env::temp_dir().join(format!("datasweep-rpc-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        Sweeper::new(dir)
    }

    fn params(value: Value) -> Value {
        value
    }

    #[test]
    fn 未知方法与预留名返回方法不存在() {
        let sweeper = temp_sweeper("unknown");
        for method in [
            "game/probability",
            "game/solve-next",
            "board/export",
            "nope",
        ] {
            let error = sweeper.dispatch(method, &json!({})).unwrap_err();
            assert_eq!(error.code, -32601, "{method} 应返回方法不存在");
        }
    }

    #[test]
    fn 没有会话时取状态报会话不存在() {
        let sweeper = temp_sweeper("no-session");
        let result = sweeper.snapshot_value().unwrap_err();
        assert_eq!(result.code, ERR_SESSION);
    }

    #[test]
    fn 参数类型不对报非法参数() {
        let error = read_usize(&params(json!({ "rows": "abc" })), "rows").unwrap_err();
        assert_eq!(error.code, ERR_INVALID);
        let error = read_bool(&params(json!({ "useMarks": 1 })), "useMarks").unwrap_err();
        assert_eq!(error.code, ERR_INVALID);
    }

    #[test]
    fn 偏好局部更新不覆盖其他键() {
        let sweeper = temp_sweeper("prefs-merge");
        let original = sweeper.prefs_value().unwrap();
        assert_eq!(original["preset"], "intermediate");
        assert_eq!(original["rule"], "win7");

        let updated = sweeper.set_prefs(&json!({ "useMarks": false })).unwrap();
        assert_eq!(updated["useMarks"], false);
        assert_eq!(updated["preset"], "intermediate");
        assert_eq!(updated["rule"], "win7");
        assert_eq!(updated["custom"]["rows"], 16);

        // 重新构造实例模拟跨进程重启，确认真的落盘了
        let reloaded = Sweeper::new(sweeper.data_dir.clone());
        let after = reloaded.prefs_value().unwrap();
        assert_eq!(after["useMarks"], false);
    }

    #[test]
    fn 自定义尺寸接受逐字段更新() {
        let sweeper = temp_sweeper("custom-partial");
        let updated = sweeper
            .set_prefs(&json!({ "custom": { "rows": 20 } }))
            .unwrap();
        assert_eq!(updated["custom"]["rows"], 20);
        assert_eq!(updated["custom"]["cols"], 30);
        assert_eq!(updated["custom"]["mineNum"], 99);
    }

    #[test]
    fn 枚举型偏好被拒而数值型被夹取() {
        let sweeper = temp_sweeper("prefs-invalid");
        // 枚举值无法"夹"，只能拒
        assert!(sweeper.set_prefs(&json!({ "rule": "nope" })).is_err());
        assert!(sweeper.set_prefs(&json!({ "preset": "godlike" })).is_err());
        assert_eq!(sweeper.prefs_value().unwrap()["rule"], "win7");
        // 数值越界 ⇒ 夹取后落盘（rows 4 → 5），不再报错
        let updated = sweeper
            .set_prefs(&json!({ "custom": { "rows": 4 } }))
            .unwrap();
        assert_eq!(updated["custom"]["rows"], 5);
    }

    #[test]
    fn 三档预设都能开局() {
        let sweeper = temp_sweeper("presets");
        for preset in ["beginner", "intermediate", "expert"] {
            sweeper.set_prefs(&json!({ "preset": preset })).unwrap();
            let snapshot = sweeper.new_game(&json!({})).unwrap();
            assert_eq!(snapshot["status"], "ready");
            assert!(snapshot["rows"].as_u64().unwrap() >= 5);
        }
    }

    #[test]
    fn 越界尺寸被夹取而不是被拒() {
        let sweeper = temp_sweeper("oversize");
        // 行 4 → 夹到 5；类型正确的越界值一律不再报错
        let value = sweeper
            .new_game(&json!({ "rows": 4, "cols": 10, "mineNum": 10 }))
            .unwrap();
        assert_eq!(value["rows"], 5);
        // 雷数 1 → 夹到该盘下限（50 格 ⇒ 10% = 5）
        let value = sweeper
            .new_game(&json!({ "rows": 10, "cols": 10, "mineNum": 1 }))
            .unwrap();
        assert_eq!(value["mineNum"], 10, "10x10 的下限应是 100 的 10%");
        // 雷数 999 → 夹到 35%
        let value = sweeper
            .new_game(&json!({ "rows": 10, "cols": 10, "mineNum": 999 }))
            .unwrap();
        assert_eq!(value["mineNum"], 35);
        // 类型不对仍然要拒：这是唯一剩下的 -32600 来源
        let error = sweeper
            .new_game(&json!({ "rows": "很多", "cols": 10, "mineNum": 10 }))
            .unwrap_err();
        assert_eq!(error.code, ERR_INVALID);
    }

    #[test]
    fn system_info_带上区间口径给前端() {
        let sweeper = temp_sweeper("info-limits");
        let info = sweeper.info().unwrap();
        assert_eq!(info["minSide"], 5);
        assert_eq!(info["maxSide"], 50);
        assert_eq!(info["minePercentMin"], 10);
        assert_eq!(info["minePercentMax"], 35);
    }

    #[test]
    fn 越界雷数被夹取后按夹取值开局() {
        // 取代原先的「完整一局能通关并写出成绩」里那条过时断言：
        // 那个用例硬写 mineNum == 1，而现在是**夹取**策略 ——
        // 5×5 = 25 格，下限 ceil(25×10%) = 3，所以 1 会被夹成 3。
        // 通关与记账的不变量在 game.rs 的「确定性通关只记一次成绩」里验，
        // 那里能读 board 的真实雷位，不必靠随机局面碰运气。
        let sweeper = temp_sweeper("clamp-mine");
        let opened = sweeper
            .new_game(&json!({ "rows": 5, "cols": 5, "mineNum": 1 }))
            .unwrap();
        assert_eq!(
            opened["mineNum"], 3,
            "25 格的 10% 下限是 3，越界应夹取而非报错"
        );
        assert_eq!(opened["mineLeft"], 3, "剩余雷数要与夹取后的雷数一致");
        assert_eq!(opened["rows"], 5);
        assert_eq!(opened["cols"], 5);
    }

    #[test]
    fn 记账条数与胜负一致() {
        // 断言**不变量**，不断言某一种结局：
        //   通关 ⇒ 恰好一条；未通关 ⇒ 一条都没有。
        //
        // 原先写成"开一格后必然没有成绩"，但 5×5/3 时首点 (0,0) 的 flood
        // 有可能一次把 22 个非雷格全开掉 ⇒ 直接通关 ⇒ 记了一条，
        // 于是这个用例也会随机翻车（与它要取代的那个 flaky 同一种错）。
        // "通关一定记得上、且只记一次"由 game.rs 的
        // 「确定性通关只记一次成绩」用真实雷位负责；这里负责另一半。
        let sweeper = temp_sweeper("record-consistency");
        sweeper
            .new_game(&json!({ "rows": 5, "cols": 5, "mineNum": 3 }))
            .unwrap();
        let after = sweeper.reveal(&json!({ "r": 0, "c": 0 })).unwrap();
        let status = after["status"].as_str().unwrap().to_string();
        let count = sweeper.list_records().unwrap().as_array().unwrap().len();
        assert_eq!(
            count,
            usize::from(status == "won"),
            "status={status} 时成绩条数应是 {}，实为 {count}",
            usize::from(status == "won")
        );

        // 终局后再拉状态不得重复记账（UI 会反复轮询 game/state）
        let _ = sweeper.snapshot_value().unwrap();
        assert_eq!(
            sweeper.list_records().unwrap().as_array().unwrap().len(),
            count,
            "反复拉状态不该刷出多条成绩"
        );
    }

    #[test]
    fn 清空成绩返回条数() {
        let sweeper = temp_sweeper("clear-records");
        let mut store = sweeper.store();
        store::push_record(
            &mut store,
            "5x5-1-win7",
            store::Record {
                elapsed_ms: 100,
                bbbv: 3,
                finished_at: "2026-01-01T00:00:00Z".to_string(),
            },
        );
        drop(store);
        sweeper.persist().unwrap();
        assert_eq!(sweeper.clear_records(&json!({})).unwrap()["removed"], 1);
        assert!(sweeper
            .list_records()
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn system_info_带上数据目录与算法库版本() {
        let sweeper = temp_sweeper("info");
        let info = sweeper.info().unwrap();
        assert_eq!(info["backendId"], PLUGIN_ID);
        assert_eq!(info["msToollibVersion"], MS_TOOLLIB_VERSION);
        assert_eq!(info["pluginVersion"], env!("CARGO_PKG_VERSION"));
        assert!(!info["dataDir"].as_str().unwrap().is_empty());
    }
}
