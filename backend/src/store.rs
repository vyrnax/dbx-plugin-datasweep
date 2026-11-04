//! `datasweep.json` 的读写：偏好 + 本地最佳成绩。
//!
//! PLAN 5.5：文件明文、无凭据；写走 temp → rename 原子替换；解析失败则备份成
//! `datasweep.json.corrupt-<戳>` 并用默认值重建——既不静默丢，也不硬失败。

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const STORE_VERSION: u32 = 1;
pub const FILE_NAME: &str = "datasweep.json";
pub const TOP_N: usize = 10;

/// 三档预设，沿用经典扫雷的行列约定（都是 rows × cols，行在前）。
pub const PRESETS: [(&str, usize, usize, usize); 3] = [
    ("beginner", 9, 10, 10),
    ("intermediate", 16, 16, 40),
    ("expert", 16, 30, 99),
];

pub const DEFAULT_PRESET: &str = "intermediate";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Size {
    pub rows: usize,
    pub cols: usize,
    pub mine_num: usize,
}

impl Size {
    pub const fn new(rows: usize, cols: usize, mine_num: usize) -> Self {
        Self {
            rows,
            cols,
            mine_num,
        }
    }

    /// 成绩维度键：`{rows}x{cols}-{mineNum}-{rule}`。自定义与预设共用同一套键。
    pub fn record_key(&self, rule: &str) -> String {
        format!("{}x{}-{}-{}", self.rows, self.cols, self.mine_num, rule)
    }

    /// 从维度键反解回来；键格式由本文件负责，解析失败返回 `None`（不该发生）。
    ///
    /// **必须从头往后逐段切**：规则名自己也带连字符（`first-safe`），所以不能先反着
    /// 切规则，而要先按 `{rows}x{cols}-{mineNum}-` 这个固定前缀把三段数字吃掉，剩下的
    /// 整串才是规则。
    pub fn parse_record_key(key: &str) -> Option<(Self, String)> {
        let (rows, rest) = key.split_once('x')?;
        let (cols, rest) = rest.split_once('-')?;
        let (mine_num, rule) = rest.split_once('-')?;
        Some((
            Self {
                rows: rows.parse().ok()?,
                cols: cols.parse().ok()?,
                mine_num: mine_num.parse().ok()?,
            },
            rule.to_string(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PanelPrefs {
    pub records: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    pub preset: String,
    pub custom: Size,
    pub rule: String,
    pub use_marks: bool,
    pub panel: PanelPrefs,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            preset: DEFAULT_PRESET.to_string(),
            custom: Size::new(16, 30, 99),
            rule: "win7".to_string(),
            use_marks: true,
            panel: PanelPrefs { records: true },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub elapsed_ms: u64,
    pub bbbv: usize,
    /// RFC3339（UTC，带 `Z`）。同一次成绩的天然去重键。
    pub finished_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    pub version: u32,
    pub prefs: Prefs,
    pub records: BTreeMap<String, Vec<Record>>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            prefs: Prefs::default(),
            records: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Io(String),
    Json(String),
}

impl Error {
    pub fn message(&self) -> &str {
        match self {
            Self::Io(message) | Self::Json(message) => message,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for Error {}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// 启动时必须先读（翻译插件踩过：只写不读 ⇒ 跨进程重启后回到兜底值）。
/// 文件不存在 ⇒ 建目录 + 默认值；解析失败 ⇒ 备份原文件 + 默认值。
///
/// 注意这里**不返回错误**：PLAN 5.5 要求"不静默丢、也不硬失败"——丢的是
/// 坏文件本身（已备份），而进程照常带着默认值起来。
pub fn load(data_dir: &Path) -> Result<Store, Error> {
    let file = path(data_dir);
    if !file.exists() {
        fs::create_dir_all(data_dir).map_err(|e| Error::Io(e.to_string()))?;
        return Ok(Store::default());
    }
    let text = fs::read_to_string(&file).map_err(|e| Error::Io(e.to_string()))?;
    match serde_json::from_str::<Store>(&text) {
        Ok(store) => Ok(normalize(store)),
        Err(error) => {
            let backup = data_dir.join(format!("{}.corrupt-{}", FILE_NAME, stamp()));
            let renamed = fs::rename(&file, &backup);
            eprintln!(
                "[datasweep] {} 解析失败（{error}），原文件另存为 {}，本次改用默认值",
                file.display(),
                renamed
                    .as_ref()
                    .map(|_| backup.display().to_string())
                    .unwrap_or_else(|_| "<备份失败>".to_string())
            );
            Ok(Store::default())
        }
    }
}

/// temp → rename 原子替换。同一目录下 rename 才有效，故临时文件一律放在 `data_dir`。
pub fn save(data_dir: &Path, store: &Store) -> Result<(), Error> {
    fs::create_dir_all(data_dir).map_err(|e| Error::Io(e.to_string()))?;
    let temp = data_dir.join(format!("{FILE_NAME}.tmp"));
    let bytes = serde_json::to_vec_pretty(store).map_err(|e| Error::Json(e.to_string()))?;
    fs::write(&temp, bytes).map_err(|e| Error::Io(e.to_string()))?;
    fs::rename(&temp, path(data_dir)).map_err(|e| Error::Io(e.to_string()))
}

/// 归一化：版本归 1、**偏好消毒**、每个维度键裁剪到 Top 10 且按用时升序。
pub fn normalize(mut store: Store) -> Store {
    store.version = STORE_VERSION;
    sanitize_prefs(&mut store.prefs);
    for records in store.records.values_mut() {
        trim(records);
    }
    store
}

/// 把磁盘上可能被手工改坏 / 被旧版本写歪的偏好夹回合法区间。
///
/// 为什么必须做：`game/new` 在不带显式入参时，直接拿 `prefs.preset` / `prefs.rule` /
/// `prefs.custom` 当默认值，而 `rpc.rs` 对非法 prefs 报的是 `-32603 ERR_INTERNAL`。
/// 于是 `{"preset":"hell"}` 或 `{"custom":{"rows":9999}}` 这种一处坏值，
/// 就足以让**每次开局都失败**，UI 只显示"侧车内部错误"——最难查的一类自助坑。
/// 这里就地夹回默认值，并在 stderr 留一行诊断。
fn sanitize_prefs(prefs: &mut Prefs) {
    if prefs.preset != "custom" && !PRESETS.iter().any(|(name, _, _, _)| *name == prefs.preset) {
        eprintln!(
            "[datasweep] 偏好里的难度档位非法（{}），改用默认 {}",
            prefs.preset, DEFAULT_PRESET
        );
        prefs.preset = DEFAULT_PRESET.to_string();
    }
    if crate::game::Rule::parse(&prefs.rule).is_none() {
        eprintln!(
            "[datasweep] 偏好里的首点规则非法（{}），改用 win7",
            prefs.rule
        );
        prefs.rule = "win7".to_string();
    }
    prefs.custom = clamp_size(prefs.custom);
}

/// 尺寸或雷数任一维越界就**整个回退默认**，而不是逐维夹取。
///
/// 与 rpc 层的 `game::clamp_size`（逐维夹取，UI 输入友好）**语义故意不同**：
/// 磁盘上的值被写坏说明各字段之间的关联已经不可信，逐维拼一个"没被改坏的组合"
/// 反而更迷惑；整体回退 + 一行 stderr 诊断是可解释性最好的选择。
/// 区间口径与 rpc 层一致，都用 `game::mine_range`（10%..35%），避免两处各自漂移。
fn clamp_size(size: Size) -> Size {
    let (lower, upper) = crate::game::mine_range(size.rows, size.cols);
    let in_range = size.rows >= crate::game::MIN_SIDE
        && size.rows <= crate::game::MAX_SIDE
        && size.cols >= crate::game::MIN_SIDE
        && size.cols <= crate::game::MAX_SIDE
        && size.mine_num >= lower
        && size.mine_num <= upper;
    if in_range {
        return size;
    }
    let fallback = Prefs::default().custom;
    eprintln!(
        "[datasweep] 偏好里的自定义尺寸非法（{}x{} / {} 雷），改用默认 {}x{} / {}",
        size.rows, size.cols, size.mine_num, fallback.rows, fallback.cols, fallback.mine_num
    );
    fallback
}

fn trim(records: &mut Vec<Record>) {
    let mut seen: Vec<String> = Vec::new();
    records.retain(|record| {
        if seen.iter().any(|known| known == &record.finished_at) {
            return false;
        }
        seen.push(record.finished_at.clone());
        true
    });
    records.sort_by_key(|record| record.elapsed_ms);
    records.truncate(TOP_N);
}

/// 插入一条成绩并返回它是否留在 Top 10 里。
/// 终局判定与写入全在侧车，前端上报改不了成绩（PLAN 4.4）。
pub fn push_record(store: &mut Store, key: &str, record: Record) -> bool {
    let finished_at = record.finished_at.clone();
    let records = store.records.entry(key.to_string()).or_default();
    records.retain(|existing| existing.finished_at != finished_at);
    records.push(record);
    trim(records);
    let survived = records
        .iter()
        .any(|existing| existing.finished_at == finished_at);
    if records.is_empty() {
        store.records.remove(key);
    }
    survived
}

/// `records/list` 的扁平结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordEntry {
    pub rows: usize,
    pub cols: usize,
    pub mine_num: usize,
    pub rule: String,
    pub elapsed_ms: u64,
    pub bbbv: usize,
    pub finished_at: String,
}

pub fn flatten(store: &Store) -> Vec<RecordEntry> {
    let mut entries = Vec::new();
    for (key, records) in &store.records {
        let Some((size, rule)) = Size::parse_record_key(key) else {
            continue;
        };
        for record in records {
            entries.push(RecordEntry {
                rows: size.rows,
                cols: size.cols,
                mine_num: size.mine_num,
                rule: rule.clone(),
                elapsed_ms: record.elapsed_ms,
                bbbv: record.bbbv,
                finished_at: record.finished_at.clone(),
            });
        }
    }
    entries
}

/// 不传 `key` 清全部。返回被删掉的**成绩条数**（两种情况口径一致）。
pub fn clear(store: &mut Store, key: Option<&str>) -> usize {
    match key {
        Some(key) => store.records.remove(key).map_or(0, |records| records.len()),
        None => {
            let removed: usize = store.records.values().map(|records| records.len()).sum();
            store.records.clear();
            removed
        }
    }
}

/// RFC3339（UTC）。不引入 chrono/time crate，手工做 civil date 换算。
pub fn now_rfc3339() -> Result<String, Error> {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::Io(e.to_string()))?;
    Ok(secs_to_rfc3339(since_epoch.as_secs()))
}

fn secs_to_rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Howard Hinnant 的 days→civil 算法。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "16x30-99-win7";

    fn record(elapsed_ms: u64, finished_at: &str) -> Record {
        Record {
            elapsed_ms,
            bbbv: 152,
            finished_at: finished_at.to_string(),
        }
    }

    #[test]
    fn 维度键双向一致() {
        let size = Size::new(16, 30, 99);
        assert_eq!(size.record_key("win7"), KEY);
        let (parsed, rule) = Size::parse_record_key(KEY).unwrap();
        assert_eq!(parsed, size);
        assert_eq!(rule, "win7");
    }

    #[test]
    fn 带连字符的规则也能反解() {
        let size = Size::new(9, 10, 10);
        let key = size.record_key("first-safe");
        assert_eq!(key, "9x10-10-first-safe");
        let (parsed, rule) = Size::parse_record_key(&key).unwrap();
        assert_eq!(parsed, size);
        assert_eq!(rule, "first-safe");
    }

    #[test]
    fn 成绩裁剪到十条且升序() {
        let mut store = Store::default();
        for index in 0..12 {
            push_record(&mut store, KEY, record(200 - index, &format!("t{index}")));
        }
        let records = &store.records[KEY];
        assert_eq!(records.len(), TOP_N);
        assert_eq!(records[0].elapsed_ms, 189);
        assert!(records
            .windows(2)
            .all(|pair| pair[0].elapsed_ms <= pair[1].elapsed_ms));
    }

    #[test]
    fn finished_at_相同则去重() {
        let mut store = Store::default();
        push_record(&mut store, KEY, record(100, "same"));
        push_record(&mut store, KEY, record(50, "same"));
        assert_eq!(store.records[KEY].len(), 1);
        assert_eq!(store.records[KEY][0].elapsed_ms, 50);
    }

    #[test]
    fn 清空分键与全部() {
        let mut store = Store::default();
        push_record(&mut store, KEY, record(1, "a"));
        push_record(&mut store, "9x10-10-win7", record(1, "b"));
        assert_eq!(clear(&mut store, Some(KEY)), 1);
        assert_eq!(store.records.len(), 1);
        assert_eq!(clear(&mut store, None), 1);
        assert!(store.records.is_empty());
    }

    #[test]
    fn 坏偏好被夹回默认而不是留着让开局报错() {
        let mut store = Store::default();
        store.prefs.preset = "hell".to_string();
        store.prefs.rule = "nope".to_string();
        store.prefs.custom = Size::new(9999, 1, 999_999);
        let fixed = normalize(store);
        assert_eq!(fixed.prefs.preset, DEFAULT_PRESET);
        assert_eq!(fixed.prefs.rule, "win7");
        assert_eq!(fixed.prefs.custom, Prefs::default().custom);

        // custom 档位本身合法，只要尺寸在范围内就该原样保留
        let mut keep = Store::default();
        keep.prefs.preset = "custom".to_string();
        keep.prefs.custom = Size::new(8, 8, 12);
        let kept = normalize(keep);
        assert_eq!(kept.prefs.preset, "custom");
        assert_eq!(kept.prefs.custom, Size::new(8, 8, 12));

        // 雷数超出格数的 35% 也要整体回退：8x8 = 64 ⇒ 合法区间 [7, 22]
        let mut ok = Store::default();
        ok.prefs.preset = "custom".to_string();
        ok.prefs.custom = Size::new(8, 8, 22);
        assert_eq!(
            normalize(ok).prefs.custom,
            Size::new(8, 8, 22),
            "恰好等于上限 22 必须保留"
        );

        let mut too_many = Store::default();
        too_many.prefs.custom = Size::new(8, 8, 23);
        assert_eq!(
            normalize(too_many).prefs.custom,
            Prefs::default().custom,
            "越过上限的雷数必须整体回退"
        );

        // 低于 10% 下限同样回退：64 的下限是 7，6 不合法
        let mut too_few = Store::default();
        too_few.prefs.custom = Size::new(8, 8, 6);
        assert_eq!(normalize(too_few).prefs.custom, Prefs::default().custom);
    }

    #[test]
    fn rfc3339_格式正确() {
        assert_eq!(secs_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(secs_to_rfc3339(1_767_225_600), "2026-01-01T00:00:00Z");
        assert_eq!(
            secs_to_rfc3339(1_767_225_600 + 86_399),
            "2026-01-01T23:59:59Z"
        );
    }

    #[test]
    fn 扁平结果含尺寸与规则() {
        let mut store = Store::default();
        push_record(&mut store, KEY, record(100, "a"));
        let entries = flatten(&store);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rows, 16);
        assert_eq!(entries[0].cols, 30);
        assert_eq!(entries[0].mine_num, 99);
        assert_eq!(entries[0].rule, "win7");
    }

    #[test]
    fn 保存后再读到同一份() {
        let dir = std::env::temp_dir().join("datasweep-store-test");
        let _ = fs::remove_dir_all(&dir);
        let mut store = Store::default();
        push_record(&mut store, KEY, record(100, "a"));
        save(&dir, &store).unwrap();
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded, normalize(store));
        assert!(!dir.join(format!("{FILE_NAME}.tmp")).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 损坏文件被备份并按默认重建() {
        let dir = std::env::temp_dir().join("datasweep-corrupt-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(path(&dir), "{ this is not json").unwrap();
        let store = load(&dir).unwrap();
        assert_eq!(store, Store::default(), "损坏时应按默认值重建且不硬失败");
        let backup = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"));
        assert!(backup.is_some(), "坏文件必须留底");
        let _ = fs::remove_dir_all(&dir);
    }
}
