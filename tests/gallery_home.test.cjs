const {test, before} = require('node:test');
const web = require('./web.cjs');   // core/ 下的模块、ui/grid.js（#199）
let gridModule, daysPage, scopes, frame;
before(async () => {
  await web.ready(); gridModule = await web.load('ui/grid.js'); await web.load('ui/days.js');
  daysPage = (await web.load('core/pages.js')).PAGES.days; scopes = await web.load('ui/scope.js'); frame = await web.load('ui/frame.js');
});
const assert = require('node:assert/strict');
// 网格的几何计算在 ui/grid.js；网格元素隐藏时 renderGrid() 不绘制。
let layouts = 0;
function grid(total = 103, fit = true) {
  const {state} = web.core(), page = web.fakePage();
  Object.assign(page.el('viewport'), {clientWidth: 1002, scrollTop: 0});
  Object.assign(page.el('grid'), {hidden: true, getBoundingClientRect: () => ({left: 10, top: 20})});
  // 每个网格用新的数据版本，布局就不会是前面测试缓存下来的。
  Object.assign(state, {size: 185, total, fit, dataVersion: 1e6 + ++layouts, items: Array.from({length: total}, (_, i) => ({width: [300, 800, 400, 1200][i % 4], height: 600})), route: {page: 'library'}});
  gridModule.viewOpts.captions = true;
  const c = {...gridModule, state, $: page.el}; c.layout(); return c;
}
const rowsOf=c=>Array.from({length:c.rowCount()},(_,r)=>{const out=[];for(let i=c.rowStart(r);i<c.rowStart(r+1);i++)out.push(c.cardRect(i));return out;});
test('justified rows pack around the target height, fill the width and keep each ratio; the last row stays target-sized and left-aligned',()=>{
  const c=grid(), target=c.state.cellW*4/3, rows=rowsOf(c);
  assert.equal(c.rowStart(c.rowCount()),c.state.total);
  rows.forEach((row,r)=>{
    const h=row[0].height-28; let left=0;
    for(const [k,b] of row.entries()){
      const i=c.rowStart(r)+k, item=c.state.items[i];
      assert.equal(b.height,row[0].height); assert.ok(Math.abs(b.width/h-item.width/item.height)<1e-9);
      assert.ok(Math.abs(b.left-left)<1e-8); left=b.left+b.width+c.state.gap;
      assert.equal(c.rowOf(i),r); assert.equal(c.indexAt(b.left+b.width/2+10,b.top+b.height/2+20),i);
    }
    const end=row.at(-1).left+row.at(-1).width;
    if(r<rows.length-1){ assert.ok(Math.abs(end-1000)<1e-8); assert.ok(h>target*.6 && h<target*1.6,`row ${r} height ${h} vs ${target}`); }
    else { assert.ok(Math.abs(h-target)<1e-8); assert.ok(end<1000); }
  });
  assert.ok(rows.some(row=>row.length!==rows[0].length),'row length follows the images, not a fixed column count');
});
test('a tiny result keeps normal size instead of one giant image; extreme ratios are clamped and shown whole',()=>{
  const c=grid(1); c.state.items=[{width:600,height:800}]; c.state.dataVersion++; c.layout();
  const b=c.cardRect(0); assert.ok(Math.abs(b.height-28-c.state.cellW*4/3)<1e-8); assert.ok(b.width<300);
  c.state.total=3; c.state.items=[{width:4000,height:300},{width:100,height:2000},{width:600,height:800}]; c.state.dataVersion++; c.layout();
  const [wide,tall]=[c.cardRect(0),c.cardRect(1)];
  assert.ok(Math.abs(wide.width/(wide.height-28)-2.4)<1e-9); assert.ok(Math.abs(tall.width/(tall.height-28)-.5)<1e-9);
});
test('sparse pagination estimates unloaded images from loaded ones; row lookup stays consistent after dimensions arrive',()=>{
  const c=grid(10000); c.state.items=new Array(10000); c.state.dataVersion++; c.layout();
  const old=c.rowTop(200); assert.ok(old>0 && Number.isFinite(old));
  for(let i=0;i<100;i++) c.state.items[i]={width:1600,height:900};
  c.state.dataVersion++; c.layout();
  assert.notEqual(c.rowTop(200),old); assert.ok(Math.abs(c.imageRatio(undefined)-16/9)<1e-9);
  for(const row of [0,1,199,c.rowCount()-1]) { assert.equal(c.rowAt(c.rowTop(row)+.5),row); assert.equal(c.rowOf(c.rowStart(row)),row); }
  assert.ok(c.rowTop(c.rowCount())<=3e7);
});
test('arrow keys move to the nearest image in the neighbouring justified row',()=>{
  const c=grid(), r1=c.rowStart(1), b=c.cardRect(r1);
  const up=c.verticalStep(r1,-1), u=c.cardRect(up), x=b.left+b.width/2;
  for(let i=0;i<c.rowStart(1);i++){ const o=c.cardRect(i); assert.ok(Math.abs(u.left+u.width/2-x)<=Math.abs(o.left+o.width/2-x)); }
  assert.equal(c.rowOf(c.verticalStep(r1,1)),2); assert.equal(c.verticalStep(0,-1),0);
});
test('square mode and zoom keep predictable geometry; toggling fit anchors the same image row',()=>{
  const c=grid(103,false); const cols=c.state.cols;
  assert.equal(c.cardRect(cols).top,c.state.rowH); assert.equal(c.verticalStep(0,1),cols);
  c.state.size=300;c.layout();assert.ok(c.state.cols<cols);
  const anchor=c.state.cols*3;c.$('viewport').scrollTop=c.rowTop(3);c.toggleFit();
  const row=c.rowAt(c.$('viewport').scrollTop); assert.ok(c.rowStart(row)<=anchor && anchor<c.rowStart(row+1));
  c.toggleFit();assert.equal(c.state.fit,false);
});
test('opening the app lands on the by-day view; the library has its own hash and old query links still open it',()=>{
  // core/route.js 读取页面的 location。
  const {parseHash, hashFor} = web.core(), c = {parseHash, hashFor, location: globalThis.location = {hash: ''}};
  for(const hash of ['','#','#/']){ c.location.hash=hash; assert.equal(c.parseHash().page,'days'); }
  c.location.hash='#/?artist=a1'; assert.deepEqual([c.parseHash().page,c.parseHash().artist],['library','a1']);
  c.location.hash=c.hashFor({page:'library',day:'2026-10-02'}); assert.equal(c.location.hash,'#/library?day=2026-10-02');
  assert.deepEqual([c.parseHash().page,c.parseHash().day],['library','2026-10-02']);
});

// 按天视图（ui/days.js，#707）在假页面上画：帧任务立刻跑，/api/home 的回复由各测试给。
const tick = () => new Promise(r => setImmediate(r));
const oneDay = {day: '2026-10-02', count: 1, people: 0, characters: [], hero: 'a', items: [{uuid: 'a', width: 100, height: 100, memberships: []}]};
function daysHarness(answer) {
  const {state} = web.core(), page = web.fakePage();
  frame.setScheduler(fn => setImmediate(fn));
  Object.assign(state, {route: {page: 'days'}, nsfw: '', focused: null, viewer: -1, guide: undefined, items: [], total: 0});
  globalThis.fetch = async (path, init) => ({status: 200, ok: true, json: async () => answer(path, init)});
  return {state, page, render: () => daysPage.render({samePage: false, scope: scopes.page('days')})};
}
test('undo and a finished sync re-read the by-day view from its own read', async () => {
  // undo() 在 ui/grid.js，同步结束在 ui/effects.js 的 afterJob()；按天视图按自己的读取更新（ui/days.js 的 refresh）。
  const homeReads = [], {edits} = web.core(), {afterJob} = await web.load('ui/effects.js');
  const h = daysHarness(path => path.startsWith('/api/home') ? (homeReads.push(path), {days: [oneDay], next: ''}) : path === '/api/undo' ? {undo: null}
    : path === '/api/sync' ? {state: {status: 'done', imported: 1}, history: [], damaged: null} : path.startsWith('/api/bootstrap') ? {token: '', total: 0, undo: null}
    : path.startsWith('/api/ask') ? {batch: null} : []);
  await h.render();
  assert.equal(homeReads.length, 1);
  assert.deepEqual(h.state.items.map(x => x.uuid), ['a']);
  edits.latest = {id: 7, label: '已将 1 张移出 A'};
  await gridModule.undo(); await tick(); await tick();
  assert.equal(homeReads.length, 2);
  assert.equal(edits.latest, null);
  const {pollSync, onJob} = web.core(), off = onJob(afterJob);
  Object.assign(h.state, {sync: {status: 'scanning'}});
  await pollSync(); await tick(); await tick();
  off();
  assert.ok(homeReads.length >= 3, 'a finished sync re-reads the newest days');
  assert.ok(homeReads.slice(1).every(p => new URL(p, 'http://x').searchParams.get('marks') === 'fav,color'));
  scopes.page('library');
});
test('a by-day answer that arrives after leaving the page draws nothing', async () => {
  let resolve;
  const h = daysHarness(() => ({batch: null}));
  globalThis.fetch = async path => ({status: 200, ok: true, json: () => path.startsWith('/api/home') ? new Promise(r => { resolve = r; }) : Promise.resolve({batch: null})});
  const pending = h.render(); await tick();
  h.state.route = {page: 'library'}; scopes.page('library');   // 换了页：作用域释放，路由对象也换了
  h.state.items = [{uuid: 'library'}];
  resolve({days: [oneDay], next: ''}); await pending;
  assert.deepEqual(h.state.items.map(x => x.uuid), ['library']);
});
// 按天页面里的按钮：dom.cjs 只认 [特性] 和标签，按 data-ref 找。
const refBtn = (page, ref) => [...page.el('homePage').children[0].querySelectorAll('button')].find(b => b.getAttribute('data-ref') === ref && !b.hidden);
test('认一认写入被拒：卡片、点选都留着，不换下一批，可以再点一次（#757）', async () => {
  const asks = [], answers = [];
  const batch = {entity: 'e1', name: 'A', kind: 'hint', items: ['a', 'b'].map(uuid => ({uuid, width: 100, height: 100, memberships: []}))};
  const h = daysHarness(() => ({}));
  globalThis.fetch = async (path, init) => {
    if (path.startsWith('/api/ask/answer')) { answers.push(JSON.parse(init.body)); return {status: 500, ok: false, json: async () => ({error: 'database is locked'})}; }
    const body = path.startsWith('/api/ask') ? (asks.push(path), {batch}) : path.startsWith('/api/home') ? {days: [oneDay], next: ''}
      : path.startsWith('/api/bootstrap') ? {token: '', total: 0, undo: null} : [];
    return {status: 200, ok: true, json: async () => body};
  };
  await h.render(); await tick();
  assert.equal(asks.length, 1);
  refBtn(h.page, 'ok').dispatchEvent({type: 'click'}); await tick(); await tick(); await tick();
  assert.equal(answers.length, 1, '发了一次回答');
  assert.equal(asks.length, 1, '没有去读下一批');
  assert.ok(refBtn(h.page, 'ok'), '卡片还在');
  assert.equal(refBtn(h.page, 'ok').disabled, false, '写完放开了');
  scopes.page('library');
});
test('切换 NSFW 以后，之前那次读取晚到的回复不画回来（#757）', async () => {
  const pending = [];
  const hidden = {...oneDay, day: '2026-10-01', hero: 'h', items: [{uuid: 'h', width: 100, height: 100, memberships: []}]};
  const h = daysHarness(() => ({batch: null}));
  globalThis.fetch = path => path.startsWith('/api/home') ? new Promise(r => pending.push({path, r})) : Promise.resolve({status: 200, ok: true, json: async () => ({batch: null})});
  const first = h.render(); await tick();
  h.state.nsfw = 'hide'; daysPage.refresh('nsfw'); await tick();
  assert.equal(pending.length, 2);
  pending[1].r({status: 200, ok: true, json: async () => ({days: [hidden], next: ''})});
  await tick(); await tick();
  pending[0].r({status: 200, ok: true, json: async () => ({days: [oneDay], next: ''})});
  await first; await tick(); await tick();
  assert.deepEqual(h.state.items.map(x => x.uuid), ['h']);
  h.state.nsfw = '';
  scopes.page('library');
});
test('第一次读的时候来了同步：读完补读最新的几天（#757）', async () => {
  const reads = [];
  let release;
  const h = daysHarness(() => ({batch: null}));
  globalThis.fetch = path => {
    if (!path.startsWith('/api/home')) return Promise.resolve({status: 200, ok: true, json: async () => ({batch: null})});
    reads.push(path);
    const body = {days: [oneDay], next: ''};
    return reads.length === 1 ? new Promise(r => { release = () => r({status: 200, ok: true, json: async () => body}); }) : Promise.resolve({status: 200, ok: true, json: async () => body});
  };
  const first = h.render(); await tick();
  daysPage.refresh('sync');
  assert.equal(reads.length, 1, '读的时候不另发');
  release(); await first; for (let i = 0; i < 6; i++) await tick();
  assert.equal(reads.length, 2, '读完补读一次');
  assert.equal(new URL(reads[1], 'http://x').searchParams.get('days'), '2');
  scopes.page('library');
});
test('同一页又画几次（筛选、搜索变了）：页面只建一次，活着的作用域数不涨（页面样板第 4 条）', async () => {
  const h = daysHarness(path => path.startsWith('/api/home') ? {days: [oneDay], next: ''} : {batch: null});
  scopes.setDev(true);
  const scope = scopes.page('days');
  await daysPage.render({samePage: false, scope});
  const page = h.page.el('homePage').children[0], alive = scopes.live().count;
  for (const q of ['a', 'ab', 'abc']) { h.state.route = {page: 'days', q}; await daysPage.render({samePage: true, scope}); await tick(); }
  assert.equal(h.page.el('homePage').children[0], page, '页面的节点没换');
  assert.equal(scopes.live().count, alive, '作用域数没涨');
  scopes.setDev(false);
  scopes.page('library');
});
