// 资料库 · 画师（ui/artist_dir.js，#448）：读 /api/artist-directory 的参数、分页接着读、迟到的回应、卡片和空状态，
// 以及经 applyRoute 进入 #/characters/artists 后的页签、标题和后退恢复。画师都是合成数据。英文见 artist_directory_en.test.cjs。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let dirs, shell, state;
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
  await web.load('ui/catalog.js');
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
});

test('the query sends order, offset and limit, and q and nsfw only when set', () => {
  const plain = new URL(dirs.dirQuery(0), 'http://x').searchParams;
  assert.deepEqual(Object.fromEntries(plain), {order: 'count', offset: '0', limit: String(dirs.PAGE)});
  Object.assign(dirs.dir, {q: '合成 画师', order: 'recent'}); state.nsfw = 'hide';
  const narrowed = new URL(dirs.dirQuery(240), 'http://x').searchParams;
  assert.deepEqual(Object.fromEntries(narrowed), {q: '合成 画师', order: 'recent', nsfw: 'hide', offset: '240', limit: String(dirs.PAGE)});
});

test('pages are read one after another until the total, without repeats', async () => {
  total = dirs.PAGE * 2 + 5;
  await dirs.renderArtistDir();
  assert.equal(dirs.dir.items.length, dirs.PAGE);
  assert.match(document.getElementById('charIndex').innerHTML, /id="artistDirMore"/);
  await dirs.more(); await dirs.more(); await dirs.more();
  assert.equal(dirs.dir.items.length, total);
  assert.equal(new Set(dirs.dir.items.map(a => a.id)).size, total);
  assert.equal(requests.filter(p => p.startsWith('/api/artist-directory')).length, 3, 'no read after the last page');
  assert.doesNotMatch(document.getElementById('charIndex').innerHTML, /artistDirMore/);
});

test('an answer for the old search that arrives late is dropped', async () => {
  hold = true;
  const first = dirs.renderArtistDir();
  total = 1;   // 新搜索的回答（在请求时按 total 切）只有一位
  dirs.setQuery('合成画师 2');
  release(); await first; release(); await tick(); await tick();
  assert.equal(dirs.dir.q, '合成画师 2');
  assert.equal(dirs.dir.total, 1);
  assert.deepEqual(dirs.dir.items.map(a => a.id), ['synthetic-0']);
});

test('a card links to the library filtered by the artist and escapes the name', () => {
  const html = dirs.card({...artist(1), id: 'a&b', name: '<合成>'});
  assert.match(html, /href="#\/library\?artist=a%26b"/);
  assert.match(html, /<b>&lt;合成&gt;<\/b>/);
  assert.match(html, /<small>99 张<\/small>/);
  assert.match(html, /src="\/image\?id=uuid-1&size=thumb"/);
  dirs.dir.order = 'recent';
  assert.doesNotMatch(dirs.card(artist(1)), /<small>99 张/, 'by recent the second line is the date');
});

test('the empty page says why: a search, R18 only, or no artists yet with the way to settings', async () => {
  total = 0;
  await dirs.renderArtistDir();
  assert.match(document.getElementById('charIndex').innerHTML, /还没有画师。.*href="#\/settings\/artists"/s);
  dirs.dir.q = '无'; assert.match(dirs.emptyText(), /没有符合的画师。/);
  dirs.dir.q = ''; state.nsfw = 'only'; assert.match(dirs.emptyText(), /只看 NSFW/);
});

test('the order is remembered and a new order reads from the start', async () => {
  await dirs.renderArtistDir();
  const sets = [];
  globalThis.localStorage = {getItem: () => null, setItem: (k, v) => sets.push([k, v]), removeItem() {}};
  dirs.setOrder('name'); await tick(); await tick();
  assert.equal(dirs.dir.order, 'name');
  assert.ok(requests.at(-1).includes('order=name') && requests.at(-1).includes('offset=0'));
  dirs.setOrder('bogus');
  assert.equal(dirs.dir.order, 'name');
});

test('#/characters/artists shows the three tabs, the artist count and the cards', async () => {
  globalThis.location = {hash: '#/characters/artists'};
  await shell.applyRoute(); await tick(); await tick();
  shell.renderHeading();   // 页面上由 wireShell 登记的 dirDrawn 触发
  const bar = document.getElementById('toolbar').innerHTML;
  assert.match(bar, /href="#\/characters\/artists" class="on" aria-selected="true">画师</);
  assert.match(bar, /id="artistSearch"/);
  assert.match(bar, /<option value="count" selected>张数最多<\/option>/);
  assert.match(web.headingText(document.getElementById('heading')), /3 位画师/);
  assert.equal((document.getElementById('charIndex').innerHTML.match(/class="cast-rs"/g) || []).length, 3);
});

test('coming back keeps the pages already read; R18 switching reads again', async () => {
  total = dirs.PAGE + 1;
  await dirs.renderArtistDir(); await dirs.more();
  const before = requests.length;
  await dirs.renderArtistDir({restore: true});
  assert.equal(requests.length, before, 'restored from what was read');
  assert.equal(dirs.dir.items.length, total);
  state.nsfw = 'hide';
  dirs.refreshArtistDir('nsfw'); await tick(); await tick();
  assert.ok(requests.at(-1).includes('nsfw=hide') && requests.at(-1).includes('offset=0'));
  assert.equal(dirs.dir.items.length, dirs.PAGE);
});
