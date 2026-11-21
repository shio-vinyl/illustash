// 整理 › 可能认错（#787）：core/doubts.js 的切卡和请求体；ui/doubts.js 的卡（共用的判断卡 ui/judge.js）按键复用、各种回答、
// 写入时锁卡、被拒时留着、晚到的读取不覆盖；以及按组整理（ui/groups.js）新加的「点掉的是…」。假 DOM 是 tests/dom.cjs，
// 接口（#789）的回复用合成的固定数据，形状见 core/doubts.js 开头。
const {test, before, after} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

const tick = () => new Promise(resolve => setImmediate(resolve));
const settle = async () => { for (let i = 0; i < 6; i++) await tick(); };
let D, S, pages, state, box, view, groups, fetchImpl = null, answer = null, posts = [];
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
    if (init?.method === 'POST') {
      const body = JSON.parse(init.body); posts.push([path, body]);
      return answer ? answer(path, body) : Promise.resolve({status: 200, ok: true, json: async () => ({undo: {id: posts.length, label: '回答', ts: ''}})});
    }
    return fetchImpl(path);
  };
  ({state} = web.core());
  D = await web.load('core/doubts.js'); S = await web.load('ui/scope.js'); pages = await web.load('core/pages.js');
  view = await web.load('ui/doubts.js'); groups = await web.load('ui/groups.js');
  for (const id of ['toast', 'toastText', 'toastUndo', 'toastAct', 'errorBanner', 'resultCount', 'hint', 'railEntries', 'picker']) {
    const el = document.createElement('div'); el.setAttribute('id', id); document.body.appendChild(el);
  }
  box = document.createElement('div'); box.setAttribute('id', 'groupsPage'); document.body.appendChild(box);
});
after(() => S.page('done').dispose());
const ok = body => Promise.resolve({status: 200, ok: true, json: async () => body});
const A = {id: 'ca', name: '合成甲'}, B = {id: 'cb', name: '合成乙'}, C = {id: 'cc', name: '合成丙'};
const pics = (prefix, n) => Array.from({length: n}, (_, i) => prefix + i);
let data = [];
const serve = path => ok(path.startsWith('/api/ambiguous') ? {groups: data} : path.startsWith('/api/bootstrap') ? {token: '', total: 0, undo: null} : []);
const open = async () => {
  state.route = {page: 'doubts'}; posts = [];
  const scope = S.page('doubts');
  await pages.PAGES.doubts.render({samePage: false, scope});
  await settle();
  return scope;
};
const cards = () => box.querySelectorAll('section').filter(x => x.getAttribute('data-key') && !x.inert);
const btn = (card, key) => card.querySelectorAll('[data-ref]').find(b => b.getAttribute('data-ref') === key);
const press = (card, uuid) => { const t = card.querySelectorAll('[data-peek]').find(x => x.dataset.peek === uuid); t.parentNode.dispatchEvent(dom.event('click', {target: t})); };

// ---------- core/doubts.js ----------
test('切卡：每对最多 16 张一张卡，同一对排在一起；卡键是这一对加卡上第一张图', () => {
  const list = D.doubtCards([{a: A, b: B, count: 20, items: pics('x', 20)}, {a: A, b: C, count: 3, items: pics('y', 3)}]);
  assert.deepEqual(list.map(c => [c.key, c.items.length, c.parts, c.total]), [['ca|cb|x0', 16, 2, 20], ['ca|cb|x16', 4, 2, 20], ['ca|cc|y0', 3, 1, 3]]);
  // 答掉同一对的一张卡：其余那张的「这一对共 N 张」跟着变
  assert.deepEqual(D.tally(list.filter(c => c.key !== 'ca|cb|x0')).map(c => [c.key, c.parts, c.total]), [['ca|cb|x16', 1, 4], ['ca|cc|y0', 1, 3]]);
});

test('回答的请求体：是 A（点掉的不是 A）、是 B、都不是（also_no）、点掉的是…', () => {
  const card = D.doubtCards([{a: A, b: B, count: 3, items: ['u', 'v', 'w']}])[0], off = new Set(['v']);
  assert.deepEqual(D.doubtAnswer(card, 'a', off), {entity: 'ca', kind: 'ambiguous', yes: ['u', 'w'], no: ['v']});
  assert.deepEqual(D.doubtAnswer(card, 'b', off), {entity: 'ca', kind: 'ambiguous', yes: [], no: ['u', 'v', 'w'], switch: 'cb'});
  assert.deepEqual(D.doubtAnswer(card, 'none', off), {entity: 'ca', kind: 'ambiguous', yes: [], no: ['u', 'v', 'w'], also_no: 'cb'});
  assert.deepEqual(D.doubtAnswer(card, 'other', off, 'cc'), {entity: 'ca', kind: 'ambiguous', yes: [], no: ['v'], switch: 'cc'});
});

test('断连后核对「都不是」：对 B 的否定也要在', async () => {
  const {askLanded} = await web.load('core/ask.js');
  const detail = statuses => ok({assertions: statuses});
  fetchImpl = () => detail([{id: 'ca', status: 'rejected'}]);
  const body = {entity: 'ca', kind: 'ambiguous', yes: [], no: ['u'], also_no: 'cb'};
  assert.equal(await askLanded(body), false);
  fetchImpl = () => detail([{id: 'ca', status: 'rejected'}, {id: 'cb', status: 'rejected'}]);
  assert.equal(await askLanded(body), true);
});

// ---------- 页面 ----------
test('卡头写「是 A 还是 B？」和张数；点掉一张，主按钮变「其余 N 张是 A」，有了「点掉的是…」', async () => {
  data = [{a: A, b: B, count: 3, items: ['u', 'v', 'w']}];
  fetchImpl = serve; Object.assign(state, {nsfw: '', pools: {ambiguous: 3}, viewer: -1});
  await open();
  const [c] = cards();
  assert.match(c.textContent, /是 合成甲 还是 合成乙？/); assert.match(c.textContent, /3 张/);
  assert.equal(btn(c, 'a').textContent, '是 合成甲'); assert.equal(btn(c, 'other').hidden, true);
  press(c, 'v');
  assert.equal(btn(c, 'a').textContent, '其余 2 张是 合成甲'); assert.equal(btn(c, 'other').hidden, false);
  btn(c, 'a').dispatchEvent(dom.event('click')); await settle();
  assert.deepEqual(posts[0], ['/api/ask/answer', {entity: 'ca', kind: 'ambiguous', yes: ['u', 'w'], no: ['v']}]);
  assert.equal(cards().length, 0, '答完的卡收起');
  assert.match(box.textContent, /没有可能认错的图/);
});

test('「是 B」「都不是」各答整张卡；「以后再说」之后这张卡不在了', async () => {
  data = [{a: A, b: B, count: 2, items: ['u', 'v']}, {a: A, b: C, count: 1, items: ['w']}, {a: B, b: C, count: 1, items: ['z']}];
  fetchImpl = serve; state.pools = {ambiguous: 4};
  await open();
  const [first, second, third] = cards();
  btn(first, 'b').dispatchEvent(dom.event('click')); await settle();
  btn(second, 'none').dispatchEvent(dom.event('click')); await settle();
  btn(third, 'later').dispatchEvent(dom.event('click')); await settle();
  assert.deepEqual(posts.map(p => p[1]), [{entity: 'ca', kind: 'ambiguous', yes: [], no: ['u', 'v'], switch: 'cb'},
    {entity: 'ca', kind: 'ambiguous', yes: [], no: ['w'], also_no: 'cc'}, {entity: 'cb', kind: 'ambiguous', later: ['z']}]);
  assert.equal(cards().length, 0);
});

test('「点掉的是…」：点掉的几张记成选中的角色，卡上留下其余的；选的时候卡已经不在了就不写', async () => {
  data = [{a: A, b: B, count: 3, items: ['u', 'v', 'w']}];
  fetchImpl = serve; state.pools = {ambiguous: 3};
  await open();
  let opened = null;
  view.current().page.pick = o => { opened = o; };
  const [c] = cards();
  press(c, 'w');
  btn(c, 'other').dispatchEvent(dom.event('click'));
  assert.deepEqual(opened.subject, ['w']); assert.deepEqual(opened.exclude, ['ca']);
  await opened.pick(C); await settle();
  assert.deepEqual(posts[0][1], {entity: 'ca', kind: 'ambiguous', yes: [], no: ['w'], switch: 'cc'});
  assert.equal(cards()[0], c, '同一张卡');
  assert.deepEqual(c.querySelectorAll('[data-peek]').filter(x => !x.inert).map(x => x.dataset.peek), ['u', 'v']);
  assert.equal(btn(c, 'a').textContent, '是 合成甲', '点掉的清空了');
  // 卡答完以后才在旧的选择器里选：不写
  btn(c, 'later').dispatchEvent(dom.event('click')); await settle();
  posts = [];
  await opened.pick(C); await settle();
  assert.deepEqual(posts, []);
});

test('写入期间这张卡锁着，不会提交两次；后端拒绝时卡和点掉的图都留着', async () => {
  data = [{a: A, b: B, count: 2, items: ['u', 'v']}];
  fetchImpl = serve; state.pools = {ambiguous: 2};
  await open();
  let release;
  answer = () => new Promise(r => { release = () => r({status: 423, ok: false, json: async () => ({error: '图库正在写入'})}); });
  const [c] = cards();
  press(c, 'v');
  btn(c, 'a').dispatchEvent(dom.event('click')); await tick();
  assert.equal(btn(c, 'a').disabled, true, '锁着');
  btn(c, 'a').dispatchEvent(dom.event('click')); await tick();
  assert.equal(posts.length, 1, '没有第二次');
  release(); await settle();
  assert.equal(cards()[0], c, '卡还在');
  assert.equal(btn(c, 'a').disabled, false, '放开了');
  assert.equal(btn(c, 'a').textContent, '其余 1 张是 合成甲', '点掉的图还在');
  answer = null;
});

test('卡片过期（后端回 400）：重读队列，卡按新的队列画', async () => {
  data = [{a: A, b: B, count: 2, items: ['u', 'v']}, {a: A, b: C, count: 1, items: ['w']}];
  let reads = 0;
  fetchImpl = path => { if (path.startsWith('/api/ambiguous')) reads++; return serve(path); };
  state.pools = {ambiguous: 3};
  await open();
  answer = () => Promise.resolve({status: 400, ok: false, json: async () => ({error: '图片的备选归属已变更，请刷新'})});
  data = [{a: A, b: C, count: 1, items: ['w']}];
  const before = reads;
  btn(cards()[0], 'none').dispatchEvent(dom.event('click')); await settle();
  assert.equal(reads, before + 1, '重读了一次');
  const said = globalThis.document.getElementById('toastText').textContent;
  assert.match(said, /已重新读取/); assert.doesNotMatch(said, /请刷新/);
  assert.deepEqual(cards().map(c => c.getAttribute('data-key')), ['ca|cc|w']);
  answer = null;
});

test('重读：卡按键复用、点掉的图不复位；先发的读取晚到不覆盖', async () => {
  data = [{a: A, b: B, count: 2, items: ['u', 'v']}, {a: A, b: C, count: 1, items: ['w']}];
  fetchImpl = serve; state.pools = {ambiguous: 3};
  await open();
  const [c] = cards();
  press(c, 'v');
  const pending = [];
  fetchImpl = path => path.startsWith('/api/ambiguous') ? new Promise(r => pending.push(body => r({status: 200, ok: true, json: async () => body}))) : serve(path);
  const first = pages.PAGES.doubts.refresh('undo'); await tick();
  const second = pages.PAGES.doubts.refresh('learned'); await tick();
  pending[1]({groups: [data[0]]}); await second; await settle();
  pending[0]({groups: data}); await first; await settle();
  assert.equal(cards().length, 1, '只认最后发出的那次');
  assert.equal(cards()[0], c, '同一个节点');
  assert.equal(btn(c, 'a').textContent, '其余 1 张是 合成甲');
});

test('全部点掉：主按钮写回「是 A」并灰掉，「点掉的是…」还能点', async () => {
  data = [{a: A, b: B, count: 2, items: ['u', 'v']}];
  fetchImpl = serve; state.pools = {ambiguous: 2};
  await open();
  const [c] = cards();
  press(c, 'u'); press(c, 'v');
  assert.equal(btn(c, 'a').textContent, '是 合成甲'); assert.equal(btn(c, 'a').disabled, true);
  assert.equal(btn(c, 'other').disabled, false);
  assert.match(c.querySelector('[data-peek]').getAttribute('title'), /改回「是 合成甲」/);
});

test('第一张卡还在写时点第二张卡：不发，提示一句', async () => {
  data = [{a: A, b: B, count: 1, items: ['u']}, {a: A, b: C, count: 1, items: ['w']}];
  fetchImpl = serve; state.pools = {ambiguous: 2};
  await open();
  let release;
  answer = () => new Promise(r => { release = () => r({status: 200, ok: true, json: async () => ({undo: {id: 1, label: '回答', ts: ''}})}); });
  const [one, two] = cards();
  btn(one, 'a').dispatchEvent(dom.event('click')); await tick();
  btn(two, 'b').dispatchEvent(dom.event('click')); await tick();
  assert.equal(posts.length, 1, '只发了第一张');
  assert.match(globalThis.document.getElementById('toastText').textContent, /还没写完/);
  release(); await settle();
  answer = null;
});

test('写入途中重读过：写完从重读回来的卡里减，不用写入前的旧卡盖掉', async () => {
  data = [{a: A, b: B, count: 20, items: pics('x', 20)}];
  fetchImpl = serve; state.pools = {ambiguous: 20};
  await open();
  view.current().page.pick = o => { opened = o; };
  let opened = null, release;
  answer = () => new Promise(r => { release = () => r({status: 200, ok: true, json: async () => ({undo: {id: 1, label: '回答', ts: ''}})}); });
  const [c] = cards();
  press(c, 'x5');
  btn(c, 'other').dispatchEvent(dom.event('click'));
  const writing = opened.pick(C); await tick();
  // 写入途中重读：别处少了 x3，x16 切进了第一张卡
  data = [{a: A, b: B, count: 19, items: pics('x', 20).filter(u => u !== 'x3')}];
  await pages.PAGES.doubts.refresh('learned'); await settle();
  release(); await writing; await settle();
  const shown = cards()[0].querySelectorAll('[data-peek]').filter(x => !x.inert).map(x => x.dataset.peek);
  assert.ok(shown.includes('x16'), '重读切进来的 x16 还在');
  assert.ok(!shown.includes('x5') && !shown.includes('x3'));
  answer = null;
});

test('换了 NSFW：旧卡先撤下，重读回来再画', async () => {
  data = [{a: A, b: B, count: 1, items: ['u']}];
  fetchImpl = serve; state.pools = {ambiguous: 1};
  await open();
  let resolve;
  fetchImpl = path => path.startsWith('/api/ambiguous') ? new Promise(r => { resolve = () => r({status: 200, ok: true, json: async () => ({groups: []})}); }) : serve(path);
  state.nsfw = 'hide';
  const read = pages.PAGES.doubts.refresh('nsfw'); await tick();
  assert.equal(cards().length, 0, '没等重读就撤下了');
  assert.doesNotMatch(box.textContent, /没有可能认错的图/, '重读回来之前不写空状态');
  resolve(); await read; await settle();
  assert.match(box.textContent, /没有可能认错的图/);
  state.nsfw = '';
});

// ---------- 按组整理的「点掉的是…」 ----------
test('按组整理：点掉一张，「点掉的是…」把它设为选中的角色，这组留着、少了那张', async () => {
  // 写入走 ui/grid.js 的 mutate（假 DOM 里没有网格，写完的收尾跑不完）：这里看发出的请求，再直接调写成以后的 shed。
  const cluster = {cid: 7, size: 3, members: ['g1', 'g2', 'g3'], traits: [], lead: {id: 'ca', name: '合成甲', count: 3}};
  fetchImpl = path => ok(path.startsWith('/api/clusters') ? {clusters: [cluster], learner: {}} : path.startsWith('/api/bootstrap') ? {token: '', total: 0, undo: null} : []);
  Object.assign(state, {nsfw: '', pools: {unmatched: 10}, viewer: -1, route: {page: 'groups'}});
  const scope = S.page('groups');
  await pages.PAGES.groups.render({samePage: false, scope}); await settle();
  let opened = null;
  groups.current().page.pick = o => { opened = o; };
  const sec = box.querySelectorAll('section').find(x => x.getAttribute('data-cid') === '7');
  assert.equal(btn(sec, 'other').hidden, true);
  press(sec, 'g2');
  assert.equal(btn(sec, 'other').hidden, false);
  btn(sec, 'other').dispatchEvent(dom.event('click'));
  assert.deepEqual(opened.subject, ['g2']); assert.deepEqual(opened.exclude, ['ca']);
  posts = [];
  await opened.pick(C); await settle();
  assert.deepEqual(posts[0], ['/api/change', {ids: ['g2'], action: 'confirmed', entity: 'cc'}]);
  groups.current().shed(7, ['g2']);
  assert.deepEqual(sec.querySelectorAll('[data-peek]').filter(x => !x.inert).map(x => x.dataset.peek), ['g1', 'g3']);
  assert.match(btn(sec, 'yes').textContent, /都是/);
  assert.equal(groups.current().page.list[0].members.length, 2);
});
