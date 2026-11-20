// 收藏的前端（#447）：core/favorites.js 的判断（筛选条件、选区该收藏还是取消、本地改值），F 键的动作，
// ui/grid.js 的写入路径（不在收藏筛选里时只在本地改心形，图会离开本页时照其他写入重读），选择栏的按钮和
// ui/favorites.js 画出的心形。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let fav, ui, grid, modes, pages, state, edits, assets;
before(async () => {
  await web.ready();
  fav = await web.load('core/favorites.js');
  modes = await web.load('core/modes.js');
  pages = await web.load('core/pages.js');
  globalThis.window = {addEventListener() {}};
  globalThis.ResizeObserver = class { observe() {} };
  globalThis.requestAnimationFrame = () => 0;
  const store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)};
  web.fakePage();
  grid = await web.load('ui/grid.js'); assets = await web.load('core/assets.js'); await web.load('ui/grid_bar.js');
  ui = await web.load('ui/favorites.js');
  ({state, edits} = web.core());
});
// 一页图：a、b 是同一组副本，c 已收藏，d 不在任何组里。
const page = () => [{uuid: 'a', fav: false, dup: {grp: 'g1', size: 2, pos: 1}, memberships: []}, {uuid: 'b', fav: false, dup: {grp: 'g1', size: 2, pos: 2}, memberships: []},
  {uuid: 'c', fav: true, memberships: []}, {uuid: 'd', fav: false, memberships: []}];
let requests;
beforeEach(() => {
  web.fakePage();
  web.core();
  Object.assign(state, {route: {page: 'library'}, items: page(), total: 4, selecting: false, sel: {all: false, ids: new Set(), exclude: new Set()},
    anchor: -1, cursor: -1, focused: null, detail: null, viewer: -1, viewerItem: null, armed: null, nsfw: ''});
  requests = [];
  globalThis.fetch = async (path, init = {}) => {
    const body = init.body ? JSON.parse(init.body) : null; requests.push({path, body});
    const answer = path === '/api/favorite' ? {undo: {id: 'u' + requests.length, label: body.add ? '已收藏 1 张' : '已取消收藏 1 张'}}
      : path.startsWith('/api/assets/count') ? {total: path.includes('not=') ? 0 : 4} : path.startsWith('/api/assets') ? {total: 0, items: []} : {};
    return {status: 200, ok: true, json: async () => answer};
  };
});

test('筛选条件读成数组：地址里的 JSON 字符串和已经读成的数组都认，读不了的当没有', () => {
  assert.deepEqual(fav.conditions('["fav","tag:x"]'), ['fav', 'tag:x']);
  assert.deepEqual(fav.conditions(['fav']), ['fav']);
  assert.deepEqual(fav.conditions(''), []);
  assert.deepEqual(fav.conditions('not json'), []);
  assert.deepEqual(fav.conditions('{"fav":1}'), []);
  // 加一个条件：已有的保留，重复的不再加。
  assert.deepEqual(fav.withCondition({has: '["tag:x"]', q: 'k'}, 'not', 'fav'), {has: '["tag:x"]', q: 'k', not: '["fav"]'});
  assert.deepEqual(fav.withCondition({not: '["fav"]'}, 'not', 'fav'), {not: '["fav"]'});
});

test('哪些写入会让图离开本页：只看收藏里取消，不含收藏里收藏', () => {
  assert.equal(fav.favLeaves(false, {page: 'library', has: '["fav"]'}), true);
  assert.equal(fav.favLeaves(true, {page: 'library', has: '["fav"]'}), false);
  assert.equal(fav.favLeaves(true, {page: 'library', not: ['fav']}), true);
  assert.equal(fav.favLeaves(false, {page: 'library'}), false);
  assert.equal(fav.favLeaves(true, {page: 'unmatched'}), false);
});

test('收藏是画廊里的一项（#706）：只在图库只按收藏筛选时活页条亮「画廊 › 收藏」，别的时候照页面登记', async () => {
  const {railAt} = await web.load('core/rail.js');
  const at = route => { const x = railAt(route); return `${x.entry}/${x.item}`; };
  assert.equal(at({page: 'library', has: '["fav"]'}), 'galleries/fav');
  assert.equal(at({page: 'library', has: ['fav']}), 'galleries/fav');
  assert.equal(at({page: 'library'}), 'library/');
  assert.equal(at({page: 'library', has: '["fav","tag:x"]'}), 'library/');
  assert.equal(at({page: 'library', has: '["fav"]', q: 'miku'}), 'library/');
  assert.equal(at({page: 'library', has: '["fav"]', not: '["tag:y"]'}), 'library/');
  assert.equal(at({page: 'library', not: '["fav"]'}), 'library/');
  assert.equal(at({page: 'duplicates', has: '["fav"]'}), 'tidy/duplicates');
});

test('收藏：标题是「收藏」，空着时说怎么收藏；加了别的条件照旧是「图库」（#679）', async () => {
  const {renderHeading} = await web.load('ui/shell.js');
  await web.load('ui/library.js');   // 图库的标题由图库这一页登记
  const heading = () => globalThis.document.getElementById('heading').innerHTML;
  Object.assign(state, {route: {page: 'library', has: ['fav']}, total: 0, totalReady: true});
  renderHeading();
  assert.match(heading(), /<h1 class="title-page">收藏<button[^>]*id="pageHelp"/);
  assert.match(heading(), /<p>在详情面板或大图里点心形，或者按 F 收藏。<\/p>/);
  const empty = pages.PAGES.library.empty;
  assert.equal(empty(state.route, {nsfw: ''}), '还没有收藏。在详情面板或大图里点心形，或者按 F 收藏。');
  // 隐藏或只看 R18 时空着不等于没有收藏
  assert.equal(empty(state.route, {nsfw: 'hide'}), '没有符合条件的图片。');
  assert.match(empty(state.route, {nsfw: 'only'}), /只看 NSFW/);
  for (const r of [{page: 'library', has: ['fav', 'tag:x']}, {page: 'library', has: ['fav'], q: 'miku'}, {page: 'library'}]) {
    state.route = r; renderHeading();
    assert.match(heading(), /<h1 class="title-page">图库/);
    assert.equal(empty(r, {nsfw: ''}), '没有符合条件的图片。');
  }
});
test('选中的图该收藏还是取消：有一张没收藏、或有选中的图还没读到就收藏，否则取消', () => {
  const [a, , c] = page();
  assert.equal(fav.wantsAdd([a, c]), true);
  assert.equal(fav.wantsAdd([c]), false);
  assert.equal(fav.wantsAdd([c], 2), true);
  // 列表里没有 fav 字段（面板页借来的列表）时看详情栏读到的那份。
  assert.equal(fav.isFav({uuid: 'x'}, {detail: {uuid: 'x', fav: true}}), true);
  assert.equal(fav.isFav({uuid: 'x'}, {detail: {uuid: 'y', fav: true}}), false);
  assert.equal(fav.isFav(null), false);
});

test('本地改值连同组的副本一起改，没点到的组不动', () => {
  const list = page(), changed = fav.markLocal(list, x => x.uuid === 'a', true);
  assert.deepEqual([...changed].sort(), ['a', 'b']);
  assert.deepEqual(list.map(x => x.fav), [true, true, true, false]);
  const back = fav.markLocal([...list, undefined], x => x.uuid === 'c', false);
  assert.deepEqual([...back], ['c']);
});

test('F：有选中时交给选择栏的「收藏」，没有选中时切换当前这张；帮助里写着 F', () => {
  const view = pages.PAGES.library;
  assert.deepEqual(modes.keyAction('f', {view, n: 2}), {bar: 'fav'});
  assert.deepEqual(modes.keyAction('f', {view, n: 0, item: {uuid: 'a'}}), {single: 'fav'});
  assert.deepEqual(modes.keyAction('f', {view: pages.PAGES.unmatched, n: 1}), {bar: 'fav'});
  assert.ok(modes.SHORTCUTS.some(s => s.codes.includes('f')));
});

test('不在收藏筛选里：收藏只发一次写入，本地改掉这张和同组副本，不重读列表；再点一次取消', async () => {
  await grid.favoriteOne('a');
  assert.deepEqual(requests.map(r => r.path), ['/api/favorite']);
  assert.deepEqual(requests[0].body, {ids: ['a'], add: true});
  assert.deepEqual(state.items.map(x => x.fav), [true, true, true, false]);
  assert.equal(edits.latest.label, '已收藏 1 张');
  await grid.favoriteOne('b');
  assert.deepEqual(requests[1].body, {ids: ['b'], add: false});
  assert.deepEqual(state.items.map(x => x.fav), [false, false, true, false]);
});

test('详情栏、大图里那张也跟着改', async () => {
  Object.assign(state, {detail: {uuid: 'd', fav: false}, viewerItem: state.items[3], viewer: 3});
  await grid.favoriteOne('d');
  assert.equal(state.detail.fav, true);
  assert.equal(state.viewerItem.fav, true);
});

test('只看收藏里取消：照其他写入重读列表，读列表时带收藏标记（marks=fav,color，主色给卡片，#708）', async () => {
  state.route = {page: 'library', has: '["fav"]'};
  await grid.favoriteOne('c');
  assert.deepEqual(requests[0], {path: '/api/favorite', body: {ids: ['c'], add: false}});
  const list = requests.find(r => r.path.startsWith('/api/assets?'));
  assert.ok(list, '取消收藏后应重读列表');
  assert.equal(new URLSearchParams(list.path.split('?')[1]).get('marks'), 'fav,color');
});

test('全选时先问后端选区里还有没有没收藏的：都收藏了就全部取消', async () => {
  state.items[3].fav = true;
  state.sel = {all: true, ids: new Set(), exclude: new Set(['d'])};
  grid.renderSelection();
  assert.deepEqual(web.barButton(document.getElementById('selectionBar'), 'fav'), {act: 'fav', label: '收藏', key: 'F', disabled: false, title: ''});
  await grid.favoriteSelection({filter: assets.gridFilters(), exclude: ['d'], confirm: false});
  const count = requests.find(r => r.path.startsWith('/api/assets/count'));
  assert.deepEqual(JSON.parse(new URLSearchParams(count.path.split('?')[1]).get('not')), ['fav']);
  const write = requests.find(r => r.path === '/api/favorite');
  assert.equal(write.body.add, false);
  assert.deepEqual(write.body.exclude, ['d']);
  // 排除在外的那张不动；写完退出选择。
  assert.deepEqual(state.items.map(x => x.fav), [false, false, false, true]);
  assert.equal(state.sel.all, false);
});

test('选择栏的按钮：选中的都已收藏时是「取消收藏」，超过 500 张要再点一次', () => {
  state.sel = {all: false, ids: new Set(['c']), exclude: new Set()};
  grid.renderSelection();
  assert.deepEqual(web.barButton(document.getElementById('selectionBar'), 'fav'), {act: 'fav', label: '取消收藏', key: 'F', disabled: false, title: ''});
  state.sel = {all: false, ids: new Set(['a', 'c']), exclude: new Set()};
  grid.renderSelection();
  assert.deepEqual(web.barButton(document.getElementById('selectionBar'), 'fav'), {act: 'fav', label: '收藏', key: 'F', disabled: false, title: ''});
});

// 网格卡片上的心形是 ui/grid_card.js 的部件，在 tests/grid_card.test.cjs 里测；详情卡片、大图的心形在各自的测试里。

test('写入后就地改掉页面上这些图的心形，别的图不动', () => {
  const heart = (id, label) => {
    const el = new web.FakeElement('button'); el.dataset.fav = id;
    const span = label ? Object.assign(new web.FakeElement('span'), {textContent: '收藏'}) : null;
    el.querySelector = sel => sel === '.fav-label' ? span : null; el.span = span; return el;
  };
  const els = [heart('a'), heart('b', true), heart('c')];
  ui.paintHearts(new Set(['a', 'b']), true, {querySelectorAll: () => els});
  assert.deepEqual(els.map(el => el.classList.contains('on')), [true, true, false]);
  assert.equal(els[0].getAttribute('aria-pressed'), 'true');
  assert.equal(els[0].title, '取消收藏（F）');
  assert.equal(els[1].span.textContent, '已收藏');
});
