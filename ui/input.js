/**
 * 指针与键盘输入。
 *
 * 手势约定（README 6 / PLAN 4.5）：
 * - 压枪只认左右键同时按住（`e.buttons === 3`），**不用 dblclick**——高频连点下
 *   dblclick 既误判又丢事件；也绝不把"单左键点已开格"当成压枪（那是一次回归：
 *   九宫格高亮闪烁 + "周围旗数与该数字不符"）；
 * - 左键在 pointerup 才生效（拖出取消）；对已翻开的格子单左键 = 无反应（经典扫雷语义）；
 * - 右键对已翻开的数字格 = 无反应：不进入旗循环、不发旗 RPC、不报错；
 * - `contextmenu` 必须 preventDefault，否则宿主弹浏览器原生右键菜单；
 * - 键盘只挂在工作台容器上，**不挂全局 document**，避免抢宿主快捷键。
 */
window.DS = window.DS || {};
DS.input = (function () {
  'use strict';

  var FLAG = 11;
  var OPEN = 10;

  var board = null;
  var stage = null;
  var pending = null;
  var drag = null;
  var chordPreview = null;

  function swallow(promise) {
    if (promise && typeof promise.then === 'function') {
      promise.then(null, function () {
        /* 失败由 game.js 的 notice 统一提示，这里不重复处理 */
      });
    }
  }

  function snapshot() {
    return DS.game.getSnapshot();
  }

  function isOver() {
    var snap = snapshot();
    return !!snap && (snap.status === 'won' || snap.status === 'lost');
  }

  /** 已翻开的数字格（0-8）：普通左键无反应、普通右键无反应，只有双键压枪。 */
  function isOpenedNumber(snap, row, col) {
    var value = snap.cells[row][col];
    return value >= 0 && value <= 8;
  }

  function isMarked(snap, row, col) {
    var marks = snap.marks || [];
    for (var i = 0; i < marks.length; i += 1) {
      if (marks[i][0] === row && marks[i][1] === col) {
        return true;
      }
    }
    return false;
  }

  /** 旗 → 问号 → 空 的循环。已翻开的格子返回 null（无反应，不做财产切换）。 */
  function cycleMode(row, col) {
    var snap = snapshot();
    if (!snap) {
      return 'flag';
    }
    if (isOpenedNumber(snap, row, col)) {
      return null;
    }
    if (isMarked(snap, row, col)) {
      return 'clear';
    }
    var prefs = DS.game.getPrefs();
    var useMarks = !prefs || prefs.useMarks !== false;
    if (snap.cells[row][col] === FLAG) {
      return useMarks ? 'mark' : 'clear';
    }
    return 'flag';
  }

  function chordPreviewCells(row, col) {
    var snap = snapshot();
    if (!snap) {
      return [];
    }
    var value = snap.cells[row][col];
    if (value < 1 || value > 8) {
      return [];
    }
    var cells = [];
    for (var r = row - 1; r <= row + 1; r += 1) {
      for (var c = col - 1; c <= col + 1; c += 1) {
        if (r === row && c === col) {
          continue;
        }
        if (r < 0 || c < 0 || r >= snap.rows || c >= snap.cols) {
          continue;
        }
        if (snap.cells[r][c] === OPEN) {
          cells.push([r, c]);
        }
      }
    }
    return cells;
  }

  function onContextMenu(event) {
    event.preventDefault();
  }

  function onPointerDown(event) {
    var cell = DS.canvas.hitTest(event.clientX, event.clientY);
    if (!cell) {
      return;
    }
    DS.game.setFocus(cell);
    DS.canvas.setFocus(cell);
    board.focus();

    var snap = snapshot();

    // 压枪：左右键同时按住（buttons === 3），只对已翻开的数字格生效。
    // 判据用位掩码而不是维护"左右都按过"的状态位；单左键点已开格绝不落进这里。
    if (event.buttons === 3) {
      pending = null;
      if (isOver()) {
        return;
      }
      if (!snap || !isOpenedNumber(snap, cell[0], cell[1])) {
        return;
      }
      chordPreview = cell;
      DS.canvas.setChordPreview(chordPreviewCells(cell[0], cell[1]));
      swallow(DS.game.chord(cell[0], cell[1]));
      return;
    }

    if (event.button === 2) {
      if (isOver()) {
        return;
      }
      // 已翻开的格子上右键静默无反应：不发旗 RPC、不弹错误（旗的语义是"这是雷"，
      // 在已开格上插旗是矛盾操作，弹 -32011 只会污染体验）。
      var mode = cycleMode(cell[0], cell[1]);
      if (!mode) {
        return;
      }
      drag = { mode: mode, visited: {} };
      drag.visited[cell[0] + ':' + cell[1]] = true;
      swallow(DS.game.flag(cell[0], cell[1], mode));
      return;
    }

    if (event.button === 0) {
      // 左键在 pointerup 才生效，允许拖出去取消（与经典扫雷一致）。
      // 已翻开的格子最终在 pointerup 处被拦截（见下），不发 reveal。
      pending = { cell: cell, force: event.ctrlKey || event.metaKey };
    }
  }

  function onPointerMove(event) {
    if (!drag || (event.buttons & 2) !== 2) {
      return;
    }
    var cell = DS.canvas.hitTest(event.clientX, event.clientY);
    if (!cell) {
      return;
    }
    var key = cell[0] + ':' + cell[1];
    if (drag.visited[key]) {
      return;
    }
    drag.visited[key] = true;
    DS.game.setFocus(cell);
    DS.canvas.setFocus(cell);
    if (!isOver()) {
      swallow(DS.game.flag(cell[0], cell[1], drag.mode));
    }
  }

  function onPointerUp(event) {
    if (chordPreview) {
      chordPreview = null;
      DS.canvas.setChordPreview(null);
      pending = null;
      return;
    }
    if (drag) {
      drag = null;
      return;
    }
    if (!pending) {
      return;
    }
    var cell = DS.canvas.hitTest(event.clientX, event.clientY);
    if (!cell || cell[0] !== pending.cell[0] || cell[1] !== pending.cell[1]) {
      pending = null;
      return;
    }
    var snap = snapshot();
    var force = pending.force;
    pending = null;
    if (isOver()) {
      return;
    }
    // 已翻开的格子上单左键无反应（经典扫雷语义）；旗格的 force 语义交后端处理。
    if (snap && snap.cells[cell[0]][cell[1]] >= 0 && snap.cells[cell[0]][cell[1]] <= 8) {
      return;
    }
    swallow(DS.game.reveal(cell[0], cell[1], force));
  }

  function onPointerCancel() {
    pending = null;
    drag = null;
    chordPreview = null;
    DS.canvas.setChordPreview(null);
  }

  function moveFocus(deltaRow, deltaCol) {
    var snap = snapshot();
    if (!snap) {
      return;
    }
    var focus = DS.game.getFocus() || [0, 0];
    var row = Math.min(snap.rows - 1, Math.max(0, focus[0] + deltaRow));
    var col = Math.min(snap.cols - 1, Math.max(0, focus[1] + deltaCol));
    DS.game.setFocus([row, col]);
    DS.canvas.setFocus([row, col]);
  }

  function onKeyDown(event) {
    var handled = true;
    var focus = DS.game.getFocus();

    if (event.key === 'ArrowUp') {
      moveFocus(-1, 0);
    } else if (event.key === 'ArrowDown') {
      moveFocus(1, 0);
    } else if (event.key === 'ArrowLeft') {
      moveFocus(0, -1);
    } else if (event.key === 'ArrowRight') {
      moveFocus(0, 1);
    } else if (event.key === 'Enter') {
      if (focus && !isOver()) {
        swallow(DS.game.reveal(focus[0], focus[1], event.ctrlKey || event.metaKey));
      }
    } else if (event.key === ' ' || event.key === 'Spacebar') {
      if (focus && !isOver()) {
        var mode = cycleMode(focus[0], focus[1]);
        if (mode) {
          swallow(DS.game.flag(focus[0], focus[1], mode));
        }
      }
    } else if (event.key === 'F2' || (event.key === 'r' && (event.ctrlKey || event.metaKey))) {
      swallow(DS.panels.restart());
    } else if (event.key === 'Escape') {
      swallow(DS.panels.giveUp());
    } else {
      handled = false;
    }

    if (handled) {
      event.preventDefault();
    }
  }

  function attach(boardEl, stageEl) {
    board = boardEl;
    stage = stageEl;
    board.setAttribute('tabindex', '0');
    board.addEventListener('contextmenu', onContextMenu);
    board.addEventListener('pointerdown', onPointerDown);
    board.addEventListener('pointermove', onPointerMove);
    board.addEventListener('pointerup', onPointerUp);
    board.addEventListener('pointercancel', onPointerCancel);
    board.addEventListener('pointerleave', onPointerCancel);
    stage.addEventListener('keydown', onKeyDown);
  }

  return { attach: attach };
})();