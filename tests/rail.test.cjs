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
  assert.equal(at({page: 'library', has: ['fav']}), 'galleries/fav');
  assert.equal(at({page: 'pending'}), 'tidy/excluded');
  assert.equal(at({page: 'sgroup', id: '2'}), 'tidy/duplicates');
  assert.equal(at({page: 'settings', section: 'data'}), 'settings/');
  assert.equal(at({page: 'artist-audit'}), '/');
});

test('整理的队列：数字和去处与原来侧栏的计数一致；随机抽查、身份整理有题或正在看时才出现', () => {
  Object.assign(state, {pools: {unmatched: 5, audit: 0}, dupInfo: {groups: 4, mixed: 2}, pendingCount: 0, identityCount: 0});
  let q = rail.queues({page: 'groups'});
  assert.deepEqual(q.map(x => [x.key, x.count, x.href]), [['unmatched', 5, '#/groups'], ['duplicates', 2, '#/duplicates?dup=mixed'], ['excluded', 0, '#/excluded']]);
  assert.equal(rail.tidyWaiting(), true);
  Object.assign(state, {pools: {unmatched: 0, audit: 3}, dupInfo: {groups: 4, mixed: 0}, pendingCount: 1, identityCount: 2});
  q = rail.queues({page: 'groups'});
  assert.deepEqual(q.map(x => [x.key, x.count, x.href]), [['unmatched', 0, '#/groups'], ['duplicates', 0, '#/duplicates'], ['excluded', 1, '#/pending'],
    ['audit', 3, '#/audit'], ['identity', 2, '#/identity']]);
  Object.assign(state, {pools: {unmatched: 0, audit: 0}, dupInfo: {groups: 0, mixed: 0}, pendingCount: 0, identityCount: 0});
  assert.deepEqual(rail.queues({page: 'audit'}).map(x => x.key), ['unmatched', 'duplicates', 'excluded', 'audit']);
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
