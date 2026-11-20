// 活页条的部件（ui/rail.js，#706）：在 tests/dom.cjs 的假 DOM 上搭一份活页条的骨架，测切换入口（连着切几次也只显示一组）、
// 选中、键盘（⌘1–4 认 e.code，⌥↑↓ 走到下一项）、临时模式，以及钉住的项要等角色目录读到以后才画得出来。
// 没有排版：offsetTop 之类是 undefined，指示条的位置不测；动画在假 DOM 上立刻结束（元素没有 animate）。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const dom = require('./dom.cjs');
const web = require('./web.cjs');
let rail, state, document;
const SKELETON = `<aside id="rail"><nav id="railEntries"><span id="railTab"></span><a data-entry="library" href="#/days"><b class="rail-dot" hidden></b></a>
  <a data-entry="catalog" href="#/characters"><b class="rail-dot" hidden></b></a><a data-entry="galleries" href="#/galleries"><b class="rail-dot" hidden></b></a>
  <a data-entry="tidy" href="#/groups"><b class="rail-dot" hidden></b></a></nav><div id="railLayer"><div id="railHint"></div><div id="railItems"></div><span id="railInd"></span></div>
  <a id="settingsLink" href="#/settings"></a></aside><button id="sidebarToggle"></button><div id="railPeek" hidden></div>`;
const tick = () => new Promise(r => setTimeout(r, 0));
const dwell = () => new Promise(r => setTimeout(r, 360));   // 指针悬停要停够停留判定（PEEK_DWELL 300ms）才出卡片
const visible = () => [...$('railItems').children].filter(x => !x.hidden).map(x => x.dataset.entry);
const $ = id => document.getElementById(id);
const keysOf = entry => [...$('railItems').children].find(x => x.dataset.entry === entry)?.children.filter(x => !x.inert).map(x => x.dataset.key) || [];

before(async () => {
  await web.ready();
  document = dom.install();
  document.body.innerHTML = SKELETON;
  document.getElementById = id => { const walk = n => { for (const c of n.children || []) { if (c.id === id) return c; const f = walk(c); if (f) return f; } return null; }; return walk(document.body); };
  const store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, String(v)), removeItem: k => store.delete(k)};
  store.set('pi.railPins@lib-a', JSON.stringify(['c:1', 'w:7']));
  globalThis.location = {hash: ''};
  globalThis.fetch = async path => ({status: 200, ok: true, headers: {get: () => null},
    json: async () => String(path).startsWith('/api/libraries') ? {info: {id: 'lib-a'}} : {days: [], total: 0, prev: null, next: null}});
  rail = await web.load('ui/rail.js');
  ({state} = web.core());
  Object.assign(state, {pools: {unmatched: 5, audit: 0}, dupInfo: {groups: 1, mixed: 0}, pendingCount: 0, identityCount: 0, charMap: new Map(), chars: [], galleries: []});
  rail.wireRail();
  await tick();
});

test('连着切几次入口（退场动画还没放完又换了）也只显示最后那一组', async () => {
  rail.railRoute({page: 'groups'});
  await tick(); await tick();
  assert.deepEqual(visible(), ['tidy']);
  rail.railRoute({page: 'characters'});   // 整理那组开始退场……
  rail.railRoute({page: 'galleries'});    // ……还没放完就换到画廊
  for (let i = 0; i < 5; i++) await tick();
  assert.deepEqual(visible(), ['galleries']);
  rail.railRoute({page: 'groups'});
  for (let i = 0; i < 5; i++) await tick();
  assert.deepEqual(visible(), ['tidy']);
  const tab = [...$('railEntries').querySelectorAll('[data-entry]')].map(a => [a.dataset.entry, a.getAttribute('aria-current')]);
  assert.deepEqual(tab, [['library', null], ['catalog', null], ['galleries', null], ['tidy', 'page']]);
});

test('选中：只有那一项带 aria-current；整理有待处理时一级入口亮点', async () => {
  rail.railRoute({page: 'duplicates'});
  await tick();
  const current = [...$('railItems').children].flatMap(g => g.children).filter(x => x.getAttribute('aria-current')).map(x => x.dataset.key);
  assert.deepEqual(current, ['duplicates']);
  rail.refresh('tidy');
  const dot = [...$('railEntries').querySelectorAll('[data-entry]')].find(a => a.dataset.entry === 'tidy').children[0];
  assert.equal(dot.hidden, false);
});

test('键盘：⌘1–4 认 e.code；⌥↓ 走到下一项（按它的链接去）；输入框里的 ⌥↓ 不管', async () => {
  const key = extra => ({key: '', code: '', metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, target: {closest: () => null}, preventDefault() { this.prevented = true; }, ...extra});
  // 法语布局：数字键的 e.key 是 é，e.code 是 Digit2
  let e = key({key: 'é', code: 'Digit2', metaKey: true});
  assert.equal(rail.railKey(e), true);
  assert.equal(location.hash, '#/characters');
  rail.railRoute({page: 'groups'});
  await tick();
  e = key({key: 'ArrowDown', altKey: true});
  assert.equal(rail.railKey(e), true);
  assert.equal(location.hash, '#/duplicates');
  e = key({key: 'ArrowDown', altKey: true, target: {closest: () => ({})}});
  assert.equal(rail.railKey(e), false);
  assert.equal(rail.railKey(key({key: 'x'})), false);
});

test('临时模式（拖放时换一组）：只显示那一组；换回来只显示原来的入口', async () => {
  rail.railRoute({page: 'groups'});
  for (let i = 0; i < 3; i++) await tick();
  rail.mode('drop', [{key: 'fav', kind: 'gallery', label: '收藏', heart: true, href: '#/x'}], '松手放进画廊');
  for (let i = 0; i < 3; i++) await tick();
  assert.deepEqual(visible(), ['mode:drop']);
  rail.mode(null);
  for (let i = 0; i < 5; i++) await tick();
  assert.deepEqual(visible(), ['tidy']);
});

test('钉住的项：钉住的列表先读到、角色目录后到，目录到了重画以后才出现（#684 重启后保持）', async () => {
  assert.deepEqual(keysOf('catalog'), []);   // 启动时角色目录还没到，一项也画不出
  state.charMap = new Map([['1', {id: '1', name: 'Karen', count: 3, cover: 'u1'}]]);
  state.chars = [{id: '2', name: 'Rio', work_id: '7', work_name: 'Blue', count: 4, cover: 'u2'}];
  rail.refresh();   // 元数据到了（ui/shell.js 的 metaShown 调它）
  assert.deepEqual(keysOf('catalog'), ['c:1', 'w:7']);
  // 作品页（只按这部作品筛选的图库）在资料库下，选中钉住的那一项
  const {railAt} = await web.load('core/rail.js');
  assert.deepEqual(railAt({page: 'library', work: '7', has: [], not: []}), {entry: 'catalog', item: 'w:7'});
});

test('时间轴读完重画时，原来选中的那项还在就留着；连着写几次只重读一次', async () => {
  const core = await web.load('core/rail.js'), session = await web.load('core/session.js');
  rail.railRoute({page: 'library', q: '', has: [], not: []});   // 网格视图
  for (let i = 0; i < 3; i++) await tick();
  core.timeline.items = core.groupTimeline(['2026-11-18', '2026-11-17', '2026-11-15'].map(day => ({day, count: 1, cover: null})));
  await core.timeline.changed.emit();
  rail.select('d:2026-11-17');   // 网格里用 ⌥↑↓ 移过去的选中
  await core.timeline.changed.emit();   // 又读完一次（写入后、同步后）
  const current = () => [...$('railItems').children].flatMap(g => g.children).filter(x => x.getAttribute('aria-current')).map(x => x.dataset.key);
  assert.deepEqual(current(), ['d:2026-11-17']);
  // 连着写三次：0.7 秒后只读一次（这里每读一次是一个 /api/calendar 请求）
  let reads = 0;
  const fetch = globalThis.fetch;
  globalThis.fetch = async path => { if (String(path).startsWith('/api/calendar')) reads++; return fetch(path); };
  for (let i = 0; i < 3; i++) await session.post('/api/favorite', {});   // 一次写入：post 回来以后通知 onPost
  await new Promise(r => setTimeout(r, 900));
  globalThis.fetch = fetch;
  assert.equal(reads, 1);
});

// 悬停预览（#706 补）：卡片建一次，悬停时换内容（图的节点按位置复用，多出来的藏起来），离开稍等一会儿再收起；滚动时立刻收。
test('悬停预览：换内容不重建，放得下几张放几张；离开稍等再收、滚动立刻收', async () => {
  const core = await web.load('core/rail.js');
  rail.railRoute({page: 'days', q: '', has: [], not: []});
  for (let i = 0; i < 3; i++) await tick();
  const pics = (n, w, h) => Array.from({length: n}, (_, i) => ({uuid: 'u' + i, width: w, height: h}));
  core.timeline.items = core.groupTimeline([
    {day: '2026-11-18', count: 9, cover: 'u0', color: '#336699', samples: pics(4, 700, 1000).map(p => ({id: p.uuid, w: p.width, h: p.height}))},
    {day: '2026-11-17', count: 2, cover: 'u0', samples: pics(4, 1200, 900).map(p => ({id: p.uuid, w: p.width, h: p.height}))},
  ]);
  await core.timeline.changed.emit();
  const item = key => [...$('railItems').children].flatMap(g => g.children).find(x => x.dataset.key === key);
  const over = el => $('railItems').dispatchEvent({type: 'pointerover', target: {closest: () => el}});   // 假 DOM 没有 closest
  const peek = $('railPeek'), shown = () => peek.querySelectorAll('img').filter(x => !x.hidden);
  const card = peek.children[0];
  over(item('d:2026-11-18'));
  await dwell();
  assert.equal(peek.hidden, false);
  assert.equal(peek.children[0], card);   // 同一张卡
  assert.equal(shown().length, 4);   // 四张竖图放得下
  assert.equal(peek.classList.contains('tint'), true);
  assert.equal(item('d:2026-11-18').classList.contains('is-hover'), true);
  assert.equal(item('d:2026-11-17').classList.contains('is-near'), true);
  over(item('d:2026-11-17'));
  assert.equal(peek.children[0], card);
  assert.equal(shown().length, 2);   // 横图只放得下两张
  assert.equal(peek.querySelectorAll('img').length, 4);   // 节点复用，多的藏起来
  assert.equal(peek.classList.contains('tint'), false);   // 这天没有主色
  $('railItems').dispatchEvent({type: 'pointerleave'});
  assert.equal(peek.hidden, false);   // 稍等再收
  await new Promise(r => setTimeout(r, 200));
  assert.equal(peek.hidden, true);
  over(item('d:2026-11-18'));
  $('railLayer').dispatchEvent({type: 'scroll'});
  assert.equal(peek.hidden, true);
});

// 虚拟机走查（PR #754）：Tab 走到二级层最后一项时二级层滚动，滚动先收起预览，延后的量和写又把它显示出来，之后一直不消失。
test('悬停预览：延后的量和写到时悬停的已经不是这一项，就不再显示', async () => {
  const items = [...$('railItems').children].flatMap(g => g.children).filter(x => x.dataset.kind === 'day');
  const peek = $('railPeek');
  // 像浏览器一样，出场动画要放一会儿（这里等计时兜底），中途 enter() 取消它时出场作废、预览留着：这正是卡住的样子
  peek.animate = () => { let reject; const finished = new Promise((_, r) => { reject = r; }); finished.catch(() => {}); return {finished, cancel: () => reject(new Error('取消')), finish() {}}; };
  globalThis.innerHeight = 800;
  $('railItems').dispatchEvent({type: 'focusin', target: {closest: () => items[0]}});
  $('railLayer').dispatchEvent({type: 'scroll'});   // 帧任务跑之前先滚动了
  await new Promise(r => setTimeout(r, 500));   // 帧任务、出场的计时兜底都过去
  assert.equal(peek.hidden, true);
  // 键盘焦点就在这一项上时，滚动只是把预览挪到它的新位置（Tab 走到头时二级层跟着滚），不收起
  items[0].focus();
  $('railItems').dispatchEvent({type: 'focusin', target: {closest: () => items[0]}});
  $('railLayer').dispatchEvent({type: 'scroll'});
  await new Promise(r => setTimeout(r, 500));
  assert.equal(peek.hidden, false);
  $('railItems').dispatchEvent({type: 'focusout'});
  await new Promise(r => setTimeout(r, 500));
  assert.equal(peek.hidden, true);
  delete peek.animate; delete globalThis.innerHeight;
});

// 可维护性另审（PR #762 review 5986175714）指出的三处，以及离开后的计时器顺延。
const fakeAnimate = el => {
  const calls = [];
  el.animate = frames => { calls.push(frames); let reject; const finished = new Promise((_, r) => { reject = r; }); finished.catch(() => {}); return {finished, cancel: () => reject(new Error('取消')), finish() {}}; };
  return calls;
};
const frames = () => new Promise(r => setTimeout(r, 60));
const dayItems = () => [...$('railItems').children].filter(g => !g.hidden).flatMap(g => g.children).filter(x => x.dataset.kind === 'day' && !x.inert);

test('换入口时预览收起：指针停在一项上按 ⌘2，旧项的预览不留在新那组上', async () => {
  rail.railRoute({page: 'days', q: '', has: [], not: []});
  for (let i = 0; i < 3; i++) await tick();
  globalThis.innerHeight = 800;
  const peek = $('railPeek');
  $('railItems').dispatchEvent({type: 'pointerover', target: {closest: () => dayItems()[0]}});
  await dwell(); await frames();
  assert.equal(peek.hidden, false);
  rail.railRoute({page: 'characters'});
  await new Promise(r => setTimeout(r, 400));
  assert.equal(peek.hidden, true);
  delete globalThis.innerHeight;
});

test('焦点进项、同一帧又滚动：卡片照常放进场，不从旧位置滑过来', async () => {
  rail.railRoute({page: 'days', q: '', has: [], not: []});
  for (let i = 0; i < 5; i++) await tick();
  globalThis.innerHeight = 800;
  const peek = $('railPeek'), calls = fakeAnimate(peek), el = dayItems()[1];
  el.focus();
  $('railItems').dispatchEvent({type: 'focusin', target: {closest: () => el}});
  $('railLayer').dispatchEvent({type: 'scroll'});
  await frames();
  assert.equal(calls.length, 1);   // 进场放了
  assert.equal(peek.classList.contains('is-glide'), false);
  $('railItems').dispatchEvent({type: 'focusout'});
  await new Promise(r => setTimeout(r, 500));
  delete peek.animate; delete globalThis.innerHeight;
});

test('宽高未知的图读到尺寸后记下比例：第二次悬停同一项还是按它的比例', async () => {
  const core = await web.load('core/rail.js');
  rail.railRoute({page: 'days', q: '', has: [], not: []});
  for (let i = 0; i < 3; i++) await tick();
  core.timeline.items = core.groupTimeline([{day: '2026-11-18', count: 1, cover: 'solo'}]);   // 老数据：只有封面、没有宽高
  await core.timeline.changed.emit();
  globalThis.innerHeight = 800;
  const peek = $('railPeek'), first = () => peek.querySelectorAll('img')[0];
  const over = async () => { $('railItems').dispatchEvent({type: 'pointerover', target: {closest: () => dayItems()[0]}}); await dwell(); };
  await over();
  assert.equal(first().style.width, '96px');   // 先按 3:4
  Object.assign(first(), {naturalWidth: 600, naturalHeight: 1000});
  first().dispatchEvent({type: 'load'});
  assert.equal(first().style.width, '77px');   // 读到 0.6
  rail.railRoute({page: 'characters'});   // 收起
  await new Promise(r => setTimeout(r, 300));
  rail.railRoute({page: 'days', q: '', has: [], not: []});
  for (let i = 0; i < 3; i++) await tick();
  await over();
  assert.equal(first().style.width, '77px');   // src 没变、不再来 load，也还是 0.6
  delete globalThis.innerHeight;
});

test('离开的计时器顺延：离开、回到另一项、又离开，从最后一次离开起再等满才收', async () => {
  rail.railRoute({page: 'days', q: '', has: [], not: []});
  for (let i = 0; i < 3; i++) await tick();
  const core = await web.load('core/rail.js');
  core.timeline.items = core.groupTimeline(['2026-11-18', '2026-11-17'].map(day => ({day, count: 1, cover: 'u'})));
  await core.timeline.changed.emit();
  globalThis.innerHeight = 800;
  const peek = $('railPeek'), [a, b] = dayItems(), wait = ms => new Promise(r => setTimeout(r, ms));
  $('railItems').dispatchEvent({type: 'pointerover', target: {closest: () => a}});
  await dwell(); await frames();
  $('railItems').dispatchEvent({type: 'pointerleave'});
  await wait(50);
  $('railItems').dispatchEvent({type: 'pointerover', target: {closest: () => b}});
  await wait(50);
  $('railItems').dispatchEvent({type: 'pointerleave'});
  await wait(60);   // 第一次离开已过 160ms，最后一次才 60ms
  assert.equal(peek.hidden, false);
  await wait(150);
  assert.equal(peek.hidden, true);
  delete globalThis.innerHeight;
});

// 停留判定（用户 2026-11-20 定）：指针在一项上停够 PEEK_DWELL 才出第一张卡；出了以后换到相邻项不再等；键盘焦点立刻出。
test('停留判定：路过不出、停够了出、出了以后移到相邻项不再等；焦点立刻出', async () => {
  const core = await web.load('core/rail.js'), peekMod = await web.load('ui/rail_peek.js');
  rail.railRoute({page: 'days', q: '', has: [], not: []});
  for (let i = 0; i < 3; i++) await tick();
  core.timeline.items = core.groupTimeline(['2026-11-18', '2026-11-17', '2026-11-15'].map(day => ({day, count: 1, cover: 'u'})));
  await core.timeline.changed.emit();
  globalThis.innerHeight = 800;
  const peek = $('railPeek'), [a, b, c] = dayItems(), wait = ms => new Promise(r => setTimeout(r, ms));
  const over = el => $('railItems').dispatchEvent({type: 'pointerover', target: {closest: () => el}});
  const leave = () => $('railItems').dispatchEvent({type: 'pointerleave'});
  assert.equal(peekMod.PEEK_DWELL, 300);
  // 路过：每项停不到 300ms 就换下一项、最后离开，一直不出
  over(a); await wait(120); over(b); await wait(120); over(c); await wait(120); leave();
  await wait(400);
  assert.equal(peek.hidden, true);
  // 停够了出
  over(a); await wait(150);
  assert.equal(peek.hidden, true);
  await wait(250); await frames();
  assert.equal(peek.hidden, false);
  assert.equal(peek.querySelector('b').textContent, a.railItem.label);
  // 出了以后移到相邻项：不再等
  over(b);
  assert.equal(peek.querySelector('b').textContent, b.railItem.label);
  leave(); await wait(300);
  assert.equal(peek.hidden, true);
  // 键盘焦点：立刻出
  c.focus();
  $('railItems').dispatchEvent({type: 'focusin', target: {closest: () => c}});
  assert.equal(peek.hidden, false);
  assert.equal(peek.querySelector('b').textContent, c.railItem.label);
  $('railItems').dispatchEvent({type: 'focusout'}); await wait(300);
  delete globalThis.innerHeight;
});
