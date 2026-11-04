//! 下发给 UI 的快照 DTO。
//!
//! `game/*` 一律返回**全量快照**，不做增量 diff——避免前后端两份语义实现不一致这类最难查的 bug（PLAN 5.2）。

use serde::Serialize;

use crate::game::Game;
use crate::msbridge;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub game_id: u64,
    pub rows: usize,
    pub cols: usize,
    pub mine_num: usize,
    pub rule: String,
    pub status: String,
    /// 游戏局面。**绝不包含真实雷位 `-1`**（泄露红线，PLAN 7.1）
    pub cells: Vec<Vec<i32>>,
    /// 问号标记。`ms_toollib` 没这一档，单独下发，不自造数字码。
    pub marks: Vec<(usize, usize)>,
    pub opened: usize,
    pub mine_left: i64,
    /// `ready` 阶段为 `null`（此时局面还没生成，3BV 尚不存在）
    pub bbbv: Option<usize>,
    /// 侧车权威用时；UI 的本地 tick 只负责显示，终局以本字段为准
    pub elapsed_ms: u64,
}

pub fn from_game(game: &Game) -> Snapshot {
    Snapshot {
        game_id: game.id,
        rows: game.rows,
        cols: game.cols,
        mine_num: game.mine_num,
        rule: game.rule.as_str().to_string(),
        status: game.status.as_str().to_string(),
        cells: game.cells.clone(),
        marks: game.marks.clone(),
        opened: game.opened,
        mine_left: game.mine_left(),
        bbbv: game.bbbv,
        elapsed_ms: game.elapsed_ms,
    }
}

/// 泄露红线自检：下发的每一个格值都必须落在算法定义的公开编码集合里，
/// 真值 `-1` 一旦出现在 `cells` 就等于把答案发给了前端。
///
/// 返回 `Option`（而不是内部 `assert!`）的原因：官方 `dbx-plugin-sdk` 的 worker
/// **没有 `catch_unwind`**（lib.rs 全文搜不到），自检一旦 panic 会掀掉整个侧车进程——
/// 那条本该报"内部错误"的响应永远写不出去，前端干等到超时，本局还凭空消失。
/// 自检不该比它保护的东西更脆。调用方（rpc.rs）负责把 `Some(值)` 转成错误 + 写 stderr；
/// 这样 debug 与 release 的行为完全一致。
pub fn leaked_cell(snapshot: &Snapshot) -> Option<i32> {
    const ALLOWED: [i32; 12] = [
        0,
        1,
        2,
        3,
        4,
        5,
        6,
        7,
        8,
        msbridge::OPEN,
        msbridge::FLAG,
        msbridge::KNOWN_SAFE,
    ];
    const FAILURE: [i32; 4] = [
        msbridge::WRONG_FLAG,
        msbridge::HIT_MINE,
        msbridge::OTHER_MINE,
        msbridge::CHORD_HIGHLIGHT,
    ];
    for row in &snapshot.cells {
        for value in row {
            if !ALLOWED.contains(value) && !FAILURE.contains(value) {
                return Some(*value);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{FlagMode, Sessions};
    use crate::store::Size;

    #[test]
    fn ready_阶段没有三bv() {
        let mut sessions = Sessions::new();
        sessions.create(Size::new(9, 10, 10), crate::game::Rule::Win7);
        let snapshot = from_game(sessions.current().unwrap());
        assert_eq!(snapshot.status, "ready");
        assert_eq!(snapshot.bbbv, None);
        assert_eq!(snapshot.elapsed_ms, 0);
        assert_eq!(snapshot.opened, 0);
        assert_eq!(snapshot.mine_left, 10);
        assert!(snapshot
            .cells
            .iter()
            .flatten()
            .all(|&v| v == msbridge::OPEN));
    }

    #[test]
    fn 翻格后快照不再泄露真实雷位() {
        let mut sessions = Sessions::new();
        sessions.create(Size::new(9, 10, 10), crate::game::Rule::Win7);
        let game = sessions.current().unwrap();
        // 首点之前是 ready（不是 playing），所以这里的循环不能拿 playing 当闸门。
        for row in 0..9 {
            for col in 0..10 {
                let _ = game.reveal(row, col, false);
                assert!(leaked_cell(&from_game(game)).is_none());
            }
        }
        let snapshot = from_game(game);
        assert!(snapshot.bbbv.is_some(), "首点之后 3BV 必须存在");
        assert!(snapshot.opened > 0);
        assert!(matches!(
            game.status,
            crate::game::Status::Playing | crate::game::Status::Won | crate::game::Status::Lost
        ));
    }

    #[test]
    fn 踩雷后有且仅有一处红雷且补了白雷() {
        let mut sessions = Sessions::new();
        sessions.create(Size::new(5, 5, 3), crate::game::Rule::Win7);
        let game = sessions.current().unwrap();
        // 先把雷翻出来：连续翻直到踩雷
        'outer: for row in 0..5 {
            for col in 0..5 {
                let _ = game.reveal(row, col, false);
                if game.status == crate::game::Status::Lost {
                    break 'outer;
                }
            }
        }
        if game.status == crate::game::Status::Lost {
            let snapshot = from_game(game);
            assert!(leaked_cell(&snapshot).is_none());
            let hit = snapshot
                .cells
                .iter()
                .flatten()
                .filter(|&&v| v == msbridge::HIT_MINE)
                .count();
            let others = snapshot
                .cells
                .iter()
                .flatten()
                .filter(|&&v| v == msbridge::OTHER_MINE)
                .count();
            assert_eq!(hit, 1, "红雷必须恰好一处");
            assert_eq!(hit + others, 3, "三颗雷都要显示出来");
            assert_eq!(snapshot.status, "lost");
        }
    }

    #[test]
    fn 问号不占困旗额度() {
        let mut sessions = Sessions::new();
        sessions.create(Size::new(9, 10, 10), crate::game::Rule::Win7);
        let game = sessions.current().unwrap();
        game.flag(0, 0, FlagMode::Flag).unwrap();
        let after_flag = from_game(game).mine_left;
        game.flag(0, 0, FlagMode::Clear).unwrap();
        game.flag(0, 0, FlagMode::Mark).unwrap();
        assert_eq!(from_game(game).mine_left, after_flag + 1);
        assert_eq!(from_game(game).marks, vec![(0, 0)]);
    }
}
