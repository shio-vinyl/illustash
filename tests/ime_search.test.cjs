// 「输入就搜」的输入框在输入法组字时不搜（#645）：core/keys.js 的 onTyped()，以及图库搜索、资料库搜索、筛选条「＋ 条件」、
// 「设为…」面板、combo 下拉、画师目录这几处对它的使用。打拼音 zh 的过程中（input 事件的 isComposing 为真）不发请求、
// 不改地址；上屏「张」以后（compositionend）按「张」搜一次。两种内核的事件顺序都走一遍：Chromium 是组字中的 input、
// 然后 compositionend；WebKit 是 compositionend、然后一次 isComposing 为假的 input。数据都是合成的。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let keys, state, page, requests;
const wait = ms => new Promise(r => setTimeout(r, ms));

before(async () => {
  await web.ready();
  keys = await web.load('core/keys.js');
  ({state} = web.core());
  globalThis.window = {addEventListener() {}};
  globalThis.ResizeObserver = class { observe() {} };
  globalThis.IntersectionObserver = class { observe() {} disconnect() {} unobserve() {} };
  globalThis.requestAnimationFrame = () => 0;
  globalThis.history = {replaceState() {}, state: null};
  const store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)};
});
beforeEach(() => {
  page = web.fakePage();
  requests = [];
  globalThis.fetch = path => {
    requests.push(decodeURIComponent(String(path)));
    const url = new URL(path, 'http://x');
    const body = url.pathname === '/api/artist-directory' ? {items: [], total: 0, offset: 0} : [];
    return Promise.resolve({status: 200, ok: true, json: async () => body});
  };
});

// 在 el 上派发事件：按 onTyped 挂的监听依次调用。
const fire = (el, type, extra = {}) => { for (const fn of el.listeners[type] || []) fn({type, target: el, ...extra}); };
// 打拼音 zh（两次组字中的 input），再上屏「张」。webkit 为真时按 WebKit 的顺序结束组字。gap 是两次按键之间隔多久：
// 给的比防抖长，模拟打字慢的人，防抖本身就挡不住按拼音搜。
async function typeZhang(el, webkit = false, gap = 0) {
  for (const v of ['z', 'zh']) { el.value = v; fire(el, 'input', {isComposing: true}); if (gap) await wait(gap); }
  el.value = '张';
  if (webkit) { fire(el, 'compositionend', {data: '张'}); fire(el, 'input', {isComposing: false}); }
  else { fire(el, 'input', {isComposing: true}); fire(el, 'compositionend', {data: '张'}); }
}
const ORDERS = [['Chromium 的顺序', false], ['WebKit 的顺序', true]];
const asked = needle => requests.filter(p => p.includes(needle));

test('onTyped：组字中的 input 不算数，compositionend 补一次，连续输入只算最后一次', async () => {
  const el = new web.FakeElement('input', 'q'), seen = [];
  keys.onTyped(el, e => seen.push(e.target.value), 30);
  await typeZhang(el, false, 50);
  await wait(60);
  assert.deepEqual(seen, ['张']);
  // 普通输入（英文、粘贴、清空按钮派发的 Event）照常；30 毫秒内的几次只算最后一次。
  for (const v of ['a', 'ab', 'abc']) { el.value = v; fire(el, 'input'); }
  await wait(60);
  assert.deepEqual(seen, ['张', 'abc']);
  // 不给 delay 就每次立即调用；WebKit 的顺序下 compositionend 和随后的 input 各调一次，框里都是「张」。
  const now = new web.FakeElement('input', 'now'), got = [];
  keys.onTyped(now, e => got.push(e.target.value));
  await typeZhang(now, true);
  assert.deepEqual(got, ['张', '张']);
});

test('onTyped：挂在外层容器上委托时，从 e.target 取触发的输入框', async () => {
  const root = new web.FakeElement('div', 'root'), box = new web.FakeElement('input', 'box'), seen = [];
  keys.onTyped(root, e => seen.push([e.target.id, e.target.value]));
  for (const v of ['z', 'zh']) { box.value = v; fire(root, 'input', {target: box, isComposing: true}); }
  box.value = '张'; fire(root, 'compositionend', {target: box});
  assert.deepEqual(seen, [['box', '张']]);
});

test('图库搜索框：组字时不改地址、不重读网格，上屏后按「张」搜', async () => {
  const shell = await web.load('ui/shell.js');
  for (const [name, webkit] of ORDERS) {
    Object.assign(state, {route: {page: 'library', q: ''}, chars: [], tagged: []});
    page = web.fakePage(); requests = [];
    shell.renderChrome();
    const search = page.el('search'), qs = [];
    const replace = globalThis.history.replaceState;
    globalThis.history.replaceState = (s, t, hash) => qs.push(hash);
    await typeZhang(search, webkit, 300);
    await wait(300);   // 防抖 250 ms
    globalThis.history.replaceState = replace;
    assert.equal(state.route.q, '张', name);
    assert.equal(qs.length, 1, `${name}：地址只改一次`);
    assert.ok(!qs.some(h => /zh|%7A/i.test(h)), `${name}：没有按拼音改地址`);
  }
});

test('资料库的角色搜索框：组字时不搜，上屏后按「张」搜一次', async () => {
  await web.load('ui/catalog_page.js');
  const {PAGES} = await web.load('core/pages.js'), scopes = await web.load('ui/scope.js');
  for (const [name, webkit] of ORDERS) {
    Object.assign(state, {route: {page: 'characters'}, chars: [], tagged: []});
    page = web.fakePage(); requests = [];
    scopes.page('characters');
    const input = PAGES.characters.tools(state.route).querySelector('input');
    await typeZhang(input, webkit, 250);
    await wait(250);   // 防抖 200 ms
    assert.equal(asked('zh').length, 0, `${name}：没有按拼音搜`);
    assert.equal(asked('张').length, 1, name);
  }
});

test('筛选条「＋ 条件」的搜索框：组字时不搜，上屏后按「张」搜', async () => {
  const filters = await web.load('ui/filters.js');
  for (const [name, webkit] of ORDERS) {
    Object.assign(state, {route: {page: 'library'}, chars: [], workTree: [], userTags: []});
    page = web.fakePage(); requests = [];
    page.el('toolbar').querySelector = () => null;
    filters.wireFilterBar(state.route);
    await typeZhang(page.el('condSearch'), webkit, 170);
    await wait(170);   // 防抖 120 ms
    assert.equal(asked('q=zh').length, 0, `${name}：没有按拼音搜`);
    assert.equal(asked('/api/general-tags?q=张').length, 1, name);
  }
});

test('「设为…」面板的搜索框：组字时不搜，上屏后按「张」搜', async () => {
  const palette = await web.load('ui/palette.js');
  for (const [name, webkit] of ORDERS) {
    Object.assign(state, {userTags: [], chars: [], charMap: new Map()});
    page = web.fakePage(); requests = [];
    page.el('picker').close = () => {};
    palette.wirePalette();
    await palette.openPicker({tagMode: true, present: new Map(), pick() {}});
    requests = [];
    await typeZhang(page.el('pickerSearch'), webkit, 190);
    await wait(190);   // 防抖 140 ms
    assert.equal(asked('q=zh').length, 0, `${name}：没有按拼音搜`);
    assert.equal(asked('/api/general-tags?q=张').length, 1, name);
  }
});

test('combo 下拉：组字时不搜，上屏后按「张」搜一次', async () => {
  const combo = await web.load('ui/combo.js');
  for (const [name, webkit] of ORDERS) {
    const input = new web.FakeElement('input', 'comboInput'), menu = new web.FakeElement('div', 'comboMenu'), searched = [], drafts = [];
    combo.combo(input, menu, (q, show) => { searched.push(q); show([]); }, () => {}, {typed: v => drafts.push(v)});
    await typeZhang(input, webkit, 210);
    await wait(210);   // 防抖 160 ms
    assert.deepEqual(searched, ['张'], name);
    assert.ok(!drafts.some(v => /z/.test(v)), `${name}：草稿里没有拼音`);
  }
});

test('画师目录的搜索框：组字时不搜，上屏后按「张」搜一次', async () => {
  const dirs = await web.load('ui/artist_dir.js'), scopes = await web.load('ui/scope.js');
  for (const [name, webkit] of ORDERS) {
    Object.assign(state, {route: {page: 'characters', section: 'artists'}, nsfw: ''});
    Object.assign(dirs.dir, {q: '', order: 'count', items: [], total: null, next: 0, failed: null, key: ''});
    page = web.fakePage(); requests = [];
    const view = dirs.artistsView(scopes.scope('ime'));
    await typeZhang(view.tools.querySelector('input'), webkit, 250);
    await wait(250);   // 防抖 200 ms
    assert.equal(asked('q=zh').length, 0, `${name}：没有按拼音搜`);
    assert.equal(asked('/api/artist-directory?q=张').length, 1, name);
  }
});
