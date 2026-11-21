const {test, before} = require('node:test');
const web = require('./web.cjs');
before(web.ready);
const assert = require('node:assert/strict');
const tick = () => new Promise(r => setImmediate(r));
test('a local search failure reports its cause and never queries remote or offers creation', async () => {
  // twoStage() 在 ui/palette.js：先本地搜索，再查 Danbooru。
  const {twoStage} = await web.load('ui/palette.js');
  let remoteCalls = 0, extraCalls = 0; const frames = [];
  twoStage(async () => { throw new Error('database locked'); }, async () => { remoteCalls++; }, () => { extraCalls++; return [{items: [{create: 'new'}]}]; })('name', f => frames.push(f));
  await tick(); assert.equal(remoteCalls, 0); assert.equal(extraCalls, 0);
  assert.equal(frames[0][0].note, '读取失败：database locked'); assert.equal(frames[0][0].items.length, 0);
});
// 资料库（#199，#711 起页面在 ui/catalog_page.js）：读取回来时已经换了页（页面作用域释放了），失败不写进面板、不弹错误条。
test('a late works failure cannot expose its panel after navigation', async () => {
  let reject; const page = web.fakePage();
  await web.load('ui/catalog_page.js');
  const {PAGES} = await web.load('core/pages.js'), scopes = await web.load('ui/scope.js'), {state, session} = web.core();
  Object.assign(state, {route: {page: 'characters', section: 'works'}, index: undefined}); session.token = 'a';
  globalThis.fetch = () => new Promise((_, r) => { reject = r; });
  scopes.page('characters');
  const pending = PAGES.characters.render({});
  const box = page.el('charIndex').children[0], before = box.textContent;
  scopes.page('settings'); state.route = {page: 'settings'}; reject(new Error('late error')); await pending;
  assert.equal(box.textContent, before);
  assert.ok(!page.el('errorBanner').textContent);
});
test('a late failed watch query cannot overwrite newer local results', async () => {
  const page = web.fakePage();
  const {wireWatchAdd} = await web.load('ui/catalog.js'), {session} = web.core();
  const pending = [], input = page.el('watchAdd'), menu = page.el('watchMenu');
  input.value = 'A'; session.token = 'a';
  globalThis.fetch = () => new Promise((resolve, reject) => pending.push({resolve: body => resolve({status: 200, ok: true, json: async () => body}), reject}));
  // 防抖立刻执行：两次读取都在等回答。
  const setTimeout = globalThis.setTimeout; globalThis.setTimeout = fn => { fn(); return 0; };
  const focus = () => { for (const fn of input.listeners.focus) fn(); };
  try {
    wireWatchAdd({watch: {works: []}}, () => {});
    focus(); input.value = 'B'; focus();
    pending[1].resolve([{id: 'b', name: 'B', tag: 'b', watchable: true}]); await tick(); await tick();
    const current = menu.innerHTML; assert.match(current, />B</);
    pending[0].reject(new Error('old failure')); await tick(); await tick(); assert.equal(menu.innerHTML, current);
  } finally { globalThis.setTimeout = setTimeout; }
});
