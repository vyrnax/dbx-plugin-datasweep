/**
 * 顶部工具条与右侧面板：秒表、剩余雷数、3BV、成绩、设置。
 *
 * 计时口径：本地每 100ms tick 只负责显示跳动；终局数字取侧车返回的 elapsedMs
 * （PLAN 4.4 —— 成绩若由前端上报就能被任意篡改）。
 */
window.DS = window.DS || {};
DS.panels = (function () {
  'use strict';

  var TICK_MS = 100;

  var els = {};
  var tickHandle = 0;
  var envHandle = 0;
  var envSig = '';
  var lastAppearance = '';
  var records = [];
  var lastStatus = '';
  var lastGameId = null;
  var starting = false;
  var pendingRestart = false;
  var running = null;

  function $(id) {
    return document.getElementById(id);
  }

  function cacheElements() {
    els.preset = $('selPreset');
    els.rows = $('inpRows');
    els.cols = $('inpCols');
    els.mines = $('inpMines');
    els.apply = $('btnApply');
    els.customRow = $('rowCustom');
    els.time = $('statTime');
    els.minesLeft = $('statMines');
    els.bbbv = $('statBbbv');
    els.bbbvs = $('statBbbvs');
    els.opened = $('statOpened');
    els.status = $('statStatus');
    els.recordsBody = $('bodyRecords');
    els.recordsDim = $('lblRecordsDim');
    els.clearRecords = $('btnClearRecords');
    els.mineHint = $('hintMineRange');
    els.marks = $('chkMarks');
    els.panelRecords = $('chkPanelRecords');
    els.notice = $('txtNotice');
  }

  function applyLocale() {
    var nodes = document.querySelectorAll('[data-i18n]');
    for (var i = 0; i < nodes.length; i += 1) {
      nodes[i].textContent = DS.i18n.t(nodes[i].getAttribute('data-i18n'));
    }
    var options = document.querySelectorAll('[data-i18n-option]');
    for (var j = 0; j < options.length; j += 1) {
      options[j].textContent = DS.i18n.t(options[j].getAttribute('data-i18n-option'));
    }
    if (els.preset) {
      els.preset.title = DS.i18n.t('preset');
    }
    // counter bar 里的 LED 只有数字没有文字标签，读屏靠 aria-label 才知道是什么
    // （time / mines 两个 i18n 键因此仍在用，只是从可见标签变成无障碍标签）
    if (els.time) {
      els.time.setAttribute('aria-label', DS.i18n.t('time'));
    }
    if (els.minesLeft) {
      els.minesLeft.setAttribute('aria-label', DS.i18n.t('mines'));
    }
    // 笑脸按钮现在只剩图标，可读名称必须靠 title（配合 HTML 里的 sr-only 文案）
    if (els.apply) {
      els.apply.title = DS.i18n.t('apply');
    }
    // 语言切换要连页面标题与 lang 一起改，不然宿主里看着是两套语言
    document.title = DS.i18n.t('appTitle');
    document.documentElement.lang = DS.i18n.current() === 'zh' ? 'zh-CN' : 'en';
    render(DS.game.getSnapshot());
    renderRecords(records, DS.game.getSnapshot());
    updateMineHint();
  }

  /**
   * 跟随宿主的明暗外观。
   *
   * 【为什么不能只靠 CSS 变量透传】：本项目的配色是 `--ds-*` 这一套自有的经典扫雷色板
   * （凸起灰面 + 1..8 的数字色），它必须**成对**切换。宿主的 `--color-*` 若逐条盖进来，
   * 盖一半就会得到"浅底浅字"这种没法玩的颜色。所以这里只把 appearance 翻成 `data-theme`，
   * 由 styles.css 的 `:root` / `[data-theme='dark']` 两套完整色板接管。
   *
   * 宿主投递的 `--radius-*` / `--font-*` 则不同：只影响圆角与字体、不涉及对比度，
   * 因此由宿主注入到文档根元素、被 `--ds-radius*` / `--ds-font*` 的 var() 兜底式引用吸收
   * （见 styles.css 的 :root），这条路径不需要 JS 参与。
   */
  function applyAppearance() {
    var appearance = DS.bridge.theme();
    var changed = appearance !== lastAppearance;
    lastAppearance = appearance;
    document.documentElement.setAttribute('data-theme', appearance === 'dark' ? 'dark' : 'light');
    return changed;
  }

  /** 环境指纹：语言 + 明暗 + token 数量。宿主换调色板时数量通常也会变。 */
  function envSignature() {
    var theme = DS.bridge.themeObject();
    var tokens = theme.tokens || {};
    return String(DS.bridge.locale()) + '|' + theme.appearance + '|' + Object.keys(tokens).length;
  }

  /**
   * 宿主环境变化（明暗 / 调色板 / 字体 / 圆角 / 语言）的唯一入口，幂等。
   *
   * 三个触发源，缺一不可：
   *   1. `dbx-plugin-env` 事件 —— window + document 双注册且 capture=true（见 bridge.onEnv）
   *   2. `onInit` / `ready` —— 有些宿主只在桥就绪时投一次环境，之后不再发 env
   *   3. 1.5 秒指纹轮询 —— 事件通道没有版本保证，而「主题不跟随」是**静默失效**：
   *      界面看着完全正常，用户只会以为插件不支持深色，是最难排查的一类问题
   * 三个源都收敛到这一个函数，保证行为一致。
   */
  function applyEnv() {
    var changed = applyAppearance();
    if (DS.bridge.locale()) {
      DS.i18n.setLocale(DS.bridge.locale());
    }
    applyLocale();
    if (changed) {
      // 调色板是从 CSS 变量里读的，明暗翻了必须整体重画
      DS.canvas.refreshTheme();
    }
    envSig = envSignature();
  }

  function startEnvWatch() {
    DS.bridge.onEnv(applyEnv);
    DS.bridge.onInit(applyEnv);
    if (envHandle) {
      return;
    }
    envSig = envSignature();
    envHandle = window.setInterval(function () {
      try {
        if (envSignature() === envSig) {
          return;
        }
        applyEnv();
      } catch (ignored) {
        /* 轮询里的异常绝不能掀掉定时器 */
      }
    }, 1500);
  }

  /** LED 风格的剩余雷数：3 位补零，负数保留符号（经典扫雷就是 -010 这种写法）。 */
  function formatMines(value) {
    var number = Math.max(-999, Math.min(999, parseInt(value, 10) || 0));
    var sign = number < 0 ? '-' : '';
    var digits = String(Math.abs(number));
    while (digits.length < 3) {
      digits = '0' + digits;
    }
    return sign + digits;
  }

  function formatTime(ms) {
    var total = Math.floor(ms / 1000);
    var minutes = Math.floor(total / 60);
    var seconds = total % 60;
    var tenths = Math.floor((ms % 1000) / 100);
    if (minutes > 99) {
      minutes = 99;
    }
    return (minutes < 10 ? '0' : '') + minutes + ':' + (seconds < 10 ? '0' : '') + seconds + '.' + String(tenths);
  }

  function render(snapshot) {
    if (!snapshot) {
      return;
    }
    els.minesLeft.textContent = formatMines(snapshot.mineLeft);
    els.bbbv.textContent = snapshot.bbbv === null || snapshot.bbbv === undefined ? '--' : String(snapshot.bbbv);
    els.opened.textContent = String(snapshot.opened);
    els.status.textContent = DS.i18n.t(
      snapshot.status === 'ready'
        ? 'statusReady'
        : snapshot.status === 'playing'
          ? 'statusPlaying'
          : snapshot.status === 'won'
            ? 'statusWon'
            : 'statusLost'
    );
    updateTime();
    updateBbbvPerSecond();
    if (snapshot.status !== lastStatus) {
      lastStatus = snapshot.status;
      if (snapshot.status === 'won') {
        loadRecords();
      }
    }
    if (snapshot.gameId !== lastGameId) {
      lastGameId = snapshot.gameId;
      // 换了维度（或重开一局）就得重新筛一遍成绩表
      renderRecords(records, snapshot);
    }
  }

  function updateTime() {
    els.time.textContent = formatTime(DS.game.displayElapsed());
  }

  function updateBbbvPerSecond() {
    var snapshot = DS.game.getSnapshot();
    if (!snapshot || !snapshot.bbbv) {
      els.bbbvs.textContent = '--';
      return;
    }
    var seconds = DS.game.displayElapsed() / 1000;
    if (seconds < 0.1) {
      els.bbbvs.textContent = '--';
      return;
    }
    els.bbbvs.textContent = (snapshot.bbbv / seconds).toFixed(2);
  }

  /**
   * 成绩是**分维度**存的（PLAN 3.1 / 4.4）：只展示当前这一档的 Top 10，
   * 不把 9×10 和 16×30 的成绩混在一张表里比。
   */
  function sameDimension(item, snapshot) {
    return (
      !!snapshot &&
      item.rows === snapshot.rows &&
      item.cols === snapshot.cols &&
      item.mineNum === snapshot.mineNum &&
      item.rule === snapshot.rule
    );
  }

  function renderRecords(list, snapshot) {
    records = list || [];
    if (!els.recordsBody) {
      return;
    }
    var snap = snapshot || DS.game.getSnapshot();
    var rows = [];
    for (var i = 0; i < records.length; i += 1) {
      if (sameDimension(records[i], snap)) {
        rows.push(records[i]);
      }
    }

    if (els.recordsDim) {
      els.recordsDim.textContent = snap
        ? snap.rows +
          '×' +
          snap.cols +
          ' · ' +
          snap.mineNum +
          ' · ' +
          (snap.rule === 'first-safe' ? DS.i18n.t('ruleFirstSafe') : DS.i18n.t('ruleWin7'))
        : '';
    }

    els.recordsBody.innerHTML = '';
    if (rows.length === 0) {
      var empty = document.createElement('div');
      empty.className = 'empty';
      empty.textContent = DS.i18n.t('recordsEmpty');
      els.recordsBody.appendChild(empty);
      return;
    }
    for (var j = 0; j < rows.length && j < 10; j += 1) {
      var item = rows[j];
      var row = document.createElement('div');
      row.className = 'record';
      var order = document.createElement('span');
      order.className = 'record-dims';
      order.textContent = '#' + (j + 1);
      var time = document.createElement('span');
      time.className = 'record-time';
      time.textContent = formatTime(item.elapsedMs);
      var bbbv = document.createElement('span');
      bbbv.className = 'record-bbbv';
      bbbv.textContent = DS.i18n.t('bbbv') + ' ' + item.bbbv;
      row.appendChild(order);
      row.appendChild(time);
      row.appendChild(bbbv);
      els.recordsBody.appendChild(row);
    }
  }

  function loadRecords() {
    return DS.game.listRecords().then(
      function (list) {
        renderRecords(list, DS.game.getSnapshot());
      },
      function () {
        renderRecords([], DS.game.getSnapshot());
      }
    );
  }

  function clampInt(value, min, max, fallback) {
    var parsed = parseInt(value, 10);
    if (!isFinite(parsed)) {
      parsed = fallback;
    }
    if (parsed < min) {
      parsed = min;
    }
    if (parsed > max) {
      parsed = max;
    }
    return parsed;
  }

  /**
   * 读表单。**行列与雷数都在这里夹取**，而不是拒绝：
   * 输入 1 ⇒ 自动变成当前行列的下限，超过 35% ⇒ 自动变成上限。
   * 区间口径来自后端 system/info，后端 rpc 也会再夹一层（算法一致 ⇒ 幂等）。
   */
  function readForm() {
    // 首点规则不再出现在界面上：偏好里存的是什么就用什么（默认 win7），
    // 这里不带 rule 字段，prefs/set 的局部更新会原样保留它。
    var form = { preset: els.preset.value };
    if (els.preset.value === 'custom') {
      var lim = DS.game.getLimits().limits;
      var rows = clampInt(els.rows.value, lim.minSide, lim.maxSide, 16);
      var cols = clampInt(els.cols.value, lim.minSide, lim.maxSide, 30);
      var range = DS.game.mineRange(rows, cols);
      var mines = clampInt(els.mines.value, range.lower, range.upper, range.lower);
      form.custom = { rows: rows, cols: cols, mineNum: mines };
      // 回写：让玩家看到输入被夹成了什么，而不是悄悄改掉
      els.rows.value = String(rows);
      els.cols.value = String(cols);
      els.mines.value = String(mines);
    }
    return form;
  }

  /** 雷数输入框下方的小字：当前行列配置下允许的雷数区间。 */
  function updateMineHint() {
    if (!els.mineHint) {
      return;
    }
    if (els.preset.value !== 'custom') {
      els.mineHint.textContent = '';
      return;
    }
    var lim = DS.game.getLimits().limits;
    var rows = clampInt(els.rows.value, lim.minSide, lim.maxSide, 16);
    var cols = clampInt(els.cols.value, lim.minSide, lim.maxSide, 30);
    var range = DS.game.mineRange(rows, cols);
    els.mineHint.textContent = DS.i18n.t('mineRangeHint', {
      rows: rows,
      cols: cols,
      lower: range.lower,
      upper: range.upper
    });
    // 同步原生 number 控件的 min/max：点上下箭头不会越过区间
    els.rows.min = String(lim.minSide);
    els.rows.max = String(lim.maxSide);
    els.cols.min = String(lim.minSide);
    els.cols.max = String(lim.maxSide);
    els.mines.min = String(range.lower);
    els.mines.max = String(range.upper);
  }

  function syncForm(prefs) {
    if (!prefs) {
      return;
    }
    els.preset.value = prefs.preset;
    els.marks.checked = prefs.useMarks !== false;
    els.panelRecords.checked = !prefs.panel || prefs.panel.records !== false;
    if (prefs.custom) {
      els.rows.value = String(prefs.custom.rows);
      els.cols.value = String(prefs.custom.cols);
      els.mines.value = String(prefs.custom.mineNum);
    }
    toggleCustomFields();
    toggleRecordsPanel();
    updateMineHint();
  }

  function toggleCustomFields() {
    els.customRow.hidden = els.preset.value !== 'custom';
    updateMineHint();
  }

  function toggleRecordsPanel() {
    var section = $('sectionRecords');
    if (section) {
      section.hidden = !els.panelRecords.checked;
    }
  }

  /**
   * 用**后端返回的真实尺寸**回写行 / 列 / 雷数输入框。
   *
   * readForm 里前端也夹一次，那只是为了让默认值好看；后端才是唯一真相。
   * 万一两端口径漂移（改了后端百分比却忘了同步前端），这里回写的仍然是
   * 棋盘的实际尺寸，界面不会出现"写着 50 结果棋盘是 40"这种自相矛盾。
   *
   * 正在被打字的输入框跳过：change 已经触发过 restart，此时回写会打断用户
   * 在另一个框里的输入。
   */
  function syncSizeInputs(snapshot) {
    if (!snapshot || els.preset.value !== 'custom') {
      return;
    }
    var pairs = [
      [els.rows, snapshot.rows],
      [els.cols, snapshot.cols],
      [els.mines, snapshot.mineNum]
    ];
    for (var i = 0; i < pairs.length; i += 1) {
      var input = pairs[i][0];
      var value = String(pairs[i][1]);
      if (input && document.activeElement !== input && input.value !== value) {
        input.value = value;
      }
    }
    updateMineHint();
  }

  /**
   * 开新一局。
   *
   * **为什么不能像以前那样"在途就直接 resolve(null)"**：原先只有笑脸按钮会触发
   * restart，丢弃无所谓；现在改行/列/雷数也会自动触发，玩家连着改两项时
   * 第二次请求会被静默吞掉，棋盘停在旧尺寸（尺寸与实际不符，成绩键也跟着错）。
   * 所以改成**合并语义**：在途期间来的新请求只打一个 pending 标记，本轮结束后
   * 用**最新表单值**再开一轮，直到没有新堆积为止。调用方拿到的是最后一局的快照。
   */
  function restart() {
    if (starting) {
      pendingRestart = true;
      return running;
    }
    starting = true;
    var attempt = DS.game
      .savePrefs(readForm())
      .then(function () {
        return DS.game.newGame({});
      })
      .then(function (snapshot) {
        // 尺寸/规则可能变了，成绩表要按新维度重筛
        return loadRecords().then(function () {
          return snapshot;
        });
      });

    function drain(snapshot, error) {
      starting = false;
      running = null;
      if (pendingRestart) {
        pendingRestart = false;
        return restart();
      }
      if (error) {
        throw error;
      }
      syncSizeInputs(snapshot);
      return snapshot;
    }

    running = attempt.then(
      function (snapshot) {
        return drain(snapshot, null);
      },
      function (error) {
        return drain(null, error);
      }
    );
    return running;
  }

  function giveUp() {
    return DS.game.abandon().then(
      function () {
        return restart();
      },
      function () {
        return restart();
      }
    );
  }

  function showNotice(text) {
    if (!els.notice) {
      return;
    }
    els.notice.textContent = text;
    els.notice.hidden = false;
  }

  function clearNotice() {
    if (!els.notice) {
      return;
    }
    els.notice.textContent = '';
    els.notice.hidden = true;
  }

  function bind() {
    els.apply.addEventListener('click', function () {
      clearNotice();
      restart().then(null, function (error) {
        showNotice(DS.bridge.text(error));
      });
    });
    /*
     * 切难度、改行/列/雷数 ⇒ **立刻开新一局**。
     * 玩家实测反馈："选了自定义、调了行列，棋盘没变化"——因为原先只有
     * 笑脸按钮才会 game/new。
     * 用 change 而不是 input：number 控件的 change 在失焦或点上下箭头时才触发，
     * 输入"50"的过程中不会刚打到"5"就换盘。
     */
    els.preset.addEventListener('change', function () {
      clearNotice();
      toggleCustomFields();
      restart().then(null, function (error) {
        showNotice(DS.bridge.text(error));
      });
    });
    [els.rows, els.cols, els.mines].forEach(function (input) {
      input.addEventListener('change', function () {
        clearNotice();
        updateMineHint();
        if (els.preset.value === 'custom') {
          restart().then(null, function (error) {
            showNotice(DS.bridge.text(error));
          });
        }
      });
    });
    els.clearRecords.addEventListener('click', function () {
      if (!window.confirm(DS.i18n.t('confirmClear'))) {
        return;
      }
      DS.game.clearRecords().then(
        function () {
          loadRecords();
        },
        function (error) {
          showNotice(DS.bridge.text(error));
        }
      );
    });
    els.marks.addEventListener('change', function () {
      DS.game.savePrefs({ useMarks: els.marks.checked }).then(null, function (error) {
        showNotice(DS.bridge.text(error));
      });
    });
    els.panelRecords.addEventListener('change', function () {
      toggleRecordsPanel();
      DS.game.savePrefs({ panel: { records: els.panelRecords.checked } }).then(null, function (error) {
        showNotice(DS.bridge.text(error));
      });
    });
  }

  function init() {
    cacheElements();
    bind();
    // 区间口径来自后端 system/info（后端才是唯一真相），所以先取口径再读偏好。
    return DS.game
      .loadLimits()
      .then(function () {
        return DS.game.loadPrefs();
      })
      .then(
      function (prefs) {
        syncForm(prefs);
        return loadRecords();
      },
      function () {
        return null;
      }
    );
  }

  function startTicker() {
    if (tickHandle) {
      return;
    }
    tickHandle = window.setInterval(function () {
      var snapshot = DS.game.getSnapshot();
      if (!snapshot || snapshot.status !== 'playing') {
        return;
      }
      updateTime();
      updateBbbvPerSecond();
    }, TICK_MS);
  }

  return {
    init: init,
    startTicker: startTicker,
    startEnvWatch: startEnvWatch,
    applyEnv: applyEnv,
    applyAppearance: applyAppearance,
    applyLocale: applyLocale,
    render: render,
    loadRecords: loadRecords,
    restart: restart,
    giveUp: giveUp,
    showNotice: showNotice,
    clearNotice: clearNotice
  };
})();
