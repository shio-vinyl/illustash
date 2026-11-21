// #771 审核里的边角（Codex 5990680560、可维护性 5990870995，复核 5991357717 剩下的几条见 #785）：都从设置页的行触发，
// 断言操作结束那一刻的 DOM，并覆盖请求还没回来时离开这一节再进来。页面是 tests/dom.cjs 的假 DOM，请求由 fetch 的假回复
// 控制先后；这一节随外壳订阅的通知更新（tests/settings_view.cjs），测试不自己补 update()。
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
// 还没回的请求都回掉（折叠保存回它发的集合，其余回空对象），直到不再有新请求。
async function drain(calls) {
  for (let i = 0; i < 6; i++) { for (const c of calls) if (!c.replied) c.reply(c.path === '/api/fold' ? {kinds: c.body.kinds} : {}); await tick(); }
}
// 每个请求挂起，测试自己决定什么时候、用什么回。
function server() {
  const calls = [];
  globalThis.fetch = (path, options) => new Promise((resolve, reject) => {
    const call = {path, body: options?.body ? JSON.parse(options.body) : null, replied: false, fail: reject,
      reply: (body, status = 200) => { call.replied = true; resolve({status, ok: status < 400, headers: {get: () => null}, json: async () => body}); }};
    calls.push(call);
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
  await drain(calls);
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
  // 不手动 update：读回设置以后的通知（onSettings）就要把这一节画对。
  assert.match(v.text, /共 2 份备份/);
  // 设置读回来了：按钮不再因为没有设置数据而不能点（还在等这次备份的收尾时，按钮自己的忙碌状态另算）。
  assert.equal(v.button('backupNow', '立即备份').dataset.off, undefined);
  assert.equal(v.button('thumbnails', '清除').dataset.off, undefined);
  v.dispose();
});

// 确认框（ui/dialog.js 的 <dialog>）换成测试自己的假对象：记下标题，由测试决定点确定还是取消。行的节点照旧是假 DOM 的。
function dialogs() {
  const opened = [], create = document.createElement.bind(document), append = document.body.append.bind(document.body);
  document.createElement = tag => tag !== 'dialog' ? create(tag) : (() => {
    const parts = {};
    return {localName: 'dialog', returnValue: '', set innerHTML(_) {}, setAttribute() {}, addEventListener() {}, remove() {},
      querySelector: sel => parts[sel] ||= {textContent: '', focus() {}}, showModal() { this.open = true; },
      close(value = '') { this.returnValue = value; this.open = false; this.onclose?.(); }};
  })();
  document.body.append = (...nodes) => { for (const n of nodes) if (n.localName === 'dialog' && typeof n.nodeType !== 'number') opened.push(n); else append(n); };
  return opened;
}

test('名称联网授权点了取消：确认框开着时开关是开，取消后复位、没发写请求', async () => {
  Object.assign(state, {names: {network: {authorized: false, granted_at: null}}, provenance: null, accounts: null, models: null, updates: null});
  const calls = server(), opened = dialogs();
  const v = (await view('network'))(), box = v.row('names').querySelector('input');
  box.checked = true; box.dispatchEvent(dom.event('change'));
  await tick();
  assert.equal(opened.length, 1, 'the confirmation is open');
  assert.equal(opened[0].open, true);
  assert.equal(box.checked, true, 'still on while the question is open');
  opened[0].close('cancel');
  await tick(8);
  assert.equal(box.checked, false);
  assert.equal(calls.filter(c => c.path === '/api/names').length, 0, 'nothing was written');
  v.dispose();
});

test('名称翻译：回车保存了 key 以后框里的旧 key 清掉，接着只改地址不会把它带上；连续回车按顺序', async () => {
  Object.assign(state, {pack: {}, index: null, names: {network: {}, model_api: {configured: false, base: 'https://a.invalid', model: 'm1'}}});
  const calls = server(), v = (await view('characters'))();
  const enter = (key, value) => {
    const input = v.row(key).querySelector('input');
    input.focus(); input.value = value;
    input.dispatchEvent(dom.event('keydown', {key: 'Enter'}));
    return input;
  };
  const key = enter('modelKey', 'sk-old');
  await tick();
  const first = calls.at(-1);
  assert.equal(first.body.key, 'sk-old');
  state.names.model_api = {configured: true, base: 'https://a.invalid', model: 'm1', hint: 'old'};
  first.reply({model_api: state.names.model_api});
  await tick(8);
  assert.equal(key.value, '', 'the saved key left the box even with the focus still in it');
  key.dispatchEvent(dom.event('change')); key.blur();
  enter('modelBase', 'https://b.invalid');
  await tick();
  assert.equal(calls.at(-1).body.base, 'https://b.invalid');
  assert.equal('key' in calls.at(-1).body, false, 'the old key does not travel to the new address');
  calls.at(-1).reply({model_api: {configured: false, base: 'https://b.invalid', model: 'm1'}});
  await tick(8);
  // 连续两次回车：第二次等第一次落定才发。
  const before = calls.length;
  enter('modelName', 'm2'); enter('modelName', 'm3');
  await tick();
  assert.equal(calls.length, before + 1, 'the second save waits for the first');
  assert.equal(calls.at(-1).body.model, 'm2', 'each save sends what was in the box when Enter was pressed');
  calls.at(-1).reply({model_api: {configured: false, base: 'https://b.invalid', model: 'm2'}});
  await tick(8);
  assert.equal(calls.length, before + 2);
  assert.equal(calls.at(-1).body.model, 'm3');
  v.dispose();
});

test('换包被宿主拒绝：另一个失败任务的「重试」不会去换那个版本', async () => {
  const packs = await web.load('ui/packs.js');
  const ready = {status: 'ready', components: {}}, lib = {enabled: true, wd: true, ccip: true};
  const failedA = {current: ready, library: lib, task: {id: 'a', kind: 'enable', status: 'failed', message: 'x'},
    offer: {version: 'B', status: 'ready'}, download: null, wd: {package: {}}, ccip: {library: {}}};
  state.pack = failedA;
  const calls = server(), bodies = () => calls.filter(c => c.path === '/api/packs').map(c => c.body);
  const p = packs.runPack('enable');   // 宿主接受了重新检查，起了任务 a
  await tick(); calls.at(-1).reply(failedA); await p;
  globalThis.confirm = () => true;
  const opened = dialogs();
  const sw = packs.runPack('offer-switch');
  await tick(); opened.at(-1)?.close('ok'); await tick();
  calls.at(-1).reply({error: '正在同步'}, 409); await sw;
  assert.deepEqual(bodies().at(-1), {action: 'enable', version: 'B'});
  state.pack = failedA;
  const retry = packs.runPack('enable-retry');
  await tick(); calls.at(-1).reply(failedA); await retry;
  assert.deepEqual(bodies().at(-1), {action: 'enable'}, 'retrying task a re-checks the card version, not B');
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

test('网络列表：自动查画师关着时写每次询问，冻结的图库单列，没读到时写状态未知', async () => {
  Object.assign(state, {names: null, accounts: null, models: null, updates: null, settings: null});
  server();
  const v = (await view('network'))();
  state.provenance = null; v.update();
  assert.match(v.text, /找回画师[^|]*\| [^|]*\| 状态未知/);
  state.provenance = {auto: false, auto_network: true, frozen: false}; v.update();
  assert.match(v.text, /找回画师[^|]*\| [^|]*\| 每次询问/);
  state.provenance = {auto: true, auto_network: true, frozen: false}; v.update();
  assert.match(v.text, /同步后自动，每次有流量上限/);
  state.provenance = {auto: true, auto_network: true, frozen: true}; v.update();
  assert.match(v.text, /找回画师[^|]*\| [^|]*\| 冻结的图库不可用/);
  v.dispose();
});

test('相似折叠快速连点三项：每次保存都带上之前点过的', async () => {
  Object.assign(state, {settings: null, libraries: null, sync: {status: 'idle'}, syncHistory: [], exclInfo: {}, fold: [], dupInfo: {groups: 0}});
  const calls = server(), v = (await view('library'))(), saves = () => calls.filter(c => c.path === '/api/fold');
  const click = kind => { const box = v.row('fold-' + kind).querySelector('input'); box.checked = true; box.dispatchEvent(dom.event('change')); };
  click('subtle'); await tick();
  click('edit'); await tick();
  // 第一个保存先回来，第三次点在第二个保存回来之前。
  saves()[0].reply({kinds: ['subtle']});
  for (let i = 0; i < 3; i++) { for (const c of calls) if (!c.replied && c.path !== '/api/fold') c.reply({}); await tick(); }
  click('crop'); await tick();
  await drain(calls);
  assert.deepEqual(saves().at(-1).body.kinds.sort(), ['crop', 'edit', 'subtle']);
  v.dispose();
});

test('新建图库：等回复时离开这一节，建好但没登记的图库回来还在表单里，选文件夹的迟到结果不写回', async () => {
  const {libForm} = await web.load('ui/settings_libraries.js');
  Object.assign(state, {settings: null, sync: {status: 'idle'}, syncHistory: [], exclInfo: {}, fold: [], importTask: undefined,
    libraries: {info: null, current: '', home: '/tmp', libraries: [], export: {}}});
  Object.assign(libForm, {open: true, name: '合成', folders: {pixiv: ['/tmp/p']}, photos: false, created: null});
  const calls = server(), open = await view('library');
  let v = open();
  v.press('formCreate', '创建并打开');
  v.press('formPick', 'Pixiv');   // 文件夹选择框开着
  await tick();
  v.dispose(); state.route = {page: 'settings'};   // 离开这一节，路由换了
  calls.find(c => c.path === '/api/libraries/new').reply({path: '/tmp/合成.pilib', name: '合成', listed: false, error: '写入失败'});
  const pickCall = calls.find(c => c.path === '/api/libraries/choose' && c.body?.kind === 'folder');
  assert.ok(pickCall, 'the folder picker was asked');
  pickCall.reply({paths: ['/tmp/late']});
  await tick(8);
  v = open();
  assert.match(v.text, /图库已经建好/);
  assert.match(v.text, /写入失败/);
  assert.ok(v.button('formCreated', '打开这个图库'));
  assert.doesNotMatch(v.text, /\/tmp\/late/, 'the late folder pick is dropped');
  assert.equal(calls.filter(c => c.path === '/api/libraries/open').length, 0);
  v.dispose();
});

// ---------- #785：复核 5991357717 剩下的几条 ----------
// 回掉第一个还挂着的这个路径的请求。
const replier = calls => (path, body, status) => {
  const c = calls.find(x => !x.replied && x.path === path);
  assert.ok(c, `没有在等的 ${path}`);
  c.reply(body, status);
};
// 假的计时器和时钟：setTimeout 只记下来，run() 时按记下的先后调用，并把时钟拨过它们的时长。
function clock() {
  const real = {setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout, now: Date.now};
  let now = real.now();
  const timers = [];
  globalThis.setTimeout = (fn, ms = 0) => { const t = {fn, ms, fake: true}; timers.push(t); return t; };
  globalThis.clearTimeout = t => { if (t?.fake) t.cleared = true; else real.clearTimeout(t); };
  Date.now = () => now;
  return {
    get now() { return now; },
    pending: () => timers.filter(t => !t.cleared && !t.ran),
    async run() {
      const due = timers.filter(t => !t.cleared && !t.ran);
      for (const t of due) { t.ran = true; now += t.ms; t.fn(); }
      await tick(8);
      return due.length;
    },
    restore() { globalThis.setTimeout = real.setTimeout; globalThis.clearTimeout = real.clearTimeout; Date.now = real.now; },
  };
}
const LIBS = (ex = {status: 'idle'}) => ({info: {name: '合成', path: '/tmp/合成.pilib', sources: []}, current: '/tmp/合成.pilib', home: '/tmp',
  libraries: [], export: ex});
const libraryState = () => Object.assign(state, {settings: null, sync: {status: 'idle'}, syncHistory: [], exclInfo: {}, fold: [], importTask: undefined});

test('#785-1 语言下拉框保存失败：焦点还在框上，落定后回到原来的语言', async () => {
  const calls = server(), v = (await view('general'))();
  const select = v.row('language').querySelector('select');
  assert.equal(select.value, 'zh-CN');
  select.focus(); select.value = 'system'; select.dispatchEvent(dom.event('change'));
  await tick();
  const save = calls.find(c => c.path === '/api/preferences');
  assert.deepEqual(save.body, {language: 'system'});
  save.reply({error: '无法写入设置'}, 500);
  await tick(8);
  assert.equal(document.activeElement, select, 'the focus stays on the select');
  assert.equal(select.value, 'zh-CN');
  assert.match(toastText.textContent, /无法写入设置/);
  v.dispose();
});

test('#785-2 进这一节时同批的 /api/names 还挂着，备份以后照样新读一次设置', async () => {
  const core = web.core();
  Object.assign(state, {settings: null, fold: null, index: null, names: null, integrity: null, human: null, models: null});
  const calls = server(), answer = replier(calls);
  core.loadSettingsData(true);   // 进设置页时外壳发的那一批
  const old = {backup_count: 1, backups: [], thumbnails: {files: 3, bytes: 10}, package: {path: '/tmp/a.pilib'}};
  answer('/api/settings', old); answer('/api/fold', {kinds: []}); answer('/api/index', {});
  await tick();
  const v = (await view('data'))();
  assert.match(v.text, /共 1 份备份/);
  v.press('backupNow', '立即备份');
  await tick();
  answer('/api/backup', {path: '/tmp/a.pilib/backups/b.sqlite'});
  await tick();
  assert.equal(calls.filter(c => c.path === '/api/settings').length, 2, 'a fresh settings read after the backup');
  answer('/api/settings', {...old, backup_count: 2});
  await tick(8);
  assert.match(v.text, /共 2 份备份/);
  assert.equal(v.button('thumbnails', '清除').dataset.off, undefined);
  answer('/api/names', {});
  await tick();
  v.dispose();
});

test('#785-4 导出：启动时读到 running 就接上轮询；从设置行开始导出，紧接着的读取失败也不丢状态', async () => {
  const lib = await web.load('ui/settings_library.js'), t = clock();
  try {
    libraryState();
    const calls = server(), answer = replier(calls);
    lib.wireLibrary();   // 页面启动
    answer('/api/libraries', LIBS({status: 'running', path: '/tmp/a.zip'}));
    await tick();
    await t.run();
    answer('/api/libraries', LIBS({status: 'done', path: '/tmp/a.zip', bytes: 1048576}));
    await tick();
    assert.equal(state.libraries.export.status, 'done', 'the startup export was followed to the end');
    const v = (await view('library'))();
    assert.match(v.text, /已导出：\/tmp\/a.zip/);
    v.press('manage', '导出当前图库');
    await tick();
    answer('/api/libraries/export', {status: 'running', path: '/tmp/b.zip'});
    await tick();
    assert.match(v.text, /正在导出到 \/tmp\/b.zip/, 'the POST reply is the first state');
    await t.run();
    answer('/api/libraries', {error: '数据库被锁'}, 500);
    await tick();
    assert.equal(state.libraries.export.status, 'running', 'a failed read keeps the last state');
    assert.match(v.text, /正在导出到 \/tmp\/b.zip/);
    await t.run();
    answer('/api/libraries', LIBS({status: 'done', path: '/tmp/b.zip', bytes: 1048576}));
    await tick();
    assert.match(v.text, /已导出：\/tmp\/b.zip/);
    v.dispose();
  } finally { t.restore(); }
});

test('#785-8 导出一直读不到：按截止时间停，约一小时', async () => {
  const t = clock();
  try {
    libraryState(); state.libraries = LIBS();
    const calls = server(), v = (await view('library'))(), start = t.now;
    v.press('manage', '导出当前图库');
    await tick();
    replier(calls)('/api/libraries/export', {status: 'running', path: '/tmp/c.zip'});
    await tick();
    for (let i = 0; i < 5000 && t.pending().length; i++) {
      await t.run();
      for (const c of calls) if (!c.replied && c.path === '/api/libraries') c.reply({error: '数据库被锁'}, 500);
      await tick();
    }
    const reads = calls.filter(c => c.path === '/api/libraries').length, minutes = (t.now - start) / 60e3;
    assert.equal(t.pending().length, 0, 'the poll stopped');
    assert.ok(minutes >= 60 && minutes <= 60.25, `stopped after ${minutes.toFixed(1)} min`);
    assert.ok(reads > 200 && reads < 300, `${reads} reads, 15 s apart once backed off`);
    assert.match(v.text, /正在导出到 \/tmp\/c.zip/, 'the last known state stays');
    v.dispose();
  } finally { t.restore(); }
});

test('#785-6 检查在两次轮询之间做完（idle 直接到 checked）：还缺几个按新的检查重读', async () => {
  const core = web.core();
  const index = checked => ({state: {status: checked ? 'checked' : 'idle'}, works: [], history: [], mb_per_character: 2,
    watch: {works: ['w1'], min_posts: 100}, check: {checked_at: checked ? 't2' : 't1', missing: []}});
  Object.assign(state, {pack: {current: {}, library: {}}, names: null, index: index(false)});
  const calls = server(), answer = replier(calls);
  const first = core.loadCoverage(state.index, true);
  answer('/api/index/coverage', {checked_at: 't1', coverage: {w1: {packs: 1, own: 0, missing: 5}}});
  await first;
  const v = (await view('characters'))();
  assert.match(v.text, /关注的作品还缺 5 个角色/);
  // 改「只提示图片较多的角色」：保存以后读一次状态，这时另一处发起的检查已经做完。
  const min = v.row('min').querySelector('input');
  min.value = '200'; min.dispatchEvent(dom.event('change'));
  await tick();
  answer('/api/index', index(false));
  await tick();
  answer('/api/index?progress=1', {state: {status: 'checked'}});
  await tick();
  answer('/api/index', index(true));
  await tick();
  answer('/api/index/coverage', {checked_at: 't2', coverage: {w1: {packs: 1, own: 0, missing: 2}}});
  await tick(8);
  assert.match(v.text, /关注的作品还缺 2 个角色/);
  v.dispose();
});

test('#785-6 覆盖读取挂着时离开这一节再进来：读到以后「还缺几个」就显示', async () => {
  const index = {state: {status: 'idle'}, works: [], history: [], mb_per_character: 2, watch: {works: ['w1'], min_posts: 100},
    check: {checked_at: 't3', missing: []}};
  Object.assign(state, {pack: {current: {}, library: {}}, names: {state: {status: 'idle'}, network: {}}, index});
  const calls = server(), answer = replier(calls), open = await view('characters');
  let v = open();
  v.load(false);
  await tick();
  answer('/api/packs', state.pack);
  await tick();
  assert.ok(calls.some(c => c.path === '/api/index/coverage' && !c.replied), 'the coverage read is out');
  v.dispose(); state.route = {page: 'settings', section: 'characters'};
  v = open();
  v.load(false);
  await tick();
  answer('/api/packs', state.pack);
  await tick();
  assert.equal(calls.filter(c => c.path === '/api/index/coverage').length, 1, 'the same check is not read twice');
  answer('/api/index/coverage', {checked_at: 't3', coverage: {w1: {packs: 1, own: 0, missing: 3}}});
  await tick(8);
  assert.match(v.text, /关注的作品还缺 3 个角色/);
  v.dispose();
});

test('#785-7 下载计时器和任务面板的读取先发后到：更新状态不从 ready 退回下载中', async () => {
  const core = web.core(), t = clock();
  try {
    for (const id of ['updateButton', 'updateLabel', 'nudge']) { const n = document.createElement('div'); n.setAttribute('id', id); document.body.appendChild(n); }
    const install = stage => ({available: true, stage, version: '1.1', received: stage === 'downloading' ? 1 : 10, total: 10});
    const updates = stage => ({newer: true, latest: '1.1', current: '1.0', checked_at: '2026-11-20T00:00:00Z', auto: true, install: install(stage)});
    Object.assign(state, {about: null, updates: updates('idle')});
    const calls = server(), v = (await view('about'))();
    v.press('updates', '下载并重启');
    await tick();
    calls.find(c => c.path === '/api/updates').reply(updates('downloading'));
    await tick();
    assert.match(v.text, /取消下载/);
    await t.run();   // 下载计时器到点：发出读取 A
    const timerRead = calls.at(-1);
    core.updatesTick();   // 任务面板：读取 B
    await tick();
    const panelRead = calls.at(-1);
    assert.notEqual(timerRead, panelRead);
    panelRead.reply(updates('ready'));
    await tick(8);
    timerRead.reply(updates('downloading'));
    await tick(8);
    assert.equal(state.updates.install.stage, 'ready');
    assert.match(v.text, /重启并安装/);
    assert.doesNotMatch(v.text, /取消下载/);
    assert.equal(t.pending().filter(x => x.ms === 1000).length, 0, 'no download poll is scheduled again');
    v.dispose();
  } finally { t.restore(); }
});
