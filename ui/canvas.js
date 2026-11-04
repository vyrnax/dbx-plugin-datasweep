/**
 * Canvas 渲染层。
 *
 * 选 Canvas 而不是 DOM 的唯一理由：最大 50×50 = 2500 格，DOM 方案在连点 + 拖拽插旗
 * 下必然掉帧（PLAN 4.5）。
 *
 * 要点：
 * - 按 devicePixelRatio 缩放，避免高分屏发虚；
 * - 格子边长由容器自适应，下限 12px；撑不下时容器滚动而不是无限缩小；
 * - 颜色全部取自 `styles.css` 里的 `--ds-*` 变量，明暗跟随宿主，不自建配色体系；
 * - 只有变化过的格子重绘（flood-fill 一次开一片，脏区重绘收益明显）。
 *
 * ES5 风格 IIFE，无构建步骤。
 */
window.DS = window.DS || {};
DS.canvas = (function () {
  'use strict';

  var MIN_CELL = 12;
  // 上限放宽到 96：格子少的时候自动放大、多的时候缩小，棋盘铺满除右侧栏以外的区域。
  // 留一个上限是为了防止在超宽屏上把 9×10 撑成每个格子一两百像素的荒谬画面。
  var MAX_CELL = 96;
  var PAD = 6;

  // 游戏局面编码，与 backend/src/msbridge.rs 逐条对应
  var OPEN = 10;
  var FLAG = 11;
  var KNOWN_SAFE = 12;
  var WRONG_FLAG = 14;
  var HIT_MINE = 15;
  var OTHER_MINE = 16;
  var CHORD_HIGHLIGHT = 18;

  var canvas = null;
  var ctx = null;
  var wrap = null;
  var snapshot = null;
  var previous = null;
  var palette = null;
  var cell = 24;
  var dpr = 1;
  var boardWidth = 0;
  var boardHeight = 0;
  var focus = null;
  var chordCells = null;
  var observer = null;
  var frame = 0;

  function readPalette() {
    var style = window.getComputedStyle(document.documentElement);
    function read(name, fallback) {
      var value = style.getPropertyValue(name);
      value = value ? String(value).trim() : '';
      return value || fallback;
    }
    palette = {
      face: read('--ds-face', '#c4c4c4'),
      faceHi: read('--ds-face-hi', '#ffffff'),
      faceLo: read('--ds-face-lo', '#7b7b7b'),
      open: read('--ds-open', '#d6d6d6'),
      grid: read('--ds-grid', '#8f8f8f'),
      text: read('--ds-text', '#1b1b1b'),
      flag: read('--ds-flag', '#c81e1e'),
      mine: read('--ds-mine', '#1b1b1b'),
      hit: read('--ds-hit', '#e03131'),
      focus: read('--ds-focus', '#4c8bf5'),
      // 数字用等宽字体：宿主换了 --font-mono 也要跟着换，不能在这里抄死一份
      mono: read('--ds-font-mono', 'ui-monospace, Consolas, monospace'),
      numbers: [
        null,
        read('--ds-n1', '#1971c2'),
        read('--ds-n2', '#2f9e44'),
        read('--ds-n3', '#e03131'),
        read('--ds-n4', '#5f3dc4'),
        read('--ds-n5', '#a61e4d'),
        read('--ds-n6', '#0c8599'),
        read('--ds-n7', '#343a40'),
        read('--ds-n8', '#868e96')
      ]
    };
  }

  function applyCanvasSize() {
    canvas.style.width = boardWidth + PAD * 2 + 'px';
    canvas.style.height = boardHeight + PAD * 2 + 'px';
    canvas.width = Math.round((boardWidth + PAD * 2) * dpr);
    canvas.height = Math.round((boardHeight + PAD * 2) * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }

  function relayout() {
    if (!snapshot || !wrap) {
      return;
    }
    // 必须用 clientWidth/clientHeight：它**不含** border 与滚动条。
    // 早先用 getBoundingClientRect() 会把 1px 边框也算进可用宽度，于是算出的
    // canvas 可能比内容区宽 2px ⇒ 出滚动条 ⇒ 可用宽度又变小 ⇒ ResizeObserver
    // 再触发一次 relayout…… 尺寸在两个值之间来回跳（格子越大越容易踩到）。
    var availableWidth = Math.max(0, wrap.clientWidth - PAD * 2);
    var availableHeight = Math.max(0, wrap.clientHeight - PAD * 2);
    var size = Math.floor(Math.min(availableWidth / snapshot.cols, availableHeight / snapshot.rows));
    if (!isFinite(size) || size < 1) {
      size = MIN_CELL;
    }
    if (size > MAX_CELL) {
      size = MAX_CELL;
    }
    if (size < MIN_CELL) {
      size = MIN_CELL;
    }
    cell = size;
    boardWidth = cell * snapshot.cols;
    boardHeight = cell * snapshot.rows;
    dpr = window.devicePixelRatio || 1;
    applyCanvasSize();
  }

  function drawTile(x, y, size) {
    // 未打开的格子：左上高光 + 右下阴影，保留经典凸起感
    ctx.fillStyle = palette.face;
    ctx.fillRect(x, y, size, size);
    ctx.fillStyle = palette.faceHi;
    ctx.fillRect(x, y, size, 2);
    ctx.fillRect(x, y, 2, size);
    ctx.fillStyle = palette.faceLo;
    ctx.fillRect(x, y + size - 2, size, 2);
    ctx.fillRect(x + size - 2, y, 2, size);
  }

  function drawOpenFace(x, y, size) {
    ctx.fillStyle = palette.open;
    ctx.fillRect(x, y, size, size);
    ctx.strokeStyle = palette.grid;
    ctx.lineWidth = 1;
    ctx.strokeRect(x + 0.5, y + 0.5, size - 1, size - 1);
  }

  function drawMineGlyph(x, y, size) {
    var cx = x + size / 2;
    var cy = y + size / 2;
    var radius = Math.max(2, size * 0.22);
    ctx.fillStyle = palette.mine;
    ctx.beginPath();
    ctx.arc(cx, cy, radius, 0, Math.PI * 2);
    ctx.fill();
    ctx.strokeStyle = palette.mine;
    ctx.lineWidth = Math.max(1, size * 0.06);
    ctx.beginPath();
    ctx.moveTo(cx - radius * 1.9, cy);
    ctx.lineTo(cx + radius * 1.9, cy);
    ctx.moveTo(cx, cy - radius * 1.9);
    ctx.lineTo(cx, cy + radius * 1.9);
    ctx.moveTo(cx - radius * 1.35, cy - radius * 1.35);
    ctx.lineTo(cx + radius * 1.35, cy + radius * 1.35);
    ctx.moveTo(cx - radius * 1.35, cy + radius * 1.35);
    ctx.lineTo(cx + radius * 1.35, cy - radius * 1.35);
    ctx.stroke();
    ctx.fillStyle = palette.faceHi;
    ctx.beginPath();
    ctx.arc(cx - radius * 0.3, cy - radius * 0.3, Math.max(1, radius * 0.28), 0, Math.PI * 2);
    ctx.fill();
  }

  function drawFlagGlyph(x, y, size) {
    var cx = x + size / 2;
    var cy = y + size / 2;
    var unit = size / 12;
    ctx.strokeStyle = palette.text;
    ctx.lineWidth = Math.max(1, unit);
    ctx.beginPath();
    ctx.moveTo(cx - unit, cy + unit * 4);
    ctx.lineTo(cx + unit, cy + unit * 4);
    ctx.moveTo(cx, cy - unit * 4);
    ctx.lineTo(cx, cy + unit * 4);
    ctx.stroke();
    ctx.fillStyle = palette.flag;
    ctx.beginPath();
    ctx.moveTo(cx + unit, cy - unit * 4);
    ctx.lineTo(cx + unit * 4.5, cy - unit * 2.2);
    ctx.lineTo(cx + unit, cy - unit * 0.4);
    ctx.closePath();
    ctx.fill();
  }

  function drawNumber(x, y, size, value) {
    ctx.fillStyle = palette.numbers[value] || palette.text;
    ctx.font = '700 ' + Math.max(8, Math.floor(size * 0.62)) + 'px ' + palette.mono;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(String(value), x + size / 2, y + size / 2 + 1);
  }

  function drawMarkGlyph(x, y, size) {
    ctx.fillStyle = palette.text;
    ctx.font = '700 ' + Math.max(8, Math.floor(size * 0.62)) + 'px ' + palette.mono;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText('?', x + size / 2, y + size / 2 + 1);
  }

  function isMarked(row, col, marks) {
    for (var i = 0; i < marks.length; i += 1) {
      if (marks[i][0] === row && marks[i][1] === col) {
        return true;
      }
    }
    return false;
  }

  function drawCell(row, col) {
    if (!snapshot) {
      return;
    }
    var code = snapshot.cells[row][col];
    var x = PAD + col * cell;
    var y = PAD + row * cell;
    var size = cell;

    ctx.clearRect(x, y, size, size);

    if (code === OPEN || code === KNOWN_SAFE) {
      drawTile(x, y, size);
      if (snapshot.marks && isMarked(row, col, snapshot.marks)) {
        drawMarkGlyph(x, y, size);
      }
    } else if (code === FLAG) {
      drawTile(x, y, size);
      drawFlagGlyph(x, y, size);
    } else if (code === WRONG_FLAG) {
      drawOpenFace(x, y, size);
      drawMineGlyph(x, y, size);
      ctx.strokeStyle = palette.hit;
      ctx.lineWidth = Math.max(1, size * 0.09);
      ctx.beginPath();
      ctx.moveTo(x + size * 0.18, y + size * 0.18);
      ctx.lineTo(x + size * 0.82, y + size * 0.82);
      ctx.moveTo(x + size * 0.82, y + size * 0.18);
      ctx.lineTo(x + size * 0.18, y + size * 0.82);
      ctx.stroke();
    } else if (code === HIT_MINE) {
      ctx.fillStyle = palette.hit;
      ctx.fillRect(x, y, size, size);
      drawMineGlyph(x, y, size);
    } else if (code === OTHER_MINE) {
      drawOpenFace(x, y, size);
      drawMineGlyph(x, y, size);
    } else if (code === CHORD_HIGHLIGHT) {
      // 压枪高亮：看起来像 0，但不参与判定
      drawOpenFace(x, y, size);
    } else if (code >= 0 && code <= 8) {
      drawOpenFace(x, y, size);
      if (code > 0) {
        drawNumber(x, y, size, code);
      }
    } else {
      drawTile(x, y, size);
    }

    if (chordCells) {
      for (var i = 0; i < chordCells.length; i += 1) {
        if (chordCells[i][0] === row && chordCells[i][1] === col) {
          drawOpenFace(x, y, size);
          break;
        }
      }
    }

    if (focus && focus[0] === row && focus[1] === col) {
      ctx.strokeStyle = palette.focus;
      ctx.lineWidth = 2;
      ctx.setLineDash([3, 2]);
      ctx.strokeRect(x + 1, y + 1, size - 2, size - 2);
      ctx.setLineDash([]);
    }
  }

  function drawAll() {
    if (!snapshot) {
      return;
    }
    readPalette();
    ctx.clearRect(0, 0, boardWidth + PAD * 2, boardHeight + PAD * 2);
    for (var row = 0; row < snapshot.rows; row += 1) {
      for (var col = 0; col < snapshot.cols; col += 1) {
        drawCell(row, col);
      }
    }
    previous = fingerprint();
  }

  /** 每格的"外观指纹"：格值 + 是否有问号。指纹变了才重绘这一格。 */
  function fingerprint() {
    var rows = [];
    for (var row = 0; row < snapshot.rows; row += 1) {
      var line = [];
      for (var col = 0; col < snapshot.cols; col += 1) {
        line.push(snapshot.cells[row][col] + (isMarked(row, col, snapshot.marks) ? 100 : 0));
      }
      rows.push(line);
    }
    return rows;
  }

  function drawChanged() {
    var next = fingerprint();
    var sameShape =
      previous && previous.length === next.length && previous[0].length === next[0].length;
    if (!sameShape) {
      drawAll();
      return;
    }
    for (var row = 0; row < next.length; row += 1) {
      for (var col = 0; col < next[row].length; col += 1) {
        if (previous[row][col] !== next[row][col]) {
          drawCell(row, col);
        }
      }
    }
    previous = next;
  }

  function mount(canvasEl, wrapEl) {
    canvas = canvasEl;
    wrap = wrapEl;
    ctx = canvas.getContext('2d');
    readPalette();
    if (typeof window.ResizeObserver === 'function') {
      observer = new window.ResizeObserver(function () {
        if (frame) {
          return;
        }
        frame = window.requestAnimationFrame(function () {
          frame = 0;
          relayout();
          drawAll();
        });
      });
      observer.observe(wrap);
    }
  }

  /** 主题切换后必须整体重画：调色板是从 CSS 变量里读的。 */
  function refreshTheme() {
    readPalette();
    drawAll();
  }

  function setSnapshot(next) {
    var shapeChanged =
      !snapshot ||
      snapshot.rows !== next.rows ||
      snapshot.cols !== next.cols ||
      snapshot.gameId !== next.gameId;
    snapshot = next;
    if (shapeChanged) {
      relayout();
      drawAll();
      return;
    }
    drawChanged();
  }

  function setFocus(next) {
    var old = focus;
    focus = next;
    if (old) {
      drawCell(old[0], old[1]);
    }
    if (next) {
      drawCell(next[0], next[1]);
    }
  }

  function getFocus() {
    return focus;
  }

  /** 压枪按住时高亮周围格子；松手传 null 取消。 */
  function setChordPreview(cells) {
    var affected = (chordCells || []).concat(cells || []);
    chordCells = cells;
    for (var i = 0; i < affected.length; i += 1) {
      drawCell(affected[i][0], affected[i][1]);
    }
  }

  function hitTest(clientX, clientY) {
    if (!snapshot || !canvas) {
      return null;
    }
    var rect = canvas.getBoundingClientRect();
    var x = clientX - rect.left - PAD;
    var y = clientY - rect.top - PAD;
    var col = Math.floor(x / cell);
    var row = Math.floor(y / cell);
    if (row < 0 || col < 0 || row >= snapshot.rows || col >= snapshot.cols) {
      return null;
    }
    return [row, col];
  }

  return {
    mount: mount,
    setSnapshot: setSnapshot,
    setFocus: setFocus,
    getFocus: getFocus,
    setChordPreview: setChordPreview,
    refreshTheme: refreshTheme,
    hitTest: hitTest,
    relayout: relayout,
    cellSize: function () {
      return cell;
    }
  };
})();
