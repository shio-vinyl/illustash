// 角色页的横幅（#712，ui/character.js）：照页面样板走一遍。同一页画几次横幅只建一次、活着的作用域数不涨；同一页上几次读取
// 不分先后回来时只认最后一次（切 NSFW、换造型途中）；离开以后网格头部清空并藏起来。角色和图片都是合成数据。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');

let state, pages, routeCore, S, shell, page;
const tick = () => new Promise(r => setTimeout(r, 0));
const pending = [];   // 横幅自己那一次 /api/assets（limit=24）：测试决定什么时候、按什么顺序回来
const reply = body => ({ok: true, status: 200, json: async () => body});
function route(path) {
  const url = new URL(path, 'http://x'), q = url.searchParams;
  if (url.pathname === '/api/characters') {
    const id = q.get('id'), opts = id === 'group' ? {group_tag: true, label_only: true, unchecked: 3}
      : id === 'original' ? {kind: 'oc', label_only: true, unchecked: 3} : {};
    return reply([{id, name: '合成甲', count: 5, unchecked: 0, variants: [{id: 'v1', name: '合成造型', count: 2}], ...opts}]);
  }
  if (url.pathname === '/api/assets' && q.get('limit') === '24') return new Promise(resolve => pending.push({nsfw: q.get('nsfw') || '', resolve: uuid => resolve(reply({items: [{uuid, width: 1600, height: 900, color: '#336699'}]}))}));
  if (url.pathname === '/api/assets') return reply({items: [], total: 0});
  if (url.pathname === '/api/assets/count') return reply({total: 0});
  if (url.pathname === '/api/character/stats') return reply({count: 5, months: [{month: '2026-11', count: 5}]});
  if (url.pathname === '/api/character/together' || url.pathname === '/api/character/activity') return reply(url.pathname.endsWith('together') ? [] : {});
  return reply({});
}

test.before(async () => {
  await web.ready();
  page = web.fakePage();
  globalThis.location = {hash: ''};
  globalThis.history = {state: null, replaceState() {}};
  globalThis.window = {addEventListener() {}};
  globalThis.requestAnimationFrame = () => 0;
  globalThis.IntersectionObserver = class { observe() {} disconnect() {} };
  globalThis.fetch = async path => route(String(path));
  // 网格头部用一个真的元素：横幅是模板建的真节点，假元素的 replaceChildren 摘不掉它们。
  const realHead = page.document.createDocumentFragment().ownerDocument.createElement('div'), byId = page.document.getElementById;
  page.document.getElementById = id => id === 'gridHead' ? realHead : byId(id);
  ({state} = web.core());
  pages = await web.load('core/pages.js');
  routeCore = await web.load('core/route.js');
  S = await web.load('ui/scope.js');
  shell = await web.load('ui/shell.js');
  await web.load('ui/character.js');
});

const enter = async hash => { globalThis.location.hash = hash; await shell.applyRoute(); await tick(); };
const art = () => page.el('gridHead').firstElementChild?.querySelector('.char-hero-art').querySelector('img')?.getAttribute('src') || '';
const hidden = el => el.hasAttribute('hidden');

test('同一页画几次，横幅只建一次，活着的作用域数不涨；切 NSFW 时先发的读取晚回来不画', async () => {
  S.setDev(true);
  state.nsfw = '';
  await enter('#/library?char=c1');
  const head = page.el('gridHead'), first = head.firstElementChild, count = S.live().count;
  assert.ok(first, '横幅放进了网格头部');
  assert.equal(hidden(head), false);
  for (const hash of ['#/library?char=c1&variant=v1', '#/library?char=c1&check=1', '#/library?char=c1']) await enter(hash);
  assert.equal(head.firstElementChild, first, '横幅只建一次');
  assert.equal(S.live().count, count, '活着的作用域数不涨');
  // 进页那次读取还没回来就切到「隐藏 NSFW」：新的读取先回来，旧的后回来，横幅停在新的那张上。
  state.nsfw = 'hide';
  pages.CHARACTER.gridHead({samePage: true, scope: S.currentPage()});
  const [old, next] = [pending.find(p => p.nsfw === ''), pending.find(p => p.nsfw === 'hide')];
  next.resolve('new'); await tick(); await tick();
  old.resolve('old'); await tick(); await tick();
  assert.match(art(), /id=new/);
  S.setDev(false);
});

test('换造型途中（state.char 暂时是 null）回来的读取照样画上', async () => {
  pending.length = 0; state.nsfw = '';
  await enter('#/library?char=c2');
  const mine = pending.at(-1);
  state.char = null;   // applyRoute 换造型时先 setChar(null)，再读角色
  mine.resolve('kept'); await tick(); await tick();
  assert.match(art(), /id=kept/);
});

test('团体页只当标签，共用原创角色保留角色操作', async () => {
  await enter('#/library?char=group');
  const head = page.el('gridHead'), ref = key => head.querySelector(`[data-ref="${key}"]`);
  assert.equal(hidden(ref('badge')), false);
  assert.equal(hidden(ref('sweep')), true);
  assert.equal(hidden(ref('checkBox')), true);
  assert.equal(routeCore.browsing(), true);
  assert.equal(routeCore.cleanup(), false);
  await enter('#/library?char=original');
  assert.equal(routeCore.browsing(), false);
  assert.equal(routeCore.cleanup(), true);
  assert.equal(hidden(ref('badge')), true);
  assert.equal(hidden(ref('sweep')), false);
  assert.equal(hidden(ref('checkBox')), false);
});

test('离开角色页：网格头部清空并藏起来，页头的标题不再藏着', async () => {
  await enter('#/library');
  const head = page.el('gridHead');
  assert.equal(head.firstElementChild, null);
  assert.equal(hidden(head), true);
});
