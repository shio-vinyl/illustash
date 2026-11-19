const {test, before} = require('node:test');
const web = require('./web.cjs');   // core/ 下的模块、ui/grid.js（#199）
let gridModule, homeModule;
before(async () => { await web.ready(); gridModule = await web.load('ui/grid.js'); homeModule = await web.load('ui/home.js'); });
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
// 首页在 ui/home.js；它的状态是 home，包括 observer；页面是假的。
function homeHarness(api){
  const {home, moreHome} = homeModule, {state} = web.core(), page = web.fakePage(), el = page.document.getElementById;
  Object.assign(home, {days: [{day: '2026-10-02', items: [{uuid: 'old', width: 100, height: 100, memberships: []}], characters: [], count: 1}], next: '2026-10-02', gen: 1, loading: false, width: 1000});
  // 读到更早的几天以后，首页按天比对（#452）：已有的块留着，新的几天接在后面，哨兵在最后。
  const sentinel = {textContent: ''};
  home.observer = {observes: 0, disconnects: 0, unobserve() {}, observe() { this.observes++; }, disconnect() { this.disconnects++; }};
  Object.assign(state, {route: {page: 'days'}, nsfw: '', items: [{uuid: 'old'}], total: 1});
  page.document.getElementById = id => id === 'homeSentinel' ? sentinel : el(id);
  globalThis.fetch = path => Promise.resolve().then(() => api(path)).then(body => ({status: 200, ok: true, json: async () => body}));
  return {c: {moreHome, state}, home, sentinel, observer: home.observer, blocks: () => el('homePage').children};
}
const keys = h => h.blocks().map(b => b._sectionKey);
test('home appends only new dates and items; repeated concurrent clicks share the request',async()=>{
  let resolve,calls=0; const h=homeHarness(()=>{calls++;return new Promise(r=>resolve=r);});
  const pending=h.c.moreHome(); await h.c.moreHome(); await new Promise(r => setImmediate(r)); assert.equal(calls,1);
  resolve({days:[{day:'2026-10-02',items:[],characters:[]},{day:'2026-10-01',items:[{uuid:'new', width: 100, height: 100, memberships: []}],characters:[],count:1},{day:'2026-10-01',items:[],characters:[]}],next:'2026-10-01'});await pending;
  assert.deepEqual(keys(h),['2026-10-02','2026-10-01','more']);assert.match(h.blocks()[1].html,/data-day="2026-10-01"/);assert.equal(h.home.days.length,2);assert.equal(h.c.state.items.length,2);assert.equal(h.observer.observes,1);
});
test('home errors release loading without resubscribing and permit an explicit retry',async()=>{
  let calls=0;const h=homeHarness(async()=>{calls++;if(calls===1)throw Error('failed');return {days:[],next:''};});
  await h.c.moreHome();assert.equal(h.home.loading,false);assert.equal(h.observer.observes,0);
  await h.c.moreHome();assert.equal(calls,2);assert.match(h.blocks().at(-1).html,/已经到底了/);
});
test('home responses after page changes are discarded',async()=>{
  let resolve;const h=homeHarness(()=>new Promise(r=>resolve=r));const pending=h.c.moreHome(); await new Promise(r => setImmediate(r));
  h.c.state.route.page='library';resolve({days:[{day:'2026-10-01',items:[]}],next:''});await pending;
  assert.equal(h.home.days.length,1);assert.deepEqual(keys(h),[]);assert.equal(h.observer.observes,0);
});
test('an older home request cannot unlock or append into a new home generation',async()=>{
  let resolve;const h=homeHarness(()=>new Promise(r=>resolve=r));const pending=h.c.moreHome(); await new Promise(r => setImmediate(r));
  h.home.gen++;h.home.loading=true;resolve({days:[],next:''});await pending;
  assert.equal(h.home.loading,true);assert.equal(h.home.next,'2026-10-02');
});

test('within-day requests after navigation cannot change the library or remove a newer generation lock', async()=>{
  let resolve;
  const {home, moreOfDay, dayLoading} = homeModule, {state} = web.core(), page = web.fakePage();
  const day={day:'2026-10-02',items:[{uuid:'old'}],count:10};
  Object.assign(home, {days: [day], gen: 1}); Object.assign(state, {route: {page: 'days'}, nsfw: '', items: [{uuid: 'library'}], total: 1});
  globalThis.fetch = () => new Promise(r => resolve = body => r({status: 200, ok: true, json: async () => body}));
  page.document.getElementById = () => { throw Error('stale response must not query DOM'); };
  const pending=moreOfDay(day.day); await new Promise(r => setImmediate(r)); state.route.page='library';resolve({items:[{uuid:'new'}]});await pending;
  assert.equal(day.items.length,1);assert.equal(state.items[0].uuid,'library');
  state.route.page='days';const old=moreOfDay(day.day); await new Promise(r => setImmediate(r)); home.gen++;
  dayLoading.clear(); dayLoading.add('2026-10-02');
  resolve({items:[]});await old;assert.equal(dayLoading.has('2026-10-02'),true);
});
test('opening the app lands on 首页; the library has its own hash and old query links still open it',()=>{
  // core/route.js 读取页面的 location。
  const {parseHash, hashFor} = web.core(), c = {parseHash, hashFor, location: globalThis.location = {hash: ''}};
  for(const hash of ['','#','#/']){ c.location.hash=hash; assert.equal(c.parseHash().page,'days'); }
  c.location.hash='#/?artist=a1'; assert.deepEqual([c.parseHash().page,c.parseHash().artist],['library','a1']);
  c.location.hash=c.hashFor({page:'library',day:'2026-10-02'}); assert.equal(c.location.hash,'#/library?day=2026-10-02');
  assert.deepEqual([c.parseHash().page,c.parseHash().day],['library','2026-10-02']);
});
test('undo and a finished sync redraw the home page, whose day groups come from their own read', async () => {
  // undo() 在 ui/grid.js，同步结束在 ui/effects.js 的 afterJob()；首页按自己的读取重画。
  const homeReads = [], {edits, state} = web.core(), {afterJob} = await web.load('ui/effects.js');
  web.fakePage();
  edits.latest = {id: 7, label: '已将 1 张移出 A'};
  Object.assign(state, {route: {page: 'days'}, sync: {status: 'scanning'}, focused: null, viewer: -1, guide: undefined});
  const answer = path => path.startsWith('/api/home') ? (homeReads.push(path), {days: [], next: ''}) : path === '/api/undo' ? {undo: null}
    : path === '/api/sync' ? {state: {status: 'done', imported: 1}, history: [], damaged: null} : path.startsWith('/api/bootstrap') ? {token: '', total: 0, undo: null} : [];
  globalThis.fetch = async path => ({status: 200, ok: true, json: async () => answer(path)});
  await gridModule.undo(); await new Promise(r => setImmediate(r));
  assert.equal(homeReads.length, 1);
  assert.equal(edits.latest, null);
  const {pollSync, onJob} = web.core(), off = onJob(afterJob);
  Object.assign(state, {sync: {status: 'scanning'}});
  await pollSync(); await new Promise(r => setImmediate(r));
  off();
  assert.equal(homeReads.length, 2);
});
test('home rows: full rows fill the width; a day with few pictures and a short last row keep the target height, left-aligned (#293)', () => {
  const {homeRows} = homeModule, {state} = web.core();
  state.focused = null;
  const pics = (...r) => r.map((x, i) => ({uuid: 'u' + i, width: Math.round(x * 1000), height: 1000, memberships: []}));
  const rows = html => [...html.matchAll(/class="home-row" style="height:(\d+)px">(.*?)<\/div>/g)].map(([, h, body]) => ({h: Number(h), tiles: [...body.matchAll(/class="home-j[^"]*" data-id="[^"]*" data-chars="[^"]*" style="([^"]*)"/g)].map(m => m[1])}));
  for (const [width, set] of [[1400, pics(.66, .66, .66)], [1400, pics(.75)], [1400, pics(1.5, .66)], [700, pics(.66, 1.5, 1, .75, 1.33)], [1400, pics(.66, 1.5, 1, .75, 1.33)]]) {
    const out = rows(homeRows(set, width, 190)), last = out.at(-1);
    assert.equal(out.reduce((a, r) => a + r.tiles.length, 0), set.length);
    for (const r of out.slice(0, -1)) { assert.ok(r.h <= 190 && r.h > 150, `full row ${r.h}`); assert.ok(r.tiles.every(s => !s.includes('none'))); }
    assert.ok(last.h <= 190 * 1.15, `last row ${last.h} at ${width}`);
    if (last.h === 190) assert.ok(last.tiles.every(s => /^flex:none;width:\d+px$/.test(s)));
  }
  // 接近满行：拉伸到整行宽度，不留空隙。
  const full = rows(homeRows(pics(1, 1, 1, 1, 1, 1, 1), 1372, 190));
  assert.equal(full.length, 1); assert.ok(full[0].tiles.every(s => s.startsWith('flex:1 ')));
});
test('认一认「都不是」常驻在底栏，一次把整批记成不是 TA；全部点掉以后只剩主按钮的「都不是」（#675）', async () => {
  const {home, wireHome} = homeModule, {state, edits} = web.core(), page = web.fakePage(), posts = [];
  globalThis.ResizeObserver ||= class { observe() {} };
  gridModule.viewOpts.ask = true;
  Object.assign(state, {route: {page: 'days'}, nsfw: '', focused: null, viewer: -1});
  const items = ['a', 'b', 'c'].map(uuid => ({uuid, width: 100, height: 100}));
  const batch = () => ({entity: 'e1', kind: 'hint', name: 'A', items});
  Object.assign(home, {days: [], ask: batch(), askSkip: [], askOff: new Set(['b'])});
  const card = () => homeModule.askCardHTML(), buttons = html => [...html.matchAll(/data-askcard="(\w+)"[^>]*>([^<]*)</g)].map(m => [m[1], m[2]]);
  // 点掉一张：常驻的「都不是」和「其余 2 张是」同时在；全部点掉：只剩主按钮的「都不是」，两个不同时出现。
  assert.deepEqual(buttons(card()), [['later', '以后再说'], ['none', '都不是'], ['ok', '其余 2 张是']]);
  home.askOff = new Set(['a', 'b', 'c']);
  assert.deepEqual(buttons(card()), [['later', '以后再说'], ['ok', '都不是']]);
  // 点「都不是」（只点掉了一张）和全部点掉再确认，发出的是同一个请求，撤销记下来。
  globalThis.fetch = async (path, init) => {
    if (path === '/api/ask/answer') posts.push(JSON.parse(init.body));
    const body = path === '/api/ask/answer' ? {undo: {id: posts.length, label: 'x'}} : path.startsWith('/api/ask') ? {batch: null} : path.startsWith('/api/home') ? {days: [], next: ''} : {};
    return {status: 200, ok: true, json: async () => body};
  };
  wireHome();
  // 第一个 click 监听是首页自己的；后面那个是 wirePeek 的捕获监听。
  const click = how => page.el('homePage').listeners.click[0]({target: {closest: sel => sel === '[data-askcard]' ? {dataset: {askcard: how}} : null}});
  for (const [off, how] of [[['b'], 'none'], [['a', 'b', 'c'], 'ok']]) {
    Object.assign(home, {ask: batch(), askOff: new Set(off)});
    click(how); await new Promise(r => setTimeout(r, 20));
    assert.equal(edits.latest?.id, posts.length);
  }
  assert.equal(posts.length, 2);
  assert.deepEqual(posts[0], {entity: 'e1', kind: 'hint', yes: [], no: ['a', 'b', 'c']});
  assert.deepEqual(posts[1], posts[0]);
});
