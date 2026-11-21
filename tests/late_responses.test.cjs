// #34：页面已经切走之后才到、或乱序到达的响应，不会让界面倒回去；
// 设置卡片在后台刷新时保留正在输入的内容。
const {test, before} = require('node:test');
const web = require('./web.cjs');   // core/ 下的模块（#199）
before(web.ready);
const assert = require('node:assert/strict');
const tick = () => new Promise(resolve => setImmediate(resolve));
const element = () => ({hidden: false, classList: {remove() {}, toggle() {}}, listeners: {},
  addEventListener(kind, fn) { this.listeners[kind] = fn; }});

test('a group page answer that arrives after navigating home does not take over the page', async () => {
  // applyRoute() 在 ui/shell.js；首页在 ui/home.js；组页进入前的读取在 ui/review.js；页面是假的。
  const requests = [], {applyRoute} = await web.load('ui/shell.js'), {state} = web.core();
  await web.load('ui/review.js');
  let answer;
  Object.assign(state, {route: {page: 'home'}, nsfw: '', guide: undefined});
  globalThis.location = {hash: '#/g/3'}; globalThis.history = {replaceState() {}};
  globalThis.requestAnimationFrame ??= () => 0;
  const page = web.fakePage(), el = page.el;
  globalThis.fetch = path => {
    requests.push(path);
    if (path.startsWith('/api/clusters')) return new Promise(resolve => { answer = body => resolve({status: 200, ok: true, json: async () => body}); });
    return Promise.resolve({status: 200, ok: true, json: async () => path.startsWith('/api/home') ? {days: [], next: ''} : []});
  };
  const toGroup = applyRoute();
  await tick();
  location.hash = '#/home';
  await applyRoute();
  assert.equal(el('homePage').hidden, false);
  answer({clusters: [{cid: 3, near: 1, traits: ['old']}]});
  await toGroup;
  assert.equal(el('homePage').hidden, false);
  assert.equal(state.groupTraits, null);   // 初值（core/state.js 声明为 null），迟到的回应没有写进来
  assert.match(el('heading').innerHTML, /首页/);
  assert.ok(requests.some(p => p.startsWith('/api/home')));
  assert.ok(!requests.some(p => p.startsWith('/api/assets')), 'the group page never reads its pictures');
});

test('a recognition batch refreshes an already imported folder while sync is running', async () => {
  const {state, pollSync, onJob} = web.core(), changes = [];
  state.sync = {status: 'recognising', imported: 5, import_done: 0};
  const reply = {state: {status: 'recognising', imported: 5, import_done: 1}, history: []};
  const fetch = globalThis.fetch, setTimeout = globalThis.setTimeout;
  globalThis.fetch = async () => ({status: 200, ok: true, json: async () => reply});
  globalThis.setTimeout = () => 0;
  const off = onJob((job, change) => { if (job === 'sync') changes.push([change.arrived, change.ended]); });
  try {
    await pollSync();
    await pollSync();
  } finally {
    off();
    globalThis.fetch = fetch;
    globalThis.setTimeout = setTimeout;
  }
  assert.deepEqual(changes, [[true, false], [false, false]]);
});

test('an older sync answer that arrives last does not move the state back', async () => {
  // core/jobs.js 轮询同步；它的定时器是页面的 setTimeout。
  const {state, pollSync} = web.core(), answers = [], timers = [];
  Object.assign(state, {sync: {status: 'scanning'}, route: {page: 'library'}});
  globalThis.fetch = () => new Promise(resolve => answers.push(body => resolve({status: 200, ok: true, json: async () => body})));
  const setTimeout = globalThis.setTimeout;
  globalThis.setTimeout = (fn, ms) => { timers.push(ms); return 0; };
  try {
    const older = pollSync(), newer = pollSync();
    answers[1]({state: {status: 'done', imported: 2}, history: [{id: 2}]});
    await newer;
    answers[0]({state: {status: 'scanning'}, history: [{id: 1}]});
    await older;
  } finally { globalThis.setTimeout = setTimeout; }
  assert.equal(state.sync.status, 'done');
  assert.deepEqual(state.syncHistory, [{id: 2}]);
  assert.deepEqual(timers, []);  // 已结束：不再轮询，过期的响应也不安排任何定时器
});

test('the new-library name is kept as it is typed, so a card refresh restores it', async () => {
  const {onLibraryInput, libForm} = await web.load('ui/settings.js');
  onLibraryInput({target: {id: 'libName', value: '合成新图库'}});
  onLibraryInput({target: {id: 'libPhotos', checked: true}});
  assert.equal(libForm.name, '合成新图库');
  assert.equal(libForm.photos, true);
});
