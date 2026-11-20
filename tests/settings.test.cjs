const {test, before, afterEach} = require('node:test');
const web = require('./web.cjs');   // core/settings.js、ui/settings.js（#199）
const assert = require('node:assert/strict');
let ui, off = () => {};
before(async () => { await web.ready(); ui = await web.load('ui/settings.js'); });
afterEach(() => { off(); off = () => {}; });
// 一个还什么都没读的设置页；api 回答它的读取。每到一部分时页面做什么，由 ui/settings.js 的 onSettings 监听器决定：
// 错误进横幅，设置页重画（renderSettings(false)）。
function harness(api) {
  const renders = [], errors = [], core = web.core(), {state} = core;
  for (const key of ['settings', 'fold', 'index', 'names']) delete state[key];
  state.route = {page: 'settings'};
  globalThis.fetch = path => { const answer = api(path); return Promise.resolve({status: 200, ok: true, json: () => answer}); };
  globalThis.document = {activeElement: null};
  off = core.onSettings((what, detail) => {
    if (what !== 'data') return;
    if (detail.error) errors.push(core.readError(detail.error));
    if (state.route.page === 'settings') renders.push(false);
  });
  const c = {state, loadSettingsData: core.loadSettingsData, shared: core.shared, document: globalThis.document};
  return {c, renders, errors};
}
const tick = () => new Promise(resolve => setImmediate(resolve));

test('cold load starts all four reads concurrently and shares in-flight requests', async () => {
  const pending = new Map(), calls = [];
  const {c, renders} = harness(path => { calls.push(path); return new Promise(resolve => pending.set(path, resolve)); });
  const load = c.loadSettingsData();
  assert.equal(c.loadSettingsData(), load);
  const same = c.shared('/api/names');
  assert.deepEqual(calls, ['/api/settings','/api/fold','/api/index','/api/names']);
  pending.get('/api/fold')({kinds: ['duplicate']}); await tick();
  assert.deepEqual(c.state.fold, ['duplicate']);
  assert.deepEqual(renders, [false]); // 慢的读取完成之前，先画出已有的有用部分
  for (const path of ['/api/settings','/api/index','/api/names']) pending.get(path)({loaded: true});
  await Promise.all([load, same]);
  assert.equal(renders.length, 4);
  c.loadSettingsData();
  assert.equal(calls.length, 4); // 已有数据时再渲染不重新请求
});

test('a failed card does not block others and remains retryable', async () => {
  let failed = true;
  const {c, errors} = harness(async path => {
    if (failed && path === '/api/names') throw new Error('offline');
    return path === '/api/fold' ? {kinds: []} : {loaded: true};
  });
  await c.loadSettingsData();
  assert.equal(c.state.names, undefined);
  assert.deepEqual(errors, ['读取失败：offline']);
  assert.equal(c.state.settings.loaded, true);
  failed = false; await c.loadSettingsData();
  assert.equal(c.state.names.loaded, true);
});

test('responses after navigating away update cached data without repainting', async () => {
  const {c, renders} = harness(async () => ({kinds: []}));
  const load = c.loadSettingsData(); c.state.route.page = 'library'; await load;
  assert.equal(renders.length, 0);
});

// 资料库 · 作品（#198）：作品树取代设置页的作品表。书架在 ui/catalog.js。
async function shelfHarness(works, watch = []) {
  const root = {className: '', innerHTML: '', querySelectorAll: () => [], setAttribute() {}, addEventListener() {}};
  const catalog = await web.load('ui/catalog.js'), {state} = web.core();
  Object.assign(state, {route: {page: 'characters', section: 'works'}, chars: [{work_id: 'a'}, {work_id: 'a'}, {work_id: 'b'}],
    index: {works, watch: {works: watch, pending_tags: []}, check: {missing: [{copyright: 'tag_a'}]}}});
  globalThis.document = {getElementById: () => root, activeElement: null};
  return {c: catalog, root};
}
test('work tree lists children under their parent, with counts and the watch switch', async () => {
  const works = [{id: 'b', name: 'child', parent_id: 'a', count: 1, watchable: false},
    {id: 'a', name: 'parent', parent_id: null, count: 9, tag: 'tag_a', watchable: true},
    {id: 'c', name: 'orphan', parent_id: 'gone', count: 3, status: 'pending', watchable: false}];
  const {c, root} = await shelfHarness(works, ['a']);
  c.drawShelf();
  const html = root.innerHTML, order = ['parent', 'child', 'orphan'].map(n => html.indexOf(`<b>${n}</b>`));
  assert.deepEqual([...order].sort((x, y) => x - y), order);
  assert.match(html, /style="--depth:1"[^]*<b>child<\/b>/);
  assert.match(html, /data-watch="a" checked/);
  assert.match(html, /data-watch="b"\s+disabled/);
  assert.match(html, /\+1 个新角色/);
  assert.match(html, /链接待确认/);
  c.shelf.filter = 'watched'; c.drawShelf();
  assert.ok(root.innerHTML.includes('<b>parent</b>') && !root.innerHTML.includes('<b>child</b>'));
});

// 索引的轮询在 core/jobs.js；它在任务面板之外重画什么，由 ui/effects.js 的 afterJob() 决定（接法与 app.js 相同）：
// 这里是画在假页面上的设置卡片。
async function indexHarness(initialStatus, nextStatus) {
  const calls = [], renders = [], {state, pollIndex, onJob} = web.core(), {afterJob} = await web.load('ui/effects.js');
  Object.assign(state, {route: {page: 'settings'}, index: {state: {status: initialStatus}, works: [], history: [], watch: {works: [], min_posts: 50}}});
  const page = web.fakePage();
  Object.defineProperty(page.el('indexCard'), 'innerHTML', {set() { renders.push('index'); }, get: () => ''});
  globalThis.fetch = async path => {
    calls.push(path);
    const body = path.includes('progress=1') ? {state: {status: nextStatus, done: 2, total: 10}} : {state: {status: nextStatus}, works: [], history: [], watch: {works: [], min_posts: 50}, check: {found: 1, missing: [], checked_at: '2026-10-18T00:00:00'}};
    return {status: 200, ok: true, json: async () => body};
  };
  const offs = [onJob(afterJob), onJob(() => { renders.push('tasks'); })];   // 第二个照任务面板的做法
  const setTimeout = globalThis.setTimeout;
  globalThis.setTimeout = () => 0;
  try { await pollIndex(); } finally { globalThis.setTimeout = setTimeout; for (const off of offs) off(); }
  return {calls, renders};
}

test('index progress does not reload work list or rebuild the editor', async () => {
  const {calls, renders} = await indexHarness('checking', 'checking');
  assert.deepEqual(calls, ['/api/index?progress=1']);
  assert.deepEqual(renders, ['tasks']);
});

test('index completion fetches updated results once', async () => {
  const {calls, renders} = await indexHarness('checking', 'checked');
  // 设置页开着时，「检查新角色」那一组随之更新，并另读一次覆盖情况（#561，覆盖情况走单独的路由；
  // tests/index_coverage.test.cjs）。这里没开设置页。
  assert.deepEqual(calls, ['/api/index?progress=1', '/api/index']);
  assert.deepEqual(renders, ['tasks']);
});

test('returning to settings refreshes cached data once without clearing it', async () => {
  const calls = [];
  const {c, errors} = harness(async path => {calls.push(path);return path === '/api/fold' ? {kinds: []} : {updated:true};});
  Object.assign(c.state,{settings:{old:true},fold:[],index:{old:true},names:{old:true}});
  const load = c.loadSettingsData(true);
  assert.equal(c.state.settings.old,true);
  assert.equal(c.loadSettingsData(true),load);
  await load;
  assert.equal(calls.length,4);
  assert.equal(c.state.names.updated,true);
});

test('the names and new-characters cards read their data when they open', async () => {
  const dom = require('./dom.cjs'), {view} = require('./settings_view.cjs');
  const {state} = web.core(), calls = [];
  state.route = {page: 'settings', section: 'characters'}; state.names = null; state.index = null; state.pack = null;
  dom.install();
  globalThis.fetch = async path => { calls.push(path); return {status: 200, ok: true, json: async () => path === '/api/names'
    ? {state: {status: 'idle'}, counts: {characters: {total: 2, zh: 1}}}
    : path === '/api/packs' ? null
    : {state: {status: 'idle'}, works: [], watch: {works: [], min_posts: 50}, history: [], mb_per_character: 2}}; };
  const {loadSection: load} = await web.load('ui/settings_characters.js'), open = await view('characters');
  await load(true);
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(calls.filter(p => p === '/api/names' || p === '/api/index').sort(), ['/api/index', '/api/names']);
  const v = open();
  assert.match(v.text, /角色名称/);
  assert.match(v.text, /检查新角色/);
  v.dispose(); dom.uninstall();
});

test('an older library is opened only after the person agrees to upgrade it (#193)', async () => {
  // openLibrary() 在 ui/settings_libraries.js：后端的拒绝指明需要升级，页面询问用户，然后带上 upgrade 再请求一次。
  const sent = [], nodes = {}, dialogs = [];
  const element = () => ({hidden: true, textContent: '', innerHTML: '', contains: () => false});
  globalThis.document = {getElementById: id => nodes[id] ||= element(), activeElement: null, body: {append(d) { dialogs.push(d); setImmediate(() => d.close('')); }},
    createElement() { const parts = {}; return {setAttribute() {}, querySelector: s => parts[s] ||= {}, showModal() {}, addEventListener() {}, remove() {}, close(v = '') { this.returnValue = v; this.onclose(); }}; }};
  globalThis.fetch = async (path, options) => {
    sent.push(JSON.parse(options.body));
    return {status: 409, ok: false, json: async () => ({error: '这个图库需要升级到新格式才能打开。升级后旧版本的 Illustash 打不开它；升级前会自动备份，升级没有完成时图库保持原样。'})};
  };
  const setTimeout = globalThis.setTimeout; globalThis.setTimeout = () => 0;
  const {openLibrary} = await web.load('ui/settings_libraries.js');
  try { await openLibrary('/tmp/synthetic.pilib', '合成'); } finally { globalThis.setTimeout = setTimeout; }
  assert.deepEqual(sent, [{path: '/tmp/synthetic.pilib'}]);
  assert.equal(dialogs.length, 1, 'asked before upgrading; declined, so nothing more was sent');
});

test('the full backup section names both backup folders, the upgrade one included (#517)', async () => {
  const {backupFolder} = await web.load('ui/settings_data.js');
  assert.equal(backupFolder('/Users/合成/Pictures/Illustash/合成.pilib', 'library-backups'), '/Users/合成/Pictures/Illustash/合成.pilib/library-backups');
  assert.equal(backupFolder('D:\\图库\\合成.pilib', 'backups'), 'D:\\图库\\合成.pilib\\backups');
});
