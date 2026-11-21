// 输入法组字时的按键（#388）：core/keys.js 的 composing()，以及几处输入框、全局快捷键、浮层 Esc 对它的使用。
// 三种事件：isComposing 为真（Chromium 的组字中）、keyCode 229 而 isComposing 为假（WebKit 结束组字的那一次按键）、
// 普通按键；前两种不该触发页面的任何操作，第三种照常。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let keys, palette, combo, overlay, grid, page;

before(async () => {
  await web.ready();
  keys = await web.load('core/keys.js');
  page = web.fakePage();
  globalThis.window = {addEventListener() {}};
  globalThis.ResizeObserver = class { observe() {} };
  globalThis.requestAnimationFrame = () => 0;
  const store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)};
  palette = await web.load('ui/palette.js');
  combo = await web.load('ui/combo.js');
  overlay = await web.load('ui/overlay.js');
  grid = await web.load('ui/grid.js');
});

// 合成键盘事件：preventDefault 记在 prevented 上。
const press = (key, extra = {}) => ({key, target: {closest: () => null}, prevented: false, preventDefault() { this.prevented = true; }, stopImmediatePropagation() {}, ...extra});
const KINDS = [
  ['isComposing 为真', {isComposing: true, keyCode: 229}, true],
  ['WebKit 结束组字：isComposing 为假、keyCode 229', {isComposing: false, keyCode: 229}, true],
  ['只有 isComposing', {isComposing: true}, true],
  ['普通回车', {isComposing: false, keyCode: 13}, false],
  ['没有这两个字段的事件', {}, false],
];

test('composing() 认出组字中的按键', () => {
  for (const [name, extra, expected] of KINDS) assert.equal(keys.composing(press('Enter', extra)), expected, name);
  assert.equal(keys.composing(null), false);
  assert.equal(keys.composing(undefined), false);
});

test('combo 下拉搜索框：组字时回车、方向键不选候选，普通按键照常', async () => {
  for (const [name, extra, expected] of KINDS) {
    const input = new web.FakeElement('input', 'comboInput'), menu = new web.FakeElement('div', 'comboMenu');
    const picked = [];
    combo.combo(input, menu, (q, show) => show([{label: '', items: [{label: '甲'}, {label: '乙'}]}]), r => picked.push(r.label));
    for (const fn of input.listeners.focus) fn();
    await new Promise(r => setTimeout(r, 220));  // 搜索有 160 ms 的防抖
    assert.equal(menu.hidden, false, name);
    const down = press('ArrowDown', extra), enter = press('Enter', extra);
    for (const fn of input.listeners.keydown) { fn(down); fn(enter); }
    assert.equal(down.prevented, !expected, `${name}：方向键`);
    assert.equal(enter.prevented, !expected, `${name}：回车`);
    assert.deepEqual(picked, expected ? [] : ['乙'], name);
  }
});

test('「设为…」搜索框：组字时回车、方向键、数字键都不动列表也不选中', async () => {
  palette.wirePalette();
  const search = page.el('pickerSearch');
  page.el('picker').close = () => {};
  // 每个按键各开一次面板：普通按键下，方向键只挪高亮，数字键选第 2 项，回车选当前高亮（第 1 项）。
  for (const [key, chosen] of [['ArrowDown', null], ['2', 'b'], ['Enter', 'a']]) {
    for (const [name, extra, expected] of KINDS) {
      const picked = [];
      await palette.openPicker({title: '合成', items: [{id: 'a', name: '甲'}, {id: 'b', name: '乙'}], pick: e => picked.push(e.id)});
      search.value = '';
      const e = press(key, extra);
      for (const fn of search.listeners.keydown) fn(e);
      await new Promise(r => setTimeout(r, 10));
      assert.equal(e.prevented, !expected, `${key}，${name}`);
      assert.deepEqual(picked, expected || !chosen ? [] : [chosen], `${key}，${name}`);
    }
  }
});

test('全局快捷键：组字时不触发；普通按键照常', () => {
  const handlers = [];
  page.document.addEventListener = (type, fn) => { if (type === 'keydown') handlers.push(fn); };
  const searchBox = page.el('search');
  page.document.querySelector = sel => sel === '#toolbar input[type=search]' ? searchBox : null;
  let focused = 0;
  searchBox.focus = () => { focused++; };
  grid.wireGrid();
  assert.ok(handlers.length, 'wireGrid 没有挂 keydown');
  for (const [name, extra, expected] of KINDS) {
    focused = 0;
    const slash = press('/', extra);
    for (const fn of handlers) fn(slash);
    assert.equal(slash.prevented, !expected, name);
    assert.equal(focused, expected ? 0 : 1, name);
  }
});

test('浮层的 Esc：组字时是取消组字，不关浮层', () => {
  const handlers = {};
  const doc = {activeElement: null, body: {}, addEventListener(type, fn) { (handlers[type] ||= []).push(fn); }};
  globalThis.document = doc;
  overlay.resetLayers(); overlay.wireOverlays();
  for (const [name, extra, expected] of KINDS) {
    let closed = 0;
    const el = {hidden: false, isConnected: true, contains: () => false, closest: () => null};
    const layer = overlay.createLayer({kind: 'popover', id: 'composing-test', el, onDismiss: () => { closed++; layer.close('escape'); }}).open();
    const esc = press('Escape', extra);
    for (const fn of handlers.keydown) fn(esc);
    assert.equal(closed, expected ? 0 : 1, name);
    assert.equal(esc.prevented, !expected, name);
    layer.close('silent');
  }
  globalThis.document = page.document;
});
