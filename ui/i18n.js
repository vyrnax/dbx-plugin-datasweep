/**
 * 文案字典。宿主语言由 `dbxPlugin.locale` 给出，未知 locale 一律退英文。
 * 只支持 zh / en（PLAN 3.3 已排除其他语言）。
 */
window.DS = window.DS || {};
DS.i18n = (function () {
  'use strict';

  var DICT = {
    zh: {
      appTitle: '扫雷',
      preset: '难度',
      presetBeginner: '初级 9×10 · 10 雷',
      presetIntermediate: '中级 16×16 · 40 雷',
      presetExpert: '高级 16×30 · 99 雷',
      presetCustom: '自定义',
      ruleWin7: 'Win7（首点开空）',
      ruleFirstSafe: '仅首点非雷',
      time: '用时',
      mines: '剩余',
      bbbv: '3BV',
      bbbvs: '3BV/s',
      opened: '已开',
      records: '最佳成绩',
      recordsEmpty: '还没有成绩',
      clearRecords: '清空',
      confirmClear: '清空全部成绩？',
      showMarks: '右键可标问号',
      panelRecords: '显示成绩面板',
      statusReady: '点击任意格子开始',
      statusPlaying: '进行中',
      statusWon: '通关',
      statusLost: '踩雷了',
      rows: '行',
      cols: '列',
      mineNum: '雷数',
      apply: '开这一局',
      mineRangeHint: '当前 {rows}×{cols}：雷数允许 {lower}–{upper}',
      focusHint: '方向键移动 · 回车翻开 · 空格插旗 · Ctrl+回车盲开 · F2 重开 · Esc 认输',
      sidecarRestarted: '侧车已重启，本局作废',
      errTimeout: '请求超时',
      errParams: '参数不合法',
      errNotFound: '这个能力还没实现',
      errInternal: '侧车内部错误',
      errAlgorithm: '埋雷失败，换个尺寸试试',
      errStorage: '偏好或成绩存不下来',
      errState: '这一步在当前局面下不允许',
      errDefault: '请求失败'
    },
    en: {
      appTitle: 'DataSweep',
      preset: 'Difficulty',
      presetBeginner: 'Beginner 9×10 · 10',
      presetIntermediate: 'Intermediate 16×16 · 40',
      presetExpert: 'Expert 16×30 · 99',
      presetCustom: 'Custom',
      ruleWin7: 'Windows 7 (opens a blank)',
      ruleFirstSafe: 'First cell only',
      time: 'Time',
      mines: 'Left',
      bbbv: '3BV',
      bbbvs: '3BV/s',
      opened: 'Opened',
      records: 'Best times',
      recordsEmpty: 'No records yet',
      clearRecords: 'Clear',
      confirmClear: 'Clear all records?',
      showMarks: 'Right click may mark "?"',
      panelRecords: 'Show records panel',
      statusReady: 'Click any cell to start',
      statusPlaying: 'Playing',
      statusWon: 'Cleared',
      statusLost: 'Boom',
      rows: 'Rows',
      cols: 'Cols',
      mineNum: 'Mines',
      apply: 'Start',
      mineRangeHint: 'Current {rows}×{cols}: mines allowed {lower}–{upper}',
      focusHint: 'Arrows move · Enter opens · Space flags · Ctrl+Enter blind · F2 restart · Esc gives up',
      sidecarRestarted: 'Sidecar restarted — this round is void',
      errTimeout: 'Request timed out',
      errParams: 'Invalid parameters',
      errNotFound: 'Not implemented yet',
      errInternal: 'Sidecar internal error',
      errAlgorithm: 'Mine layout failed, try another size',
      errStorage: 'Could not save preferences or records',
      errState: 'That move is not allowed right now',
      errDefault: 'Request failed'
    }
  };

  var current = 'en';

  function setLocale(locale) {
    var value = String(locale || '').toLowerCase();
    current = value.indexOf('zh') === 0 ? 'zh' : 'en';
  }

  /** t(key, params)：params 提供时把 {name} 占位符替换掉。 */
  function t(key, params) {
    var table = DICT[current] || DICT.en;
    var text = table[key] || DICT.en[key] || key;
    if (!params) {
      return text;
    }
    return text.replace(/\{(\w+)\}/g, function (whole, name) {
      return params[name] === undefined ? whole : String(params[name]);
    });
  }

  return { setLocale: setLocale, t: t, current: function () { return current; } };
})();
