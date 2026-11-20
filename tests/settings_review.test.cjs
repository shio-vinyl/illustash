// #771 审核里的边角（Codex 5990680560、可维护性 5990870995）：都从设置页的行触发，断言操作结束那一刻的 DOM，
// 并覆盖请求还没回来时离开这一节再进来。页面是 tests/dom.cjs 的假 DOM，请求由 fetch 的假回复控制先后。
const {test, before, beforeEach, afterEach, after} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');
const {view, toastNodes} = require('./settings_view.cjs');

let state, toastText;
const tick = async (n = 4) => { for (let i = 0; i < n; i++) await new Promise(resolve => setImmediate(resolve)); };
before(async () => { await web.ready(); state = web.core().state; });
beforeEach(() => { dom.install(); toastText = toastNodes(); state.route = {page: 'settings'}; });
// 提示条 3.5 秒后自己收起（ui/dom.js 的计时器），所以假 DOM 留到整个文件跑完再拆。
afterEach(() => { delete globalThis.fetch; });
after(() => dom.uninstall());
// 每个请求挂起，测试自己决定什么时候、用什么回。
function server() {
  const calls = [];
  globalThis.fetch = (path, options) => new Promise((resolve, reject) => {
    calls.push({path, body: options?.body ? JSON.parse(options.body) : null,
      reply: (body, status = 200) => resolve({status, ok: status < 400, headers: {get: () => null}, json: async () => body}), fail: reject});
  });
  return calls;
}

test('启动开关保存失败：落定那一刻开关回到原值', async () => {
  state.settings = {startup: {sync: false}, forced_off: []};
  Object.assign(state, {libraries: null, sync: {status: 'idle'}, syncHistory: [], exclInfo: {}, fold: []});
  const calls = server(), v = (await view('library'))();
  const box = v.row('startup-sync').querySelector('input');
  box.checked = true; box.dispatchEvent(dom.event('change'));
  calls.at(-1).reply({error: '数据库被锁'}, 409);
  await tick();
  assert.equal(box.checked, false);
  assert.match(toastText.textContent, /数据库被锁/);
  v.dispose();
});

test('相似折叠快速连点两项：第二次保存的集合两项都有', async () => {
  Object.assign(state, {settings: null, libraries: null, sync: {status: 'idle'}, syncHistory: [], exclInfo: {}, fold: [], dupInfo: {groups: 0}});
  const calls = server(), v = (await view('library'))();
  for (const kind of ['subtle', 'edit']) {
    const box = v.row('fold-' + kind)?.querySelector('input'); if (!box) continue;
    box.checked = true; box.dispatchEvent(dom.event('change'));
  }
  await tick();
  for (let i = 0; i < 3; i++) { for (const c of calls) c.reply({kinds: c.body?.kinds || []}); await tick(); }
  const saves = calls.filter(c => c.path === '/api/fold');
  assert.ok(saves.length >= 1);
  assert.ok(saves.every(c => c.body.kinds.includes('subtle')), 'the second click does not drop the first');
  assert.deepEqual(saves.at(-1).body.kinds.sort(), ['edit', 'subtle']);
  v.dispose();
});

test('整库备份以后读回设置：份数更新、按钮可点', async () => {
  state.settings = {backup_count: 1, backups: [], thumbnails: {files: 3, bytes: 10}, package: {path: '/tmp/a.pilib'}};
  Object.assign(state, {integrity: null, human: null, models: null});
  const calls = server(), v = (await view('data'))();
  v.press('backupNow', '立即备份');
  await tick();
  calls.find(c => c.path === '/api/backup').reply({path: '/tmp/a.pilib/backups/b.sqlite'});
  await tick();
  for (let i = 0; i < 3; i++) {
    for (const c of calls) if (c.path !== '/api/backup') c.reply(c.path === '/api/settings'
      ? {backup_count: 2, backups: [], thumbnails: {files: 3, bytes: 10}, package: {path: '/tmp/a.pilib'}} : c.path === '/api/fold' ? {kinds: []} : {});
    await tick(8);
  }
  v.update();
  assert.match(v.text, /共 2 份备份/);
  // 设置读回来了：按钮不再因为没有设置数据而不能点（还在等这次备份的收尾时，按钮自己的忙碌状态另算）。
  assert.equal(v.button('backupNow', '立即备份').dataset.off, undefined);
  assert.equal(v.button('thumbnails', '清除').dataset.off, undefined);
  v.dispose();
});

test('名称联网授权点了取消：开关立刻回到关', async () => {
  Object.assign(state, {names: {network: {authorized: false, granted_at: null}}, provenance: null, accounts: null, models: null, updates: null});
  server();
  // 确认框：假 DOM 的 <dialog> 由测试关掉（点取消）。
  const append = document.body.append.bind(document.body);
  document.body.append = d => { if (d.localName === 'dialog') { d.showModal = () => { d.returnValue = 'cancel'; queueMicrotask(() => d.dispatchEvent(dom.event('close'))); }; } append(d); };
  const v = (await view('network'))(), box = v.row('names').querySelector('input');
  box.checked = true; box.dispatchEvent(dom.event('change'));
  await tick(8);
  assert.equal(box.checked, false);
  v.dispose();
});

test('名称翻译的草稿和待确认的移除，离开这一节再进来就没有了', async () => {
  Object.assign(state, {pack: {}, names: {network: {}, model_api: {configured: false}}, index: {state: {status: 'idle'}, works: [], watch: {works: [], min_posts: 50},
    check: null, mb_per_character: 2, history: [{at: '2026-11-20 10:00', characters: [{status: 'added', short_tag: 'a', entity: {id: 1, name: '甲', count: 3}}]}]}});
  const calls = server(), open = await view('characters');
  let v = open();
  const base = v.row('modelBase').querySelector('input');
  base.value = 'https://draft.invalid'; base.dispatchEvent(dom.event('change'));
  v.row('history').querySelector('button').dispatchEvent(dom.event('click'));
  v.dispose();
  v = open();
  assert.equal(v.row('modelBase').querySelector('input').value, '', 'the draft went with the section');
  v.row('history').querySelector('button').dispatchEvent(dom.event('click'));
  await tick();
  assert.equal(calls.filter(c => c.body?.action === 'drop').length, 0, 'one click in the new section only arms');
  v.dispose();
});

test('新建图库等回复时点了取消：回复回来也不切库', async () => {
  const {libForm} = await web.load('ui/settings_libraries.js');
  Object.assign(state, {settings: null, sync: {status: 'idle'}, syncHistory: [], exclInfo: {}, fold: [], importTask: undefined,
    libraries: {info: null, current: '', home: '/tmp', libraries: [], export: {}}});
  Object.assign(libForm, {open: true, name: '合成', folders: {pixiv: ['/tmp/p']}, photos: false, created: null});
  const calls = server(), v = (await view('library'))();
  v.press('formCreate', '创建并打开');
  await tick();
  v.press('formCreate', '取消');
  calls.find(c => c.path === '/api/libraries/new').reply({path: '/tmp/合成.pilib', name: '合成', listed: true});
  await tick(8);
  assert.equal(calls.filter(c => c.path === '/api/libraries/open').length, 0);
  v.dispose();
});

test('网络列表：自动查画师关着时写每次询问，没读到时写状态未知', async () => {
  Object.assign(state, {names: null, accounts: null, models: null, updates: null, settings: null});
  server();
  const v = (await view('network'))();
  state.provenance = null; v.update();
  assert.match(v.text, /找回画师[^|]*\| [^|]*\| 状态未知/);
  state.provenance = {auto: false, auto_network: true, frozen: false}; v.update();
  assert.match(v.text, /找回画师[^|]*\| [^|]*\| 每次询问/);
  state.provenance = {auto: true, auto_network: true, frozen: false}; v.update();
  assert.match(v.text, /同步后自动，每次有流量上限/);
  v.dispose();
});
