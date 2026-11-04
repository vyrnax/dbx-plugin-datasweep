//! 会话注册表与一局的全部规则：首点埋雷、翻格、压枪、旗/问号、胜负、计时。
//!
//! 局面权威在侧车，真实雷位永不出侧车（PLAN 4.3）。

use std::collections::HashMap;
use std::time::Instant;

use crate::msbridge;
use crate::store::{self, Size};

pub const MIN_SIDE: usize = 5;
pub const MAX_SIDE: usize = 50;

/// 雷数占格数的比例区间（百分数）：下限 10%，上限 35%。
pub const MINE_PERCENT_MIN: usize = 10;
pub const MINE_PERCENT_MAX: usize = 35;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Win7 规则：首点必开空，且首点 8 邻域无雷
    Win7,
    /// 仅保证首点不是雷
    FirstSafe,
}

impl Rule {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "win7" => Some(Self::Win7),
            "first-safe" => Some(Self::FirstSafe),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Win7 => "win7",
            Self::FirstSafe => "first-safe",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 已开局，但真实局面尚未生成（bbbv 仍为 null，计时未启动）
    Ready,
    Playing,
    Won,
    Lost,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Playing => "playing",
            Self::Won => "won",
            Self::Lost => "lost",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagMode {
    Flag,
    Mark,
    Clear,
}

impl FlagMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "flag" => Some(Self::Flag),
            "mark" => Some(Self::Mark),
            "clear" => Some(Self::Clear),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    /// -32010 会话不存在
    Session,
    /// -32600 参数不合法
    Invalid(String),
    /// -32011 状态不允许
    State(String),
    /// -32013 算法失败
    Algorithm(String),
}

impl Error {
    pub fn message(&self) -> String {
        match self {
            Self::Session => "会话不存在：侧车可能已被宿主重启，本局作废".to_string(),
            Self::Invalid(message) | Self::State(message) | Self::Algorithm(message) => {
                message.clone()
            }
        }
    }
}

pub struct Game {
    pub(crate) id: u64,
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) mine_num: usize,
    pub(crate) rule: Rule,
    pub(crate) status: Status,
    /// 真实局面。只有首点之后才存在；含 `-1`，**永不下发给 UI**。
    board: Option<Vec<Vec<i32>>>,
    /// 游戏局面，是唯一会发给 UI 的那一面
    pub(crate) cells: Vec<Vec<i32>>,
    /// 问号标记。`ms_toollib` 没有问号这一档，故不自造数字码，单独维护。
    pub(crate) marks: Vec<(usize, usize)>,
    pub(crate) opened: usize,
    pub(crate) bbbv: Option<usize>,
    pub(crate) elapsed_ms: u64,
    started_at: Option<Instant>,
    /// 胜利成绩只写一次，防止 UI 反复拉 `game/state` 刷出多条
    recorded: bool,
}

impl Game {
    fn new(id: u64, size: Size, rule: Rule) -> Self {
        Self {
            id,
            rows: size.rows,
            cols: size.cols,
            mine_num: size.mine_num,
            rule,
            status: Status::Ready,
            board: None,
            cells: vec![vec![msbridge::OPEN; size.cols]; size.rows],
            marks: Vec::new(),
            opened: 0,
            bbbv: None,
            elapsed_ms: 0,
            started_at: None,
            recorded: false,
        }
    }

    pub fn size(&self) -> Size {
        Size::new(self.rows, self.cols, self.mine_num)
    }

    pub fn record_key(&self) -> String {
        self.size().record_key(self.rule.as_str())
    }

    /// 翻一格。首次调用时才真正埋雷并启动计时（PLAN 5.3）。
    /// `force` 用于 `Ctrl` + 左键的盲开——跳过插旗检查。
    pub fn reveal(&mut self, row: usize, col: usize, force: bool) -> Result<(), Error> {
        // 坐标是"参数合法性"，与对局状态无关 ⇒ 必须先于状态检查，
        // 否则终局后一次越界点击会被报成 -32011 而不是 -32600。
        check_cell(self, row, col)?;
        self.ensure_actionable()?;
        // 旗格上的左键 = **无反应**，与经典扫雷一致，而不是抛一条错误。
        // 旗的语义是"我认为这里是雷"，玩家紧接着左键点它本就是矛盾操作；
        // 弹一条"这一步不允许"（宿主还可能把它显示成"侧车内部错误"）远不如静默忽略。
        // 真要强行翻开用 force（Ctrl + 左键）。
        if self.cells[row][col] == msbridge::FLAG && !force {
            return Ok(());
        }
        self.ensure_board(row, col)?;
        let board = self
            .board
            .take()
            .ok_or_else(|| Error::Algorithm("真实局面尚未生成".to_string()))?;
        msbridge::refresh(&board, &mut self.cells, vec![(row, col)]);
        self.board = Some(board);
        self.drop_marks_on_opened();
        self.settle();
        Ok(())
    }

    /// 压枪（chord）。只对已开的数字格生效，周围旗数须等于该数字（PLAN 5.4）。
    /// `is_good_chording` 不做前置校验——玩家有权做局部最差的压枪。
    pub fn chord(&mut self, row: usize, col: usize) -> Result<(), Error> {
        check_cell(self, row, col)?;
        if self.status != Status::Playing {
            return Err(Error::State("只有进行中的对局才能压枪".to_string()));
        }
        let number = self.cells[row][col];
        if !(1..=8).contains(&number) {
            return Err(Error::State("压枪只能作用在已翻开的数字格上".to_string()));
        }
        let around = around(row, col, self.rows, self.cols);
        let flagged = around
            .iter()
            .filter(|&&(r, c)| self.cells[r][c] == msbridge::FLAG)
            .count();
        if flagged != number as usize {
            return Err(Error::State("周围旗数与该数字不符".to_string()));
        }
        let targets: Vec<(usize, usize)> = around
            .into_iter()
            .filter(|&(r, c)| self.cells[r][c] == msbridge::OPEN)
            .collect();
        if targets.is_empty() {
            return Err(Error::State("周围没有可翻开的格子".to_string()));
        }
        let board = self
            .board
            .take()
            .ok_or_else(|| Error::Algorithm("真实局面尚未生成".to_string()))?;
        msbridge::refresh(&board, &mut self.cells, targets);
        self.board = Some(board);
        self.drop_marks_on_opened();
        self.settle();
        Ok(())
    }

    /// 旗 / 问号 / 空。`marks` 是问号，旗写进 `cells` 的 `FLAG`。
    pub fn flag(&mut self, row: usize, col: usize, mode: FlagMode) -> Result<(), Error> {
        check_cell(self, row, col)?;
        self.ensure_actionable()?;
        let value = self.cells[row][col];
        match mode {
            FlagMode::Flag => {
                if (0..=8).contains(&value) {
                    return Err(Error::State("已翻开的格子不能插旗".to_string()));
                }
                self.cells[row][col] = msbridge::FLAG;
                self.marks.retain(|each| *each != (row, col));
            }
            FlagMode::Mark => {
                if !matches!(
                    value,
                    msbridge::OPEN | msbridge::FLAG | msbridge::KNOWN_SAFE
                ) {
                    return Err(Error::State("已翻开的格子不能标问号".to_string()));
                }
                // 旗 → 问号必须**先把旗撤下**。原先只在 value == OPEN 时写标记，
                // 于是对旗格标问号是空操作：右键循环（空 → 旗 → 问号 → 空）卡死在
                // 第二步、旗永远撤不掉，玩家只好改用左键去点它 —— 那正是撞上
                // "该格已插旗"那条报错的来源。
                if matches!(value, msbridge::FLAG | msbridge::KNOWN_SAFE) {
                    self.cells[row][col] = msbridge::OPEN;
                }
                if !self.marks.contains(&(row, col)) {
                    self.marks.push((row, col));
                }
            }
            FlagMode::Clear => {
                if matches!(value, msbridge::FLAG | msbridge::KNOWN_SAFE) {
                    self.cells[row][col] = msbridge::OPEN;
                }
                self.marks.retain(|each| *each != (row, col));
            }
        }
        Ok(())
    }

    /// 刷新权威用时。**任何要下发快照的路径都必须先调它。**
    ///
    /// 修掉的 bug：`elapsed_ms` 原先只在 `settle()`（翻格 / 压枪之后）重算，
    /// 而 `game/flag` 与 `game/state` 走的是 `snapshot_value()`，于是它们把
    /// **上一次翻格时刻的陈旧用时**当成当前用时发出去；前端 `applySnapshot()`
    /// 又把秒表基准无条件重置成快照里的 `elapsedMs` ——
    /// 玩家看到的就是「插个旗，开始时间被重置」。
    ///
    /// 终局冻结：`won` / `lost` 之后不再推进，成绩仍由 `settle()` 定稿。
    pub fn tick(&mut self) {
        if self.status != Status::Playing {
            return;
        }
        if let Some(started) = self.started_at {
            self.elapsed_ms = started.elapsed().as_millis() as u64;
        }
    }

    /// 取走胜利成绩（每局只给一次），由上层落盘。
    pub fn claim_win(&mut self) -> Option<(u64, usize)> {
        if self.status != Status::Won || self.recorded {
            return None;
        }
        self.recorded = true;
        // 胜利必然发生在翻格之后，而翻格已在 ensure_board() 里定好 3BV；
        // 兜底成 0 会把"不变量被违反"这件事悄悄写进成绩文件。
        debug_assert!(self.bbbv.is_some(), "won 时 bbbv 必须已确定");
        Some((self.elapsed_ms, self.bbbv.unwrap_or_default()))
    }

    fn ensure_actionable(&self) -> Result<(), Error> {
        match self.status {
            Status::Ready | Status::Playing => Ok(()),
            Status::Won | Status::Lost => Err(Error::State("对局已结束，请开新局".to_string())),
        }
    }

    /// 首点埋雷：只有 `ready` 状态才允许，`playing` 之后不再触碰真实局面。
    fn ensure_board(&mut self, row: usize, col: usize) -> Result<(), Error> {
        if self.status != Status::Ready {
            return Ok(());
        }
        let board = msbridge::lay(self.rows, self.cols, self.mine_num, row, col, self.rule)
            .map_err(|message| Error::Algorithm(message.to_string()))?;
        self.bbbv = Some(msbridge::bbbv(&board));
        self.board = Some(board);
        self.status = Status::Playing;
        self.started_at = Some(Instant::now());
        Ok(())
    }

    /// 重算已开格数与侧车权威用时，再判定胜负。`won`/`lost` 时用时冻结。
    fn settle(&mut self) {
        self.opened = self
            .cells
            .iter()
            .flatten()
            .filter(|&&value| (0..=8).contains(&value))
            .count();
        if self.status != Status::Playing {
            return;
        }
        self.tick();
        if self
            .cells
            .iter()
            .flatten()
            .any(|&value| value == msbridge::HIT_MINE)
        {
            self.status = Status::Lost;
            self.reveal_remaining_mines();
            return;
        }
        if self.opened
            >= self
                .rows
                .saturating_mul(self.cols)
                .saturating_sub(self.mine_num)
        {
            self.status = Status::Won;
        }
    }

    /// `refresh_board` 从来不写 `OTHER_MINE`（PLAN 2.4 B），失败后由这里补刷：
    /// 所有真实是雷、且没被玩家标成 `FLAG` 的格，显示成白雷。
    fn reveal_remaining_mines(&mut self) {
        let Some(board) = self.board.as_ref() else {
            return;
        };
        for (cells_row, board_row) in self.cells.iter_mut().zip(board.iter()) {
            for (cell, value) in cells_row.iter_mut().zip(board_row.iter()) {
                // 只刷"还盖着"的真雷为白雷：踩中的那颗已经是 15、标错的旗已经是 14，
                // 都不能被覆盖，否则红雷会被抹掉。
                if *value == msbridge::REAL_MINE
                    && matches!(*cell, msbridge::OPEN | msbridge::KNOWN_SAFE)
                {
                    *cell = msbridge::OTHER_MINE;
                }
            }
        }
        self.opened = self
            .cells
            .iter()
            .flatten()
            .filter(|&&value| (0..=8).contains(&value))
            .count();
    }

    /// 格子被翻开后，挂在它上面的问号要跟着消失。
    fn drop_marks_on_opened(&mut self) {
        let cells = &self.cells;
        self.marks
            .retain(|&(row, col)| cells[row][col] == msbridge::OPEN);
    }

    /// 剩余雷数 = 总雷数 − 玩家标的旗（含失败后已转成叉雷的那些，保持读数稳定）。
    pub fn mine_left(&self) -> i64 {
        let flagged = self
            .cells
            .iter()
            .flatten()
            .filter(|&&value| matches!(value, msbridge::FLAG | msbridge::WRONG_FLAG))
            .count();
        self.mine_num as i64 - flagged as i64
    }
}

/// 进程级会话注册表。`gameId` 单调递增，`game/new` 会丢掉旧的一局
/// （PLAN 4.3：只保留一个活动对局）。
pub struct Sessions {
    next_id: u64,
    current: Option<u64>,
    games: HashMap<u64, Game>,
}

impl Sessions {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            current: None,
            games: HashMap::new(),
        }
    }

    pub fn create(&mut self, size: Size, rule: Rule) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.games.clear();
        self.games.insert(id, Game::new(id, size, rule));
        self.current = Some(id);
        id
    }

    pub fn current(&mut self) -> Result<&mut Game, Error> {
        let id = self.current.ok_or(Error::Session)?;
        self.games.get_mut(&id).ok_or(Error::Session)
    }

    /// 认输：清掉会话，不计成绩。
    pub fn abandon(&mut self) {
        self.games.clear();
        self.current = None;
    }
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new()
    }
}

fn check_cell(game: &Game, row: usize, col: usize) -> Result<(), Error> {
    if row >= game.rows || col >= game.cols {
        return Err(Error::Invalid(format!(
            "坐标越界：({row}, {col}) 不在 {rows}x{cols} 内",
            rows = game.rows,
            cols = game.cols
        )));
    }
    Ok(())
}

/// 8 邻域，已按边界裁剪。
fn around(row: usize, col: usize, rows: usize, cols: usize) -> Vec<(usize, usize)> {
    let mut cells = Vec::with_capacity(8);
    for r in row.saturating_sub(1)..=(row + 1).min(rows - 1) {
        for c in col.saturating_sub(1)..=(col + 1).min(cols - 1) {
            if (r, c) != (row, col) {
                cells.push((r, c));
            }
        }
    }
    cells
}

/// 给定尺寸，返回允许的雷数**闭区间**。
///
/// 下限向上取整、上限向下取整：区间宁可窄一点，也不能把越界方向放进来。
/// 行列最小 5 ⇒ 格子数最小 25 ⇒ 区间 [3, 8]，所以正常调用下 `lower <= upper` 恒成立；
/// 末尾仍保留一道兜底，避免区间反转把 `clamp` 变成 panic。
pub fn mine_range(rows: usize, cols: usize) -> (usize, usize) {
    let cells = rows.saturating_mul(cols);
    // 先乘百分比、再对 100 取整：下限向上（div_ceil）、上限向下（整除）。
    // 别写成 cells.div_ceil(100 / MINE_PERCENT_MIN) —— 那只在下限能整除 100 时
    // 才等价（10/20/25/50 恰好都成立），将来改成 12% 之类就会静默算错。
    let lower = (cells * MINE_PERCENT_MIN).div_ceil(100);
    let upper = (cells * MINE_PERCENT_MAX) / 100;
    if lower > upper {
        (upper, upper)
    } else {
        (lower, upper)
    }
}

/// 把任意尺寸**夹取**进合法范围：行列 5..50，雷数落在格数的 10%..35%。
///
/// 为什么是夹取而不是拒绝（玩家实测反馈）：在输入框里打一行数字就报
/// `-32600 参数不合法`，体验是"我输入 1 它就骂我"。现在
/// 输入 1 → 自动取下限，输入超过 35% → 自动取上限，行列同理夹进 5..50。
/// 真正会报错的只剩"类型不对"（非整数等），那由 rpc 层的 read_usize 负责。
///
/// 三档预设天然落在新区间内（初级 10/90=11.1%、中级 40/256=15.6%、
/// 高级 99/480=20.6%），所以这里可以无差别应用，不必区分预设与自定义。
pub fn clamp_size(size: Size) -> Size {
    let rows = size.rows.clamp(MIN_SIDE, MAX_SIDE);
    let cols = size.cols.clamp(MIN_SIDE, MAX_SIDE);
    let (lower, upper) = mine_range(rows, cols);
    Size::new(rows, cols, size.mine_num.clamp(lower, upper))
}

/// 由偏好解出尺寸：`custom` 档用自定义值，其余查预设表。
pub fn resolve_size(preset: &str, custom: Size) -> Result<Size, Error> {
    if preset == "custom" {
        return Ok(custom);
    }
    store::PRESETS
        .iter()
        .find(|(name, _, _, _)| *name == preset)
        .map(|(_, rows, cols, mine_num)| Size::new(*rows, *cols, *mine_num))
        .ok_or_else(|| Error::Invalid(format!("未知难度档位：{preset}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(rows: usize, cols: usize, mine_num: usize) -> Size {
        Size::new(rows, cols, mine_num)
    }

    fn sessions(size: Size) -> Sessions {
        let mut sessions = Sessions::new();
        sessions.create(size, Rule::Win7);
        sessions
    }

    #[test]
    fn 尺寸校验接受常规三档() {
        // 三档预设必须原样穿过夹取（详见下面"三档预设都在新雷数区间内"）
        assert_eq!(clamp_size(size(9, 10, 10)), size(9, 10, 10));
        assert_eq!(clamp_size(size(16, 16, 40)), size(16, 16, 40));
        assert_eq!(clamp_size(size(16, 30, 99)), size(16, 30, 99));
    }

    #[test]
    fn 雷数区间按十分之一到百分之三十五取整() {
        // cells=25（最小盘）：ceil(2.5)=3 .. floor(8.75)=8
        assert_eq!(mine_range(5, 5), (3, 8));
        // cells=90：ceil(9)=9 .. floor(31.5)=31
        assert_eq!(mine_range(9, 10), (9, 31));
        // cells=100：10 .. 35，两端都取整到整数本身
        assert_eq!(mine_range(10, 10), (10, 35));
        // cells=480：48 .. 168
        assert_eq!(mine_range(16, 30), (48, 168));
    }

    #[test]
    fn 越界尺寸被夹取而不是报错() {
        // 行列越界 ⇒ 夹进 5..50
        // 夹完行列之后还要按**新尺寸**重算雷数区间，所以雷数也会跟着动：
        // 5x10=50 ⇒ [5,17]，10 合法；50x10=500 ⇒ [50,175]，10 被抬到下限 50
        assert_eq!(clamp_size(size(4, 10, 10)), size(5, 10, 10));
        assert_eq!(clamp_size(size(51, 10, 10)), size(50, 10, 50));
        assert_eq!(clamp_size(size(9, 51, 10)), size(9, 50, 45));
        // 雷数太小 ⇒ 自动取下限（玩家要求的行为：输入 1 就是下限）
        assert_eq!(clamp_size(size(9, 10, 1)), size(9, 10, 9));
        assert_eq!(clamp_size(size(9, 10, 0)), size(9, 10, 9));
        // 雷数太大 ⇒ 自动取上限；23 在新区间里其实已合法（旧规则 /4=22 会拒）
        assert_eq!(clamp_size(size(9, 10, 23)), size(9, 10, 23));
        assert_eq!(clamp_size(size(9, 10, 999)), size(9, 10, 31));
        // 50×50 的上限
        assert_eq!(clamp_size(size(50, 50, 99999)).mine_num, 875);
    }

    #[test]
    fn 三档预设都在新雷数区间内() {
        for (_, rows, cols, mines) in crate::store::PRESETS {
            let clamped = clamp_size(size(rows, cols, mines));
            assert_eq!(
                clamped,
                size(rows, cols, mines),
                "预设 {rows}x{cols}/{mines} 不该被夹改"
            );
        }
    }

    #[test]
    fn 预设解析与自定义() {
        assert_eq!(
            resolve_size("beginner", size(1, 1, 1)).unwrap(),
            size(9, 10, 10)
        );
        assert_eq!(
            resolve_size("expert", size(1, 1, 1)).unwrap(),
            size(16, 30, 99)
        );
        assert_eq!(
            resolve_size("custom", size(20, 20, 50)).unwrap(),
            size(20, 20, 50)
        );
        assert!(resolve_size("nope", size(1, 1, 1)).is_err());
    }

    #[test]
    fn 坐标越界报参数错() {
        let mut sessions = sessions(size(9, 10, 10));
        let error = sessions.current().unwrap().reveal(9, 0, false).unwrap_err();
        assert!(matches!(error, Error::Invalid(_)));
    }

    #[test]
    fn 坐标越界优先于终局状态() {
        // 参数合法性跟对局状态无关，所以终局后的越界点击仍应报 -32600 而不是 -32011。
        let mut sessions = sessions(size(9, 10, 10));
        let game = sessions.current().unwrap();
        game.reveal(0, 0, false).unwrap();
        game.status = Status::Lost;
        assert!(matches!(
            game.reveal(99, 0, false).unwrap_err(),
            Error::Invalid(_)
        ));
        assert!(matches!(game.chord(99, 0).unwrap_err(), Error::Invalid(_)));
        assert!(matches!(
            game.flag(99, 0, FlagMode::Flag).unwrap_err(),
            Error::Invalid(_)
        ));
    }

    #[test]
    fn 首点之前不埋雷且不计时() {
        let mut sessions = sessions(size(9, 10, 10));
        let game = sessions.current().unwrap();
        game.flag(0, 0, FlagMode::Flag).unwrap();
        assert_eq!(game.status, Status::Ready);
        assert_eq!(game.bbbv, None);
        assert_eq!(game.elapsed_ms, 0);
        assert!(game.board.is_none());
        // 已翻开的 snapshot 里 bbbv 仍应为 null（PLAN 5.2）
        assert_eq!(crate::snapshot::from_game(game).bbbv, None);
    }

    #[test]
    fn 首点保护两种规则各采样二百次() {
        let size = size(16, 30, 99);
        for rule in [Rule::Win7, Rule::FirstSafe] {
            for row in [0usize, 8, 15] {
                for col in [0usize, 15, 29] {
                    for _ in 0..200 {
                        let board =
                            msbridge::lay(size.rows, size.cols, size.mine_num, row, col, rule)
                                .unwrap();
                        let mines = board
                            .iter()
                            .flatten()
                            .filter(|&&v| v == msbridge::REAL_MINE);
                        assert_eq!(mines.count(), size.mine_num);
                        assert_ne!(board[row][col], msbridge::REAL_MINE);
                        // 数字算法用的：邻域必须一致
                        if rule == Rule::Win7 {
                            assert_eq!(board[row][col], 0, "Win7 规则首点必须开空");
                            for (r, c) in around(row, col, size.rows, size.cols) {
                                assert_ne!(board[r][c], msbridge::REAL_MINE);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn 右键循环空到旗到问号到空走得完() {
        let mut sessions = sessions(size(9, 10, 10));
        let game = sessions.current().unwrap();
        game.flag(3, 3, FlagMode::Flag).unwrap();
        assert_eq!(game.cells[3][3], msbridge::FLAG);
        assert_eq!(game.mine_left(), 9);
        // 旗 → 问号：必须真的回到"未打开 + 问号"，而不是原地不动
        game.flag(3, 3, FlagMode::Mark).unwrap();
        assert_eq!(game.cells[3][3], msbridge::OPEN, "旗格标问号后必须撤下旗");
        assert!(game.marks.contains(&(3, 3)));
        assert_eq!(game.mine_left(), 10, "问号不占旗额度");
        // 问号 → 空
        game.flag(3, 3, FlagMode::Clear).unwrap();
        assert_eq!(game.cells[3][3], msbridge::OPEN);
        assert!(!game.marks.contains(&(3, 3)));
    }

    #[test]
    fn 旗格左键是无反应而不是错误() {
        let mut sessions = sessions(size(9, 10, 10));
        let game = sessions.current().unwrap();
        game.flag(0, 0, FlagMode::Flag).unwrap();
        game.reveal(0, 0, false).unwrap();
        assert_eq!(game.status, Status::Ready, "旗格左键不应开局");
        assert_eq!(game.cells[0][0], msbridge::FLAG, "旗必须还在");
        assert_eq!(game.opened, 0);
    }

    #[test]
    fn 插旗与查状态会推进权威用时() {
        // 回归：这两条路径原先返回上一次翻格时刻的陈旧 elapsed，前端秒表表现为"被重置"。
        let mut sessions = sessions(size(9, 10, 10));
        {
            let game = sessions.current().unwrap();
            game.reveal(4, 4, false).unwrap();
            assert_eq!(game.status, Status::Playing);
        }
        let started = {
            let game = sessions.current().unwrap();
            game.tick();
            game.elapsed_ms
        };
        std::thread::sleep(std::time::Duration::from_millis(60));
        // 首点 flood 会把一大片格子翻开，**不能硬编码 (0,0)**——它很可能已经是数字格，
        // 插旗会被正确拒绝（那是另一条规则，不是本测试要验证的东西）。
        let closed = {
            let game = sessions.current().unwrap();
            (0..game.rows)
                .flat_map(|row| (0..game.cols).map(move |col| (row, col)))
                .find(|&(row, col)| game.cells[row][col] == msbridge::OPEN)
                .expect("还有未翻开的格子")
        };
        {
            let game = sessions.current().unwrap();
            game.flag(closed.0, closed.1, FlagMode::Flag).unwrap();
            game.tick();
            assert!(
                game.elapsed_ms > started,
                "插旗后用时必须继续推进（started={started}，现在={})",
                game.elapsed_ms
            );
        }
    }

    #[test]
    fn 终局后用时冻结() {
        let mut sessions = sessions(size(5, 5, 1));
        let game = sessions.current().unwrap();
        game.reveal(2, 2, false).unwrap();
        for row in 0..5 {
            for col in 0..5 {
                let _ = game.reveal(row, col, false);
            }
        }
        if game.status == Status::Playing {
            return; // 5×5/1 偶发一次 flood 打不完，跳过而不是造假断言
        }
        let frozen = game.elapsed_ms;
        std::thread::sleep(std::time::Duration::from_millis(30));
        game.tick();
        assert_eq!(game.elapsed_ms, frozen, "终局后用时不得继续推进");
    }

    #[test]
    fn 插旗阻止翻格而盲开可以() {
        let mut sessions = sessions(size(9, 10, 10));
        {
            let game = sessions.current().unwrap();
            game.flag(0, 0, FlagMode::Flag).unwrap();
            // 无反应：不再报错，但确实翻不开
            game.reveal(0, 0, false).unwrap();
            assert_eq!(game.cells[0][0], msbridge::FLAG);
        }
        {
            let game = sessions.current().unwrap();
            game.reveal(0, 0, true).unwrap();
            // 稀疏雷盘上一次 flood 有可能直接通关，所以不能断言"进行中"，
            // 只断言"不再是 ready"——后者才是"首点已生效"的准确判据。
            assert_ne!(game.status, Status::Ready);
            assert!(game.bbbv.is_some());
        }
    }

    #[test]
    fn 问号不占困旗额度且能被清空() {
        let mut sessions = sessions(size(9, 10, 10));
        let game = sessions.current().unwrap();
        // 插旗在 ready 阶段就允许，不需要先翻格，避免 flood 直接终局影响断言。
        game.flag(1, 1, FlagMode::Mark).unwrap();
        assert!(game.marks.contains(&(1, 1)));
        assert_eq!(game.mine_left(), 10, "问号不减剩余雷数");

        game.flag(2, 2, FlagMode::Flag).unwrap();
        assert_eq!(game.mine_left(), 9);
        game.flag(2, 2, FlagMode::Clear).unwrap();
        assert_eq!(game.mine_left(), 10);

        game.flag(1, 1, FlagMode::Clear).unwrap();
        assert!(!game.marks.contains(&(1, 1)));
    }

    #[test]
    fn 格子被翻开后其上的问号自动消失() {
        let mut sessions = sessions(size(9, 10, 10));
        let game = sessions.current().unwrap();
        game.flag(1, 1, FlagMode::Mark).unwrap();
        game.reveal(1, 1, false).unwrap();
        // 不变量：问号只能挂在未打开的格子上，与一局是否终局无关。
        assert!(game
            .marks
            .iter()
            .all(|&(row, col)| game.cells[row][col] == msbridge::OPEN));
    }

    #[test]
    fn 压枪需要旗数与数字相符() {
        let mut sessions = sessions(size(9, 10, 10));
        let game = sessions.current().unwrap();
        game.reveal(4, 4, false).unwrap();
        let numbers: Vec<(usize, usize)> = (0..9)
            .flat_map(|r| (0..10).map(move |c| (r, c)))
            .filter(|&(r, c)| (1..=8).contains(&game.cells[r][c]))
            .collect();
        assert!(!numbers.is_empty(), "稀疏雷盘上总有几个数字格");
        let target = numbers[0];
        assert!(
            game.chord(target.0, target.1).is_err(),
            "旗数为 0 而数字非 0 时应拒"
        );
        let closed = (0..9)
            .flat_map(|r| (0..10).map(move |c| (r, c)))
            .find(|&(r, c)| game.cells[r][c] == msbridge::OPEN);
        if let Some((r, c)) = closed {
            assert!(game.chord(r, c).is_err(), "未翻开的格不能压枪");
        }
    }

    #[test]
    fn 合法压枪打开周围的非雷格() {
        // 单测能看到真实局面（UI 看不到），所以可以把旗精确插到真的雷上，
        // 构造出一个必然合法的压枪。
        let mut sessions = Sessions::new();
        let mut target = None;
        'search: for _ in 0..30 {
            sessions.create(size(9, 10, 10), Rule::Win7);
            let game = sessions.current().unwrap();
            game.reveal(4, 4, false).unwrap();
            let board = game.board.clone().unwrap();
            for row in 0..game.rows {
                for col in 0..game.cols {
                    let value = game.cells[row][col];
                    if !(1..=8).contains(&value) {
                        continue;
                    }
                    let neighbours = around(row, col, game.rows, game.cols);
                    let mines: Vec<(usize, usize)> = neighbours
                        .iter()
                        .copied()
                        .filter(|&(r, c)| board[r][c] == msbridge::REAL_MINE)
                        .collect();
                    let closable = neighbours
                        .iter()
                        .filter(|&&(r, c)| {
                            game.cells[r][c] == msbridge::OPEN && board[r][c] != msbridge::REAL_MINE
                        })
                        .count();
                    if mines.len() == value as usize && closable > 0 {
                        target = Some((row, col, mines));
                        break 'search;
                    }
                }
            }
        }
        let Some((row, col, mines)) = target else {
            panic!("三十局里应当能凑出一个可压枪的位置");
        };
        let game = sessions.current().unwrap();
        for (r, c) in mines {
            game.flag(r, c, FlagMode::Flag).unwrap();
        }
        let before = game.opened;
        game.chord(row, col).unwrap();
        assert!(game.opened > before, "合法压枪必须真的打开格子");
    }

    #[test]
    fn 终局后不能再动作() {
        let mut sessions = sessions(size(5, 5, 1));
        let game = sessions.current().unwrap();
        game.reveal(0, 0, false).unwrap();
        if game.status == Status::Playing {
            game.status = Status::Lost;
            assert!(game.reveal(1, 1, false).is_err());
            assert!(game.chord(1, 1).is_err());
            assert!(game.flag(1, 1, FlagMode::Flag).is_err());
        }
    }

    #[test]
    fn 会话不存在返回正确错误() {
        let mut sessions = Sessions::new();
        // 不给 Game 派生 Debug：那会把真实雷位暴露到诊断输出里，测试改用穷举匹配。
        match sessions.current() {
            Err(Error::Session) => {}
            _ => panic!("无会话时应返回 Error::Session"),
        }
    }

    #[test]
    fn 新开局会丢掉旧局面且编号递增() {
        let mut sessions = Sessions::new();
        let first = sessions.create(size(9, 10, 10), Rule::Win7);
        sessions.current().unwrap().reveal(0, 0, false).unwrap();
        let second = sessions.create(size(9, 10, 10), Rule::Win7);
        assert_eq!(second, first + 1);
        assert_eq!(sessions.current().unwrap().status, Status::Ready);
    }

    #[test]
    fn 认输后会话清空() {
        let mut sessions = sessions(size(9, 10, 10));
        assert!(sessions.current().is_ok());
        sessions.abandon();
        assert!(matches!(sessions.current(), Err(Error::Session)));
    }

    #[test]
    fn 八邻域已按边界裁剪() {
        assert_eq!(around(0, 0, 5, 5).len(), 3);
        assert_eq!(around(2, 2, 5, 5).len(), 8);
        assert_eq!(around(4, 4, 5, 5).len(), 3);
        assert!(!around(2, 2, 5, 5).contains(&(2, 2)));
    }

    /// 把 board 里的真实雷位收集出来（`board` 是 private，但本 mod 是 game.rs
    /// 的子模块，可以访问父模块私有项）。
    ///
    /// 只有测试用得到：UI 侧永远看不到真实雷位（PLAN 4.1），
    /// 所以"确定性通关"这类不变量只能在 game.rs 内部构造。
    fn mine_positions_of(game: &Game) -> Vec<(usize, usize)> {
        let board = game.board.as_ref().expect("首点之后 board 必须已生成");
        let mut out = Vec::new();
        for (row, line) in board.iter().enumerate() {
            for (col, value) in line.iter().enumerate() {
                if *value == msbridge::REAL_MINE {
                    out.push((row, col));
                }
            }
        }
        out
    }

    #[test]
    fn 确定性通关只记一次成绩() {
        // 这条不变量原先写在 rpc.rs 的「完整一局能通关并写出成绩」里，
        // 但那段断言被 `if status == "won"` 包着，而穷举点击经常踩雷 ⇒
        // 绝大多数运行根本不执行断言，只在少数通关时随机翻车（实测 25 轮红 1 轮）。
        // 这里用真实雷位避开所有雷，通关是**必然**的，断言每次都跑。
        let mut sessions = sessions(size(5, 5, 3));
        let game = sessions.current().unwrap();
        game.reveal(0, 0, false).unwrap();
        let mines = mine_positions_of(game);
        assert_eq!(mines.len(), 3);

        // **不要**在这里断言首点后仍是 Playing：3 颗雷若聚在角落，非雷的 22 格里
        // 会有大片连通的 0，flood 从角上一点就能把 22 格全开掉 ⇒ 首点直接通关。
        // 所以只保证"逐格翻开所有非雷格之后一定 Won"，与首点开掉多少无关。
        'outer: for row in 0..5 {
            for col in 0..5 {
                if game.status != Status::Playing {
                    break 'outer;
                }
                if mines.contains(&(row, col)) {
                    continue;
                }
                game.reveal(row, col, false).unwrap();
            }
        }
        assert_eq!(game.status, Status::Won, "翻开全部 22 个非雷格必然通关");
        assert_eq!(game.opened, 22);
        assert!(game.bbbv.is_some(), "通关时 3BV 必须已确定");

        // 成绩只能被取走一次：UI 反复拉 game/state 不该刷出多条
        let first = game.claim_win();
        assert!(first.is_some(), "通关后应能取到成绩");
        assert!(game.claim_win().is_none(), "同一局不得重复记账");
    }

    #[test]
    fn 未通关前取不到成绩() {
        let mut sessions = sessions(size(9, 10, 10));
        let game = sessions.current().unwrap();
        assert!(game.claim_win().is_none());
    }
}
