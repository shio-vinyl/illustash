// 运行：node --test tests/name_network.test.cjs
const {test, before} = require('node:test');
const web = require('./web.cjs');
const dom = require('./dom.cjs');
const {view, toastNodes} = require('./settings_view.cjs');   // core/ 和 ui/settings.js（#199）
let settings;
before(async () => { await web.ready(); settings = await web.load('ui/settings_network.js'); });
const assert = require('node:assert/strict');
// 查询在 core/settings.js（remoteNames、remoteNote）；征求同意是 ui/settings_network.js 的 setNamesNetwork()，
// 用页面自己的对话框（ui/dialog.js）和提示条（ui/dom.js），这里放在假页面上。
function harness({fail, confirm = true} = {}) {
  const calls = [], toasts = [], dialogs = [], core = web.core(), nodes = {};
  core.state.names = {counts: {}}; core.state.route = {page: 'library'};
  globalThis.fetch = async (path, options) => {
    calls.push([path, options?.body]);
    if (fail) return {status: 400, ok: false, json: async () => ({error: fail})};
    return {status: 200, ok: true, json: async () => path === '/api/names' ? {network: {authorized: JSON.parse(options.body).allow}} : [{tag: 'synthetic'}]};
  };
  const element = () => ({hidden: true, set textContent(v) { if (this === nodes.toastText) toasts.push(v); }});
  globalThis.document = {getElementById: id => nodes[id] ||= element(), body: {append(d) {
    dialogs.push(d.querySelector('h2').textContent);
    setImmediate(() => d.close(confirm ? 'ok' : ''));   // 用户作答
  }}, createElement() {
    const parts = {};
    return {returnValue: '', setAttribute() {}, querySelector: selector => parts[selector] ||= {}, showModal() {}, addEventListener() {}, remove() {},
      close(value = '') { this.returnValue = value; this.onclose(); }};
  }};
  const setTimeout = globalThis.setTimeout; globalThis.setTimeout = () => 0;
  const context = {state: core.state, remoteNames: core.remoteNames, remoteNote: core.remoteNote,
    setNamesNetwork: async allow => { try { return await settings.setNamesNetwork(allow); } finally { globalThis.setTimeout = setTimeout; } }};
  return {context, calls, toasts, dialogs};
}

test('a refusal for want of consent is told apart from a real failure', async () => {
  let {context} = harness({fail: '显示名联网尚未授权'});
  let result = await context.remoteNames('/api/entities?q=x&remote=1');
  assert.equal(result.found, null);
  assert.equal(result.unauthorized, true);
  assert.equal(context.remoteNote(result), '查 Danbooru 需要先允许这个图库联网查询名称（只发送文字）');
  ({context} = harness({fail: '请求超时'}));
  result = await context.remoteNames('/api/entities?q=x&remote=1');
  assert.equal(result.unauthorized, false);
  assert.equal(context.remoteNote(result), 'Danbooru 暂时连不上：请求超时');
  ({context} = harness());
  assert.deepEqual((await context.remoteNames('/api/works?q=x&remote=1')).found, [{tag: 'synthetic'}]);
});

test('granting asks first and sends nothing when declined; revoking does not ask', async () => {
  let h = harness({confirm: false});
  assert.equal(await h.context.setNamesNetwork(true), false);
  assert.deepEqual(h.calls, []);
  assert.deepEqual(h.dialogs, ['允许联网查询名称']);
  h = harness();
  assert.equal(await h.context.setNamesNetwork(true), true);
  assert.deepEqual(h.calls, [['/api/names', '{"action":"network","allow":true}']]);
  assert.equal(h.context.state.names.network.authorized, true);
  assert.deepEqual(h.context.state.names.counts, {}, 'other name state is kept');
  h = harness();
  assert.equal(await h.context.setNamesNetwork(false), true);
  assert.deepEqual(h.dialogs, []);
  assert.deepEqual(h.calls, [['/api/names', '{"action":"network","allow":false}']]);
  assert.deepEqual(h.toasts, ['已撤销名称联网查询']);
});

// 名称翻译那一组在 ui/settings_characters.js；保存经过 core/settings.js 的 saveModelApi()。
async function modelCard(names, fetch) {
  const {state} = web.core();
  dom.install();
  const toastText = toastNodes(), open = await view('characters');
  Object.assign(state, {route: {page: 'library'}, names, pack: null, index: null});
  globalThis.fetch = fetch;
  const v = open();
  const press = async label => { v.press('modelSave', label); for (let i = 0; i < 3; i++) await new Promise(resolve => setImmediate(resolve)); };
  const type = (key, value) => { const input = v.row(key).querySelector('input'); input.value = value; input.dispatchEvent(dom.event('change')); };
  return {state, v, press, type, toast: () => toastText.textContent, done: () => { v.dispose(); dom.uninstall(); }};
}

test('saving or clearing model settings keeps the grant and revoke control available', async () => {
  for (const authorized of [true, false]) {
    const network = {authorized, this_run: false, granted_at: authorized ? 'synthetic-time' : null}, calls = [];
    const c = await modelCard({network, model_api: {configured: true}}, async (path, options) => {
      const body = JSON.parse(options.body); calls.push([path, body]);
      return {status: 200, ok: true, json: async () => ({model_api: {configured: body.key !== null}, counts: {total: 2}})};
    });
    await c.press('保存');
    assert.equal(c.state.names.network, network);
    c.v.update();
    await c.press('清除 key');
    assert.equal(c.state.names.network, network);
    assert.equal(c.state.names.model_api.configured, false);
    assert.equal(calls.length, 2);
    assert.ok(calls.every(([path, body]) => path === '/api/names' && body.action === 'model_api'));
    c.done();
  }
});

test('a model credential error keeps the settings save and clear controls available', async () => {
  // 系统凭据存储读不出 key（#780）：「名称翻译」那一组的状态写这个原因，可以清除 key、重新保存，不给测试连接。
  const credentialError = '模型 API key 尚未迁移，请重新打开图库或保存模型设置', calls = [];
  const c = await modelCard({network: {}, model_api: {configured: false, credential_error: credentialError}}, async (path, options) => {
    calls.push([path, JSON.parse(options.body)]);
    return {status: 200, ok: true, json: async () => ({model_api: {configured: false}})};
  });
  assert.ok(c.v.row('modelSave').textContent.includes(credentialError));
  const labels = c.v.buttons('modelSave').map(b => b.textContent);
  assert.deepEqual(labels, ['清除 key', '保存']);
  assert.ok(c.v.buttons('modelSave').every(b => !b.disabled));
  await c.press('清除 key');
  assert.deepEqual(calls, [['/api/names', {action: 'model_api', key: null}]]);
  c.done();
});

test('saving a new model address that drops the key says the key must be entered again', async () => {
  // 后端只为填写 key 时的那个地址保留 key（app_names.rs 的 configure）。
  for (const [kept, said] of [[true, '设置已保存'], [false, '接口地址已更改，原来的 API key 已清除，请重新填写']]) {
    const c = await modelCard({network: {}, model_api: {configured: true, base: 'https://api.deepseek.com'}},
      async () => ({status: 200, ok: true, json: async () => ({model_api: {configured: kept}})}));
    c.type('modelBase', 'https://synthetic.invalid');
    await c.press('保存');
    assert.equal(c.toast(), said);
    c.done();
  }
});

test('refreshing names from the task panel keeps the network status', async () => {
  // 面板上的按钮运行 core/jobs.js 的 refreshNames()。
  const network = {authorized: true, this_run: false, granted_at: 'synthetic-time'}, {state, refreshNames} = web.core();
  state.names = {network, state: {status: 'idle'}};
  globalThis.fetch = async (path, options) => {
    assert.equal(path, '/api/names'); assert.equal(JSON.parse(options.body).action, 'refresh');
    return {status: 200, ok: true, json: async () => ({state: {status: 'fetching'}})};
  };
  await refreshNames();
  assert.equal(state.names.network, network);
  assert.equal(state.names.state.status, 'fetching');
});

test('names waiting for consent are not shown as a failure', () => {
  const {namesLine} = web.core();
  const line = namesLine({status: 'unauthorized'});
  assert.doesNotMatch(line, /失败/);
  assert.match(line, /网络与隐私/);
  assert.equal(namesLine({status: 'error'}), '获取失败，将在下次启动时重试');
});
