/**
 * 前端状态机与请求队列。
 *
 * 最关键的一条：`game/*` 同一时刻只允许一个在途请求，后续动作排 FIFO 队。
 * 否则高速连点会收到乱序快照，画面出现"回退"（PLAN 4.5 / README 6）。
 *
 * 计时口径：UI 的秒表只是本地 tick 显示，终局数字一律以侧车返回的 elapsedMs 为准
 * （权威计时在侧车，前端改不了成绩）。
 */
window.DS = window.DS || {};
DS.game = (function () {
  'use strict';

  var ACTION_TIMEOUT = 5000;
  var SETUP_TIMEOUT = 30000;

  var snapshot = null;
  var prefs = null;
  /* system/info 里的区间口径（后端是唯一真相）。拿不到时用同样的兜底值，
     但注意：兜底值必须与后端 game.rs 的常量逐字对应，改后端就要改这里。 */
  var limits = { minSide: 5, maxSide: 50, minePercentMin: 10, minePercentMax: 35 };
  var limitsReady = false;
  var queue = [];
  var inflight = false;
  var focus = null;
  var listeners = { snapshot: [], stale: [], notice: [] };
  var tickBase = 0;
  var tickAt = 0;

  function on(event, callback) {
    if (listeners[event]) {
      listeners[event].push(callback);
    }
  }

  function emit(event, payload) {
    var list = listeners[event] || [];
    for (var i = 0; i < list.length; i += 1) {
      list[i](payload);
    }
  }

  function applySnapshot(next) {
    snapshot = next;
    tickBase = next && next.elapsedMs ? next.elapsedMs : 0;
    tickAt = new Date().getTime();
    if (focus) {
      if (focus[0] >= next.rows) {
        focus[0] = next.rows - 1;
      }
      if (focus[1] >= next.cols) {
        focus[1] = next.cols - 1;
      }
    } else {
      focus = [Math.floor(next.rows / 2), Math.floor(next.cols / 2)];
    }
    emit('snapshot', next);
    return next;
  }

  /**
   * `game/*` 走这里：单在途 + FIFO。
   * 队列的作用不是限流（IPC 是毫秒级），而是保证快照按动作顺序到达。
   */
  function send(method, params, options) {
    var settings = options || {};
    return new Promise(function (resolve, reject) {
      queue.push({
        method: method,
        params: params || {},
        timeout: settings.timeout || ACTION_TIMEOUT,
        apply: !!settings.apply,
        resolve: resolve,
        reject: reject
      });
      pump();
    });
  }

  function pump() {
    if (inflight || queue.length === 0) {
      return;
    }
    var job = queue.shift();
    inflight = true;
    DS.bridge
      .call(job.method, job.params, job.timeout)
      .then(
        function (result) {
          if (job.apply) {
            applySnapshot(result);
          }
          job.resolve(result);
        },
        function (error) {
          var info = DS.bridge.failure(error);
          // 侧车被宿主回收过：本局作废，UI 提示并按新一局处理
          if (info.code === -32010) {
            snapshot = null;
            emit('stale', info);
          } else {
            emit('notice', info);
          }
          job.reject(info);
        }
      )
      .then(function () {
        inflight = false;
        pump();
      });
  }

  function queueLength() {
    return queue.length + (inflight ? 1 : 0);
  }

  /** 非 `game/*` 的方法不进队列，免得拉成绩列表挡住点击。 */
  function call(method, params, timeoutMs) {
    return DS.bridge.call(method, params, timeoutMs);
  }

  function newGame(options) {
    return send('game/new', options || {}, { apply: true, timeout: SETUP_TIMEOUT });
  }

  function reveal(row, col, force) {
    return send('game/reveal', { r: row, c: col, force: !!force }, { apply: true });
  }

  function chord(row, col) {
    return send('game/chord', { r: row, c: col }, { apply: true });
  }

  function flag(row, col, mode) {
    return send('game/flag', { r: row, c: col, mode: mode }, { apply: true });
  }

  function refresh() {
    return send('game/state', {}, { apply: true, timeout: SETUP_TIMEOUT });
  }

  function abandon() {
    return send('game/abandon', {}, { timeout: SETUP_TIMEOUT });
  }

  function getSnapshot() {
    return snapshot;
  }

  function getPrefs() {
    return prefs;
  }

  function setPrefs(next) {
    prefs = next;
    emit('prefs', next);
    return prefs;
  }

  function loadPrefs() {
    return call('prefs/get', {}, SETUP_TIMEOUT).then(function (result) {
      setPrefs(result);
      return result;
    });
  }

  /**
   * 读一次 system/info，把区间口径存下来。
   * 失败不阻塞启动：面板会用兜底值，后端也会照样夹取，最坏情况只是提示数字不准。
   */
  function loadLimits() {
    return call('system/info', {}, SETUP_TIMEOUT).then(
      function (info) {
        if (info && typeof info.minSide === 'number') {
          limits = {
            minSide: info.minSide,
            maxSide: info.maxSide,
            minePercentMin: info.minePercentMin,
            minePercentMax: info.minePercentMax
          };
          limitsReady = true;
        }
        return limits;
      },
      function () {
        return limits;
      }
    );
  }

  /** 当前行列下允许的雷数闭区间，与后端 game::mine_range 逐条对齐：
   *  下限向上取整、上限向下取整（宁可窄，不能把越界方向放进来）。 */
  function mineRange(rows, cols) {
    var cells = rows * cols;
    var lower = Math.ceil((cells * limits.minePercentMin) / 100);
    var upper = Math.floor((cells * limits.minePercentMax) / 100);
    if (lower > upper) {
      upper = lower;
    }
    return { lower: lower, upper: upper };
  }

  function getLimits() {
    return { limits: limits, ready: limitsReady };
  }

  function savePrefs(patch) {
    return call('prefs/set', patch, SETUP_TIMEOUT).then(function (result) {
      setPrefs(result);
      return result;
    });
  }

  function listRecords() {
    return call('records/list', {}, SETUP_TIMEOUT);
  }

  function clearRecords(key) {
    return call('records/clear', key ? { key: key } : {}, SETUP_TIMEOUT);
  }

  function setFocus(next) {
    focus = next;
  }

  function getFocus() {
    return focus;
  }

  /** 秒表读数：进行中时在本地补上增量，终局直接用侧车的权威值。 */
  function displayElapsed() {
    if (!snapshot) {
      return 0;
    }
    if (snapshot.status === 'playing') {
      return tickBase + (new Date().getTime() - tickAt);
    }
    return snapshot.elapsedMs;
  }

  return {
    on: on,
    newGame: newGame,
    reveal: reveal,
    chord: chord,
    flag: flag,
    refresh: refresh,
    abandon: abandon,
    call: call,
    queueLength: queueLength,
    getSnapshot: getSnapshot,
    getPrefs: getPrefs,
    setPrefs: setPrefs,
    loadPrefs: loadPrefs,
    loadLimits: loadLimits,
    mineRange: mineRange,
    getLimits: getLimits,
    savePrefs: savePrefs,
    listRecords: listRecords,
    clearRecords: clearRecords,
    setFocus: setFocus,
    getFocus: getFocus,
    displayElapsed: displayElapsed,
    applySnapshot: applySnapshot
  };
})();
