const test = require('node:test');
const web = require('./web.cjs');   // core/ 下的模块（#199）
const dom = require('./dom.cjs');
test.before(web.ready);
const assert = require('node:assert/strict');
const tick = () => new Promise(resolve => setImmediate(resolve));
// 元数据读取在 core/meta.js；api 回答页面的请求。
function metaHarness(api) {
  const {session, state, refreshMeta} = web.core();
  Object.assign(state, {nsfw: '', chars: [], dupInfo: {}, exclInfo: {}, route: {page: 'library'}, char: null, readEpoch: 0, countDirty: undefined});
  globalThis.fetch = path => Promise.resolve(api(path)).then(body => ({status: 200, ok: true, json: async () => body}));
  return {state, session, refreshMeta};
}
function value(path, name = 'current') {
  if (path.startsWith('/api/bootstrap')) return {token: name, undo: null, total: 1};
  if (path.startsWith('/api/identity/count')) return {total: 0};
  if (path.startsWith('/api/characters')) return [{id: name}];
  return [];
}
test('same-turn metadata requests coalesce, initial bootstrap is reused', async () => {
  const calls = [];
  const c = metaHarness(async path => {calls.push(path); return value(path);});
  const first = c.refreshMeta({boot: {token: 'seed', total: 1}});
  assert.equal(first, c.refreshMeta()); assert.equal(first, c.refreshMeta());
  await first;
  assert.equal(calls.length, 4);
  assert.ok(calls.every(p => !p.startsWith('/api/bootstrap')));
  assert.equal(c.session.token, 'seed');
});
test('in-flight changes trigger one trailing refresh and stale responses never apply', async () => {
  const pending = [];
  const c = metaHarness(path => new Promise(resolve => pending.push({path, resolve})));
  const first = c.refreshMeta(); await tick();
  c.state.nsfw = 'hide';
  assert.equal(c.refreshMeta(), first); assert.equal(c.refreshMeta(), first);
  for (const p of pending.splice(0)) p.resolve(value(p.path, 'old'));
  await tick();
  assert.equal(c.session.token, '');
  assert.equal(pending.length, 5);
  assert.ok(pending.find(p => p.path.startsWith('/api/identity/count')).path.includes('nsfw=hide'));
  for (const p of pending.splice(0)) p.resolve(value(p.path, 'fresh'));
  await first;
  assert.equal(c.session.token, 'fresh');
  assert.equal(c.state.chars[0].id, 'fresh');
});
test('tag-only refresh avoids characters, pools and counts', async () => {
  const calls = [];
  const c = metaHarness(async path => {calls.push(path); return ['tag'];});
  await c.refreshMeta({tagsOnly: true});
  assert.deepEqual(calls, ['/api/user-tags']);
  assert.equal(c.state.userTags[0], 'tag');
});
test('failed metadata refresh releases the in-flight request for retry', async () => {
  let fail = true;
  const c = metaHarness(async path => {if (fail) throw Error('offline'); return value(path);});
  await assert.rejects(c.refreshMeta(), /offline/);
  fail = false; await c.refreshMeta(); assert.equal(c.session.token, 'current');
});

class Element {
  constructor() {this.children = []; this.dataset = {}; this.style = {}; this.classList = {toggle() {}}; this.moves = 0;}
  get firstElementChild() {return this.children[0] || null;}
  get nextElementSibling() {return this.parent?.children[this.parent.children.indexOf(this) + 1] || null;}
  remove() {if (this.parent) {const a = this.parent.children; a.splice(a.indexOf(this), 1); this.parent = null;}}
  insertBefore(el, before) {if (el === before) return; el.remove(); const i = before ? this.children.indexOf(before) : this.children.length; assert.ok(i >= 0); this.children.splice(i, 0, el); el.parent = this; this.moves++;}
  replaceWith(el) {const p = this.parent; p.insertBefore(el, this); this.remove();}
  setAttribute() {}
  querySelector() {return {addEventListener() {}};}
}
function documentMock() {
  return {createElement(type) {
    if (type !== 'template') return new Element();
    return {set innerHTML(value) {this.content = {firstElementChild: Object.assign(new Element(), {html: value})};}};
  }};
}
// reconcileSections() 在 ui/dom.js（#199）。
let reconcileSections;
test.before(async () => { ({reconcileSections} = await web.load('ui/dom.js')); });
test('section updates preserve unaffected nodes and support reorder/removal', () => {
  globalThis.document = documentMock(); const c = {reconcileSections};
  const root = new Element();
  c.reconcileSections(root, [['a','A'],['b','B'],['c','C']]);
  const [a,b,cc] = root.children; const before = root.moves;
  c.reconcileSections(root, [['a','changed'],['b','B'],['c','C']]);
  assert.notEqual(root.children[0], a); assert.equal(root.children[1], b); assert.equal(root.children[2], cc);
  assert.equal(root.moves - before, 1);
  c.reconcileSections(root, [['c','C'],['b','B']]);
  assert.deepEqual(root.children, [cc,b]);
});
// 网格在 ui/grid.js，放在假页面上；它的定时器和读取由测试掌控。
let grid;
test.before(async () => { grid = await web.load('ui/grid.js'); });
function gridPage({items = [], total = items.length, scrollTop = 0} = {}) {
  const page = web.fakePage(), {state} = web.core();
  Object.assign(page.el('viewport'), {scrollTop, clientHeight: 200, clientWidth: 202});
  Object.assign(state, {size: 100, fit: false, total, items, route: {page: 'library'}, dataVersion: 2e6 + Math.random(), gridKey: '',
    pages: new Set(), pending: new Set(), generation: 1, readEpoch: 0, countDirty: false, sel: {all: false, ids: new Set(), exclude: new Set()}});
  return {page, state};
}
const quietTimers = () => { const saved = [globalThis.setTimeout, globalThis.requestAnimationFrame]; globalThis.setTimeout = () => 0; return () => { [globalThis.setTimeout, globalThis.requestAnimationFrame] = saved; }; };
// 网格的卡片是 ui/grid_card.js 的部件（#708，用 ui/el.js 的模板），这一项放在有真节点树的假 DOM（tests/dom.cjs）上。
test('virtual grid keeps overlapping cards, updates changed items in place and relayouts columns', () => {
  const restore = quietTimers(), document = dom.install();
  try {
    const els = {viewport: document.createElement('div'), grid: document.createElement('div'), gridHead: document.createElement('div')};
    document.getElementById = id => els[id] || null;
    els.grid.append(els.gridHead); els.viewport.append(els.grid);
    Object.assign(els.viewport, {scrollTop: 0, clientHeight: 200, clientWidth: 202});
    const {state} = web.core();
    Object.assign(state, {size: 100, fit: false, total: 30, items: Array.from({length: 30}, (_, i) => ({uuid: String(i), memberships: [], width: 100, height: 100})),
      route: {page: 'library'}, dataVersion: 2e6 + Math.random(), gridKey: '', pages: new Set(), pending: new Set(), generation: 1, readEpoch: 0, countDirty: false,
      sel: {all: false, ids: new Set(), exclude: new Set()}});
    const cards = () => els.grid.children.filter(c => c !== els.gridHead);
    const shown = top => grid.rowStart(Math.min(grid.rowCount(), grid.rowAt(top + 200) + 2)) - grid.rowStart(Math.max(0, grid.rowAt(top) - 2));
    grid.renderGrid(); const originals = cards(); assert.equal(originals.length, shown(0));
    assert.equal(els.grid.children[0], els.gridHead, '头部留在最前');
    els.viewport.scrollTop = 100; grid.renderGrid();
    assert.equal(cards().length, shown(100)); originals.forEach((card, i) => assert.equal(cards()[i], card));
    // 同一张图的数据变了：节点不换，只改变了的地方
    state.items[5] = {...state.items[5], memberships: [{id: 'c1', name: '合成甲', status: 'confirmed'}]}; state.dataVersion++; grid.renderGrid();
    assert.equal(cards()[5], originals[5]); assert.match(cards()[5].getAttribute('aria-label'), /合成甲/);
    els.viewport.scrollTop = 400; grid.renderGrid();
    const first = grid.rowStart(Math.max(0, grid.rowAt(400) - 2));
    assert.equal(Number(cards()[0].getAttribute('data-index')), first); assert.equal(cards()[0], originals[first]);
    state.size = 60; grid.renderGrid();
    assert.match(cards()[0].style.getPropertyValue('transform'), /^translate\(0px, /);
  } finally { restore(); dom.uninstall(); }
});
test('scroll bursts schedule a single render per frame', async () => {
  const frame = await web.load('ui/frame.js'), callbacks = [];
  frame.setScheduler(f => { callbacks.push(f); });
  try {
    const {page} = gridPage(); page.el('grid').hidden = true;
    for (let i = 0; i < 50; i++) grid.scheduleGrid();
    assert.equal(callbacks.length, 1); callbacks.shift()();
    grid.scheduleGrid(); assert.equal(callbacks.length, 1);
  } finally { frame.setScheduler(() => 0); }
});
test('pagination skips recount and obsolete completion cannot clear a newer pending page', async () => {
  let finish; const calls = [], restore = quietTimers();
  try {
    const {state} = gridPage({total: 200}); state.items = [];
    globalThis.fetch = path => { calls.push(path); return new Promise(resolve => finish = body => resolve({status: 200, ok: true, json: async () => body})); };
    const old = grid.loadPage(100);
    assert.match(calls[0], /offset=100/); assert.match(calls[0], /count=0/);
    state.generation++; state.pending = new Set([100]);
    finish({items: []}); await old;
    assert.ok(state.pending.has(100)); assert.equal(state.pages.size, 0);
    // 可能已变的总数随第一页重新读取，不随这一页读。
    state.pending.clear(); state.countDirty = true; const recount = grid.loadPage(100);
    await new Promise(r => setImmediate(r));
    assert.equal(calls.length, 2); assert.doesNotMatch(calls[1], /count=0/);
    finish({total: 200, items: []}); await recount;
  } finally { restore(); }
});

test('successful writes invalidate an in-flight metadata read before UI refresh starts', async () => {
  const pending = [];
  const c = metaHarness(path => path === '/api/tag' || path === '/api/suggest' ? {ok: true} : new Promise(resolve => pending.push({path,resolve})));
  c.state.readEpoch = 0;
  // 写入经过 core/session.js；ui/effects.js 的 afterWrite() 对它们计数，接法与 app.js 相同。
  const {post, onPost} = web.core(), {afterWrite} = await web.load('ui/effects.js');
  c.state.guide = undefined;
  Object.assign(c, {post, unlisten: onPost(afterWrite)});
  const first = c.refreshMeta(); await tick();
  await c.post('/api/suggest', {});
  assert.equal(c.state.readEpoch, 0); assert.equal(c.state.countDirty, undefined);
  await c.post('/api/tag', {});
  assert.equal(c.state.readEpoch, 1); assert.equal(c.state.countDirty, true);
  for (const p of pending.splice(0)) p.resolve(value(p.path, 'stale'));
  await tick(); assert.equal(c.session.token, '');
  for (const p of pending.splice(0)) p.resolve(value(p.path, 'after-write'));
  await first; assert.equal(c.session.token, 'after-write');
  c.unlisten();
});
test('reload counts only its first page and retries if data changed during the read', async () => {
  const pending = [], restore = quietTimers();
  try {
    const {state} = gridPage({total: 0}); state.items = [];
    globalThis.fetch = path => new Promise(resolve => pending.push({path, resolve: body => resolve({status: 200, ok: true, json: async () => body})}));
    const result = grid.reload(); state.readEpoch++;
    await new Promise(r => setImmediate(r));
    const first = pending.splice(0); assert.doesNotMatch(first[0].path, /count=0/); assert.ok(first.slice(1).every(p => /count=0/.test(p.path)));
    first.forEach((p, i) => p.resolve({total: i === 0 ? 200 : null, items: []})); await new Promise(r => setImmediate(r)); await new Promise(r => setImmediate(r));
    assert.notEqual(state.total, 200);   // 读取期间数据变了：不采用这次的响应
    const again = pending.splice(0); assert.ok(again.length >= 1); assert.doesNotMatch(again[0].path, /count=0/);
    again.forEach((p, i) => p.resolve({total: i === 0 ? 201 : null, items: []})); await result;
    assert.equal(state.total, 201); assert.equal(state.countDirty, false);
  } finally { restore(); }
});
test('bootstrap seed is discarded when data or NSFW changes before the batch starts', async () => {
  for (const mutate of [c => c.state.readEpoch++, c => c.state.nsfw = 'hide']) {
    const calls = [];
    const c = metaHarness(async path => {calls.push(path); return value(path, 'fresh');});
    c.state.readEpoch = 0;
    const result = c.refreshMeta({boot:{token:'old',total:1}});
    mutate(c); await result;
    assert.equal(calls.filter(p => p.startsWith('/api/bootstrap')).length, 1);
    assert.equal(c.session.token, 'fresh');
  }
});

test('metadata requests summaries outside directory and full rows inside directory', async () => {
  const calls = [];
  const c = metaHarness(async path => { calls.push(path); return value(path); });
  await c.refreshMeta();
  assert.ok(calls.find(p => p.startsWith('/api/characters')).includes('summary=1'));
  calls.length = 0; c.state.route.page = 'characters';
  await c.refreshMeta();
  assert.ok(!calls.find(p => p.startsWith('/api/characters')).includes('summary=1'));
});
test('active character retains full variant counts rather than summary rows', async () => {
  const c = metaHarness(async path => path.startsWith('/api/characters?id=') ? [{id: 'person', variants: [{id: 'skin', count: 7}]}] : value(path, 'person'));
  c.state.char = {id: 'person'};
  await c.refreshMeta();
  assert.equal(c.state.char.variants[0].count, 7);
});
test('navigation into directory during summary read triggers a full trailing read', async () => {
  const pending = [];
  const c = metaHarness(path => new Promise(resolve => pending.push({path, resolve})));
  const task = c.refreshMeta(); await tick();
  assert.ok(pending.some(p => p.path.includes('summary=1')));
  c.state.route.page = 'characters';
  for (const p of pending.splice(0)) p.resolve(value(p.path));
  await tick();
  assert.equal(pending.length, 5);
  assert.ok(pending.every(p => !p.path.includes('summary=1')));
  for (const p of pending.splice(0)) p.resolve(value(p.path));
  await task;
});
// 相似组页面在 ui/review.js。
test('group append, answer and selection preserve unaffected section nodes', async () => {
  const review = await web.load('ui/review.js'), {state} = web.core();
  const sections = new Element(), controls = {}, more = {}, nodes = {regroup: {}, moreGroups: {addEventListener() {}}};
  const root = {querySelector: selector => ({'.gsecs': sections, '.groups-controls': controls, '.groups-more': more})[selector],
    set innerHTML(_) { throw Error('must not replace all groups'); }};
  globalThis.document = Object.assign(documentMock(), {getElementById: id => id === 'groupsPage' ? root : nodes[id] || null});
  state.pools = {unmatched: 100};
  const group = cid => ({cid, size: 2, members: ['u', 'v'].map(x => x + cid)});
  Object.assign(review.groups, {list: [1, 2, 3].map(group), shown: 2, off: new Map(), learner: {}});
  review.drawGroups(); const [a, b] = sections.children;
  review.groups.shown = 3; review.drawGroups(); const third = sections.children[2];
  assert.equal(sections.children[0], a); assert.equal(sections.children[1], b);
  review.groups.list.shift(); review.drawGroups();
  assert.deepEqual(sections.children, [b, third]);
  review.groups.off.set(2, new Set(['u2'])); review.drawGroups();
  assert.notEqual(sections.children[0], b); assert.equal(sections.children[1], third);
});
test('older group reads cannot overwrite a newer R18-filtered response', async () => {
  const review = await web.load('ui/review.js'), {state} = web.core(), pending = [];
  web.fakePage(); globalThis.requestAnimationFrame ??= () => 0;   // 关闭详情面板会在下一帧重画网格
  Object.assign(state, {route: {page: 'groups'}, nsfw: '', pools: {unmatched: 0}});
  globalThis.fetch = path => new Promise(resolve => pending.push({path, resolve: body => resolve({status: 200, ok: true, json: async () => body})}));
  const old = review.renderGroups(); await new Promise(r => setImmediate(r));
  state.nsfw = 'hide'; const fresh = review.renderGroups(); await new Promise(r => setImmediate(r));
  pending[1].resolve({clusters: [{cid: 2, size: 2, members: ['a', 'b']}], learner: {}}); await fresh;
  pending[0].resolve({clusters: [{cid: 1, size: 2, members: ['c', 'd']}], learner: {}}); await old;
  assert.equal(review.groups.list[0].cid, 2);
});

test('a library page finishing after navigation cannot overwrite home items', async () => {
  let resolve; const restore = quietTimers();
  try {
    const {state} = gridPage({total: 200}); state.items = [];
    globalThis.fetch = () => new Promise(r => resolve = body => r({status: 200, ok: true, json: async () => body}));
    const before = state.dataVersion, pending = grid.loadPage(100); state.route = {page: 'days'}; state.items = [{uuid: 'home'}];
    resolve({items: [{uuid: 'old-library'}]}); await pending;
    assert.equal(state.items.length, 1); assert.equal(state.items[0].uuid, 'home'); assert.equal(state.dataVersion, before);
  } finally { restore(); }
});
