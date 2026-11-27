// 标签选择器和角色面板的「最近使用」按图库分开存（#523）：键里带图库 ID（core/prefs.js 的 libraryPrefs），图库 ID
// 每个页面取一次（core/settings.js 的 currentLibraryId）；0.2.0 及以前不分图库的旧记录在 wirePalette() 时丢掉。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
before(web.ready);

let store, libraryCalls, libraryId;
const tick = () => new Promise(r => setImmediate(r));
beforeEach(() => {
  store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, String(v)), removeItem: k => store.delete(k)};
  libraryCalls = 0;
  globalThis.fetch = async path => {
    if (path === '/api/libraries') { libraryCalls++; return {status: 200, ok: true, json: async () => ({info: {id: libraryId, name: 'x'}})}; }
    return {status: 200, ok: true, json: async () => []};
  };
});

test('libraryPrefs：同一个键按图库 ID 各存各的；没有 ID 时不读也不存', () => {
  const {libraryPrefs, prefs} = web.core();
  libraryPrefs('A').set('tagPicks', ['sky']);
  libraryPrefs('B').set('tagPicks', ['sea']);
  assert.deepEqual(libraryPrefs('A').get('tagPicks', []), ['sky']);
  assert.deepEqual(libraryPrefs('B').get('tagPicks', []), ['sea']);
  libraryPrefs('').set('tagPicks', ['lost']);
  assert.deepEqual(libraryPrefs('').get('tagPicks', []), []);
  assert.deepEqual(prefs.get('tagPicks', []), [], '不写到不分图库的旧键');
});

test('取不到图库 ID 时不列也不记最近使用，下次打开再取', async () => {
  const page = web.fakePage();
  const {state, session} = web.core(); session.token = 'a';
  Object.assign(state, {userTags: [{tag: 'sky', count: 2}], chars: [], charMap: new Map()});
  const palette = await web.load('ui/palette.js');
  palette.wirePalette();
  libraryId = '';
  let picked = null;
  await palette.openPicker({tagMode: true, present: new Map(), pick: e => { picked = e; }});
  page.el('pickerSearch').listeners.keydown[0]({key: 'Enter', preventDefault() {}, isComposing: false, keyCode: 13});
  assert.equal(picked.tag, 'sky');
  assert.deepEqual([...store.keys()], [], '什么都没记');
  libraryId = 'A';
  assert.equal(await (await web.load('core/settings.js')).currentLibraryId(), 'A');
  assert.equal(libraryCalls, 2, '上次没取到，这次重新取');
});

test('选择器只列出当前图库用过的标签，选中的记在当前图库下；旧的共用记录被丢掉', async () => {
  const page = web.fakePage();
  const {state, session} = web.core(); session.token = 'a';
  Object.assign(state, {userTags: [{tag: 'sky', count: 2}, {tag: 'sea', count: 1}], chars: [], charMap: new Map()});
  store.set('pi.tagPicks', JSON.stringify(['from-old-version']));
  store.set('pi.picks', JSON.stringify([{id: 7, name: 'old'}]));
  store.set('pi.tagPicks@B', JSON.stringify(['sea']));
  libraryId = 'A';
  const palette = await web.load('ui/palette.js');
  palette.wirePalette();   // 每次载入页面时
  assert.equal(store.has('pi.tagPicks'), false); assert.equal(store.has('pi.picks'), false);

  let picked = null;
  await palette.openPicker({tagMode: true, present: new Map(), pick: e => { picked = e; }});
  const results = page.el('pickerResults').innerHTML;
  assert.doesNotMatch(results, /最近使用/, '图库 A 还没用过标签');
  assert.doesNotMatch(results, /from-old-version/);
  // 选中「我的标签」里的 sky（第一项）
  const keydown = page.el('pickerSearch').listeners.keydown[0];
  keydown({key: 'Enter', preventDefault() {}, isComposing: false, keyCode: 13});
  assert.equal(picked.tag, 'sky');
  assert.deepEqual(JSON.parse(store.get('pi.tagPicks@A')), ['sky']);
  assert.deepEqual(JSON.parse(store.get('pi.tagPicks@B')), ['sea'], '别的图库的记录不变');

  await palette.openPicker({tagMode: true, present: new Map(), pick() {}});
  assert.match(page.el('pickerResults').innerHTML, /最近使用.*sky/s);
  assert.equal(libraryCalls, 0, '图库 ID 上一个测试已经取到，同一个页面不再取');
});


test('最近使用按 ID 读取当前形态和人工名，选择后仍保存原名', async () => {
  const page = web.fakePage();
  const {state, session, libraryPrefs} = web.core();
  session.token = 'a'; libraryId = 'A';
  const base = {id: 'base', name: '自取的名字'}, guard = {id: 'guard', name: '合成角色', name_disambiguation: {qualifier: '近卫'}};
  Object.assign(state, {chars: [base], charMap: new Map([[base.id, base]])});
  const recent = libraryPrefs('A');
  recent.set('picks', [base, guard].map(({id}) => ({id, name: '旧名字'})));
  const reads = [], fetchOther = globalThis.fetch;
  globalThis.fetch = async (url, opts) => {
    if (!url.startsWith('/api/characters?')) return fetchOther(url, opts);
    reads.push(new URL(url, 'http://localhost').searchParams.get('id'));
    return {status: 200, ok: true, json: async () => [guard]};
  };
  const palette = await web.load('ui/palette.js');
  palette.wirePalette();
  let picked = null;
  await palette.openPicker({jump: true, pick: e => { picked = e; }});
  assert.deepEqual(reads, [guard.id]);
  assert.match(page.el('pickerResults').innerHTML, /自取的名字.*合成角色（近卫）/s);
  assert.doesNotMatch(page.el('pickerResults').innerHTML, /旧名字/);
  const key = key => page.el('pickerSearch').listeners.keydown[0]({key, preventDefault() {}, isComposing: false, keyCode: 13});
  key('ArrowDown');
  assert.match(page.el('pickerFoot').innerHTML, /合成角色（近卫）/);
  key('Enter'); await tick();
  assert.equal(picked.id, guard.id);
  assert.deepEqual(recent.get('picks', [])[0], {id: guard.id, name: guard.name});
});
