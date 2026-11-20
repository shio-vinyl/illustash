// 「找回画师」的轮询（core/jobs.js loadProvenance()，#199）：运行中一次读取失败不能停止轮询；任务面板在任何页面都要看到进度，运行结束就停；进入设置页总是重新读取。
const {test, before, afterEach} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const web = require('./web.cjs');
before(web.ready);
let restore = () => {};
afterEach(() => { restore(); restore = () => {}; });
function harness(answers) {
  const timers = [], {state, loadProvenance} = web.core();
  Object.assign(state, {route: {page: 'settings'}, provenance: null});
  globalThis.fetch = async () => { const a = answers.shift(); if (a instanceof Error) throw a; return {status: 200, ok: true, json: async () => a}; };
  const setTimeout = globalThis.setTimeout;
  globalThis.setTimeout = (fn, ms) => { timers.push(ms); return timers.length; };
  restore = () => { globalThis.setTimeout = setTimeout; };
  return {state, loadProvenance, timers};
}
test('a failed read during a run keeps polling and keeps what was shown', async () => {
  const h = harness([{state: {status: 'running'}}, new Error('busy')]);
  await h.loadProvenance();
  assert.deepEqual(h.timers, [1500]);
  await h.loadProvenance();
  assert.deepEqual(h.timers, [1500, 4000]);
  assert.equal(h.state.provenance.state.status, 'running');
});
test('a run is polled from any page and polling stops once it is over', async () => {
  const h = harness([{state: {status: 'running'}}, {state: {status: 'done'}}]);
  h.state.route.page = 'library';
  await h.loadProvenance();
  assert.deepEqual(h.timers, [1500]);
  await h.loadProvenance();
  assert.deepEqual(h.timers, [1500]);
});
test('entering the settings page always reads again', () => {
  // 「设置 › 画师」每次进来都读一次（ui/settings.js 进这一节时调它的 loadSection）。
  const artists = fs.readFileSync(require('node:path').join(__dirname, '../web/static/ui/settings_artists.js'), 'utf8');
  assert.match(artists, /export function loadSection\(\) \{ loadProvenance\(\); loadAccounts\(\); \}/);
});
