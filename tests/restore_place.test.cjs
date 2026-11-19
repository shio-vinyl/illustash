// #402：离开网格页再回到同一地址，回到离开时的位置：先读那附近的几页、一步定位，不先读顶部再跳；详情栏里那张图和
// 键盘光标也恢复。页面是假的（tests/web.cjs），图片是合成的。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
before(web.ready);

const tick = () => new Promise(resolve => setImmediate(resolve));
let shell, grid, page, state, requests;
const TOTAL = {library: 5000, unmatched: 300};
async function wired() {
  if (shell) return;
  page = web.fakePage();
  globalThis.window = {addEventListener() {}};
  globalThis.history = {replaceState() {}};
  globalThis.requestAnimationFrame = () => 0;
  globalThis.matchMedia = () => ({matches: true});
  globalThis.IntersectionObserver = class { observe() {} unobserve() {} disconnect() {} };   // 资料库的海报懒加载
  const store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)};
  ({state} = web.core());
  shell = await web.load('ui/shell.js'); grid = await web.load('ui/grid.js');
  globalThis.fetch = path => {
    const q = new URLSearchParams(path.split('?')[1] || '');
    let body = [];
    if (path.startsWith('/api/assets?')) {
      requests.push(path);
      const pool = q.get('pool') || 'library', total = TOTAL[pool], offset = Number(q.get('offset')), limit = Number(q.get('limit'));
      body = {total: q.get('count') === '0' ? null : total,
        items: Array.from({length: Math.max(0, Math.min(limit, total - offset))}, (_, j) => ({uuid: `${pool}-${offset + j}`, memberships: [], width: 3, height: 4}))};
    } else if (path.startsWith('/api/detail')) body = {uuid: q.get('id'), memberships: [], alternatives: [], user_tags: []};
    else if (path.startsWith('/api/assets/count')) body = {total: 0};
    return Promise.resolve({status: 200, ok: true, json: async () => body});
  };
}
async function visit(hash) {
  requests = [];
  location.hash = hash;
  await shell.applyRoute(); await tick();
}
const offsetsOf = paths => paths.map(p => Number(new URLSearchParams(p.split('?')[1]).get('offset')));
const firstShown = () => grid.rowStart(grid.rowAt(page.el('viewport').scrollTop));

test('回到图库时先读离开时那一屏的几页，并定位到同一张', async () => {
  await wired();
  globalThis.location = {hash: ''};
  Object.assign(state, {route: {page: 'days'}, fit: false, size: 185, nsfw: '', guide: undefined});
  await visit('#/library');
  assert.deepEqual(offsetsOf(requests), [0]);
  // 滚到大约第 2,400 张，停在一行的中间
  const v = page.el('viewport'), row = grid.rowOf(2400);
  v.scrollTop = grid.rowTop(row) + grid.rowHeight(row) / 2;
  const anchor = firstShown();
  // 去未识别页：从顶部开始
  await visit('#/unmatched');
  assert.deepEqual(offsetsOf(requests), [0]);
  assert.equal(v.scrollTop, 0);
  // 回到图库：第一个请求就是那附近的一页（带计数），没有先读第 0 页
  await visit('#/library');
  const offsets = offsetsOf(requests);
  assert.equal(offsets[0], Math.floor(anchor / 100) * 100);
  assert.ok(!offsets.includes(0), `不先读顶部：${offsets}`);
  assert.doesNotMatch(requests[0], /count=0/);
  assert.equal(firstShown(), anchor);
  assert.ok(Math.abs(v.scrollTop - (grid.rowTop(grid.rowOf(anchor)) + grid.rowHeight(grid.rowOf(anchor)) / 2)) < 1, `停在同一行的中间：${v.scrollTop}`);
  assert.ok(state.items[anchor], '那一张已经读到');
  assert.equal(state.total, TOTAL.library);
});

test('详情栏里的那张图和键盘光标一起恢复；另一个地址各记各的', async () => {
  await wired();
  await visit('#/library');
  const v = page.el('viewport');
  v.scrollTop = grid.rowTop(grid.rowOf(1000));
  const anchor = firstShown();
  state.cursor = anchor + 1;
  const detail = await web.load('ui/detail.js');
  await detail.inspect(`library-${anchor + 1}`);
  // 进角色页（图库加 char 参数，算换页：详情栏关掉），再回来
  await visit('#/library?char=c1');
  assert.equal(state.focused, null);
  assert.equal(page.el('inspector').hidden, true);
  assert.equal(v.scrollTop, 0);
  await visit('#/library');
  assert.equal(firstShown(), anchor);
  assert.equal(state.focused, `library-${anchor + 1}`);
  assert.equal(page.el('inspector').hidden, false);
  assert.equal(state.cursor, anchor + 1);
  // 未识别页上次停在顶部，回去还是顶部
  await visit('#/unmatched');
  assert.equal(v.scrollTop, 0);
  assert.equal(state.focused, null);
});

test('列表变短了：定位到最后一张，不越界', async () => {
  await wired();
  await visit('#/unmatched');
  const v = page.el('viewport');
  v.scrollTop = grid.rowTop(grid.rowOf(250));
  await visit('#/library');
  TOTAL.unmatched = 120;
  try {
    await visit('#/unmatched');
    assert.equal(state.total, 120);
    assert.ok(firstShown() <= 119);
    assert.ok(v.scrollTop <= grid.rowTop(grid.rowCount()));
  } finally { TOTAL.unmatched = 300; }
});

test('详情栏里那张已经滚出读到的几页，回来照样打开它', async () => {
  await wired();
  await visit('#/library');
  const v = page.el('viewport'), detail = await web.load('ui/detail.js');
  await detail.inspect('library-10');
  v.scrollTop = grid.rowTop(grid.rowOf(3000));
  const anchor = firstShown();
  await visit('#/unmatched');
  await visit('#/library');
  assert.equal(firstShown(), anchor);
  assert.ok(!state.items[10], '第 10 张不在这次读的页里');
  assert.equal(state.focused, 'library-10');
  assert.equal(page.el('inspector').hidden, false);
});

test('换到自己渲染的页面（资料库、设置）从顶部开始，不沿用图库的滚动位置', async () => {
  await wired();
  const v = page.el('viewport');
  for (const hash of ['#/characters', '#/settings']) {
    await visit('#/library');
    v.scrollTop = grid.rowTop(grid.rowOf(2000));
    assert.ok(v.scrollTop > 0);
    await visit(hash);
    assert.equal(v.scrollTop, 0, hash);
  }
});

test('大图开着时换页再回来：详情栏和光标都是大图里最后看的那张', async () => {
  await wired();
  await visit('#/library');
  const detail = await web.load('ui/detail.js');
  globalThis.Image = class {};
  // 开详情栏看第 6 张，Space 进大图，往后翻到第 9 张
  await detail.inspect('library-5');
  await detail.showViewer(8);
  assert.equal(state.viewer, 8);
  await visit('#/unmatched');
  assert.equal(state.viewer, -1);
  await visit('#/library');
  assert.equal(state.focused, 'library-8');
  assert.equal(state.cursor, 8);
  assert.equal(page.el('inspector').hidden, false);
  const v = page.el('viewport'), top = grid.rowTop(grid.rowOf(8));
  assert.ok(top >= v.scrollTop && top < v.scrollTop + v.clientHeight, '光标那张在屏幕上');
});
