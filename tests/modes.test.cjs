// 图片交互的模式（core/modes.js，#344）：浏览、选择、详情、大图怎样由 state 组成、叠在一起时谁在上面，当前图取
// 哪个来源，快捷键在哪一页、哪个模式下做什么；以及键盘处理（ui/grid.js）和换页（ui/shell.js）按这些规则改过的
// 四处行为：换页关大图、大图盖住选择栏、⌘＋／⌘－ 只在有网格的页面生效、整理页里没有动作的键给提示。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let modes, pages, state;
before(async () => {
  await web.ready();
  modes = await web.load('core/modes.js');
  pages = await web.load('core/pages.js');
  ({state} = web.core());
});
// 一个干净的 state：默认的浏览，列表里有三张图。
const items = [{uuid: 'a', memberships: []}, {uuid: 'b', memberships: [{id: 'c1'}]}, {uuid: 'c', memberships: []}];
function reset(patch = {}) {
  Object.assign(state, {route: {page: 'library'}, items: items.slice(), total: items.length, selecting: false,
    sel: {all: false, ids: new Set(), exclude: new Set()}, anchor: -1, cursor: -1, focused: null, detail: null,
    viewer: -1, viewerItem: null, question: null, armed: null}, patch);
}
beforeEach(() => reset());
const pick = (...ids) => ({all: false, ids: new Set(ids), exclude: new Set()});

// docs/WEB_UI_MAP.md 6.1 的每一行组合：最上面的模式、叠着的模式、快捷键作用于几张选中的图。
test('6.1 的状态组合读成明确的模式', () => {
  const rows = [
    ['全无', {}, 'browse', {browse: true}, 0],
    ['选择模式、还没选', {selecting: true}, 'select', {select: true}, 0],
    ['整理页点一张就选中', {sel: pick('a')}, 'select', {select: true}, 1],
    ['选择模式下选了', {selecting: true, sel: pick('a', 'b')}, 'select', {select: true}, 2],
    ['详情栏 + 选中', {focused: 'a', sel: pick('b')}, 'detail', {detail: true, select: true}, 1],
    ['从详情栏放大', {focused: 'a', viewer: 0, viewerItem: items[0]}, 'viewer', {viewer: true, detail: true}, 0],
    ['双击直接开大图', {viewer: 1, viewerItem: items[1]}, 'viewer', {viewer: true}, 0],
    ['大图盖住选择栏', {viewer: 1, viewerItem: items[1], sel: pick('a', 'c')}, 'viewer', {viewer: true, select: true}, 0],
    ['全选减去排除', {sel: {all: true, ids: new Set(), exclude: new Set(['a'])}}, 'select', {select: true}, 2],
  ];
  for (const [name, patch, top, on, n] of rows) {
    reset(patch);
    assert.equal(modes.topMode(), top, name);
    assert.deepEqual(modes.modes(), {viewer: false, detail: false, select: false, browse: false, ...on}, name);
    assert.equal(modes.keySelection(), n, name);
  }
  // 大图打开时选中仍在，只是快捷键不作用于它；关掉大图后照旧。
  reset({viewer: 1, viewerItem: items[1], sel: pick('a', 'c')});
  assert.equal(modes.selectionSize(), 2);
  state.viewer = -1; state.viewerItem = null;
  assert.equal(modes.keySelection(), 2);
});

test('当前图的来源：大图 ＞ 详情栏 ＞ 光标', () => {
  reset({cursor: 2, focused: 'a', viewer: 1, viewerItem: items[1]});
  assert.deepEqual(modes.currentSource(), {from: 'viewer', uuid: 'b'});
  state.viewer = -1; state.viewerItem = null;
  assert.deepEqual(modes.currentSource(), {from: 'focused', uuid: 'a'});
  state.focused = null;
  assert.deepEqual(modes.currentSource(), {from: 'cursor', index: 2});
  state.cursor = -1;
  assert.equal(modes.currentSource(), null);
  // 大图刚打开、图还没读到：退到光标，不取详情栏。
  reset({viewer: 2, viewerItem: null, focused: 'a', cursor: 2});
  assert.deepEqual(modes.currentSource(), {from: 'cursor', index: 2});
  assert.equal(modes.shownUuid(), undefined);
});

test('Space 打开哪一张：光标 ＞ 详情栏 ＞ 第一张', () => {
  reset({cursor: 2, focused: 'a'});
  assert.equal(modes.viewerTarget(), 2);
  state.cursor = -1;
  assert.equal(modes.viewerTarget(), 0);
  state.focused = 'c';
  assert.equal(modes.viewerTarget(), 2);
  state.focused = null;
  assert.equal(modes.viewerTarget(), 0);
});

// 快捷键表（docs/WEB_UI_MAP.md 第 5 节）：有选中时交给选择栏，没有时作用于当前图，没有对应动作时为 null。
test('单图／选择快捷键在各页做什么', () => {
  const P = pages.PAGES, C = pages.CHARACTER, act = (key, view, n, item = items[0], char = view === C, more = {}) => modes.keyAction(key, {view, char, n, item, ...more});
  // 角色页
  assert.deepEqual(act('remove', C, 2), {bar: 'remove'});
  assert.deepEqual(act('remove', C, 0), {single: 'remove'});
  assert.deepEqual(act('c', C, 2), {bar: 'swap'});
  assert.deepEqual(act('c', C, 0), {single: 'swap'});
  assert.deepEqual(act('a', C, 2), {bar: 'also'});
  assert.deepEqual(act('t', C, 2, items[0], true, {variants: true}), {bar: 'variant'});
  assert.equal(act('t', C, 2), null, 'no variants: the bar has no T button');
  assert.equal(act('t', C, 0), null);
  // 角色页「可能也是 TA」：选择栏只有「是 TA」「不是 TA ⌫」和通用按钮，C、A、T 没有对应的按钮（#422 的 2-21）
  const maybe = {maybe: true, variants: true};
  for (const key of ['c', 'a', 't']) assert.equal(act(key, C, 2, items[0], true, maybe), null, key);
  assert.deepEqual(act('remove', C, 2, items[0], true, maybe), {bar: 'remove'});
  assert.deepEqual(act('x', C, 2, items[0], true, maybe), {bar: 'exclude'});
  assert.deepEqual(act('c', C, 0, items[0], true, maybe), {single: 'swap'}, 'no selection: still the current image');
  assert.equal(act('s', C, 2), null);
  // 浏览页（图库、重复）
  for (const view of [P.library, P.duplicates]) {
    assert.deepEqual(act('remove', view, 3), {bar: 'notlib'}, view.name);
    // C＝换成、A＝追加（#399）：有选中时 C 换掉选中的图共同的角色，A 加角色；没有选中时 C 换掉当前图的角色
    assert.deepEqual(act('c', view, 3), {bar: 'replace'}, view.name);
    assert.deepEqual(act('a', view, 3), {bar: 'assign'}, view.name);
    assert.deepEqual(act('c', view, 0, items[1]), {single: 'swap'}, view.name);
    assert.deepEqual(act('c', view, 0, items[0]), {single: 'swap'}, view.name);   // 还没有角色：singleAction 退成设为
    assert.deepEqual(act('a', view, 0, items[1]), {single: 'also'}, view.name);
    assert.deepEqual(act('x', view, 0), {single: 'exclude'}, view.name);
  }
  // 当前图有角色时 ⌫ 移出它，没有角色就没有可移出的
  assert.deepEqual(act('remove', P.library, 0, items[1]), {single: 'remove'});
  assert.equal(act('remove', P.library, 0, items[0]), null);
  // 整理页：⌫ 有选中时没有对应动作（已知问题 13 说的那一种）
  for (const view of [P.unmatched, P.skipped, P.excluded, P.pending, P.group, P.audit]) assert.equal(act('remove', view, 2), null, view.name);
  // S：未识别一类跳过，没有选中时「全部未识别」和组页都跳过当前图（#385）；已跳过页恢复到未识别（#399）
  assert.deepEqual(act('s', P.unmatched, 2), {bar: 'skip'});
  assert.deepEqual(act('s', P.unmatched, 0), {single: 'skip'});
  assert.deepEqual(act('s', P.group, 2), {bar: 'skip'});
  assert.deepEqual(act('s', P.group, 0), {single: 'skip'});
  assert.deepEqual(act('s', P.skipped, 2), {bar: 'resume'});
  assert.deepEqual(act('s', P.skipped, 0), {single: 'resume'});
  for (const view of [P.excluded, P.pending, P.audit, P.library]) assert.equal(act('s', view, 1), null, view.name);
  // 未识别一类的图还没有角色：C 是设为，A 也是（#399）
  for (const view of [P.unmatched, P.skipped, P.group]) {
    assert.deepEqual(act('c', view, 2), {bar: 'assign'}, view.name);
    assert.deepEqual(act('a', view, 2), {bar: 'assign'}, view.name);
  }
  // 已排除、待确认、随机抽查的选择栏上没有「设为角色」：有选中时 C、A 没有对应的动作（#422 的 2-21）；没有选中时照旧作用于当前图
  for (const view of [P.excluded, P.pending, P.audit]) {
    for (const key of ['c', 'a', 't']) assert.equal(act(key, view, 2), null, `${view.name} ${key}`);
    assert.deepEqual(act('c', view, 0), {single: 'swap'}, view.name);
  }
  // X：已排除页恢复收录，其余排除
  assert.deepEqual(act('x', P.excluded, 1), {bar: 'include'});
  assert.deepEqual(act('x', P.excluded, 0), {single: 'include'});
  assert.deepEqual(act('x', P.pending, 1), {bar: 'exclude'});
  assert.deepEqual(act('g', P.unmatched, 4), {bar: 'tag'});
  assert.deepEqual(act('g', P.unmatched, 0), {single: 'tag'});
  // 相似组页（#353 起是整理页）：选择栏是默认的那种，⌫ 交给它的「不是…」，和浏览页一样
  assert.deepEqual(act('remove', P.sgroup, 2), {bar: 'notlib'});
  assert.deepEqual(act('c', P.sgroup, 2), {bar: 'replace'});
});

// 关大图后详情栏跟到大图里最后看的那张（#399）：之后「当前图」、Space 打开的、光标三者是同一张。
test('关大图时详情栏换到大图那张', () => {
  reset({focused: 'a', viewer: 2, viewerItem: items[2], cursor: 2});
  assert.equal(modes.detailAfterViewer(), 'c');
  // 没翻页、没开详情栏、大图还没读到图：不用换
  reset({focused: 'a', viewer: 0, viewerItem: items[0], cursor: 0});
  assert.equal(modes.detailAfterViewer(), null);
  reset({viewer: 2, viewerItem: items[2], cursor: 2});
  assert.equal(modes.detailAfterViewer(), null);
  reset({focused: 'a', viewer: 2, viewerItem: null, cursor: 2});
  assert.equal(modes.detailAfterViewer(), null);
});

// ---------- 键盘处理与换页 ----------
// 在假页面上挂上 ui/grid.js 的键盘处理，按一个键：返回事件（看 preventDefault）。
let keydown, page;
let gridModule;
// 按 state.items 真的挂上卡片（ui/grid_card.js 的部件），写入时网格经部件的 leave() 标记要飞走的卡片。
function mountCards() {
  Object.assign(page.el('viewport'), {clientWidth: 602, clientHeight: 400, scrollTop: 0});
  // 假页面的 #grid 和卡片节点不是同一套假 DOM，挪动不会从原处摘下：先清空（留下头部），再整排挂一次。
  const grid = page.el('grid'); grid.children = grid.children.filter(c => c.id === 'gridHead');
  gridModule.renderGrid(true);
}
async function wired() {
  if (keydown) return;
  page = web.fakePage();
  const handlers = {};
  page.document.addEventListener = (type, fn) => { (handlers[type] ||= []).push(fn); };
  globalThis.window = {addEventListener() {}};
  globalThis.ResizeObserver = class { observe() {} };
  globalThis.requestAnimationFrame = () => 0;
  globalThis.innerWidth = 1000; globalThis.innerHeight = 800;
  const store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)};
  gridModule = await web.load('ui/grid.js');
  await web.load('ui/grid_bar.js');   // 选择栏载入时登记进网格
  gridModule.wireGrid();
  keydown = (key, mods = {}) => {
    const e = {key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, target: {closest: () => null}, prevented: false,
      preventDefault() { this.prevented = true; }, ...mods};
    for (const fn of handlers.keydown) fn(e);
    return e;
  };
}

test('⌘＋／⌘－ 只在有网格的页面改网格缩放（已知问题 12）', async () => {
  await wired();
  const before = state.size;
  for (const r of [{page: 'settings', section: 'general'}, {page: 'days'}, {page: 'characters'}, {page: 'groups'}]) {
    reset({route: r});
    assert.equal(keydown('+', {metaKey: true}).prevented, true, r.page);   // 也不交给浏览器缩放整页
    assert.equal(keydown('-', {metaKey: true}).prevented, true, r.page);
    assert.equal(state.size, before, r.page);
    assert.equal(localStorage.getItem('pi.size'), null, r.page);
  }
});

test('大图打开时被盖住的选择栏不响应快捷键（已知问题 7）', async () => {
  await wired();
  page.el('grid').hidden = false;
  // ⌘A 不再在大图下面改选择
  reset({viewer: 0, viewerItem: items[0]});
  assert.equal(keydown('a', {metaKey: true}).prevented, true);
  assert.equal(state.sel.all, false);
  // 关了大图，⌘A 照常全选
  reset();
  keydown('a', {metaKey: true});
  assert.equal(state.sel.all, true);
  // 有选中、开着大图：Y／N 不回答选中的抽查
  reset({route: {page: 'audit'}, viewer: 0, viewerItem: items[0], sel: pick('a', 'b')});
  assert.equal(modes.keySelection(), 0);
});

test('整理页里没有对应动作的键给出提示（已知问题 13）', async () => {
  await wired();
  page.el('grid').hidden = false;
  const toast = () => page.el('toast').hidden === false ? page.el('toastText').textContent : '';
  const press = key => { page.el('toast').hidden = true; page.el('toastText').textContent = ''; keydown(key); return toast(); };
  reset({route: {page: 'unmatched'}, sel: pick('a')});
  assert.equal(press('Backspace'), '这一页没有 ⌫ 对应的操作');
  assert.equal(press('t'), '这一页没有 T 对应的操作');
  reset({route: {page: 'excluded'}, sel: pick('a')});
  assert.equal(press('s'), '这一页没有 S 对应的操作');
  // 没有选中时 ⌫ 作用于当前图：未识别的图没有角色可移出
  reset({route: {page: 'unmatched'}, cursor: 0});
  assert.equal(press('Delete'), '这一页没有 ⌫ 对应的操作');
  // 没有选中也没有当前图：什么都不做，也不提示
  reset({route: {page: 'unmatched'}});
  assert.equal(press('Backspace'), '');
  // 选择栏上没有这个键的按钮时，浏览页也提示（#422 的 2-21）；浏览页没有选中时不提示
  reset({route: {page: 'library'}, sel: pick('a')});
  assert.equal(press('s'), '这一页没有 S 对应的操作');
  reset({route: {page: 'library'}, cursor: 0});
  assert.equal(press('s'), '');
  // 随机抽查、已排除的选择栏上没有「设为角色」：C、A 提示，不打开命令面板
  for (const p of ['audit', 'excluded', 'pending']) {
    reset({route: {page: p}, sel: pick('a')});
    assert.equal(press('c'), '这一页没有 C 对应的操作', p);
    assert.equal(press('a'), '这一页没有 A 对应的操作', p);
  }
});

test('换页时关闭大图（已知问题 6）', async () => {
  await wired();
  const {applyRoute} = await web.load('ui/shell.js');
  globalThis.history = {replaceState() {}};
  globalThis.fetch = path => Promise.resolve({status: 200, ok: true, json: async () => path.startsWith('/api/assets') ? {total: 0, items: []} : []});
  reset({route: {page: 'library'}, viewer: 1, viewerItem: items[1], cursor: 1, sel: pick('a'), selecting: true});
  page.el('viewer').hidden = false;
  globalThis.location = {hash: '#/unmatched'};
  await applyRoute();
  assert.equal(state.viewer, -1);
  assert.equal(state.viewerItem, null);
  assert.equal(page.el('viewer').hidden, true);
  assert.equal(modes.topMode(), 'browse');
  assert.equal(state.cursor, -1);
  assert.equal(state.selecting, false);
});

// ---------- #353 ----------
// 首页没有网格光标：详情栏或大图打开时，单图快捷键作用于那张图；两者都没开时方向键、Space 不响应。
test('首页：详情栏和大图里的单图快捷键生效，详情栏外方向键不动光标', async () => {
  await wired();
  const calls = [];
  // 详情接口回一份最小的详情；其余写入接口回空对象
  const detail = path => ({uuid: new URLSearchParams(path.split('?')[1]).get('id'), memberships: [], alternatives: [], user_tags: []});
  globalThis.fetch = (path, opts = {}) => {
    calls.push([path, opts.body ? JSON.parse(opts.body) : null]);
    return Promise.resolve({status: 200, ok: true, json: async () => path.startsWith('/api/detail') ? detail(path) : {}});
  };
  const settle = () => new Promise(r => setTimeout(r, 0));
  globalThis.Image = class {};
  page.el('grid').hidden = true;
  // 什么都没开：方向键、Space、↵ 都不处理，也不阻止页面滚动
  reset({route: {page: 'days'}});
  for (const key of ['ArrowRight', 'ArrowDown', ' ', 'Enter']) assert.equal(keydown(key).prevented, false, key);
  assert.equal(state.cursor, -1);
  assert.equal(state.viewer, -1);
  // 详情栏打开：→ 换到下一张，不按网格的行去滚首页
  reset({route: {page: 'days'}, focused: 'a'});
  page.el('viewport').scrollTop = 1234;
  assert.equal(keydown('ArrowRight').prevented, true);
  assert.equal(state.focused, 'b');
  assert.equal(page.el('viewport').scrollTop, 1234);
  // ↓ 照常滚动页面
  assert.equal(keydown('ArrowDown').prevented, false);
  assert.equal(state.focused, 'b');
  // X、⌫ 作用于详情栏里那张
  calls.length = 0; keydown('x'); await settle();
  assert.deepEqual(calls.find(([p]) => p === '/api/curate')?.[1], {ids: ['b'], action: 'exclude'});
  calls.length = 0; keydown('Backspace'); await settle();
  assert.deepEqual(calls.find(([p]) => p === '/api/change')?.[1], {ids: ['b'], action: 'rejected', entity: 'c1'});
  // Space 打开的是详情栏里那张（不是别处留下的光标），大图里 ← → 换图，再按 Space 关掉
  reset({route: {page: 'days'}, focused: 'c', cursor: 0});
  assert.equal(keydown(' ').prevented, true);
  assert.equal(state.viewer, 2);
  keydown('ArrowLeft'); await settle();
  assert.equal(state.viewer, 1);
  keydown(' ');
  assert.equal(state.viewer, -1);
  // 面板页（设置、身份整理、画师抽查）照旧不响应
  for (const r of [{page: 'settings', section: 'general'}, {page: 'identity'}, {page: 'artist-audit'}]) {
    reset({route: r, focused: 'a'});
    assert.equal(keydown('ArrowRight').prevented, false, r.page);
    assert.equal(state.focused, 'a', r.page);
  }
  globalThis.fetch = undefined;
});

test('相似组页是整理页：↵ 在有光标时切换选中（#353）', async () => {
  await wired();
  page.el('grid').hidden = false;
  reset({route: {page: 'sgroup', id: '4'}, cursor: 0});
  keydown('Enter');
  assert.deepEqual([...state.sel.ids], ['a']);
  keydown('Enter');
  assert.deepEqual([...state.sel.ids], []);
  assert.equal(state.focused, null);
});

test('⌘滚轮、捏合只在主区是网格的页面缩放网格，也不交给浏览器缩放整页', async () => {
  await wired();
  // 缩放排到下一帧做（ui/frame.js）：数排了几次帧。每次先换一个新的调度器，清掉上一次没跑的任务。
  const frame = await web.load('ui/frame.js');
  let frames = 0;
  const counting = () => frame.setScheduler(() => { frames++; });
  const viewport = page.el('viewport');
  const wheel = route => { reset({route}); frames = 0; counting(); const e = {ctrlKey: false, metaKey: true, deltaY: -50, clientX: 10, clientY: 10, prevented: false, preventDefault() { this.prevented = true; }}; for (const fn of viewport.listeners.wheel) fn(e); return e; };
  const pinch = route => { reset({route}); frames = 0; counting(); const e = {scale: 2, clientX: 10, clientY: 10, prevented: false, preventDefault() { this.prevented = true; }}; for (const fn of viewport.listeners.gesturestart) fn(e); for (const fn of viewport.listeners.gesturechange) fn(e); return e; };
  for (const r of [{page: 'settings', section: 'general'}, {page: 'days'}, {page: 'characters'}, {page: 'identity'}]) {
    assert.equal(wheel(r).prevented, true, r.page); assert.equal(frames, 0, r.page);
    assert.equal(pinch(r).prevented, true, r.page); assert.equal(frames, 0, r.page);
  }
  for (const r of [{page: 'library'}, {page: 'unmatched'}]) {
    wheel(r); assert.equal(frames, 1, r.page);
    pinch(r); assert.equal(frames, 1, r.page);
  }
  // 不按 ⌘ 的滚轮是普通滚动
  reset({route: {page: 'library'}}); frames = 0; counting();
  const plain = {ctrlKey: false, metaKey: false, deltaY: 50, prevented: false, preventDefault() { this.prevented = true; }};
  for (const fn of viewport.listeners.wheel) fn(plain);
  assert.deepEqual([plain.prevented, frames], [false, 0]);
  frame.setScheduler(() => 0);
});

// 组页上不选图按 S：和「全部未识别」一样，当前图先飞走（卡片带 leaving），再发出「跳过」（#385）。
test('不选图按 S：全部未识别和组页跳过当前这张，已跳过页恢复它（#399）', async () => {
  await wired();
  const requests = [];
  const grid = page.el('grid');
  globalThis.fetch = async (path, init = {}) => {
    const flying = grid.children.filter(c => c.classList.contains('leaving')).map(c => c.dataset.id);
    requests.push({path, body: init.body ? JSON.parse(init.body) : null, flying});
    return {status: 500, ok: false, text: async () => 'stop', json: async () => ({error: 'stop'})};
  };
  const skipFrom = async (route, action) => {
    requests.length = 0;
    reset({route, cursor: 0});
    mountCards();
    page.el('toast').hidden = true; page.el('toastText').textContent = '';
    keydown('s');
    await new Promise(done => setTimeout(done, 20));
    const changes = requests.filter(r => r.path === '/api/change');
    assert.equal(changes.length, 1, route.page);
    assert.deepEqual(changes[0].body, {ids: ['a'], action}, route.page);
    assert.deepEqual(changes[0].flying, ['a'], `${route.page}：请求发出时这张已经在飞走`);
  };
  await skipFrom({page: 'unmatched'}, 'unknown');
  await skipFrom({page: 'group', id: '3'}, 'unknown');
  await skipFrom({page: 'skipped'}, 'resume');
});

// 详情栏里「也可能是」设角色：lead 页（全部未识别、已跳过、组页）上图先飞走，别的页不飞（#385）。
test('详情栏设角色后图飞走：未识别一类三页都飞，图库不飞', async () => {
  await wired();
  const detail = await web.load('ui/detail.js');
  const grid = page.el('grid');
  const seen = [];
  globalThis.fetch = async (path, init = {}) => {
    seen.push({path, flying: grid.children.filter(c => c.classList.contains('leaving')).map(c => c.dataset.id)});
    return {status: 500, ok: false, text: async () => 'stop', json: async () => ({error: 'stop'})};
  };
  for (const [route, flies] of [[{page: 'unmatched'}, true], [{page: 'skipped'}, true], [{page: 'group', id: '3'}, true], [{page: 'library'}, false]]) {
    seen.length = 0;
    reset({route});
    mountCards();
    detail.pickAlternative({memberships: [], alternatives: [{id: 'c9'}]}, 'a', 0);
    await new Promise(done => setTimeout(done, 20));
    const sent = seen.filter(r => r.path === '/api/change');
    assert.equal(sent.length, 1, route.page);
    assert.deepEqual(sent[0].flying, flies ? ['a'] : [], route.page);
  }
});

// ---------- #399 ----------
// 详情接口按 id 回 items 里那张的角色；/api/suggest 回空，命令面板只列 state.chars。
function stubDetail(calls = []) {
  globalThis.fetch = async (path, opts = {}) => {
    calls.push([path, opts.body ? JSON.parse(opts.body) : null]);
    const id = path.startsWith('/api/detail') ? new URLSearchParams(path.split('?')[1]).get('id') : null;
    const memberships = (state.items.find(x => x?.uuid === id)?.memberships || []).map(m => ({name: m.id.toUpperCase(), ...m}));
    const body = id ? {uuid: id, memberships, alternatives: [], user_tags: []} : path === '/api/suggest' ? [] : {};
    return {status: 200, ok: true, json: async () => body};
  };
  globalThis.Image = class {};
  return calls;
}
const settle = () => new Promise(r => setTimeout(r, 20));
// 命令面板底部那句话：说明打开的是换成还是追加。
const pickerSays = () => page.el('picker').open ? page.el('pickerFoot').innerHTML.replace(/<[^>]+>/g, '').trim() : '';
// 关掉命令面板，并像浏览器那样发出 close 事件，好让浮层管理器把它出栈。
const closePicker = () => { const el = page.el('picker'); el.close(); for (const fn of (el.listeners.close || []).splice(0)) fn(); };

test('详情栏开着时进大图翻几张，关大图后详情栏和快捷键都在最后看的那张（#399）', async () => {
  await wired();
  const calls = stubDetail();
  page.el('grid').hidden = false;
  reset({route: {page: 'library'}, focused: 'a', cursor: 0});
  keydown(' ');
  assert.equal(state.viewer, 0);
  keydown('ArrowRight'); await settle();
  keydown('ArrowRight'); await settle();
  assert.equal(state.viewer, 2);
  keydown(' '); await settle();
  assert.equal(state.viewer, -1);
  assert.equal(state.focused, 'c');
  assert.equal(state.cursor, 2);
  assert.deepEqual(modes.currentSource(), {from: 'focused', uuid: 'c'});
  assert.equal(modes.viewerTarget(), 2);
  calls.length = 0; keydown('x'); await settle();
  assert.deepEqual(calls.find(([p]) => p === '/api/curate')?.[1], {ids: ['c'], action: 'exclude'});
  // 没开详情栏时关大图不开详情栏
  reset({route: {page: 'library'}, cursor: 0});
  keydown(' '); keydown('ArrowRight'); await settle(); keydown(' ');
  assert.equal(state.focused, null);
  globalThis.fetch = undefined;
});

test('C 换成、A 追加：单图（#399）', async () => {
  await wired();
  stubDetail();
  state.chars = [{id: 'c2', name: 'C2'}];
  page.el('grid').hidden = false;
  // 图上已有角色：C 是换成，A 是追加
  reset({route: {page: 'library'}, focused: 'b', cursor: 1});
  keydown('c'); await settle();
  assert.equal(pickerSays(), '↵ 把这张从 C1 换成 C2⇧↵ 多选');
  closePicker();
  // A 和详情栏的「＋ 也是…」是同一个面板，能多选（#424 的 2-24）
  keydown('a'); await settle();
  assert.equal(pickerSays(), '↵ 给这张加上 C2⇧↵ 多选');
  closePicker();
  // 还没有角色：C、A 都是设为
  reset({route: {page: 'library'}, focused: 'a', cursor: 0});
  keydown('c'); await settle();
  assert.equal(pickerSays(), '↵ 把这张设为 C2⇧↵ 多选');
  closePicker();
  globalThis.fetch = undefined; state.chars = [];
});

test('C 换成、A 追加：图库的选择栏（#399）', async () => {
  await wired();
  stubDetail();
  state.chars = [{id: 'c2', name: 'C2'}];
  page.el('grid').hidden = false;
  const grid = await web.load('ui/grid.js');
  const bar = act => web.barButton(page.el('selectionBar'), act);
  const toast = () => page.el('toast').hidden === false ? page.el('toastText').textContent : '';
  const c1 = {id: 'c1', name: 'C1'}, c3 = {id: 'c3', name: 'C3'};
  const many = [{uuid: 'a', memberships: []}, {uuid: 'b', memberships: [c1]}, {uuid: 'c', memberships: []}, {uuid: 'd', memberships: [c1, c3]}, {uuid: 'e', memberships: [c1, c3]}];
  // 都有同一个角色：C 换掉它，A 加角色
  reset({route: {page: 'library'}, items: many, total: many.length, sel: pick('b', 'd')});
  grid.renderSelection();
  assert.deepEqual(bar('replace'), {act: 'replace', label: '换成…', key: 'C', disabled: false, title: ''});
  assert.deepEqual(bar('assign'), {act: 'assign', label: '加角色…', key: 'A', disabled: false, title: ''});
  keydown('c'); await settle();
  assert.equal(pickerSays(), '↵ 把 2 张从 C1 换成 C2⇧↵ 多选');
  closePicker();
  // A 是追加，面板底部也写追加（#423）
  keydown('a'); await settle();
  assert.equal(pickerSays(), '↵ 给 2 张加上 C2⇧↵ 多选');
  closePicker();
  // 共同的角色不止一个：先问换掉哪个
  reset({route: {page: 'library'}, items: many, total: many.length, sel: pick('d', 'e')});
  keydown('c'); await settle();
  assert.equal(pickerSays(), '↵ 把 2 张的 C1 换掉');
  closePicker();
  // 没有共同的角色：按钮不可用，按 C 说明原因
  reset({route: {page: 'library'}, items: many, total: many.length, sel: pick('a', 'b')});
  grid.renderSelection();
  assert.deepEqual(bar('replace'), {act: 'replace', label: '换成…', key: 'C', disabled: true, title: '所选图片没有共同的角色'});
  page.el('toast').hidden = true; keydown('c'); await settle();
  assert.equal(toast(), '所选图片没有共同的角色');
  assert.equal(page.el('picker').open, false);
  // 都还没有角色：只有「设为角色 C」，C 打开设为
  reset({route: {page: 'library'}, items: many, total: many.length, sel: pick('a', 'c')});
  grid.renderSelection();
  assert.equal(bar('replace'), null);
  assert.deepEqual(bar('assign'), {act: 'assign', label: '设为角色…', key: 'C', disabled: false, title: ''});
  keydown('c'); await settle();
  assert.equal(pickerSays(), '↵ 把 2 张设为 C2⇧↵ 多选');
  closePicker();
  globalThis.fetch = undefined; state.chars = [];
});
