// 资料库 · 画师（ui/artist_dir.js，#448；#711 改成部件）：读 /api/artist-directory 的参数、分页接着读、迟到的回应、卡片和空状态，
// 以及经 applyRoute 进入 #/characters/artists 后的页签切换、标题和后退恢复。画师都是合成数据。英文见 artist_directory_en.test.cjs。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let dirs, shell, state, view, scopes;
const tick = () => new Promise(resolve => setImmediate(resolve));
const artist = i => ({id: `synthetic-${i}`, name: `合成画师 ${i}`, count: 100 - i, last: '2026-11-10 21:03:00', cover: `uuid-${i}`, homes: []});
// 假的后端：按 offset、limit 切 total 位合成画师；hold 为真时先不回答，由测试调 release()。
let requests, total, hold, pending;
function serve(path) {
  requests.push(path);
  const url = new URL(path, 'http://x');
  if (url.pathname === '/api/artist-directory') {
    const offset = Number(url.searchParams.get('offset')), limit = Number(url.searchParams.get('limit'));
    const body = {items: Array.from({length: Math.max(0, Math.min(limit, total - offset))}, (_, k) => artist(offset + k)), total, offset};
    const reply = {status: 200, ok: true, json: async () => body};
    if (hold) return new Promise(resolve => pending.push(() => resolve(reply)));
    return Promise.resolve(reply);
  }
  return Promise.resolve({status: 200, ok: true, json: async () => (url.pathname === '/api/works' || url.pathname === '/api/tagged' ? [] : {})});
}
const release = () => { const list = pending; pending = []; list.forEach(f => f()); };

before(async () => {
  await web.ready();
  dirs = await web.load('ui/artist_dir.js');
  shell = await web.load('ui/shell.js');
  await web.load('ui/catalog_page.js');
  scopes = await web.load('ui/scope.js');
  ({state} = web.core());
  globalThis.history = {replaceState() {}, state: null};
  globalThis.requestAnimationFrame ??= () => 0;
  globalThis.localStorage ??= {getItem: () => null, setItem() {}, removeItem() {}};
});
beforeEach(() => {
  requests = []; total = 3; hold = false; pending = [];
  globalThis.fetch = serve;
  web.fakePage();
  Object.assign(dirs.dir, {q: '', order: 'count', items: [], total: null, next: 0, failed: null, key: ''});
  Object.assign(state, {route: {page: 'characters', section: 'artists'}, nsfw: ''});
  view = dirs.artistsView(scopes.scope('test'));
});
// 页签里的卡片（圆头像卡的名字）和「再显示一些」是否还在。
const names = () => view.el.querySelectorAll('.cat-person').map(a => a.querySelector('b').textContent);
const moreShown = () => !view.el.querySelector('.cat-more').hidden;

test('the query sends order, offset and limit, and q and nsfw only when set', () => {
  const plain = new URL(dirs.dirQuery(0), 'http://x').searchParams;
  assert.deepEqual(Object.fromEntries(plain), {order: 'count', offset: '0', limit: String(dirs.PAGE)});
  Object.assign(dirs.dir, {q: '合成 画师', order: 'recent'}); state.nsfw = 'hide';
  const narrowed = new URL(dirs.dirQuery(240), 'http://x').searchParams;
  assert.deepEqual(Object.fromEntries(narrowed), {q: '合成 画师', order: 'recent', nsfw: 'hide', offset: '240', limit: String(dirs.PAGE)});
});

test('pages are read one after another until the total, without repeats', async () => {
  total = dirs.PAGE * 2 + 5;
  await view.start();
  assert.equal(dirs.dir.items.length, dirs.PAGE);
  assert.equal(names().length, dirs.PAGE);
  assert.ok(moreShown());
  await dirs.more(); await dirs.more(); await dirs.more();
  assert.equal(dirs.dir.items.length, total);
  assert.equal(new Set(dirs.dir.items.map(a => a.id)).size, total);
  assert.equal(requests.filter(p => p.startsWith('/api/artist-directory')).length, 3, 'no read after the last page');
  assert.equal(names().length, total);
  assert.ok(!moreShown());
});

test('an answer for the old search that arrives late is dropped', async () => {
  hold = true;
  const first = view.start();
  total = 1;   // 新搜索的回答（在请求时按 total 切）只有一位
  dirs.setQuery('合成画师 2');
  release(); await first; release(); await tick(); await tick();
  assert.equal(dirs.dir.q, '合成画师 2');
  assert.equal(dirs.dir.total, 1);
  assert.deepEqual(dirs.dir.items.map(a => a.id), ['synthetic-0']);
});

test('a card links to the library filtered by the artist; the second line is the count or, by recent, the date', () => {
  const card = dirs.cardData({...artist(1), id: 'a&b', name: '<合成>'});
  assert.equal(card.href, '#/library?artist=a%26b');
  assert.equal(card.name, '<合成>');
  assert.equal(card.sub, '99 张');
  assert.equal(card.cover, 'uuid-1');
  dirs.dir.order = 'recent';
  assert.notEqual(dirs.cardData(artist(1)).sub, '99 张', 'by recent the second line is the date');
});

test('the empty page says why: a search, NSFW only, or no artists yet with the way to settings', async () => {
  total = 0;
  await view.start();
  const empty = view.el.querySelector('.empty-state');
  assert.equal(empty.querySelector('.empty-fact').textContent, '还没有画师');
  assert.equal(dirs.emptyState().next[0].href, '#/settings/artists');
  dirs.dir.q = '无'; assert.equal(dirs.emptyState().fact, '没有符合的画师。');
  dirs.dir.q = ''; state.nsfw = 'only'; assert.match(dirs.emptyState().fact, /只看 NSFW/);
});

test('the order is remembered and a new order reads from the start', async () => {
  await view.start();
  const sets = [];
  globalThis.localStorage = {getItem: () => null, setItem: (k, v) => sets.push([k, v]), removeItem() {}};
  dirs.setOrder('name'); await tick(); await tick();
  assert.equal(dirs.dir.order, 'name');
  assert.ok(requests.at(-1).includes('order=name') && requests.at(-1).includes('offset=0'));
  dirs.setOrder('bogus');
  assert.equal(dirs.dir.order, 'name');
});

test('#/characters/artists shows the three tabs in the heading, the artist count and the cards', async () => {
  globalThis.location = {hash: '#/characters/artists'};
  await shell.applyRoute(); await tick(); await tick();
  const {PAGES} = await web.load('core/pages.js');
  const tabs = PAGES.characters.switches(state.route).list;
  assert.deepEqual(tabs.map(x => [x.label, x.href, x.on]), [['角色', '#/characters', false], ['作品', '#/characters/works', false], ['画师', '#/characters/artists', true]]);
  assert.equal(PAGES.characters.head(state.route).sub, '3 位画师');
  assert.equal(dirs.dir.items.length, 3);
});

test('coming back keeps the pages already read; R18 switching reads again', async () => {
  total = dirs.PAGE + 1;
  await view.start(); await dirs.more();
  const before = requests.length;
  await view.start({restore: true});
  assert.equal(requests.length, before, 'restored from what was read');
  assert.equal(dirs.dir.items.length, total);
  state.nsfw = 'hide';
  view.reload('nsfw'); await tick(); await tick();
  assert.ok(requests.at(-1).includes('nsfw=hide') && requests.at(-1).includes('offset=0'));
  assert.equal(dirs.dir.items.length, dirs.PAGE);
});
