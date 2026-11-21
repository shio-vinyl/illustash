// 运行：node --test tests/like.test.cjs
// 「更多像这张的」（#449）：地址 #/library?like=[…]&by=ccip 读成图库的相似结果视图，查询带上 like、like_by，默认排序
// 「最相似」；入口在详情栏、大图和选择栏（1–20 张、非全选）；例图全都没有特征时网格空着写明原因、不弹错误条；
// 多张例图里没有特征的在标题里变淡并计数。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let page, pages, route, like, grid, detail, state, assets;
const store = new Map();
before(async () => {
  await web.ready();
  page = web.fakePage();
  page.document.addEventListener = () => {};
  globalThis.window = {addEventListener() {}};
  globalThis.ResizeObserver = class { observe() {} };
  globalThis.requestAnimationFrame = () => 0;
  globalThis.innerWidth = 1000; globalThis.innerHeight = 800;
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)};
  globalThis.history = {state: null, replaceState() {}};
  grid = await web.load('ui/grid.js'); assets = await web.load('core/assets.js'); grid.wireGrid(); await web.load('ui/grid_bar.js');
  detail = await web.load('ui/detail.js');
  await web.load('ui/like.js');
  pages = await web.load('core/pages.js');
  like = await web.load('core/like.js');
  ({state} = web.core()); route = web.core();
});
beforeEach(() => { store.clear(); globalThis.location = {hash: ''}; globalThis.fetch = undefined; });
const at = hash => { globalThis.location.hash = hash; return route.parseHash(); };
const examples = (...ids) => encodeURIComponent(JSON.stringify(ids));
const reply = (status, body) => ({status, ok: status < 300, headers: {get: () => null}, json: async () => body});
const settle = () => new Promise(r => setTimeout(r, 0));

test('地址读成相似结果：图库加例图，比较方式只认 ccip', () => {
  const r = at(`#/library?like=${examples('a', 'b')}&by=ccip&tag=x`);
  assert.equal(r.page, 'library');
  assert.deepEqual(like.likeIds(r), ['a', 'b']);
  assert.equal(r.by, 'ccip');
  assert.equal(pages.viewOf(r), pages.LIKE, '是相似结果视图，不是图库或角色页');
  assert.equal(pages.viewOf(r).entry, 'library', '活页条亮「图库」');
  assert.equal(pages.viewOf(r).click, 'browse', '点卡片打开详情');
  assert.equal(at(`#/library?like=${examples('a')}&by=wd`).by, '', '按画面是省略 by');
  // 读不出的例图、角色页、别的页面上都不认
  assert.equal(at('#/library?like=%5Bx').like, '');
  assert.equal(at('#/library?like=%5B%5D').like, '');
  assert.equal(at(`#/library?char=c1&like=${examples('a')}`).like, '');
  assert.equal(at(`#/unmatched?like=${examples('a')}`).like, '');
  assert.equal(pages.viewOf(at(`#/library?char=c1&like=${examples('a')}`)), pages.CHARACTER);
});

test('查询带上 like、like_by，叠加图库的筛选；默认排序是「最相似」', () => {
  const r = at(`#/library?like=${examples('a', 'b', 'a')}&by=ccip&tag=x&q=y`);
  r.sort = r.sort || route.defaultSort(r);
  state.route = r;
  assert.equal(r.sort, 'similar');
  assert.deepEqual(assets.gridFilters(), {order: 'similar', tag: 'x', q: 'y', like: '["a","b"]', like_by: 'ccip'});
  assert.deepEqual(pages.LIKE.sorts, ['similar', 'newest', 'oldest']);
  // 默认排序不写进地址；改了的记在相似结果自己的键里，不动图库的默认排序
  assert.equal(route.hashFor(r), `#/library?like=${examples('a', 'b', 'a')}&by=ccip&q=y&tag=x`);
  assert.equal(route.sortKey(r), 'sortLike');
  route.prefs.set('sortLike', 'newest');
  assert.equal(route.defaultSort(r), 'newest');
  assert.equal(route.defaultSort({page: 'library'}), 'newest', '图库的默认排序没被带走');
  route.prefs.set('sortLibrary', 'oldest');
  assert.equal(route.defaultSort({...r, sort: ''}), 'newest');
  store.clear();
  // 没有例图的图库照旧
  const plain = at('#/library?tag=x'); plain.sort = route.defaultSort(plain); state.route = plain;
  assert.deepEqual(assets.gridFilters(), {order: 'newest', tag: 'x'});
  assert.equal(at('#/library?sort=similar').sort, '', '离开例图「最相似」不再有效');
  state.route = {page: 'library'};
});

test('换一组例图：去重、最多 20 张；去掉最后一张回到同样筛选的图库', () => {
  const ids = Array.from({length: 25}, (_, i) => 'p' + i);
  assert.equal(like.likeIds(like.likeRoute([...ids, 'p0'])).length, 20);
  assert.deepEqual(like.likeRoute(['a', 'a'], 'ccip'), {page: 'library', like: '["a"]', by: 'ccip'});
  assert.deepEqual(like.likeRoute(['a'], 'nope').by, '');
  const r = {page: 'library', like: '["a","b"]', by: 'ccip', tag: 'x', sort: 'similar'};
  assert.deepEqual(like.withExamples(r, ['b']), {...r, like: '["b"]'});
  assert.deepEqual(like.withExamples(r, []), {page: 'library', like: '', by: '', tag: 'x', sort: ''});
  assert.equal(like.withExamples({...r, sort: 'oldest'}, []).sort, 'oldest');
  assert.ok(like.canLike(1) && like.canLike(20) && !like.canLike(0) && !like.canLike(21));
});

test('选择栏：选了 1–20 张时有「更多像这张的／这些的」，全选和超过 20 张时没有', () => {
  const items = Array.from({length: 21}, (_, i) => ({uuid: 'p' + i, memberships: []}));
  const bar = sel => { Object.assign(state, {route: {page: 'library'}, items, total: items.length, selecting: true, sel}); grid.renderSelection(); return web.barButton(page.el('selectionBar'), 'like'); };
  const pick = (...ids) => ({all: false, ids: new Set(ids), exclude: new Set()});
  assert.equal(bar(pick('p0'))?.label, '更多像这张的');
  assert.equal(bar(pick('p0', 'p1'))?.label, '更多像这些的');
  assert.ok(bar(pick(...items.slice(0, 20).map(x => x.uuid))));
  assert.equal(bar(pick(...items.map(x => x.uuid))), null);
  assert.equal(bar({all: true, ids: new Set(), exclude: new Set()}), null);
  state.sel = pick(); state.selecting = false; grid.renderSelection();
});

test('详情卡片的身份块里有「更多像这张的」', async () => {
  globalThis.fetch = async path => path.startsWith('/api/detail')
    ? reply(200, {uuid: 'a', memberships: [], alternatives: [], tags: [], user_tags: []}) : reply(404, {error: 'stop'});
  state.route = {page: 'library'}; state.items = [{uuid: 'a'}]; state.total = 1;
  await detail.inspect('a');
  // 卡片是 ui/detail_card.js 的部件（#709），挂在 #inspector 里。
  const all = el => (el.children || []).flatMap(c => [c, ...all(c)]);
  const button = all(page.el('inspector')).find(x => x.dataset?.act === 'like');
  assert.equal(button.textContent, '更多像这张的');
  assert.equal(button.parentNode.hidden, false, '还没有角色时也在');
  detail.closeInspector();
});

test('例图全都没有特征：网格空着写明原因，不弹错误条', async () => {
  globalThis.fetch = async () => reply(400, {error: '这张图还没有特征，识别完成后再试'});
  state.route = {...like.likeRoute(['a']), sort: 'similar'};
  page.el('errorBanner').hidden = true;
  await grid.reload(true);
  assert.equal(page.el('errorBanner').hidden, true);
  assert.equal(page.el('empty').hidden, false);
  assert.equal(web.emptyState(page.el('empty')).fact, '例图还没有特征，识别完成后再试。');
  assert.equal(state.total, 0);
  // 别的失败照旧走错误条
  globalThis.fetch = async () => reply(400, {error: '未知排序'});
  await grid.reload(true);
  assert.equal(page.el('errorBanner').hidden, false);
  assert.equal(page.el('errorBanner').textContent, '未知排序');
  page.el('errorBanner').hidden = true;
  state.route = {page: 'library'};
});

test('标题：例图缩略图、按画面／按角色；多张里没有特征的变淡并计数', async () => {
  const asked = [];
  globalThis.fetch = async path => {
    asked.push(decodeURIComponent(path));
    return path.includes('%22b%22') ? reply(400, {error: '这张图还没有特征，识别完成后再试'}) : reply(200, {total: 3});
  };
  const r = {...like.likeRoute(['a', 'b'], 'ccip'), sort: 'similar'};
  Object.assign(state, {route: r, total: 7, totalReady: true});
  const h = page.el('heading');
  pages.LIKE.drawHeading(h, r);
  assert.match(h.innerHTML, /<h1 class="title-page">更多像这些的/);
  assert.match(h.innerHTML, /<p class="page-sub">7 张<\/p>/);
  await settle(); await settle();
  assert.deepEqual(asked.sort(), ['/api/assets/count?like=["a"]&like_by=ccip', '/api/assets/count?like=["b"]&like_by=ccip']);
  // 问完自己重画一次
  assert.match(h.innerHTML, /<p class="page-sub">7 张 · 1 张例图没有特征<\/p>/);
  assert.match(h.innerHTML, /<span class="like-example off"><button data-like-show="b" title="还没有特征，没有算进去">/);
  assert.match(h.innerHTML, /<span class="like-example"><button data-like-show="a" title="查看这张">/);
  assert.match(h.innerHTML, /data-like-drop="a"/);
  assert.match(h.innerHTML, /<button data-like-by="" class="" aria-pressed="false">按画面<\/button><button data-like-by="ccip" class="on" aria-pressed="true">按角色<\/button>/);
  // 再画不再问；读取轮次变了（写入、同步之后）只重问没有特征的那张
  asked.length = 0;
  pages.LIKE.drawHeading(h, r); await settle();
  assert.deepEqual(asked, []);
  state.readEpoch++;
  pages.LIKE.drawHeading(h, r); await settle(); await settle();
  assert.deepEqual(asked, ['/api/assets/count?like=["b"]&like_by=ccip']);
  // 只有一张例图：不另外问，也没有 ×
  asked.length = 0;
  const one = {...like.likeRoute(['a']), sort: 'similar'}; state.route = one;
  pages.LIKE.drawHeading(h, one);
  assert.match(h.innerHTML, /<h1 class="title-page">更多像这张的/);
  assert.doesNotMatch(h.innerHTML, /data-like-drop/);
  assert.deepEqual(asked, []);
  state.route = {page: 'library'};
});
