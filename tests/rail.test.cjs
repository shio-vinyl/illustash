// 活页条的数据（core/rail.js，#706）：图库时间轴怎么分组、按天视图滚到某天时选中哪一项、整理的队列和数字、
// 地址落在哪个入口，以及资料库钉住的项按图库存取、排序（#684）。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let rail, state, store;
before(async () => {
  await web.ready();
  rail = await web.load('core/rail.js');
  ({state} = web.core());
  globalThis.location = {hash: ''};
});
beforeEach(() => {
  store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, String(v)), removeItem: k => store.delete(k)};
});
const day = (d, count = 3, cover = 'u' + d) => ({day: d, count, cover});

test('时间轴：最近 5 天各一项，同一年更早的按月，再早的按年；月和年记着最新的那一天', () => {
  const days = ['2026-11-18', '2026-11-17', '2026-11-15', '2026-11-12', '2026-11-03', '2026-11-01', '2026-10-30', '2026-10-02', '2026-01-05', '2025-12-31', '2025-03-01', '2024-07-07']
    .map(d => day(d));
  const items = rail.groupTimeline(days);
  assert.deepEqual(items.map(x => x.key), ['d:2026-11-18', 'd:2026-11-17', 'd:2026-11-15', 'd:2026-11-12', 'd:2026-11-03', 'm:2026-11', 'm:2026-10', 'm:2026-01', 'y:2025', 'y:2024']);
  const nov = items.find(x => x.key === 'm:2026-11'), y2025 = items.find(x => x.key === 'y:2025');
  assert.deepEqual([nov.kind, nov.first, nov.days, nov.count, nov.cover], ['month', '2026-11-01', 1, 3, 'u2026-11-01']);
  assert.deepEqual([y2025.kind, y2025.first, y2025.last, y2025.days, y2025.count, y2025.cover2], ['year', '2025-12-31', '2025-03-01', 2, 6, 'u2025-03-01']);
  assert.deepEqual(rail.groupTimeline([]), []);
});

test('按天视图滚到某一天：在时间轴里就选它，否则选它的月，再否则选它的年', () => {
  const items = rail.groupTimeline(['2026-11-18', '2026-11-17', '2026-11-15', '2026-11-12', '2026-11-03', '2026-11-01', '2025-05-05'].map(d => day(d)));
  assert.equal(rail.itemForDay(items, '2026-11-17'), 'd:2026-11-17');
  assert.equal(rail.itemForDay(items, '2026-11-01'), 'm:2026-11');
  assert.equal(rail.itemForDay(items, '2025-05-05'), 'y:2025');
  assert.equal(rail.itemForDay(items, '2020-01-01'), '');
  assert.equal(rail.itemForDay(items, ''), '');
});

test('地址落在哪个入口、哪一项：整理五个队列、画廊和收藏、角色页属于资料库、设置', () => {
  const at = route => { const x = rail.railAt(route); return `${x.entry}/${x.item}`; };
  assert.equal(at({page: 'days'}), 'library/');
  assert.equal(at({page: 'calendar'}), 'library/');
  assert.equal(at({page: 'library', char: 'c1'}), 'catalog/c:c1');
  assert.equal(at({page: 'gallery', id: '3'}), 'galleries/g:3');
  assert.equal(at({page: 'gallery', id: 'fav', has: ['fav']}), 'galleries/fav');
  assert.equal(at({page: 'pending'}), 'tidy/excluded');
  assert.equal(at({page: 'sgroup', id: '2'}), 'tidy/duplicates');
  assert.equal(at({page: 'settings', section: 'data'}), 'settings/');
  assert.equal(at({page: 'artist-audit'}), '/');
});

test('整理的队列：数字和去处与原来侧栏的计数一致；随机抽查、身份整理有题或正在看时才出现', () => {
  Object.assign(state, {pools: {unmatched: 5, audit: 0}, dupInfo: {groups: 4, mixed: 2}, pendingCount: 0, identityCount: 0});
  let q = rail.queues({page: 'groups'});
  assert.deepEqual(q.map(x => [x.key, x.count, x.href]), [['unmatched', 5, '#/groups'], ['doubts', 0, '#/doubts'], ['duplicates', 2, '#/duplicates?dup=mixed'],
    ['excluded', 0, '#/excluded']]);
  assert.equal(rail.tidyWaiting(), true);
  Object.assign(state, {pools: {unmatched: 0, ambiguous: 7, audit: 3}, dupInfo: {groups: 4, mixed: 0}, pendingCount: 1, identityCount: 2});
  q = rail.queues({page: 'groups'});
  assert.deepEqual(q.map(x => [x.key, x.count, x.href]), [['unmatched', 0, '#/groups'], ['doubts', 7, '#/doubts'], ['duplicates', 0, '#/duplicates'], ['excluded', 1, '#/pending'],
    ['audit', 3, '#/audit'], ['identity', 2, '#/identity']]);
  Object.assign(state, {pools: {unmatched: 0, audit: 0}, dupInfo: {groups: 0, mixed: 0}, pendingCount: 0, identityCount: 0});
  assert.deepEqual(rail.queues({page: 'audit'}).map(x => x.key), ['unmatched', 'doubts', 'duplicates', 'excluded', 'audit']);
  assert.equal(rail.tidyWaiting(), false);
  // 一级入口：整理去第一个有待处理的队列
  Object.assign(state, {pendingCount: 4});
  assert.equal(rail.entryHref('tidy'), '#/pending');
});

test('图库三种视图之间切换：筛选和搜索跟着走，角色、排序不带过去；再点「图库」回到上次的视图', () => {
  const r = {page: 'library', q: 'miku', has: ['fav'], not: [], char: 'c1', sort: 'oldest', from: '2026-01-01'};
  assert.deepEqual(rail.viewRoute(r, 'days'), {page: 'days', q: 'miku', has: ['fav'], from: '2026-01-01'});
  assert.deepEqual(rail.viewRoute(r, 'calendar').page, 'calendar');
  rail.noteLibraryView({page: 'calendar'});
  assert.equal(rail.entryHref('library'), '#/calendar');
  rail.noteLibraryView({page: 'unmatched'});   // 别的入口不改记着的视图
  assert.equal(rail.entryHref('library'), '#/calendar');
  rail.noteLibraryView({page: 'days'});
  assert.equal(rail.entryHref('library'), '#/days');
});

test('钉住的项（#684）：按图库存在浏览器本地，钉到末尾、取消、挪位置，重新载入后还在', async () => {
  globalThis.fetch = async () => ({status: 200, ok: true, headers: {get: () => null}, json: async () => ({info: {id: 'lib-a'}})});
  const changes = [];
  const off = rail.pins.changed.on(change => changes.push(change));
  await rail.pin('c:1'); await rail.pin('w:7'); await rail.pin('c:2'); await rail.pin('c:1');
  assert.deepEqual(rail.pins.keys, ['c:1', 'w:7', 'c:2']);
  await rail.movePin('c:2', 0);
  assert.deepEqual(rail.pins.keys, ['c:2', 'c:1', 'w:7']);
  await rail.unpin('w:7');
  assert.deepEqual(rail.pins.keys, ['c:2', 'c:1']);
  assert.deepEqual(changes, [{added: 'c:1'}, {added: 'w:7'}, {added: 'c:2'}, {moved: 'c:2'}, {removed: 'w:7'}]);
  // 存在这个图库的键下（libraryPrefs），换个图库各存各的
  assert.deepEqual(JSON.parse(store.get('pi.railPins@lib-a')), ['c:2', 'c:1']);
  assert.equal([...store.keys()].some(k => k === 'pi.railPins'), false);
  off();
  // 钉住的键画成二级项：角色和作品不在了的不画，键留着
  state.charMap = new Map([['1', {id: '1', name: 'Karen', count: 3, cover: 'u9'}]]);
  state.chars = [];
  assert.deepEqual(rail.pinItems(['c:1', 'c:404']).map(x => [x.key, x.kind, x.label, x.cover, x.href]), [['c:1', 'char', 'Karen', 'u9', '#/library?char=1']]);
});

// 时间轴的读法（#706）：从给的月份起按 prev 一个月一个月往前读；参数没变、没有写入时不重读；同步到新图只重读最新的月；
// 写入之后整条重读；读失败留着上一次的结果；换了参数时上一轮的回复不覆盖。
test('时间轴的读法：按 prev 逐月读、不重复读、写入后整条重读、失败留着上一次的', async () => {
  const months = {
    '2026-11': {days: [{day: '2026-11-18', count: 2, samples: [{uuid: 'a'}, {uuid: 'b'}]}], prev: '2026-10'},
    '2026-10': {days: [{day: '2026-10-02', count: 5, samples: [{uuid: 'c'}]}], prev: null},
  };
  let asked = [], fail = false;
  globalThis.fetch = async path => {
    const q = new URL('http://x' + path).searchParams; asked.push(q.get('month') + (q.get('q') ? '?' + q.get('q') : ''));
    if (fail) return {status: 500, ok: false, headers: {get: () => null}, json: async () => ({error: '读取失败'})};
    return {status: 200, ok: true, headers: {get: () => null}, json: async () => months[q.get('month')] || {days: [], prev: null}};
  };
  state.nsfw = '';
  const route = {page: 'days', q: '', has: [], not: []};
  await rail.loadTimeline(route, {month: '2026-11'});
  assert.deepEqual(asked, ['2026-11', '2026-10']);
  assert.deepEqual(rail.timeline.items.map(x => x.key), ['d:2026-11-18', 'd:2026-10-02']);
  asked = [];
  await rail.loadTimeline(route, {month: '2026-11'});   // 参数没变、没有写入：不读
  assert.deepEqual(asked, []);
  await rail.loadTimeline(route, {month: '2026-11', newest: true});   // 同步到了新图：只重读最新的月
  assert.deepEqual(asked, ['2026-11']);
  asked = [];
  rail.forgetTimeline();   // 写入、撤销之后
  await rail.loadTimeline(route, {month: '2026-11'});
  assert.deepEqual(asked, ['2026-11', '2026-10']);
  // 读失败：时间轴留着上一次的，下次再读
  fail = true; rail.forgetTimeline(); asked = [];
  await rail.loadTimeline(route, {month: '2026-11'});
  assert.deepEqual(rail.timeline.items.map(x => x.key), ['d:2026-11-18', 'd:2026-10-02']);
  assert.equal(rail.timeline.dirty, true);
  fail = false;
  // 换了参数（搜索词）：新的一轮开始时取消上一轮，只认新的
  const slow = rail.loadTimeline({...route, q: 'old'}, {month: '2026-11'});
  const fresh = rail.loadTimeline({...route, q: 'new'}, {month: '2026-11'});
  await Promise.all([slow, fresh]);
  assert.equal(JSON.parse(rail.timeline.params).q, 'new');
});

test('时间轴的一项点了去哪：日历里去那个月，按天、网格里去按天视图的那一天，都带着筛选；at 不是筛选', () => {
  const r = {page: 'library', q: 'miku', has: ['fav'], not: [], day: '2026-11-01', sort: 'oldest', at: ''};
  assert.equal(rail.timelineHref(r, '2026-10-02'), '#/days?day=2026-11-01&q=miku&has=%5B%22fav%22%5D&at=2026-10-02');
  assert.equal(rail.timelineHref({...r, page: 'calendar'}, '2026-10-02'), '#/calendar?day=2026-11-01&q=miku&has=%5B%22fav%22%5D&at=2026-10-02');
  assert.deepEqual(rail.viewRoute({...r, at: '2026-10-02'}, 'grid'), {page: 'library', q: 'miku', day: '2026-11-01', has: ['fav']});
});

// 悬停预览（#706 补）：从项里挑图、定行高，排法交给 core/layout.js 的 strip（那边的测试管放几张、上下限）。
test('预览的一行：挑哪些图、行高；记下的比例当作已知的', () => {
  const {PEEK, peekRow} = rail;
  const tall = id => ({id, w: 700, h: 1000});
  const day = peekRow({peek: ['a', 'b', 'c', 'd'].map(tall)});
  assert.deepEqual([day.h, day.pics.map(p => p.id), day.pics.map(p => p.w)], [PEEK.h, ['a', 'b', 'c', 'd'], [59, 59, 59, 59]]);
  assert.equal(day.width, 59 * 4 + PEEK.gap * 3);
  // 没给 peek：只放封面，排高一些；宽高未知时问 ratioOf，再没有按 3:4
  assert.deepEqual(peekRow({cover: 'c'}).pics, [{id: 'c', x: 0, w: 96}]);
  assert.equal(peekRow({cover: 'c'}).h, PEEK.solo);
  assert.deepEqual(peekRow({cover: 'c'}, id => id === 'c' ? .6 : 0).pics[0].w, 77);
  // 作品的角色：圆头像，正圆、小一些
  const faces = peekRow({peek: [{id: 'a'}, {id: 'b'}], peekRound: true});
  assert.deepEqual([faces.h, ...faces.pics.map(p => p.w)], [PEEK.face, PEEK.face, PEEK.face]);
  assert.deepEqual(peekRow({}).pics, []);
  assert.deepEqual(peekRow({peek: [{id: ''}, null]}).pics, []);
});

// PEEK.gap 要和 css/rail.css 里 .rail-peek-pics 的 gap（--space-4）一致：脚本按它算一行多宽。
test('预览图之间的缝和样式一致', () => {
  const fs = require('node:fs'), path = require('node:path'), root = path.join(__dirname, '..', 'web', 'static');
  const rule = fs.readFileSync(path.join(root, 'css', 'rail.css'), 'utf8').match(/\.rail-peek-pics\{[^}]*gap:var\((--space-\d+)\)/);
  const token = fs.readFileSync(path.join(root, 'css', 'tokens.css'), 'utf8').match(new RegExp(rule[1] + ':(\\d+)px'));
  assert.equal(Number(token[1]), rail.PEEK.gap);
});

// 二级层滚动时上下边缘的渐隐（2026-11-20 用户提）：滚到头的那一边不淡；键盘焦点、选中的项让开渐隐完整露出来。
test('渐隐：上边只在滚过时有，下边只在下面还有内容时有', () => {
  const {railEdges} = rail;
  assert.deepEqual(railEdges({scroll: 0, view: 400, total: 400}), {top: false, bottom: false});   // 不够长，不滚
  assert.deepEqual(railEdges({scroll: 0, view: 400, total: 900}), {top: false, bottom: true});
  assert.deepEqual(railEdges({scroll: 200, view: 400, total: 900}), {top: true, bottom: true});
  assert.deepEqual(railEdges({scroll: 500, view: 400, total: 900}), {top: true, bottom: false});
  assert.deepEqual(railEdges({scroll: 499.5, view: 400, total: 900}), {top: true, bottom: false});   // 缩放下 scrollTop 带小数
});

test('渐隐：项被盖住时滚到让它完整露出来，滚到头的那一边不用让', () => {
  const {FADE, railReveal} = rail, box = {view: 400, total: 900};
  // 中间、完整露出：不滚
  assert.equal(railReveal({...box, scroll: 200, top: 300, h: 46}), null);
  // 被下边的渐隐盖住一半：只滚到它下面留出一段渐隐
  assert.equal(railReveal({...box, scroll: 200, top: 560, h: 46}), 560 + 46 - 400 + FADE);
  // 被上边的渐隐盖住：滚到它上面留出一段渐隐
  assert.equal(railReveal({...box, scroll: 200, top: 210, h: 46}), 210 - FADE);
  // 离顶不到一段渐隐：滚到顶（那时上边没有渐隐）
  assert.equal(railReveal({...box, scroll: 100, top: 110, h: 46}), 110 - FADE);
  assert.equal(railReveal({...box, scroll: 100, top: 20, h: 46}), 0);
  // 滚到底时最后一项贴着下边：下边没有渐隐，不滚
  assert.equal(railReveal({...box, scroll: 500, top: 846, h: 46}), null);
  // 选中时放到中间；夹在能滚的范围里
  assert.equal(railReveal({...box, scroll: 0, top: 500, h: 46}, true), 523 - 200);
  assert.equal(railReveal({...box, scroll: 0, top: 850, h: 46}, true), 500);
  // 没有排版（量出来不是数）时不滚
  assert.equal(railReveal({view: 400, total: 900, scroll: undefined, top: 10, h: 46}), null);
});

// FADE 要和 css/rail.css 里 .rail-scroll 遮罩的渐隐高度一致：脚本按它让开键盘焦点。
test('渐隐的高度和样式一致', () => {
  const fs = require('node:fs'), path = require('node:path');
  const css = fs.readFileSync(path.join(__dirname, '..', 'web', 'static', 'css', 'rail.css'), 'utf8');
  const rule = css.match(/\.rail-scroll\{[^}]*\}/)[0];
  assert.match(rule, new RegExp(`linear-gradient\\(transparent,black ${rail.FADE}px,black calc\\(100% - ${rail.FADE}px\\),transparent\\)`));
  assert.match(rule, new RegExp(`scroll-padding-block:${rail.FADE}px`));
  assert.match(rule, new RegExp(`mask-size:100% calc\\(100% \\+ ${rail.FADE * 2}px\\);mask-position:0 -${rail.FADE}px`));
});

test('时间轴的预览：日期是这一天的样图，月、年是每天的头一张；都带主色', () => {
  const pic = (id, w = 600, h = 900) => ({id, w, h});
  const days = ['2026-11-18', '2026-11-17', '2026-11-15', '2026-11-12', '2026-11-03', '2026-11-01', '2026-10-30'].map((d, i) =>
    ({day: d, count: 4, cover: 'p' + i, color: i === 5 ? null : '#a0b0c' + i, samples: [pic('p' + i), pic('q' + i)]}));
  const items = rail.groupTimeline(days);
  assert.deepEqual(items[0].peek.map(p => p.id), ['p0', 'q0']);
  assert.equal(items[0].color, '#a0b0c0');
  const nov = items.find(x => x.key === 'm:2026-11');
  assert.deepEqual(nov.peek.map(p => p.id), ['p5']);
  assert.equal(nov.color, null);   // 那天没有主色
  const oct = items.find(x => x.key === 'm:2026-10');
  assert.deepEqual([oct.peek[0], oct.color], [pic('p6'), '#a0b0c6']);
  // 没有样图的老数据：用封面
  assert.deepEqual(rail.groupTimeline([{day: '2026-11-18', count: 1, cover: 'z'}])[0].peek, [{id: 'z'}]);
});

test('读时间轴时样图带上原图宽高，日期带上主色', async () => {
  globalThis.fetch = async () => ({status: 200, ok: true, headers: {get: () => null}, json: async () => ({
    days: [{day: '2026-11-18', count: 2, color: '#123456', samples: [{uuid: 'a', width: 800, height: 1200}, {uuid: 'b'}]}], prev: null})});
  state.nsfw = '';
  await rail.loadTimeline({page: 'days', q: 'peek', has: [], not: []}, {month: '2026-11'});
  const [first] = rail.timeline.items;
  assert.deepEqual([first.cover, first.color, first.peek], ['a', '#123456', [{id: 'a', w: 800, h: 1200}, {id: 'b', w: 0, h: 0}]]);
});
