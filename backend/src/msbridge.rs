//! `ms_toollib` 的唯一出口。除本模块外，全项目任何地方都不得直接 use ms_toollib。
//!
//! 局面编码常量也放在这里，全项目只用这一套（PLAN 事实 21）。
//!
//! # feature 选择的坑（改这里务必先看一眼）
//! 阶段 0 的探针结论是 `default-features = false, features = ["rand","web-time"]`（剔 tract-onnx），
//! 它**能编译**，看起来是结果 A。但这条结论后来被证明是错的：
//! `get_board_1d` 只在 `#[cfg(any(feature = "py", feature = "rs"))]` 下洗牌（utils.rs:456-470），
//! 关掉 `rs` 后洗牌静默消失，埋雷退化成「所有雷按构造顺序堆在一维数组末尾」，二维化后就是
//! 整行或整列连成一排 —— 后果是首点一次 flood 就能开完全盘，游戏直接结束。
//! 所以本 crate **必须**带 `rs` 这一档（默认 feature 即 `rs`），代价是 `tract-onnx` 一起进来，
//! 这一点已作为修正后的事实回写进 PLAN 第二节。下面的 `tests` 里有对应的回归闸门。

use ms_toollib::{cal_bbbv, laymine, laymine_op, refresh_board};

use crate::game::Rule;

// —— 游戏局面（玩家可以看见的那一面）的编码 ——
/// 未打开
pub const OPEN: i32 = 10;
/// 雷：算法判定为雷 **或** 玩家插旗。二者语义不共存，所以共用同一个数字。
pub const FLAG: i32 = 11;
/// 算法已判定非雷、但玩家还没点开（模样仍是未打开）
pub const KNOWN_SAFE: i32 = 12;
/// 失败后显示的标错的雷（叉雷）
pub const WRONG_FLAG: i32 = 14;
/// 踩中的雷（红雷）
pub const HIT_MINE: i32 = 15;
/// 失败后显示出来的其他雷（白雷）
pub const OTHER_MINE: i32 = 16;
/// 压枪高亮造成的「看起来像 0」。本侧车**不产生**这个值，属于前端渲染态。
pub const CHORD_HIGHLIGHT: i32 = 18;

// —— 真实局面的编码 ——
/// 真实局面里代表雷
pub const REAL_MINE: i32 = -1;

/// 埋雷并算好数字。
///
/// **张量约定**：`ms_toollib` 内部 `x` = 行、`y` = 列，所以 `laymine(row, column, minenum, x0, y0)`
/// 里 `x0` 是**行**、`y0` 是**列**（与 PLAN 5.2 一致）。返回值已算过数字，可以直接交给 `refresh`。
pub fn lay(
    row: usize,
    column: usize,
    minenum: usize,
    x0: usize,
    y0: usize,
    rule: Rule,
) -> Result<Vec<Vec<i32>>, &'static str> {
    if row == 0 || column == 0 {
        return Err("局面尺寸为空");
    }
    // 首点坐标必须真的在盘内：`laymine` 会算 `pointer = x0 + y0*row` 并按它索引一维数组，
    // 越界坐标直接是 index panic；`laymine_op` 的邻域判断也会算错保留格数。
    if x0 >= row || y0 >= column {
        return Err("首点坐标越界");
    }
    if minenum == 0 || minenum >= row * column {
        return Err("雷数不合法：必须大于 0 且小于总格数");
    }
    // **第二道防线：挡住会让算法下溢的雷数。**
    // ms_toollib 的 get_board_1d 内部直接做 `vec![0; area - minenum]`，其中
    //   - laymine    ：area = row*column - 1
    //   - laymine_op ：area = row*column - area_op（首点所在的 3×3 邻域被裁边后：角 4 / 边 6 / 内部 9）
    // minenum 一旦超过 area 就是 usize 下溢 —— 轻则 panic，重则申请天量内存直接 abort。
    // 而官方 SDK 的 worker 没有 catch_unwind（见 rpc.rs），panic 的表现是
    // "侧车进程消失 + 本局凭空作废 + 前端干等到超时"，而不是一条可读的错误。
    // rpc 层的 clamp_size（雷数 ≤ 格数的 35%）已让正常路径不可能越界，
    // 这里是不依赖调用方的第二道自我防护；area_op 的取值与 laymine_op 源码逐条对齐。
    let area = match rule {
        Rule::FirstSafe => row * column - 1,
        Rule::Win7 => {
            let on_edge = x0 == 0 || y0 == 0 || x0 == row - 1 || y0 == column - 1;
            let on_corner =
                on_edge && ((x0 == 0 || x0 == row - 1) && (y0 == 0 || y0 == column - 1));
            let area_op = if on_corner {
                4
            } else if on_edge {
                6
            } else {
                9
            };
            row * column - area_op
        }
    };
    if minenum > area {
        return Err("雷数超出该首点位置可容纳的上限，换个尺寸或雷数");
    }
    let board = match rule {
        Rule::Win7 => laymine_op(row, column, minenum, x0, y0),
        Rule::FirstSafe => laymine(row, column, minenum, x0, y0),
    };
    // **这里不能再调 cal_board_numbers**：`laymine` / `laymine_op` 在铺雷时就把数字算好了
    // （源码 utils.rs:473-501 与 506-534，铺一个 -1 就给邻域做 += 1）。而
    // `cal_board_numbers` 不幂等（utils.rs:1607 对邻域内 >=0 的格继续 += 1），
    // 再调一次会让每个数字翻倍。PLAN 5.3 里"埋雷后调 cal_board_numbers"这条对
    // laymine* 不适用，已回写进 PLAN 第二节；它只在手工搭盘时才需要（0.2.0 的
    // board/from-array 会用，届时再加回来）。
    Ok(board)
}

/// 3BV。局面生成后即固定，不随操作变化。
#[allow(clippy::ptr_arg)]
pub fn bbbv(board: &Vec<Vec<i32>>) -> usize {
    // BoardSize 只为 Vec<Vec<i32>> 与其引用实现过，没有切片 impl，故参数是 &Vec。
    cal_bbbv(board)
}

/// 翻格 / flood-fill / 踩雷标记。
///
/// 实测语义（PLAN 2.4 B）：只做三件事——写数字、写 0 并在真值为 0 时向 8 邻域扩散
/// （只扩散值为 `OPEN` 或 `KNOWN_SAFE` 的格，**旗不会被 flood 打开**）、踩雷时写 `HIT_MINE`。
/// **写 `OTHER_MINE`(16) 与 `WRONG_FLAG`(14) 的时机不全由它负责**：14 只在「已有旗且标错」时写，
/// 16 它从来不写 ⇒ 失败后显示其他雷必须由上层自己刷。
#[allow(clippy::ptr_arg)]
pub fn refresh(
    board: &Vec<Vec<i32>>,
    game_board: &mut Vec<Vec<i32>>,
    clicked_poses: Vec<(usize, usize)>,
) {
    refresh_board(board, game_board, clicked_poses);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Size;
    use std::collections::HashSet;

    fn mine_positions(board: &[Vec<i32>]) -> Vec<(usize, usize)> {
        board
            .iter()
            .enumerate()
            .flat_map(|(row, cells)| {
                cells
                    .iter()
                    .enumerate()
                    .filter(|(_, value)| **value == REAL_MINE)
                    .map(move |(col, _)| (row, col))
            })
            .collect()
    }

    #[test]
    fn 埋雷必须是随机的而不是排成一整列() {
        // 这条是 feature 陷阱的回归闸门：如果哪天有人把 ms_toollib 的 feature 调回去，
        // 洗牌会静默失效、雷退化为固定布局，这条会先炸而不是等到上线。
        let size = Size::new(16, 30, 99);
        let mut layouts = HashSet::new();
        for _ in 0..20 {
            let board = lay(size.rows, size.cols, size.mine_num, 8, 15, Rule::Win7).unwrap();
            let positions = mine_positions(&board);
            assert_eq!(positions.len(), size.mine_num);
            layouts.insert(positions);
        }
        assert!(
            layouts.len() >= 18,
            "20 次埋雷只产生了 {} 种布局，疑似退化成确定性分布",
            layouts.len()
        );
    }

    #[test]
    fn 零格占比应在合理区间() {
        // 雷随机散开时零格约占非雷格的三到五成；退化成整块雷时会接近 100%
        //（此时一次 flood 就能开完全盘）。
        let rows = 16;
        let cols = 30;
        let mine_num = 99;
        let board = lay(rows, cols, mine_num, 8, 15, Rule::Win7).unwrap();
        let non_mine = rows * cols - mine_num;
        let zeros = board.iter().flatten().filter(|&&value| value == 0).count();
        assert!(
            zeros < non_mine * 7 / 10,
            "零格占比过高（{zeros}/{non_mine}），雷可能被挤到了一起"
        );
    }

    /// 邻域内的真实雷数。用来核对盘面上的数字有没有被算重。
    fn neighbour_mines(board: &[Vec<i32>], row: usize, col: usize) -> usize {
        let row_from = row.saturating_sub(1);
        let row_to = (row + 1).min(board.len() - 1);
        let mut count = 0;
        for (r, cells) in board.iter().enumerate().take(row_to + 1).skip(row_from) {
            let col_from = col.saturating_sub(1);
            let col_to = (col + 1).min(cells.len() - 1);
            for (c, value) in cells.iter().enumerate().take(col_to + 1).skip(col_from) {
                if (r, c) != (row, col) && *value == REAL_MINE {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn 埋雷返回的数字必须等于实际邻雷数() {
        // 专门守住"重复调用 cal_board_numbers"那个坑：一旦数字被算两遍，每个数字
        // 都会翻倍，压枪与胜负判定会跟着一起错。
        let rows = 16;
        let cols = 30;
        let mine_num = 99;
        for rule in [Rule::Win7, Rule::FirstSafe] {
            let board = lay(rows, cols, mine_num, 8, 15, rule).unwrap();
            for row in 0..rows {
                for col in 0..cols {
                    let value = board[row][col];
                    if value == REAL_MINE {
                        continue;
                    }
                    let actual = neighbour_mines(&board, row, col);
                    assert_eq!(
                        value, actual as i32,
                        "({row},{col}) 数字 {value} 与实际邻雷数 {actual} 不符"
                    );
                }
            }
        }
    }

    #[test]
    fn 雷数超出首点可容纳上限时报错而不是让算法下溢() {
        // 边界与 laymine_op 的 area_op 逐条对齐（5×5）：
        //   角上首点保留 4 格 ⇒ 可用 21；边上保留 6 ⇒ 19；内部保留 9 ⇒ 16
        assert!(
            lay(5, 5, 21, 0, 0, Rule::Win7).is_ok(),
            "角上首点可用 21 格"
        );
        assert!(
            lay(5, 5, 22, 0, 0, Rule::Win7).is_err(),
            "22 颗必须被拒而不是下溢"
        );
        assert!(
            lay(5, 5, 19, 0, 2, Rule::Win7).is_ok(),
            "边上首点可用 19 格"
        );
        assert!(lay(5, 5, 20, 0, 2, Rule::Win7).is_err());
        assert!(
            lay(5, 5, 16, 2, 2, Rule::Win7).is_ok(),
            "内部首点可用 16 格"
        );
        assert!(lay(5, 5, 17, 2, 2, Rule::Win7).is_err());
        // FirstSafe 只保留首点自己：area = 25-1 = 24
        assert!(lay(5, 5, 24, 2, 2, Rule::FirstSafe).is_ok());
    }

    #[test]
    fn 首点坐标越界报错而不是让算法索引越界() {
        assert!(lay(5, 5, 3, 5, 0, Rule::Win7).is_err());
        assert!(lay(5, 5, 3, 0, 5, Rule::Win7).is_err());
        assert!(
            lay(5, 5, 3, 4, 4, Rule::Win7).is_ok(),
            "右下角本身是合法首点"
        );
    }

    #[test]
    fn 非法雷数直接报错而不是崩() {
        assert!(lay(9, 10, 0, 0, 0, Rule::Win7).is_err());
        assert!(lay(9, 10, 90, 0, 0, Rule::Win7).is_err());
        assert!(lay(0, 10, 1, 0, 0, Rule::Win7).is_err());
    }
}
