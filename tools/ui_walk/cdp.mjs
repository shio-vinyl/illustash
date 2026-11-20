// 界面走查（#453）的浏览器驱动：经 Chrome DevTools Protocol 控制一个无头 Chromium 页面。
// 只用 Node 22 自带的 WebSocket 和 fetch，没有 npm 依赖。键盘和鼠标都走 Input.dispatch*（真实输入事件），
// 所以 <dialog> 的原生 Esc、焦点移动、默认动作和用户按键时一样；不用 element.click() 或合成事件。

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

// 键名 → CDP 需要的 key、code、windowsVirtualKeyCode 和插入的文字。字母、数字按规则生成。
const NAMED = {
  Escape: ['Escape', 'Escape', 27], Enter: ['Enter', 'Enter', 13, '\r'], Tab: ['Tab', 'Tab', 9],
  Backspace: ['Backspace', 'Backspace', 8], Delete: ['Delete', 'Delete', 46], Space: [' ', 'Space', 32, ' '],
  ArrowLeft: ['ArrowLeft', 'ArrowLeft', 37], ArrowUp: ['ArrowUp', 'ArrowUp', 38],
  ArrowRight: ['ArrowRight', 'ArrowRight', 39], ArrowDown: ['ArrowDown', 'ArrowDown', 40],
  '/': ['/', 'Slash', 191, '/'], '=': ['=', 'Equal', 187, '='], '-': ['-', 'Minus', 189, '-'],
};
const MODIFIER = {Alt: 1, Ctrl: 2, Meta: 4, Shift: 8};
// 预期内的后端错误：合成图库的隔离环境里没有授权联网，联网查名字的请求会这样回答，页面本来就要处理这种情况。
const EXPECTED_ERRORS = ['显示名联网尚未授权'];

function keyInfo(name) {
  if (NAMED[name]) { const [key, code, vk, text] = NAMED[name]; return {key, code, vk, text}; }
  if (/^[a-z]$/i.test(name)) {
    const upper = name.toUpperCase();
    return {key: name, code: 'Key' + upper, vk: upper.charCodeAt(0), text: name};
  }
  if (/^[0-9]$/.test(name)) return {key: name, code: 'Digit' + name, vk: name.charCodeAt(0), text: name};
  throw new Error('走查驱动不认识的键：' + name);
}

// 一个页面目标的 CDP 会话。
export class Page {
  constructor(ws, {timeout = 15000} = {}) {
    this.ws = ws; this.timeout = timeout; this.nextId = 1; this.waiting = new Map(); this.listeners = [];
    this.console = []; this.inflight = new Map(); this.failed = new Map();
    ws.addEventListener('message', event => this.onMessage(JSON.parse(event.data)));
    ws.addEventListener('close', () => { for (const {reject} of this.waiting.values()) reject(new Error('CDP 连接已关闭')); });
  }

  static async open(browserPort, {width = 1440, height = 900, timeout} = {}) {
    // 上一条流程被看门狗打断时可能留下页面；后台页面的 requestAnimationFrame 不跑，先全部关掉。
    try {
      for (const t of await (await fetch(`http://127.0.0.1:${browserPort}/json/list`)).json()) {
        if (t.type === 'page') await fetch(`http://127.0.0.1:${browserPort}/json/close/${t.id}`);
      }
    } catch { /* 列不出来就算了 */ }
    const res = await fetch(`http://127.0.0.1:${browserPort}/json/new?about:blank`, {method: 'PUT'});
    if (!res.ok) throw new Error('无法新建页面：' + res.status);
    const target = await res.json();
    const ws = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = () => reject(new Error('无法连接页面的 CDP')); });
    const page = new Page(ws, {timeout});
    page.targetId = target.id; page.browserPort = browserPort;
    await page.send('Page.enable');
    await page.send('Runtime.enable');
    await page.send('Log.enable');
    await page.send('Network.enable');
    await page.send('Emulation.setDeviceMetricsOverride', {width, height, deviceScaleFactor: 1, mobile: false});
    // 无头页面不在前台时定时器会被节流，先把它当成有焦点的前台页面。
    await page.send('Emulation.setFocusEmulationEnabled', {enabled: true});
    await page.send('Page.bringToFront');
    return page;
  }

  async close() {
    try { await fetch(`http://127.0.0.1:${this.browserPort}/json/close/${this.targetId}`); } catch { /* 浏览器已经退出 */ }
    try { this.ws.close(); } catch { /* 已关闭 */ }
  }

  send(method, params = {}) {
    const id = this.nextId++;
    this.ws.send(JSON.stringify({id, method, params}));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.waiting.delete(id); reject(new Error(`CDP ${method} 超时`)); }, this.timeout);
      this.waiting.set(id, {resolve: v => { clearTimeout(timer); resolve(v); }, reject: e => { clearTimeout(timer); reject(e); }});
    });
  }

  onMessage(msg) {
    if (msg.id) {
      const w = this.waiting.get(msg.id);
      if (!w) return;
      this.waiting.delete(msg.id);
      if (msg.error) w.reject(new Error(`${msg.error.message}（${msg.error.code}）`)); else w.resolve(msg.result);
      return;
    }
    const p = msg.params || {};
    switch (msg.method) {
      case 'Runtime.consoleAPICalled':
        if (p.type === 'error' || p.type === 'assert') this.noteConsole('console.' + p.type, p.args.map(argText).join(' '));
        break;
      case 'Runtime.exceptionThrown':
        this.noteConsole('exception', p.exceptionDetails?.exception?.description || p.exceptionDetails?.text || '未知异常');
        break;
      case 'Log.entryAdded':
        if (p.entry?.level === 'error' && p.entry.source !== 'network') this.noteConsole('log.' + p.entry.source, p.entry.text);
        break;
      case 'Page.javascriptDialogOpening':
        this.noteConsole('alert', p.message);
        this.send('Page.handleJavaScriptDialog', {accept: false}).catch(() => {});
        break;
      case 'Network.requestWillBeSent': this.inflight.set(p.requestId, p.request.url); this.trace(p.requestId, 'sent'); break;
      case 'Network.responseReceivedExtraInfo': this.trace(p.requestId, 'headers ' + p.statusCode); break;
      case 'Network.dataReceived': this.trace(p.requestId, 'data'); break;
      case 'Network.loadingFinished':
        this.inflight.delete(p.requestId);
        if (this.failed.has(p.requestId)) this.checkFailure(p.requestId);
        break;
      case 'Network.loadingFailed':
        if (this.inflight.has(p.requestId) && !p.canceled) this.noteConsole('network', `${p.errorText} ${shortUrl(this.inflight.get(p.requestId))}`);
        this.inflight.delete(p.requestId);
        break;
      case 'Network.responseReceived':
        this.trace(p.requestId, 'response ' + p.response.status + (p.response.fromDiskCache ? ' disk' : '') + ' ' + (p.response.connectionId ?? ''));
        if (p.response.status >= 500) this.failed.set(p.requestId, `${p.response.status} ${shortUrl(p.response.url)}`);
        break;
      default: break;
    }
    for (const fn of this.listeners) fn(msg);
  }

  // 每个请求的事件顺序（排查卡住的请求用）。
  trace(id, what) { if (!this.traces) this.traces = new Map(); const t = this.traces.get(id) || []; t.push(what); this.traces.set(id, t); }
  stuck() { return [...this.inflight].map(([id, url]) => url.replace(/^https?:\/\/[^/]+/, '') + ': ' + (this.traces?.get(id) || []).join(' → ')); }

  // 5xx 回应：读出错误文字，预期内的（EXPECTED_ERRORS）不算错误。
  async checkFailure(id) {
    const what = this.failed.get(id);
    this.failed.delete(id);
    let said = '';
    try { said = JSON.parse((await this.send('Network.getResponseBody', {requestId: id})).body).error || ''; } catch { /* 读不到正文照常记 */ }
    if (!EXPECTED_ERRORS.includes(said)) this.noteConsole('http', what + (said ? ' ' + said : ''));
  }

  noteConsole(kind, text) { this.console.push({kind, text: String(text).slice(0, 2000)}); }
  // 取走自上次以来的控制台错误。
  drainConsole() { const out = this.console; this.console = []; return out; }

  async eval(expression, {timeout} = {}) {
    const r = await this.send('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true, userGesture: true,
      ...(timeout ? {timeout} : {})});
    if (r.exceptionDetails) throw new Error('页面内执行出错：' + (r.exceptionDetails.exception?.description || r.exceptionDetails.text));
    return r.result.value;
  }

  // 轮询页面里的一个表达式直到为真；超时返回 false，不抛错（调用方决定这算不算失败）。
  async waitFor(expression, timeout = 8000, every = 50) {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      try { if (await this.eval(`!!(${expression})`)) return true; } catch { /* 页面在换，再等 */ }
      await sleep(every);
    }
    return false;
  }

  // 等页面静下来：没有进行中的请求持续 quiet 毫秒，再等进行中的有限时长动画走完（打开、关掉、滑入滑出这类，#709；
  // 转圈这种无限循环的不等，最多等 1.5 秒），再等两帧让渲染跟上。最长 max 毫秒。
  async settle({quiet = 250, max = 6000} = {}) {
    const end = Date.now() + max;
    let calmSince = Date.now();
    while (Date.now() < end) {
      const busy = [...this.inflight.values()].some(url => !/\/(thumb|image|file)s?\//.test(url) || false);
      if (busy) calmSince = Date.now();
      else if (Date.now() - calmSince >= quiet) break;
      await sleep(40);
    }
    try {
      await this.eval(`Promise.race([Promise.all(document.getAnimations().filter(a => a.effect?.getComputedTiming().iterations !== Infinity)
        .map(a => a.finished.catch(() => {}))), new Promise(r => setTimeout(r, 1500))])`, {timeout: 3000});
    } catch { /* 页面在换 */ }
    try { await this.eval('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))', {timeout: 2000}); } catch { /* 页面在换 */ }
  }

  async navigate(url) {
    const loaded = new Promise(resolve => {
      const fn = msg => { if (msg.method === 'Page.loadEventFired') { this.listeners = this.listeners.filter(f => f !== fn); resolve(); } };
      this.listeners.push(fn);
    });
    await this.send('Page.navigate', {url});
    await Promise.race([loaded, sleep(this.timeout)]);
  }

  async key(name, {modifiers = []} = {}) {
    const info = keyInfo(name), mods = modifiers.reduce((sum, m) => sum | MODIFIER[m], 0);
    const shifted = mods & MODIFIER.Shift && info.text && /[a-z]/.test(info.text) ? info.text.toUpperCase() : info.text;
    const base = {key: shifted && info.key.length === 1 ? shifted : info.key, code: info.code,
      windowsVirtualKeyCode: info.vk, nativeVirtualKeyCode: info.vk, modifiers: mods};
    // 有 Ctrl、Meta、Alt 时不插入文字，和真实键盘一样。
    const text = mods & (MODIFIER.Ctrl | MODIFIER.Meta | MODIFIER.Alt) ? undefined : shifted;
    await this.send('Input.dispatchKeyEvent', {type: text ? 'keyDown' : 'rawKeyDown', ...base, ...(text ? {text, unmodifiedText: info.text} : {})});
    await this.send('Input.dispatchKeyEvent', {type: 'keyUp', ...base});
  }

  // 往当前焦点里打字：每个字符一次按键（输入框的 input 事件照常触发）；非 ASCII 用 insertText。
  async type(text) {
    for (const ch of text) {
      if (/^[a-z0-9]$/i.test(ch)) await this.key(ch, {modifiers: /[A-Z]/.test(ch) ? ['Shift'] : []});
      else if (ch === ' ') await this.key('Space');
      else await this.send('Input.insertText', {text: ch});
    }
  }

  // 元素中心点的坐标；找不到或不可见时返回 null。selector 也可以是 {x, y}。
  async center(selector) {
    if (typeof selector === 'object') return selector;
    return this.eval(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) return null;
      el.scrollIntoView({block: 'nearest', inline: 'nearest'});
      const r = el.getBoundingClientRect(); if (!r.width || !r.height) return null;
      return {x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2)}; })()`);
  }

  // 元素可能还在画（详情栏读完才有按钮），最多等 wait 毫秒。
  async click(selector, {modifiers = [], count = 1, button = 'left', wait = 4000} = {}) {
    let at = await this.center(selector);
    for (const end = Date.now() + wait; !at && Date.now() < end;) { await sleep(80); at = await this.center(selector); }
    if (!at) throw new Error('点不到：' + JSON.stringify(selector));
    const mods = modifiers.reduce((sum, m) => sum | MODIFIER[m], 0);
    await this.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: at.x, y: at.y, modifiers: mods});
    for (let i = 1; i <= count; i++) {
      await this.send('Input.dispatchMouseEvent', {type: 'mousePressed', x: at.x, y: at.y, button, clickCount: i, modifiers: mods});
      await this.send('Input.dispatchMouseEvent', {type: 'mouseReleased', x: at.x, y: at.y, button, clickCount: i, modifiers: mods});
    }
    return at;
  }

  async scroll(selector, dy) {
    const at = await this.center(selector);
    await this.send('Input.dispatchMouseEvent', {type: 'mouseWheel', x: at.x, y: at.y, deltaX: 0, deltaY: dy});
  }

  async screenshot() {
    const r = await this.send('Page.captureScreenshot', {format: 'png'});
    return Buffer.from(r.data, 'base64');
  }
}

function argText(arg) {
  if (arg.type === 'string') return arg.value;
  if (arg.value !== undefined) return JSON.stringify(arg.value);
  return arg.description || arg.type;
}

const shortUrl = url => { try { const u = new URL(url); return u.pathname + (u.search ? '?…' : ''); } catch { return url; } };
export {sleep};
