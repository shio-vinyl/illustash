// 「标为原创角色」一律设为共用的「原创角色（未细分）」（#694）：整组标注（按组整理页的组头、组页标题）和角色选择器
// 都不再新建独立的原创角色（POST /api/entities {kind:'oc'}），只调 POST /api/oc/shared。数据都是合成的。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');

let page, state, requests, entities;
const wait = ms => new Promise(r => setTimeout(r, ms));
const SHARED = {id: 'oc-shared', name: '原创角色（未细分）', kind: 'oc', artist: ''};
before(async () => {
  await web.ready();
  ({state} = web.core());
  web.core().session.token = 'a';
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
  page.el('picker').close = () => {};
  requests = []; entities = [];
  globalThis.fetch = (path, init = {}) => {
    const url = new URL(String(path), 'http://x');
    requests.push({path: url.pathname, query: url.search, method: init.method || 'GET', body: init.body ? JSON.parse(init.body) : null});
    const body = url.pathname === '/api/oc/shared' ? SHARED
      : url.pathname === '/api/entities' && init.method === 'POST' ? {id: 'new', name: 'new'}
      : url.pathname === '/api/entities' && !url.searchParams.has('remote') ? entities
      : url.pathname === '/api/libraries' ? {info: {id: 'L', name: 'x'}}
      : [];
    return Promise.resolve({status: 200, ok: true, json: async () => body});
  };
  Object.assign(state, {userTags: [], chars: [], charMap: new Map()});
});
const posted = path => requests.filter(r => r.method === 'POST' && r.path === path);
async function search(palette, q) {
  const box = page.el('pickerSearch');
  box.value = q;
  for (const fn of box.listeners.input || []) fn({type: 'input', target: box, isComposing: false});
  await wait(450);   // 防抖 140 ms，Danbooru 那一段再等 380 ms
  return page.el('pickerResults').innerHTML;
}

test('角色选择器：没有「新建原创角色」，改为选共用的「原创角色（未细分）」，选中只调 /api/oc/shared', async () => {
  const palette = await web.load('ui/palette.js');
  palette.wirePalette();
  let picked = null;
  await palette.openPicker({pick: e => { picked = e; }});
  const html = await search(palette, '小明');
  assert.doesNotMatch(html, /新建原创角色/);
  assert.match(html, /新建「小明」/);
  assert.match(html, /原创角色（未细分）/);
  // 两项都是兜底：默认高亮第一项「新建」，往下一项是共用原创角色。
  const keydown = page.el('pickerSearch').listeners.keydown[0];
  keydown({key: 'ArrowDown', preventDefault() {}, isComposing: false, keyCode: 40});
  keydown({key: 'Enter', preventDefault() {}, isComposing: false, keyCode: 13});
  await wait(0);
  assert.equal(picked?.id, SHARED.id);
  assert.equal(posted('/api/oc/shared').length, 1);
  assert.equal(posted('/api/entities').length, 0, '没有新建角色');
});

test('角色选择器：共用原创角色已在本地结果里时不再多列一项', async () => {
  const palette = await web.load('ui/palette.js');
  palette.wirePalette();
  await palette.openPicker({pick() {}});
  entities = [{...SHARED, label_only: true, count: 3}];
  const html = await search(palette, '原创');
  assert.equal(html.split('原创角色（未细分）').length - 1, 1);
});

test('角色选择器：搜索结果里的团体标签不会隐藏共用原创角色', async () => {
  const palette = await web.load('ui/palette.js');
  palette.wirePalette();
  await palette.openPicker({pick() {}});
  entities = [{id: 'group', name: '合成团体', label_only: true, kind: '', count: 1}];
  const html = await search(palette, '合成');
  assert.match(html, /合成团体/);
  assert.match(html, /原创角色（未细分）/);
});

test('markOc：整组标为原创角色设为共用的那一个，不新建独立原创角色', async () => {
  const palette = await web.load('ui/palette.js');
  await palette.markOc({ids: ['u1', 'u2']}, false);
  assert.equal(posted('/api/oc/shared').length, 1);
  assert.equal(posted('/api/entities').length, 0);
  const change = posted('/api/change');
  assert.equal(change.length, 1);
  assert.deepEqual(change[0].body, {ids: ['u1', 'u2'], action: 'confirmed', entity: SHARED.id});
});
