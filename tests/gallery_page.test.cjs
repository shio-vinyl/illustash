// 运行：node --test tests/gallery_page.test.cjs
// 一个画廊的页面（#445）：条件失效时（条件引用的角色、作品被删掉或合并，后端回 400）原因只在空状态里写一次，下面给
// 「删除画廊」，不弹错误条，进页面时也不为页头另数一次（#647）。条件、名称都是合成的。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let page, grid, shell, pages, state;
const store = new Map();
before(async () => {
  await web.ready();
  page = web.fakePage();
  globalThis.window = {addEventListener() {}};
  globalThis.ResizeObserver = class { observe() {} };
  globalThis.requestAnimationFrame = () => 0;
  globalThis.innerWidth = 1000; globalThis.innerHeight = 800;
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)};
  globalThis.history = {state: null, replaceState() {}};
  grid = await web.load('ui/grid.js'); grid.wireGrid();
  shell = await web.load('ui/shell.js');
  await web.load('ui/galleries.js');
  pages = await web.load('core/pages.js');
  ({state} = web.core());
});
const reply = (status, body) => ({status, ok: status < 300, headers: {get: () => null}, json: async () => body});
const gallery = {id: 3, name: '海边', params: {has: '["char:gone"]'}, cover_manual: false, position: 0};
let asked;
beforeEach(() => {
  store.clear(); asked = [];
  globalThis.location = {hash: '#/gallery/3?has=%5B%22char%3Agone%22%5D'};
  Object.assign(state, {galleries: [gallery], nsfw: '', route: {page: 'gallery', id: '3', has: ['char:gone']}});
  page.el('errorBanner').hidden = true;
});

test('条件失效：原因只写在空状态里，下面给「删除画廊」；不弹错误条，进页面不另数一次', async () => {
  globalThis.fetch = async path => {
    asked.push(path);
    return path.startsWith('/api/assets') ? reply(400, {error: '角色不存在'}) : reply(200, [gallery]);
  };
  assert.equal(await pages.PAGES.gallery.prepare(state.route), true);
  await grid.reload(true);
  assert.equal(page.el('errorBanner').hidden, true);
  assert.equal(page.el('empty').hidden, false);
  assert.deepEqual(web.emptyState(page.el('empty')), {fact: '条件失效：角色不存在', next: [{label: '删除画廊', act: 'delete-gallery'}]});
  assert.ok(!asked.some(p => p.startsWith('/api/assets/count')), '不为页头另数一次');
  shell.renderHeading();
  const h = web.headingText(page.el('heading'));
  assert.doesNotMatch(h, /条件失效/, '页头不再写一遍原因');
  assert.match(h, /按条件.* · 0 张/);
});

test('别的失败照旧走错误条', async () => {
  globalThis.fetch = async path => path.startsWith('/api/assets') ? reply(500, {error: '数据库忙'}) : reply(200, [gallery]);
  await grid.reload(true);
  assert.equal(page.el('errorBanner').hidden, false);
  assert.equal(page.el('errorBanner').textContent, '数据库忙');
});
