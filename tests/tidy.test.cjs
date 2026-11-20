// 整理页（#713）：core/tidy.js 的纯函数和写入内容；按组整理（ui/groups.js）的组按 cid 复用、答完收起、晚到的回复不覆盖；
// 相似的图（ui/similar.js）的看过的组（#427）、轮询与分页（#601）、相似组 N 页头的「看过了」。假 DOM 是 tests/dom.cjs，
// 组和图都是合成的。
const {test, before, after} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

const tick = () => new Promise(resolve => setImmediate(resolve));
const settle = async () => { for (let i = 0; i < 4; i++) await tick(); };
let T, S, pages, state, box, fetchImpl = null, posts = [], requests = [];
before(async () => {
  await web.ready();
  const document = dom.install();
  const store = new Map();
  Object.assign(globalThis, {
    location: {hash: ''}, history: {replaceState() {}}, window: {addEventListener() {}}, matchMedia: () => ({matches: true, addEventListener() {}}),
    requestAnimationFrame: cb => setTimeout(() => cb(0), 0), cancelAnimationFrame: clearTimeout, IntersectionObserver: class { observe() {} unobserve() {} disconnect() {} },
    localStorage: {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)},
  });
  globalThis.fetch = (path, init) => {
    if (init?.method === 'POST') { posts.push([path, JSON.parse(init.body)]); return Promise.resolve({status: 200, ok: true, json: async () => ({undo: {id: posts.length, label: '已标为看过', ts: ''}})}); }
    requests.push(path);
    return fetchImpl(path);
  };
  ({state} = web.core());
  T = await web.load('core/tidy.js'); S = await web.load('ui/scope.js'); pages = await web.load('core/pages.js');
  await web.load('ui/groups.js'); await web.load('ui/similar.js');
  // 提示条、错误条、结果数：写入以后的 toast 和身份整理的「N 对待判断」写在这里。
  for (const id of ['toast', 'toastText', 'toastUndo', 'toastAct', 'errorBanner', 'resultCount']) {
    const el = document.createElement('div'); el.setAttribute('id', id); document.body.appendChild(el);
  }
  box = document.createElement('div'); box.setAttribute('id', 'groupsPage'); document.body.appendChild(box);
});
// 释放最后一页的作用域（轮询的计时器在它上面）。假 DOM 不撤：写入后的提示条几秒后还要藏起来（每个测试文件是单独的进程）。
after(() => S.page('done').dispose());
const ok = body => Promise.resolve({status: 200, ok: true, json: async () => body});
const open = async (route, page = route.page) => {
  state.route = route; requests = []; posts = [];
  const scope = S.page(page);
  await pages.PAGES[page].render({samePage: false, scope});
  await settle();
  return scope;
};
const click = (root, target) => root.dispatchEvent(dom.event('click', {target}));

// ---------- core/tidy.js ----------
test('组的先后：能直接确认的在前，大的在前；「下一组」取原来排在后面、还在的第一组', () => {
  const c = (cid, size, lead) => ({cid, size, lead: lead ? {id: 'x'} : null});
  assert.deepEqual(T.groupOrder([c(1, 3), c(2, 9), c(3, 2, true), c(4, 5, true)]).map(g => g.cid), [4, 3, 2, 1]);
  assert.equal(T.nextGroupId(['4', '3', '2', '1'], '3', [c(4, 5, true), c(1, 3)]), '1');
  assert.equal(T.nextGroupId(['4', '3', '2', '1'], '1', [c(4, 5, true), c(1, 3)]), '4');
  assert.equal(T.nextGroupId(['3'], '3', [c(3, 2)]), null);
});

test('身份整理：建议的做法；记为同一角色时小的一边挂到大的下面', async () => {
  const x = {a: {id: 'A', tag: 'a_tag', name: '合成甲', count: 9}, b: {id: 'B', tag: 'b_tag', name: '合成乙', count: 2}, hint: {kind: 'form_of', subject: 'b_tag', object: 'a_tag'}, distance: 0.005};
  assert.equal(T.identitySuggestion(x), 'b_of_a');
  assert.equal(T.identitySuggestion({...x, hint: {kind: 'duplicate_of'}}), 'same');
  assert.match(T.identityEvidence(x), /0\.005（几乎分不开）/);
  posts = [];
  const {message} = await T.decideIdentity(x, 'same');
  assert.deepEqual(posts[0], ['/api/identity/decision', {status: 'accepted', reason: '身份整理', a: 'B', b: 'A', kind: 'duplicate_of'}]);
  assert.match(message, /合成乙.*合成甲/);
  await T.decideIdentity(x, 'skip');
  assert.deepEqual(posts[1], ['/api/identity/skip', {a: 'A', b: 'B'}]);
});

test('画师抽查的统计：准确率、按方法分开，像素哈希优先', () => {
  const items = [{artists: [{methods: ['ocr', 'pixel_hash'], confirmed: true}, {methods: ['ocr'], rejected: true}]}, {artists: [{methods: ['gallery_match']}]}];
  const s = T.auditTally(items);
  assert.deepEqual(s.all, {yes: 1, no: 1, done: 2, all: 3});
  assert.equal(s.accuracy, 0.5);
  assert.deepEqual(s.methods.map(([m, x]) => [m, x.done]), [['pixel_hash', 1], ['gallery_match', 0], ['ocr', 1]]);
});

// ---------- 按组整理 ----------
const cluster = cid => ({cid, size: 2, members: ['u', 'v'].map(x => x + cid), traits: []});
const sections = () => box.querySelectorAll('.tidy-group');
test('按组整理：重读时组按 cid 复用；排除一张只改那一格；组不在了就收起，其余的组留着', async () => {
  let clusters = [1, 2, 3].map(cluster);
  fetchImpl = path => ok(path.startsWith('/api/clusters') ? {clusters, learner: {}} : {});
  Object.assign(state, {nsfw: '', pools: {unmatched: 100}, viewer: -1});
  await open({page: 'groups'});
  const [a, b, c] = sections();
  assert.equal(sections().length, 3);
  assert.match(a.textContent, /第 1 组/);
  clusters = [1, 2, 3, 4].map(cluster);
  await pages.PAGES.groups.refresh('undo'); await settle();
  assert.deepEqual(sections().slice(0, 3), [a, b, c]);
  // 点第 2 组的第一张：它标为不算，节点还是原来的。
  const list = box.querySelector('.tidy-list'), thumb = b.querySelector('[data-gimg]');
  click(list, thumb);
  assert.equal(sections()[1], b); assert.ok(thumb.classList.contains('is-off'));
  // 第 1 组答完了（写入走 ui/grid.js 的 mutate，这里只看重读）：它收起、摘掉，其余的组还是原来的节点。
  clusters = [2, 3, 4].map(cluster);
  await pages.PAGES.groups.refresh('learned'); await settle();
  assert.equal(sections()[0], b); assert.equal(sections()[1], c);
  assert.equal(sections().length, 3);
  assert.ok(!a.parentNode, '答完的组摘掉了');
});

test('按组整理：先发的读取晚到，不覆盖换了 NSFW 以后的新回复', async () => {
  const pending = [];
  fetchImpl = path => new Promise(resolve => pending.push({path, resolve: body => resolve({status: 200, ok: true, json: async () => body})}));
  Object.assign(state, {nsfw: '', pools: {unmatched: 0}});
  state.route = {page: 'groups'};
  const scope = S.page('groups');
  const old = pages.PAGES.groups.render({samePage: false, scope}); await tick();
  state.nsfw = 'hide'; const fresh = pages.PAGES.groups.refresh('nsfw'); await tick();
  pending[1].resolve({clusters: [cluster(2)], learner: {}}); await fresh;
  pending[0].resolve({clusters: [cluster(1)], learner: {}}); await old; await settle();
  assert.deepEqual(sections().map(s => s.dataset.cid), ['2']);
  state.nsfw = '';
});

// ---------- 相似的图 ----------
let groups = [], scan = {status: 'done'};
const sgroup = (gid, seen, size = 2) => ({gid, size, covers: ['a', 'b', 'c'].slice(0, size), kinds: {duplicate: 1}, seen});
const similarFetch = path => {
  if (path.startsWith('/api/similar?')) return ok({groups, state: scan});
  return ok({});
};
const cards = () => box.querySelectorAll('.tidy-card');
const hrefs = () => cards().map(c => c.getAttribute('href'));
test('相似的图：看过的组不在列表里，「看过的 N 组」只列它们；都看过了有空状态', async () => {
  fetchImpl = similarFetch; groups = [sgroup(3, false), sgroup(7, true)];
  await open({page: 'similar'});
  assert.ok(requests.some(p => p.startsWith('/api/similar?') && p.includes('seen=1')));
  assert.deepEqual(hrefs(), ['#/s/3']);
  const other = box.querySelectorAll('a').find(a => /seen=|看过/.test(a.textContent + a.getAttribute('href')) && a.classList.contains('btn'));
  assert.equal(other.getAttribute('href'), '#/similar?seen=1'); assert.match(other.textContent, /看过的 1 组 →/);
  await open({page: 'similar', seen: '1'});
  assert.deepEqual(hrefs(), ['#/s/7']);
  assert.match(box.querySelector('.tidy-status').textContent, /看过的 1 组 · 共 2 张/);
  groups = [sgroup(7, true)];
  await open({page: 'similar'});
  assert.deepEqual(hrefs(), []);
  assert.match(box.querySelector('.empty-state').textContent, /相似的组都看过了。/);
});

test('相似的图：轮询和撤销按组号比对，没变的卡片留着；先显示 40 组，「再显示」接着往下', async () => {
  fetchImpl = similarFetch; scan = {status: 'comparing', done: 1, total: 4, found: 2};
  groups = [sgroup(3, false), sgroup(4, false), sgroup(5, false)];
  await open({page: 'similar'});
  const [three, four, five] = cards();
  assert.equal(box.querySelector('.progress').hidden, false);
  const bar = box.querySelector('.progress').firstElementChild;
  assert.equal(bar.style.getPropertyValue?.('width') ?? bar.style.width, '25%');
  groups = [sgroup(5, false), sgroup(3, false), sgroup(4, false, 3), sgroup(6, false)];
  scan = {status: 'done'};
  await pages.PAGES.similar.refresh('undo'); await settle();
  assert.equal(cards()[0], five); assert.equal(cards()[1], three); assert.equal(cards()[2], four);
  assert.match(cards()[2].textContent, /3 张/);
  assert.equal(box.querySelector('.progress').hidden, true);
  groups = Array.from({length: 45}, (_, i) => sgroup(i + 1, false));
  await open({page: 'similar'});
  assert.equal(cards().length, 40);
  const more = box.querySelector('.tidy-more').firstElementChild;
  assert.match(more.textContent, /再显示 5 组/);
  more.dispatchEvent(dom.event('click'));
  assert.equal(cards().length, 45);
  groups = groups.slice(1);
  await pages.PAGES.similar.refresh('undo'); await settle();
  assert.equal(cards().length, 44, '撤销以后留在已显示的组数');
});

test('相似组 N：页头的「看过了」标记这组、回到列表；看过的组按钮是「标为没看过」', async () => {
  fetchImpl = similarFetch; groups = [sgroup(3, false), sgroup(7, true)];
  Object.assign(state, {total: 20, nsfw: ''});
  const route = {page: 'sgroup', id: '3'}; state.route = route;
  await pages.PAGES.sgroup.prepare(route);
  let head = pages.PAGES.sgroup.head(route, {n: '20'});
  assert.equal(head.title, '相似组 3');
  const seen = head.actions.find(b => b.id === 'sgroupSeen');
  assert.equal(seen.textContent, '看过了');
  posts = [];
  await seen.onclick(); await settle();
  assert.deepEqual(posts[0], ['/api/similar/seen', {gid: 3, size: 2, nsfw: '', seen: true}]);
  const r7 = {page: 'sgroup', id: '7'}; state.route = r7;
  await pages.PAGES.sgroup.prepare(r7);
  head = pages.PAGES.sgroup.head(r7, {n: '2'});
  assert.equal(head.actions.find(b => b.id === 'sgroupSeen').textContent, '标为没看过');
});
