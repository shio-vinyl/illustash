// 运行：node --test tests/packs_panels.test.cjs
// 角色资料包（#602）在设置页和任务面板里的位置：「设置 › 角色资料」的一张卡片（范围标「本图库」，原来 WD 角色扩展和 CCIP
// 长尾包的两张合成一张）、读取失败时的提示，以及任务面板里下载、启用、撤回共用的一项。各状态的文字和按钮见 tests/packs.test.cjs。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');
const {view} = require('./settings_view.cjs');
let tasks, state, open;
before(async () => {
  await web.ready();
  web.fakePage();
  globalThis.window = {addEventListener() {}};
  globalThis.requestAnimationFrame = () => 0;
  const store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)};
  tasks = await web.load('ui/tasks.js');
  open = await view('characters');
  state = web.core().state;
});
// 「设置 › 角色资料」这一节画出来，返回资料包那一组的文字和几样东西。
function card() {
  dom.install();
  const v = open(), group = v.box.querySelector('[data-group="pack"]');
  const out = {text: require('./settings_view.cjs').visible(group), scope: group.querySelector('.set-scope'),
    buttons: v.buttons('packStatus').map(b => b.textContent)};
  const rows = [v.row('packMissing'), v.row('packMissingLink')].filter(r => !r.hidden);
  out.missingText = rows.map(r => require('./settings_view.cjs').visible(r)).join(' | ');
  v.dispose(); dom.uninstall();
  return out;
}
const part = (path, heads) => ({format: 'x', heads, candidate_only: 0, license: 'MIT', file: {path, size: 1000, sha256: 'ab'.repeat(32)}});
const READY = {catalog: {configured: true, serial: null, checked_at: null, error: null},
  current: {version: '0.1', stage: 'preview', revision: 1, heads: 282, candidate_only: 0, license: 'Apache-2.0 AND MIT', size: 2214393,
    status: 'ready', have: 2214393, components: {wd: part('wd-extension-v1.bin', 228), ccip: part('A-longtail-p75.zip', 54)}},
  offer: null, download: null, task: null,
  library: {enabled: false, version: null, wd: false, ccip: false, prompt: 'declined', images: 0, assigned: 0, vectors: 12, checked_at: null, blocked: null},
  location: '/synthetic/Shared/extensions', wd: {package: {}}, ccip: {library: {}}};

test('一张卡片在「角色资料」里，范围标「本图库」，状态和按钮来自 ui/packs.js', () => {
  state.route = {page: 'settings', section: 'characters'};
  state.pack = READY; state.index = null; state.names = null;
  const c = card();
  assert.match(c.text, /^角色资料包 \| 本图库 \| 多认 282 个 WD 词表和角色索引里没有的角色。/);
  assert.deepEqual([c.scope.getAttribute('data-scope'), c.scope.getAttribute('title')], ['library', '每个图库各自保存']);
  assert.match(c.text, /本图库未启用/);
  assert.deepEqual(c.buttons, ['启用…']);
});

test('等着默认启用（#630）的卡片底部照样写还缺几个角色（#606）', async () => {
  state.route = {page: 'settings', section: 'characters'};
  const index = {state: {status: 'idle'}, works: [], watch: {works: ['work_a'], min_posts: 50}, check: {checked_at: 't', missing: []},
    history: [], mb_per_character: 2};
  const coverage = {checked_at: 't', coverage: {work_a: {packs: 3, own: 0, missing: 2}}};
  globalThis.fetch = async () => ({status: 200, ok: true, headers: {get: () => null}, json: async () => coverage});
  state.index = index;
  await web.core().loadCoverage(index, true);
  state.pack = {...READY, current: {...READY.current, status: 'missing', have: 0},
    download: {kind: 'download', status: 'failed', message: '连接失败'},
    library: {...READY.library, prompt: 'pending', vectors: 12}};
  const c = card();
  assert.match(c.text, /角色资料包没能下载：连接失败/);
  assert.match(c.text, /下载完成后启用/);
  assert.equal(c.buttons[0], '重试');
  assert.ok(!c.buttons.includes('不用'));
  assert.match(c.missingText, /关注的作品还缺 2 个角色/);
  assert.match(c.missingText, /检查新角色…/);
  // 包到了、等后台任务结束时同样。
  state.pack = {...READY, library: {...READY.library, prompt: 'pending', vectors: 12}};
  const after = card();
  assert.match(after.text, /等后台任务结束后启用/);
  assert.match(after.missingText, /关注的作品还缺 2 个角色/);
});

test('还没读到时读一次；读取失败写在卡片里', async () => {
  state.route = {page: 'settings', section: 'characters'};
  state.pack = null; state.names = {state: {status: 'idle'}};
  state.index = {state: {status: 'idle'}, works: [], watch: {works: [], min_posts: 50}, check: null, history: [], mb_per_character: 2};
  const calls = [];
  globalThis.fetch = async path => { calls.push(path); return {status: 404, ok: false, headers: {get: () => null}, json: async () => ({error: '没有这个接口'})}; };
  dom.install();
  const v = open();
  await v.load(false);
  assert.deepEqual(calls.filter(p => p !== '/api/index/coverage'), ['/api/packs'], 'index and names were already read');
  v.update();
  assert.match(require('./settings_view.cjs').visible(v.box), /读取失败：没有这个接口/);
  v.dispose(); dom.uninstall();
});

test('任务面板：有下载或任务时才列出，失败时打警告', () => {
  state.pack = READY;
  assert.equal(tasks.taskList().find(x => x.key === 'pack'), undefined);
  state.pack = {...READY, task: {id: 't', kind: 'enable', status: 'running', phase: 'scan', done: 3, total: 12}};
  const running = tasks.taskList().find(x => x.key === 'pack');
  assert.equal(running.name, '角色资料包');
  assert.equal(running.active, true);
  assert.equal(running.line, '正在检查 3 / 12 张');
  assert.deepEqual(running.buttons.map(b => [b.attr, b.act]), [['pack', 'cancel']]);
  state.pack = {...READY, task: {id: 't', kind: 'withdraw', status: 'failed', message: '磁盘已满', diagnostic: {id: 'D-1', stored: true}}};
  const failed = tasks.taskList().find(x => x.key === 'pack');
  assert.equal(failed.warn, true);
  assert.equal(failed.line, '撤回没有完成，图库没有改动：磁盘已满');
  assert.match(failed.body, /<p class="task-side">错误编号：D-1<\/p>/);
});
