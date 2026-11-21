// 运行：node --test tests/drag.test.cjs
// 按住拖动的手势和拖图进画廊（ui/press.js、ui/drag.js，#710）：拖到别的应用以后的状态复位、松手时焦点不落进输入框。
// 桌面 App（WebKit）里的实际表现由虚拟机走查验；这里只测页面自己记的状态。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let press, drag, page, win;
const fire = (listeners, type, extra = {}) => (listeners[type] || []).slice().forEach(f => f({type, button: 0, buttons: 1, clientX: 0, clientY: 0, ...extra}));
before(async () => {
  await web.ready();
  page = web.fakePage();
  win = {};
  globalThis.addEventListener = (type, fn) => { (win[type] ||= []).push(fn); };
  globalThis.removeEventListener = (type, fn) => { win[type] = (win[type] || []).filter(f => f !== fn); };
  globalThis.window = {addEventListener: globalThis.addEventListener, removeEventListener: globalThis.removeEventListener};
  globalThis.ResizeObserver = class { observe() {} disconnect() {} };
  globalThis.requestAnimationFrame = () => 0;
  globalThis.innerWidth = 1000; globalThis.innerHeight = 800;
  press = await web.load('ui/press.js');
  drag = await web.load('ui/drag.js');
});
beforeEach(() => { globalThis.document.getSelection = () => ({removeAllRanges() {}}); });

test('按下：平时认 pointerdown；没有 pointerdown 的那次（拖到别的应用以后 WebKit 只派 mousedown）由 mousedown 补上', () => {
  const el = new web.FakeElement('div'), got = [];
  press.onPress(el, e => got.push(e.type));
  fire(el.listeners, 'pointerdown'); fire(el.listeners, 'mousedown');   // 正常的一次：只算一次
  fire(el.listeners, 'mousedown');                                     // 只有 mousedown 的一次：补上
  assert.deepEqual(got, ['pointerdown', 'mousedown']);
});

test('拖到别的应用以后：页面不可选，到下一次按下复位；这次按下照样能开始拖', () => {
  const body = globalThis.document.body;
  press.handedOff();
  assert.ok(press.handedOffNow());
  assert.ok(body.classList.contains('handed-off'));
  fire(win, 'mousedown');
  assert.ok(!press.handedOffNow());
  assert.ok(!body.classList.contains('handed-off'));
  // 只有 mousedown、接着按住移动：wirePress 照样开始拖
  const el = new web.FakeElement('div'), starts = [];
  press.wirePress(el, {start: (down, move) => { starts.push([down.type, move.clientX]); return true; }});
  fire(el.listeners, 'mousedown', {clientX: 10, clientY: 10});
  fire(el.listeners, 'pointermove', {clientX: 30, clientY: 10});
  assert.deepEqual(starts, [['mousedown', 30]]);
});

test('拿起以后松手：焦点要是落进了拿起时不在的输入框，就让它失焦（搜索框不吃下一个按键）', async () => {
  const doc = globalThis.document;
  let blurred = 0;
  const input = Object.assign(new web.FakeElement('input'), {matches: s => s.includes('input'), blur() { blurred++; }});
  doc.activeElement = doc.body;
  drag.lift(['a'], {clientX: 100, clientY: 100});
  assert.ok(drag.lifting());
  doc.activeElement = input;   // 松手时浏览器把焦点给了指针下的输入框
  fire(win, 'pointerup', {clientX: 100, clientY: 100, buttons: 0});
  assert.ok(!drag.lifting());
  await new Promise(r => setTimeout(r, 5));
  assert.equal(blurred, 1);
  // 拿起前焦点就在输入框里：不动它
  doc.activeElement = input;
  drag.lift(['a'], {clientX: 100, clientY: 100});
  fire(win, 'keydown');
  fire(win, 'pointerup', {clientX: 100, clientY: 100, buttons: 0});
  await new Promise(r => setTimeout(r, 5));
  assert.equal(blurred, 1);
});

test('拿起后按 Esc（键还按着）：输入框到松手以后才重新接指针，松手后焦点落进的输入框照样失焦（#716）', async () => {
  const doc = globalThis.document, body = doc.body, overlay = await web.load('ui/overlay.js');
  let blurred = 0;
  const input = Object.assign(new web.FakeElement('input'), {matches: s => s.includes('input'), blur() { blurred++; }});
  doc.activeElement = doc.body;
  drag.lift(['a'], {clientX: 100, clientY: 100});
  assert.deepEqual(overlay.layerIds().slice(-1), ['lift']);
  overlay.dismissTop('escape');
  assert.ok(!drag.lifting());
  assert.ok(body.classList.contains('settling'), '键还按着：输入框仍不接指针');
  await new Promise(r => setTimeout(r, 5));
  assert.equal(blurred, 0);
  doc.activeElement = input;   // 松手时浏览器把焦点给了指针下的输入框
  fire(win, 'pointerup', {clientX: 100, clientY: 100, buttons: 0});
  assert.ok(!body.classList.contains('settling'));
  await new Promise(r => setTimeout(r, 5));
  assert.equal(blurred, 1);
});
