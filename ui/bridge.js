/**
 * `window.dbxPlugin` 的封装层。UI 里只有这个文件碰宿主桥。
 *
 * 两条必须遵守的规则：
 * 1. `timeoutMs` 会被宿主夹到 1–120000，我们自己也先夹一次，免得传 0 被宿主当成默认。
 * 2. API 对象永远是 `window.dbxPlugin` **本身**；`ready` 只是初始化信号，
 *    它的结果不是宿主对象，不能拿它去调 invoke。
 */
window.DS = window.DS || {};
DS.bridge = (function () {
  'use strict';

  var MIN_TIMEOUT = 1;
  var MAX_TIMEOUT = 120000;
  var DEFAULT_TIMEOUT = 30000;

  var CODE_KEYS = {
    // -32001 不是侧车发的，是下面 fromRaw 给"宿主超时"归出来的本地码：
    // 宿主超时时 reject 的是一个**不带 code 的纯 Error**（见 dev-runtime 的
    // `reject(new Error("Mock host request timed out"))`），不专门认一下就
    // 会兜到 -32603，把"等太久"显示成"侧车内部错误"——完全是两回事。
    '-32001': 'errTimeout',
    '-32010': 'sidecarRestarted',
    '-32011': 'errState',
    '-32013': 'errAlgorithm',
    '-32014': 'errStorage',
    '-32600': 'errParams',
    '-32601': 'errNotFound',
    '-32603': 'errInternal'
  };

  function api() {
    return window.dbxPlugin;
  }

  function clamp(timeoutMs) {
    var value = typeof timeoutMs === 'number' && isFinite(timeoutMs) ? timeoutMs : DEFAULT_TIMEOUT;
    if (value < MIN_TIMEOUT) {
      return MIN_TIMEOUT;
    }
    if (value > MAX_TIMEOUT) {
      return MAX_TIMEOUT;
    }
    return value;
  }

  var BRIDGE_WAIT_MS = 5000;

  /**
   * 宿主注入 `window.dbxPlugin` 的时机不确定，而 UI 可能在注入前就发起调用。
   * 直接 reject 会让那一次调用【永久失败且无人重试】（翻译插件踩过：引擎表单永远空白）。
   * 所以这里等它有上限，超时才报错。
   */
  function waitForBridge() {
    var deadline = Date.now() + BRIDGE_WAIT_MS;
    return new Promise(function (resolve, reject) {
      function poll() {
        var host = api();
        if (host && typeof host.invoke === 'function') {
          resolve(host);
          return;
        }
        if (Date.now() >= deadline) {
          // 这是"等宿主桥等超时"，归到超时码，别再说成侧车内部错误
          reject(makeError(-32001, '宿主桥接未就绪（没有 dbxPlugin.invoke）'));
          return;
        }
        window.setTimeout(poll, 50);
      }
      poll();
    });
  }

  function call(method, params, timeoutMs) {
    return waitForBridge()
      .then(function (host) {
        return host.invoke(method, params || {}, { timeoutMs: clamp(timeoutMs) });
      })
      .then(function (value) {
        return unwrap(method, value);
      });
  }

  /**
   * 解开宿主的返回包装。
   *
   * 【真宿主实证的关键契约】侧车的业务错误**不会以 reject 形式出现**，
   * 宿主把它包成 `{ error: { code, message } }` **正常返回**；成功结果则可能
   * 包成 `{ result: ... }`。原先这里直接把返回值当成功结果用，于是
   * -32011（这一步不允许）、-32600（参数不合法）这些**预期内的业务失败**
   * 全部落到 `failure()` 的兜底分支，被统一显示成「侧车内部错误」——
   * 玩家实测到的正是这条彻底误导的文案。
   */
  function unwrap(method, value) {
    if (value && typeof value === 'object' && value.error && value.ok === undefined) {
      throw fromRaw(value.error, method);
    }
    if (value && typeof value === 'object' && value.result !== undefined && value.ok === undefined) {
      return value.result;
    }
    return value;
  }

  function makeError(code, message) {
    var error = new Error(message);
    error.code = code;
    return error;
  }

  /** 把宿主/侧车的各种错误形态归一成带数字 code 的 Error。 */
  function fromRaw(raw, method) {
    // 再下钻一层：宿主可能把侧车错误又套一次 { error: ... }
    var inner = raw && raw.error ? raw.error : raw;
    var code = -32603;
    var message = '';
    if (inner && typeof inner.code === 'number') {
      code = inner.code;
    }
    if (typeof inner === 'string') {
      message = inner;
    } else if (inner && typeof inner.message === 'string') {
      message = inner.message;
    }
    if (!message) {
      message = (method ? method + ' ' : '') + '调用失败';
    }
    // 宿主超时：reject 的是纯 Error，没有 code，于是上面兜成了 -32603。
    // 只在**没有明确数字码**时按文案识别，避免盖掉侧车真实返回的码。
    if (code === -32603 && /timed out|timeout/i.test(message)) {
      code = -32001;
    }
    return makeError(code, message);
  }

  /**
   * 宿主初始化可能晚于本脚本执行，所以启动流程**不能**阻塞在 ready 上：
   * 页面先画出来，ready 到了再补一次数据与主题。
   */
  function whenReady(callback) {
    var host = api();
    if (host && host.ready && typeof host.ready.then === 'function') {
      host.ready.then(
        function () {
          callback(host);
        },
        function () {
          callback(null);
        }
      );
      return;
    }
    window.setTimeout(function () {
      callback(api());
    }, 0);
  }

  /** 工作台上下文变化（连接、surface 等）。注意：**主题与语言不在这里**。 */
  function onContext(callback) {
    var host = api();
    if (host && typeof host.onContext === 'function') {
      host.onContext(callback);
    }
  }

  /**
   * 监听宿主的**环境变化**（明暗、调色板、字体、圆角、语言）。
   *
   * 三个必须同时用的注册方式，缺一个就会在某些宿主上静默失效：
   *
   * 1. `window` + `document` **都注册**且 `capture: true`。
   *    新版宿主把 `dbx-plugin-env` 的 target 设在 document 上**且不冒泡**；
   *    document 是 window 的祖先，事件不冒泡时挂在 window 冒泡阶段的监听器
   *    永远收不到，只有 capture 阶段（window → document 向下传播）能截获。
   *    老宿主则把 target 设在 window 上，此时两种阶段都收得到。
   * 2. `dbxPlugin.onEvent`（存在就用）。dev-runtime 里 `type:"env"` 的消息
   *    同样会喂给 event 监听器，等于第三条独立通路。
   *
   * 症状学：漏掉这层的失败是**静默**的 —— 界面看起来完全正常，
   * 用户只会觉得"这插件不支持深色"，不会想到是事件没收到。
   * 所以调用方还要配一层轮询兜底（panels.js 的 startEnvPolling）。
   */
  function onEnv(callback) {
    window.addEventListener('dbx-plugin-env', callback, true);
    document.addEventListener('dbx-plugin-env', callback, true);
    var host = api();
    if (host && typeof host.onEvent === 'function') {
      try {
        host.onEvent(callback);
      } catch (ignored) {
        /* 老宿主没有 onEvent：前两条通路已经够用 */
      }
    }
  }

  /** 宿主桥就绪时的初始化回调（有些宿主只在 ready 时投一次环境，不再发 env）。 */
  function onInit(callback) {
    var host = api();
    if (!host || typeof host.onInit !== 'function') {
      return;
    }
    try {
      host.onInit(callback);
    } catch (ignored) {
      /* 没有 onInit 就靠 whenReady + 轮询 */
    }
  }

  function locale() {
    var host = api();
    return host && host.locale ? host.locale : '';
  }

  /** 把任何形态的外观值归一成 'dark' / 'light'，认不出来返回 ''（不是兜底值）。 */
  function normalizeAppearance(value) {
    if (value && typeof value === 'object') {
      // 兼容把 {appearance} 又套一层的宿主
      value = value.appearance;
    }
    var text = String(value === undefined || value === null ? '' : value).toLowerCase();
    if (text.indexOf('dark') >= 0) {
      return 'dark';
    }
    if (text.indexOf('light') >= 0) {
      return 'light';
    }
    return '';
  }

  /**
   * 读宿主投递的**完整主题负载** { appearance, tokens, editor }。
   *
   * 这是主题与字体的唯一正源：官方文档「主题负载携带什么」明确
   * 明暗 → theme.appearance、调色板 → --color-* tokens、圆角 → --radius-*、
   * UI 字体 → --font-sans、编辑器字体 → --font-mono / theme.editor.*。
   * 老宿主可能把 theme 直接给成字符串，这里一并兼容。
   */
  function themeObject() {
    var host = api();
    var raw = null;
    try {
      raw = host && host.theme ? host.theme : null;
    } catch (ignored) {
      raw = null;
    }
    if (raw && typeof raw === 'object') {
      return {
        appearance: normalizeAppearance(raw.appearance),
        tokens: raw.tokens && typeof raw.tokens === 'object' ? raw.tokens : {},
        editor: raw.editor && typeof raw.editor === 'object' ? raw.editor : null
      };
    }
    return { appearance: normalizeAppearance(raw), tokens: {}, editor: null };
  }

  /**
   * 主题归一成 'dark' / 'light'。
   *
   * 顺序是**有讲究的**：先 `dbxPlugin.theme`（宿主真值），再看 context 里恰好带的
   * theme（部分宿主会把环境并进 context），再退 prefers-color-scheme，最后浅色。
   * 原先只看 `context.theme || context.colorScheme` —— 而 dev-runtime 的 context
   * 里根本没有这两个字段，于是永远落到 prefers-color-scheme：
   * **DBX 里切深浅色，插件纹丝不动**（宿主主题与操作系统主题是两回事）。
   */
  function theme(context) {
    var fromHost = themeObject().appearance;
    if (fromHost) {
      return fromHost;
    }
    var fromContext = normalizeAppearance(context && (context.theme || context.colorScheme));
    if (fromContext) {
      return fromContext;
    }
    if (window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches) {
      return 'dark';
    }
    return 'light';
  }

  /** 把宿主抛出的东西（Error / JSON-RPC 错误对象 / {error} 包装 / 字符串）统一成 { code, message }。 */
  function failure(error) {
    if (!error) {
      return { code: -32603, message: '' };
    }
    if (typeof error.code === 'number') {
      return { code: error.code, message: String(error.message || '') };
    }
    var normalized = fromRaw(error, '');
    return { code: normalized.code, message: normalized.message };
  }

  function text(error) {
    var info = failure(error);
    var key = CODE_KEYS[String(info.code)];
    // 业务状态错误记住原始 code：-32011 的语义应保留，避免再被改成通用文案；
    // 用户看到的仍然是 i18n 文案。
    if (key === 'errInternal') {
      return info.message || DS.i18n.t('errDefault');
    }
    return info.message || DS.i18n.t('errDefault');
  }

  return {
    MIN_TIMEOUT: MIN_TIMEOUT,
    MAX_TIMEOUT: MAX_TIMEOUT,
    DEFAULT_TIMEOUT: DEFAULT_TIMEOUT,
    call: call,
    whenReady: whenReady,
    onContext: onContext,
    onEnv: onEnv,
    onInit: onInit,
    themeObject: themeObject,
    normalizeAppearance: normalizeAppearance,
    locale: locale,
    theme: theme,
    failure: failure,
    text: text
  };
})();
