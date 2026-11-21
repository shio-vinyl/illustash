// #88：导入归档或识别结果作为后台任务运行。图库卡片显示它进行到哪一步（只用真实计数）、运行时的取消按钮，
// 以及结束的方式；迟到或乱序的响应（#34）不会让它倒回去，结束只通知一次。
const {test, before, afterEach} = require('node:test');
const web = require('./web.cjs');   // 导入的轮询和文案在 core/jobs.js（#199）
const dom = require('./dom.cjs');
const {view} = require('./settings_view.cjs');
let settings, effects, open;
before(async () => { await web.ready(); settings = await web.load('ui/settings_libraries.js'); effects = await web.load('ui/effects.js'); open = await view('library'); });
let cleanup = () => {};
afterEach(() => { cleanup(); cleanup = () => {}; });
const assert = require('node:assert/strict');

// 图库那一组在 ui/settings_libraries.js；导入的文案和状态在 core/jobs.js。返回这一组的文字、导入那一行的按钮，以及「导入…」
// 能不能点。
function card(importTask) {
  const {state, importLine} = web.core();
  Object.assign(state, {route: {page: 'settings'}, importTask, libraries: {
    info: {name: '合成图库', path: '/tmp/synthetic.pilib', sources: [], test: null},
    current: '/tmp/synthetic.pilib', home: '/tmp', libraries: [], export: {status: 'idle'}}});
  dom.install();
  const v = open(), row = v.row('import');
  const out = {text: v.text, shown: !row.hidden, buttons: row.hidden ? [] : v.buttons('import').map(b => b.textContent),
    zipOff: v.button('manage', '导入…').disabled, line: importLine};
  v.dispose(); dom.uninstall();
  return out;
}
const running = (extra = {}) => ({status: 'running', kind: 'archive', id: 't1', file: '合成.zip', ...extra});

test('a running import shows its stage with real counts and a cancel button, and blocks another import', () => {
  const {text, buttons, zipOff, line} = card(running({stage: 'unpacking', done: 3e6, total: 256e6}));
  assert.match(text, /导入归档「合成\.zip」：正在解压 3\.0\/256 MB…/);
  assert.deepEqual(buttons, ['取消导入']);
  assert.equal(zipOff, true);
  assert.equal(line(running()), '导入归档「合成.zip」：正在准备…', 'no stage yet: no invented progress');
  assert.equal(line(running({stage: 'checking'})), '导入归档「合成.zip」：正在检查新图库能否打开…');
  const recognition = {status: 'running', kind: 'recognition', id: 't2', file: 'result'};
  assert.equal(line({...recognition, stage: 'models'}), '导入识别结果「result」：正在核对模型指纹…');
  assert.equal(line({...recognition, stage: 'reading', done: 1, total: 3}), '导入识别结果「result」：正在读取结果包（第 2/3 个文件）…');
  assert.equal(line({...recognition, stage: 'writing', done: 40, total: 120}), '导入识别结果「result」：正在写入识别结果 40/120 张…');
});

test('a stopping or finished import has no cancel button and says how it ended', () => {
  for (const [task, text] of [
    [{...running(), status: 'stopping', stage: 'unpacking'}, '正在取消导入归档「合成.zip」…'],
    [{...running(), status: 'cancelled', reason: '图库已切换，导入已取消'}, '图库已切换，导入已取消；图库没有改动。'],
    [{...running(), status: 'error', error: '导入的图库无法打开，已取消导入', code: 400}, '导入归档「合成.zip」失败：导入的图库无法打开，已取消导入'],
    [{...running(), status: 'done', result: {name: '合成', path: '/tmp/合成.pilib'}}, '已导入「合成」，模型接口与名称联网授权需重新设置。'],
    [{status: 'done', kind: 'recognition', id: 't2', result: {status: 'done', duplicate: true}}, '这份识别结果之前已经导入过。'],
    [{status: 'done', kind: 'recognition', id: 't2', result: {status: 'pending', error: '合成'}}, '识别结果已导入，后续处理尚未完成（合成）。'],
  ]) {
    const c = card(task);
    assert.ok(c.text.includes(text), `${task.status}: ${c.text}`);
    assert.ok(!c.buttons.includes('取消导入'));
    // 正在停止的导入仍占着位置，它结束前不能开始新的导入。
    assert.equal(c.zipOff, task.status === 'stopping');
  }
  assert.equal(card({status: 'idle'}).shown, false);
  assert.deepEqual(card({...running(), status: 'done', result: {name: '合成', path: '/tmp/合成.pilib'}}).buttons, []);
  assert.equal(card(undefined).shown, false);
});

// core/jobs.js 负责轮询；图库那一组（ui/settings_libraries.js）开始导入；ui/effects.js 的 afterJob()（接法与 app.js 相同）
// 通知结束并列出新图库。页面是假的；计数的是导入自己的定时器。
function poller(token = 'session') {
  const answers = [], timers = [], toasts = [], posted = [], offs = [];
  let libraries = 0;
  const core = web.core(), {state, session, onJob} = core;
  session.token = token;
  Object.assign(state, {route: {page: 'settings'}, importTask: undefined});
  const page = web.fakePage();
  Object.defineProperty(page.el('toastText'), 'textContent', {set: m => toasts.push(m), get: () => toasts.at(-1) || ''});
  globalThis.fetch = (path, options) => {
    if (path === '/api/imports/cancel') { posted.push(options); return Promise.resolve({status: 200, ok: true, json: async () => ({status: 'stopping'})}); }
    if (path === '/api/libraries/import') return Promise.resolve({status: 200, ok: true, json: async () => ({started: true, task: running()})});
    if (path === '/api/libraries') { libraries++; return Promise.resolve({status: 200, ok: true, json: async () => ({})}); }
    return new Promise(resolve => answers.push(body => resolve({status: 200, ok: true, json: async () => body})));
  };
  const setTimeout = globalThis.setTimeout;
  globalThis.setTimeout = (fn, ms) => { if (ms === 700 || ms === 5000) timers.push(ms); return timers.length; };
  offs.push(onJob(effects.afterJob));
  const c = {state, session, pollImports: core.pollImports, cancelImport: core.cancelImport, startImport: settings.startImport};
  cleanup = () => { globalThis.setTimeout = setTimeout; for (const off of offs) off(); };
  return {c, answers, timers, toasts, posted, libraries: () => libraries};
}
test('starting polls until the end; an older answer arriving last does not wind the state back', async () => {
  const p = poller();
  await p.c.startImport('/tmp/合成.zip');
  assert.equal(p.c.state.importTask.status, 'running');
  const older = p.c.pollImports();  // 第二次轮询与第一次重叠
  p.answers[1]({...running(), status: 'done', result: {name: '合成'}});
  await older;
  p.answers[0](running({stage: 'unpacking', done: 1, total: 2}));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(p.c.state.importTask.status, 'done');
  assert.deepEqual(p.toasts, ['已导入「合成」，模型接口与名称联网授权需重新设置。']);
  assert.equal(p.libraries(), 1, 'the new library is listed once it is registered');
  assert.deepEqual(p.timers, [], 'finished: nothing polls again, and the stale answer schedules nothing');
});

test('a running answer schedules the next poll and the end is announced only once', async () => {
  const p = poller();
  p.c.state.importTask = running();
  const first = p.c.pollImports();
  p.answers[0](running({stage: 'unpacking', done: 1, total: 2}));
  await first;
  assert.deepEqual(p.timers, [700]);
  const second = p.c.pollImports();
  p.answers[1]({...running(), status: 'cancelled', reason: '导入已取消'});
  await second;
  const third = p.c.pollImports();
  p.answers[2]({...running(), status: 'cancelled', reason: '导入已取消'});
  await third;
  assert.deepEqual(p.toasts, ['导入已取消；图库没有改动。']);
});

test('an answer for a page whose session changed meanwhile is dropped', async () => {
  const p = poller();
  p.c.state.importTask = running();
  const pending = p.c.pollImports();
  p.c.session.token = 'replacement';
  p.answers[0]({...running(), status: 'done', result: {name: '合成'}});
  await pending;
  assert.equal(p.c.state.importTask.status, 'running');
  assert.deepEqual(p.toasts, []);
});

test('the cancel button asks the server and then polls for the end', async () => {
  const p = poller();
  p.c.state.importTask = running();
  const cancelling = p.c.cancelImport();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(p.posted.length, 1);
  assert.equal(p.posted[0].method, 'POST');
  p.answers[0]({...running(), status: 'cancelled', reason: '导入已取消'});
  await cancelling;
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(p.c.state.importTask.status, 'cancelled');
});

test('a published import that could not be listed is offered to open instead of importing again (#111)', () => {
  const task = {...running(), status: 'done', result: {name: '合成', path: '/Users/me/Pictures/Illustash/合成 2.pilib', listed: false, error: '写入失败（合成）'}};
  const {buttons, zipOff, line} = card(task);
  assert.equal(line(task), '已导入「合成」，保存在 ~/Pictures/Illustash/合成 2.pilib，但没能加入最近图库列表：写入失败（合成）。打开它会再试一次加入列表；再导入会另建一个带编号的图库。模型接口与名称联网授权需重新设置。');
  assert.deepEqual(buttons, ['打开这个图库']);
  assert.equal(zipOff, false);
});

test('a Windows library path shortens the user folder too (#518)', () => {
  const task = {...running(), status: 'done', result: {name: '合成', path: 'C:\\Users\\合成用户\\Pictures\\Illustash\\合成.pilib', listed: false, error: '写入失败'}};
  assert.match(card(task).line(task), /保存在 ~\\Pictures\\Illustash\\合成\.pilib，/);
});
