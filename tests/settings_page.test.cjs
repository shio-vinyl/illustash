// 设置页的外壳（ui/settings.js，#714）：从页面入口（pages.js 登记的 render）进设置页、离开以后，迟到的读取回复或失败
// 不动设置页、不在新页面上弹错误横幅（#785 第 3 条、第 9 条）。页面是 tests/dom.cjs 的假 DOM，请求由测试控制先后。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');
const {visible} = require('./settings_view.cjs');

let state, PAGES, scopes, settings;
const tick = async (n = 6) => { for (let i = 0; i < n; i++) await new Promise(resolve => setImmediate(resolve)); };
const calls = [];
before(async () => {
  await web.ready();
  dom.install();
  for (const id of ['settingsPage', 'viewport', 'selectionBar', 'resultCount', 'hint', 'errorBanner', 'inspector', 'updateButton', 'updateLabel',
    'brandLib', 'brand', 'nudge', 'grid', 'gridHead', 'toast', 'toastText', 'toastUndo', 'toastAct']) {
    const n = document.createElement('div'); n.setAttribute('id', id); document.body.appendChild(n);
  }
  const store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)};
  // 每个请求挂起，测试自己决定什么时候、用什么回。
  globalThis.fetch = (path, options) => new Promise((resolve, reject) => {
    const call = {path, body: options?.body ? JSON.parse(options.body) : null, replied: false,
      fail: e => { call.replied = true; reject(e); },
      reply: (body, status = 200) => { call.replied = true; resolve({status, ok: status < 400, headers: {get: () => null}, json: async () => body}); }};
    calls.push(call);
  });
  state = web.core().state;
  ({PAGES} = await web.load('core/pages.js'));
  scopes = await web.load('ui/scope.js');
  settings = await web.load('ui/settings.js');
  settings.wireSettings();
});
const enter = section => {
  state.route = {page: 'settings', section};
  return PAGES.settings.render({samePage: false, scope: scopes.page('settings')});
};
const leave = () => { scopes.page('library'); state.route = {page: 'library'}; };
const banner = () => document.getElementById('errorBanner');

// 回掉第一个还挂着的这个路径的请求。
const pending = path => calls.find(c => !c.replied && c.path === path);
const answer = (path, body, status = 200) => { const c = pending(path); assert.ok(c, `没有在等的 ${path}`); c.reply(body, status); };
const SETTINGS = {backup_count: 2, backups: [], thumbnails: {files: 3, bytes: 10}, package: {path: '/tmp/a.pilib'}, startup: {}, forced_off: []};

test('从页面入口进「角色资料」再离开：共享的 /api/names 迟到失败，新页面不弹错误横幅', async () => {
  Object.assign(state, {settings: null, fold: null, index: null, names: null, pack: null});
  enter('characters');
  await tick();
  answer('/api/settings', SETTINGS); answer('/api/fold', {kinds: []}); answer('/api/packs', null);
  answer('/api/index', {state: {status: 'idle'}, works: [], history: [], watch: {works: [], min_posts: 50}, check: null, mb_per_character: 2});
  await tick();
  assert.ok(pending('/api/names'), 'the names read is still out');
  leave();
  answer('/api/names', {error: '数据库被锁'}, 500);
  await tick(10);
  assert.equal(banner().textContent, '');
});

test('还在设置页时读取失败照常报', async () => {
  Object.assign(state, {settings: null, fold: null, index: null, names: null});
  enter('general');
  await tick();
  answer('/api/settings', {error: '数据库被锁'}, 500);
  for (const [path, body] of [['/api/fold', {kinds: []}], ['/api/index', {}], ['/api/names', {}]]) answer(path, body);
  await tick(10);
  assert.match(banner().textContent, /读取失败：数据库被锁/);
  banner().textContent = '';
});

// 原来 tests/settings.test.cjs 里由测试自己的回调判断页面；现在走真的外壳。
test('离开设置页以后才回来的读取：数据照样存下，旧的那一节不重画', async () => {
  Object.assign(state, {settings: null, fold: null, index: null, names: null, integrity: {}, human: null, models: null});
  enter('data');
  await tick();
  const row = document.getElementById('settingsPage').querySelector('[data-row="backupNow"]');
  assert.match(visible(row), /^…/);
  leave();
  answer('/api/settings', SETTINGS);
  for (const [path, body] of [['/api/fold', {kinds: []}], ['/api/index', {}], ['/api/names', {}]]) answer(path, body);
  await tick(10);
  assert.equal(state.settings.backup_count, 2, 'the late reply is kept');
  assert.match(visible(row), /^…/, 'the section that was left is not painted again');
  assert.equal(banner().textContent, '');
});
