// 页面自己的确认对话框（ui/dialog.js）把守移除来源（ui/settings_libraries.js）和第一次查画师（ui/artists.js）：
// 在同一会话、同一页面里确认之前，什么都不发送（#199）。
const {test, before, afterEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const tick = () => new Promise(resolve => setImmediate(resolve));
let settings, artists, overlay;
before(async () => { await web.ready(); settings = await web.load('ui/settings_libraries.js'); artists = await web.load('ui/artists.js'); overlay = await web.load('ui/overlay.js'); });
let restore = () => {};
afterEach(() => { restore(); restore = () => {}; });

function harness() {
  const dialogs = [], calls = [], consent = new Map(), core = web.core();
  core.session.token = 'synthetic-token'; core.state.route = {page: 'settings'};
  const element = () => ({hidden: true, textContent: '', classList: {toggle() {}}, contains: () => true});
  const nodes = {};
  const handlers = {};
  globalThis.document = {body: {append(d) { dialogs.push(d); }}, getElementById: id => nodes[id] ||= element(),
    addEventListener(type, fn) { (handlers[type] ||= []).push(fn); }, dispatch(type, e) { for (const fn of handlers[type] || []) fn(e); }, createElement() {
    const nodes = {};
    return {returnValue: '', setAttribute() {}, querySelector(selector) {return nodes[selector] ||= {};},
      showModal() {this.open = true;}, addEventListener(type, fn) {if (type === 'close') this.afterClose = fn;},
      close(value = '') {this.returnValue = value; this.open = false; this.onclose(); this.afterClose?.();},
      remove() {this.removed = true;}, getBoundingClientRect: () => ({left:10,right:100,top:10,bottom:100})};
  }};
  // 移除来源先问它有多少张图，再带 confirm: 1 移除。
  globalThis.fetch = async (path, options) => {
    const body = options?.body ? JSON.parse(options.body) : {};
    if (path === '/api/libraries/sources/remove') calls.push({action: 'sources/remove', body});
    if (path === '/api/provenance') calls.push({path, options});
    const answer = path === '/api/libraries/sources/remove' ? (body.confirm ? {removed: 2} : {count: 2}) : {};
    return {status: 200, ok: true, json: async () => answer};
  };
  // 查画师把同意记在 localStorage 里；这里不运行它的进度轮询。
  const {localStorage, setTimeout} = globalThis;
  globalThis.localStorage = {getItem: k => consent.get(k), setItem: (k, v) => consent.set(k, v)};
  globalThis.setTimeout = () => 0;
  restore = () => { globalThis.localStorage = localStorage; globalThis.setTimeout = setTimeout; };
  overlay.resetLayers(); overlay.wireOverlays();
  const c = {session: core.session, state: core.state, findArtists: ids => artists.findArtists(ids), findArtistWorks: artist => artists.findArtistWorks(artist, 'synthetic')};
  const remove = () => settings.removeSource('synthetic-source');
  return {c, dialogs, calls, consent, remove};
}

test('source removal waits with N images visible and submits only after explicit confirmation', async () => {
  const h = harness(), pending = h.remove(); await tick();
  assert.equal(h.calls.length,1);
  const d = h.dialogs[0]; assert.equal(d.open,true);
  assert.match(d.querySelector('p').textContent,/2 张图/);
  d.close('ok'); await pending;
  assert.equal(h.calls.length,2);
  assert.equal(h.calls[1].body.confirm,1); assert.equal(d.removed,true);
});

for (const dismiss of ['cancel','close','escape','backdrop']) test(`${dismiss} leaves source removal unsubmitted`, async () => {
  const h = harness(), pending = h.remove(); await tick(); const d = h.dialogs[0];
  if (dismiss === 'close') d.onclick({target: {closest: selector => selector === '[data-close]' ? {} : null}});
  else if (dismiss === 'backdrop') document.dispatch('click', {target:d,clientX:0,clientY:0});
  else if (dismiss === 'escape') document.dispatch('keydown', {key: 'Escape', target: {}, preventDefault() {}, stopImmediatePropagation() {}});
  else d.close('cancel');
  await pending; assert.equal(h.calls.length,1); assert.equal(d.removed,true);
});

test('artist lookup waits for authorization and cancellation saves no consent or request', async () => {
  const h = harness(); const pending = h.c.findArtists(['synthetic']);
  await tick(); assert.equal(h.calls.length,0); assert.equal(h.consent.size,0);
  h.dialogs[0].close(); await pending;
  assert.equal(h.calls.length,0); assert.equal(h.consent.size,0);
  const accepted = h.c.findArtists(['synthetic']);
  await tick(); assert.equal(h.calls.length,0);
  h.dialogs[1].close('ok'); await accepted;
  assert.equal(h.consent.get('artistLookupOk'),'1'); assert.equal(h.calls.length,1);
  await h.c.findArtistWorks('synthetic');
  assert.equal(h.dialogs.length,2); assert.equal(h.calls.length,2);
});


test('rapid repeated removal has one preview, dialog and commit', async () => {
  const h = harness(), first = h.remove(), second = h.remove(); await tick();
  assert.equal(h.calls.length,1); assert.equal(h.dialogs.length,1);
  h.dialogs[0].close('ok'); await Promise.all([first,second]);
  assert.equal(h.calls.length,2);
});
for (const change of ['token','route']) test(`confirmation after ${change} change cannot submit`, async () => {
  const h = harness(), pending = h.remove(); await tick();
  if (change === 'token') h.c.session.token = 'replacement-session';
  else h.c.state.route = {page:'days'};
  h.dialogs[0].close('ok'); await pending;
  assert.equal(h.calls.length,1);
});
