// 返回按来路走（#711）：core/route.js 每条历史记着从哪来（cameFrom），core/rail.js 的 backTo 写回到哪里；以及资料库的页签读失败
// 一次以后，再读成功时部件照常画（失败写在页签上面的一行，不整块换掉部件）。数据都是合成的。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let rail, route, state;
before(async () => {
  await web.ready();
  rail = await web.load('core/rail.js'); route = await web.load('core/route.js');
  ({state} = web.core());
});

test('cameFrom：新的一条记上一条的地址，后退、前进、刷新回到的那条沿用记下的，第一条没有来路', () => {
  const stack = []; let at = -1;
  globalThis.location = {hash: ''};
  globalThis.history = {get state() { return stack[at]?.state ?? null; }, replaceState(s, t, url) { stack[at] = {state: s, url}; location.hash = url; }};
  const visit = hash => { stack.splice(at + 1); stack.push({state: null, url: hash}); at++; location.hash = hash; route.noteHistory(); };
  const travel = n => { at += n; location.hash = stack[at].url; route.noteHistory(); };
  visit('#/characters/works');
  assert.equal(route.cameFrom(), '');
  visit('#/w/a');
  assert.equal(route.cameFrom(), '#/characters/works');
  visit('#/library?char=c1');
  assert.equal(route.cameFrom(), '#/w/a');
  travel(-1);
  assert.equal(route.cameFrom(), '#/characters/works', '后退回到作品页，来路还是作品页签');
  travel(1);
  assert.equal(route.cameFrom(), '#/w/a');
  route.noteHistory();   // 刷新：history.state 里还记着
  assert.equal(route.cameFrom(), '#/w/a');
});

test('backTo：作品、角色写名字，资料库的页签写页签名，别的页写入口名；没有来路时用调用方给的', () => {
  Object.assign(state, {chars: [{id: 'c1', name: '合成甲', work_id: 'a', work_name: '合成作品', count: 3}], workTree: [{id: 'a', name: '合成作品'}]});
  state.charMap = new Map(state.chars.map(c => [c.id, c]));
  const fallback = {label: '资料库', href: '#/characters'};
  assert.deepEqual(rail.backTo('', fallback), {label: '资料库', href: '#/characters', step: false});
  const label = hash => rail.backTo(hash, fallback).label;
  assert.equal(label('#/w/a'), '合成作品');
  assert.equal(label('#/library?char=c1'), '合成甲');
  assert.equal(label('#/characters/works'), '作品');
  assert.equal(label('#/characters/artists'), '画师');
  assert.equal(label('#/characters'), '资料库');
  assert.equal(label('#/days'), '图库');
  assert.equal(rail.backTo('#/days', fallback).step, true);
});

test('作品页签读失败一次以后，再读成功照常画出作品卡', async () => {
  const page = web.fakePage();
  await web.load('ui/catalog_page.js');
  const {PAGES} = await web.load('core/pages.js'), scopes = await web.load('ui/scope.js');
  const works = [{id: 'a', name: '合成作品', count: 3, watchable: false}];
  let fail = true;
  globalThis.fetch = path => {
    const url = new URL(path, 'http://x');
    if (url.pathname === '/api/index' && fail) return Promise.resolve({status: 500, ok: false, json: async () => ({error: '合成的失败'})});
    const body = url.pathname === '/api/index' ? {works, watch: {works: [], pending_tags: []}, check: {missing: []}} : url.pathname === '/api/works' ? works : [];
    return Promise.resolve({status: 200, ok: true, json: async () => body});
  };
  Object.assign(state, {route: {page: 'characters', section: 'works'}, index: null, chars: [], charMap: new Map()});
  scopes.page('characters');
  await PAGES.characters.render({});
  const root = page.el('charIndex').children[0];
  assert.equal(root.querySelector('.cat-error').hidden, false, '失败写在页签上面');
  assert.equal(root.querySelectorAll('.cat-work').length, 0);
  fail = false; state.route = {page: 'characters', section: 'works'};
  await PAGES.characters.render({samePage: true});
  assert.equal(root.querySelector('.cat-error').hidden, true, '成功后清掉');
  assert.equal(root.querySelectorAll('.cat-work').length, 1);
});
