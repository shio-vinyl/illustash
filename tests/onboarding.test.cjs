// #96：首次使用的引导，在真实界面上教。哪个图库会开始引导、每次改动后清单勾上什么、什么时候问哪个问题。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');   // core/ 和 ui/guide.js（#199）
const plain = v => JSON.parse(JSON.stringify(v));
let guideUi, core;
before(async () => { await web.ready(); guideUi = await web.load('ui/guide.js'); });

// 引导（ui/guide.js）放在假页面上；后端用 `reply` 作答。运行的任务就是运行的同步。
function guide({status = 'active', step = 'welcome', ticked = [], seen = [], hidden = false, results = {}, works, names, sync = {status: 'done'}, route = 'days', index, updates} = {}) {
  const posted = [];
  core = web.core(); core.session.token = 't';
  Object.assign(core.state, {route: {page: route}, sync, names, index, updates, guide: undefined});
  const {el} = web.fakePage();
  globalThis.requestAnimationFrame = () => 0;
  const c = {...guideUi, state: core.state, posted, nodes: new Proxy({}, {get: (_, id) => el(id)}),
    reply: {guide: {status, step, ticked, seen, hidden}, capability: {recognition: true, index: true, anchors: 0, check: {status: 'ok'}},
      results: {total: 0, recognized: 0, with_character: 0, unmatched: 0, characters: 0, ...results}, works}};
  globalThis.fetch = async (path, options) => {
    if (options?.method === 'POST') posted.push([path, JSON.parse(options.body)]);
    return {status: 200, ok: true, json: async () => path.startsWith('/api/onboarding') ? c.reply : {}};
  };
  return c;
}

test('a new library records that its guide started; a library without one stays quiet', async () => {
  const fresh = guide({status: 'new'});
  await fresh.guideOnOpen();
  assert.deepEqual(plain(fresh.posted), [['/api/onboarding', {action: 'step', step: 'welcome'}]]);
  const old = guide({status: 'off'});
  await old.guideOnOpen();
  assert.deepEqual(plain(old.posted), []);
  old.drawGuide();
  assert.equal(old.nodes.checklist.hidden, true);
  assert.equal(old.nodes.guideNotice.hidden, true);
});

test('each kind of change ticks its checklist item once', async () => {
  const c = guide({route: 'groups'});
  await c.loadGuide();
  c.guideAfterPost('/api/change', {action: 'confirmed', ids: ['a']});
  c.state.route = {page: 'library'};
  c.guideAfterPost('/api/change', {action: 'rejected', ids: ['a']});
  c.guideAfterPost('/api/undo', {id: 3});
  c.guideAfterPost('/api/undo', {id: 4});
  c.guideAfterPost('/api/index', {action: 'watch', works: []});
  c.guideAfterPost('/api/index', {action: 'watch', works: ['w1']});
  assert.deepEqual(plain(c.state.guide.guide.ticked), ['group', 'correct', 'undo', 'follow']);
  assert.deepEqual(plain(c.posted.filter(([, b]) => b.action === 'tick').map(([, b]) => b.item)), ['group', 'correct', 'undo', 'follow']);
  c.drawChecklist();
  assert.match(c.nodes.checklist.innerHTML, /4\/5/);
});

test('names are asked once characters show, and not again once answered', async () => {
  const names = {network: {authorized: false}};
  const c = guide({results: {characters: 3, with_character: 4}, names, sync: {status: 'recognising'}});
  await c.loadGuide();
  c.drawNudge();
  assert.match(c.nodes.nudge.innerHTML, /认出了 3 个角色。要联网查它们的中文名吗/);
  const answered = guide({results: {characters: 3}, names, seen: ['names']});
  await answered.loadGuide();
  answered.drawNudge();
  assert.equal(answered.nodes.nudge.innerHTML, '');
});

test('after the first sync the most-pictured followable work is offered, with its Chinese name', async () => {
  const works = [{id: 'w0', tag: '', name: '未核对', count: 9, watchable: false, watched: false},
    {id: null, tag: 'blue_archive', name: 'Blue Archive', count: 5, watchable: true, watched: false}];
  const c = guide({results: {characters: 2, with_character: 7}, names: {network: {granted_at: '2026-10-12'}}, works, index: {watch: {works: []}}});
  await c.loadGuide(true);
  c.drawNudge();
  assert.match(c.nodes.nudge.innerHTML, /你的图里《蔚蓝档案》最多（5 张）/);
  const busy = guide({results: {characters: 2, with_character: 7}, names: {network: {granted_at: 'x'}}, works, sync: {status: 'recognising'}});
  await busy.loadGuide(true);
  busy.drawNudge();
  assert.equal(busy.nodes.nudge.innerHTML, '', 'not while the sync is still running');
});

test('automatic update checks are asked once by the guide, app-wide (#191)', async () => {
  const names = {network: {granted_at: '2026-10-12'}};
  const c = guide({names, updates: {auto: null, newer: false}});
  await c.loadGuide();
  c.drawNudge();
  assert.match(c.nodes.nudge.innerHTML, /要自动检查新版本吗/);
  assert.match(c.nodes.nudge.innerHTML, /不发送图库里的任何信息/);
  assert.match(c.nodes.nudge.innerHTML, /data-nudge="allow"/);
  // 不论怎么回答过（在这里或在另一个图库）：都不再问。
  for (const auto of [true, false]) {
    const answered = guide({names, updates: {auto, newer: false}});
    await answered.loadGuide();
    answered.drawNudge();
    assert.equal(answered.nodes.nudge.innerHTML, '', String(auto));
  }
  // 名称的问题先问；引导已结束的图库从不询问。
  const both = guide({results: {characters: 3}, names: {network: {authorized: false}}, updates: {auto: null}});
  await both.loadGuide();
  both.drawNudge();
  assert.match(both.nodes.nudge.innerHTML, /要联网查它们的中文名吗/);
  const over = guide({status: 'off', updates: {auto: null}});
  await over.loadGuide();
  over.drawNudge();
  assert.equal(over.nodes.nudge.innerHTML, '');
});

test('a newer release is offered outside the guide, but not while a task runs or once dismissed (#191)', async () => {
  const updates = {auto: true, newer: true, dismissed: false, latest: '0.2.0', current: '0.1.0', url: 'https://github.com/shio-vinyl/illustash/releases/tag/v0.2.0'};
  const c = guide({status: 'off', updates});
  await c.loadGuide();
  c.drawNudge();
  assert.match(c.nodes.nudge.innerHTML, /有新版本 0\.2\.0/);
  assert.match(c.nodes.nudge.innerHTML, /现在是 0\.1\.0/);
  assert.match(c.nodes.nudge.innerHTML, /data-nudge="open"/);
  const busy = guide({status: 'off', updates, sync: {status: 'recognising'}});
  await busy.loadGuide();
  busy.drawNudge();
  assert.equal(busy.nodes.nudge.innerHTML, '', 'not while a task runs');
  for (const other of [{dismissed: true}, {newer: false}]) {
    const quiet = guide({status: 'off', updates: {...updates, ...other}});
    await quiet.loadGuide();
    quiet.drawNudge();
    assert.equal(quiet.nodes.nudge.innerHTML, '', JSON.stringify(other));
  }
});

test('the tour speaks of the running sync at the tasks stop', async () => {
  const c = guide({step: 'tasks', sync: {status: 'recognising'}, results: {total: 30}});
  await c.loadGuide();
  assert.ok(c.tourStop('tasks').text.includes(core.syncLine(c.state.sync)));
  assert.match(c.tourStop('welcome').next, /带我看看/);
});

test('after a stopped or failed sync the problem bar says nothing is lost and what to do next (#97)', async () => {
  for (const [sync, expected] of [
    [{status: 'interrupted', message: '任务已取消'}, /同步没有做完（任务已取消）.*再同步一次会接着处理剩下的/s],
    [{status: 'error', message: '文件夹不存在或无法访问：/gone'}, /同步出错了.*\/gone.*外接硬盘/s],
    [{status: 'error', message: 'disk I/O error'}, /诊断信息/],
  ]) {
    const c = guide({sync});
    await c.loadGuide();
    c.drawGuideNotice();
    assert.equal(c.nodes.guideNotice.hidden, false, sync.message);
    assert.match(c.nodes.guideNotice.innerHTML, expected);
    assert.match(c.nodes.guideNotice.innerHTML, /data-guide-sync/);
  }
  // 系统拒绝访问照片图库（#202）：按后端给的代码说明该做什么、去哪里做；再同步一次没有用。
  const refused = guide({sync: {status: 'error', problem: 'photos_permission', message: '[Errno 1] Operation not permitted: \'/合成/Photos.sqlite\''}});
  await refused.loadGuide();
  refused.drawGuideNotice();
  assert.match(refused.nodes.guideNotice.innerHTML, /「系统设置 › 隐私与安全性 › 照片」里打开 Illustash，退出并重新打开后再同步/);
  assert.match(refused.nodes.guideNotice.innerHTML, /data-guide-photos>打开系统设置</);
  assert.doesNotMatch(refused.nodes.guideNotice.innerHTML, /data-guide-sync|Operation not permitted/);
  globalThis.location = {href: ''}; globalThis.addEventListener = () => {};
  refused.wireGuide();
  delete globalThis.addEventListener;
  for (const click of refused.nodes.guideNotice.listeners.click) await click({target: {closest: selector => selector === '[data-guide-photos]' ? {} : null}});
  assert.equal(globalThis.location.href, core.PHOTOS_PRIVACY);
  const fine = guide({sync: {status: 'done'}});
  await fine.loadGuide();
  fine.drawGuideNotice();
  assert.equal(fine.nodes.guideNotice.hidden, true);
});

test('the open checklist is a popover: Esc folds it, the pill toggles it, a link folds it quietly (#398)', async () => {
  const overlay = await web.load('ui/overlay.js');
  const c = guide();
  await c.loadGuide();
  globalThis.addEventListener = () => {};
  c.wireGuide();
  delete globalThis.addEventListener;
  overlay.resetLayers();
  c.drawChecklist();
  const click = cl => c.nodes.checklist.listeners.click[0]({target: {closest: s => s === '[data-cl]' && cl ? {dataset: {cl}} : s === 'a[href]' && !cl ? {} : null}});
  await click('toggle');
  assert.match(c.nodes.checklist.innerHTML, /cl-panel/);
  assert.deepEqual(overlay.layerIds(), ['checklist']);
  assert.equal(overlay.dismissTop('escape'), true);
  assert.doesNotMatch(c.nodes.checklist.innerHTML, /cl-panel/);
  assert.deepEqual(overlay.layerIds(), []);
  await click('toggle');
  await click('toggle');
  assert.doesNotMatch(c.nodes.checklist.innerHTML, /cl-panel/);
  assert.deepEqual(overlay.layerIds(), []);
  await click('toggle');
  await click(null);
  assert.doesNotMatch(c.nodes.checklist.innerHTML, /cl-panel/);
  assert.deepEqual(overlay.layerIds(), []);
});
