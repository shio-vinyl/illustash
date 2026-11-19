// core/days.js（#707）：按天视图的取数参数、同步后并进已读到的天、芯片行放几个、各天的框、去网格的地址、认一认的请求体。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');

let D;
test.before(async () => { await web.ready(); D = await web.load('core/days.js'); });

const item = (uuid, extra = {}) => ({uuid, width: 300, height: 400, memberships: [], ...extra});
const day = (d, uuids, extra = {}) => ({day: d, count: uuids.length, people: 0, characters: [], hero: uuids[0] || null, items: uuids.map(u => item(u)), ...extra});

test('取数参数：带图库的筛选、搜索、旧的单值 day 和 NSFW 设置，不带 like、at', () => {
  const q = D.dayQuery({page: 'days', q: 'miku', has: ['fav'], not: ['tag:x'], from: '2026-01-01', day: '2026-02-02', like: 'u1', by: 'face', at: '2026-01-05'}, 'hide');
  assert.deepEqual(q, {day: '2026-02-02', q: 'miku', has: '["fav"]', not: '["tag:x"]', from: '2026-01-01', nsfw: 'hide'});
});

test('读几天：带上收藏和主色，before 为空时不发', async () => {
  const seen = [];
  globalThis.fetch = async path => { seen.push(path); return {status: 200, ok: true, json: async () => ({days: [day('2026-11-19', ['a'])], next: '2026-11-19'})}; };
  const r = await D.readDays({nsfw: 'hide'});
  assert.equal(r.next, '2026-11-19'); assert.equal(r.days.length, 1);
  await D.readDays({nsfw: 'hide'}, {before: '2026-11-15', days: 4});
  const [a, b] = seen.map(p => new URL(p, 'http://x').searchParams);
  assert.equal(a.get('marks'), 'fav,color'); assert.equal(a.get('days'), '5'); assert.equal(a.has('before'), false);
  assert.equal(b.get('before'), '2026-11-15'); assert.equal(b.get('days'), '4');
});

test('同步后并进：没变的天沿用旧对象，变了的换掉，新的一天插到前面，更早的原样留着', () => {
  const old = [day('2026-11-18', ['a', 'b']), day('2026-11-17', ['c']), day('2026-11-10', ['z'])];
  const fresh = [day('2026-11-19', ['n']), day('2026-11-18', ['m', 'a', 'b']), day('2026-11-17', ['c'])];
  const {days, changed} = D.mergeDays(old, fresh);
  assert.deepEqual(days.map(g => g.day), ['2026-11-19', '2026-11-18', '2026-11-17', '2026-11-10']);
  assert.equal(days[2], old[1]); assert.equal(days[3], old[2]);
  assert.deepEqual([...changed].sort(), ['2026-11-18', '2026-11-19']);
});

test('同步后并进：重读范围里没了的一天去掉；收藏、主色变了也算变了', () => {
  const old = [day('2026-11-18', ['a']), day('2026-11-17', ['c'])];
  const fresh = [day('2026-11-18', ['a'], {items: [item('a', {fav: true})]}), day('2026-11-16', ['d'])];
  const {days, changed} = D.mergeDays(old, fresh);
  assert.deepEqual(days.map(g => g.day), ['2026-11-18', '2026-11-16']);
  assert.ok(changed.has('2026-11-18'));
  assert.deepEqual(D.mergeDays(old, []).days, old);
});

test('重读中间一段（改动之后重读视野附近）：只换这一段，前后的天原样留着；dayAfter 给游标', () => {
  const old = [day('2026-11-19', ['n']), day('2026-11-18', ['a']), day('2026-11-17', ['c']), day('2026-11-10', ['z'])];
  const fresh = [day('2026-11-18', ['a'], {count: 2, items: [item('a'), item('b')]}), day('2026-11-17', ['c'])];
  const {days, changed} = D.mergeDays(old, fresh, D.dayAfter('2026-11-18'));
  assert.deepEqual(days.map(g => g.day), ['2026-11-19', '2026-11-18', '2026-11-17', '2026-11-10']);
  assert.equal(days[0], old[0]); assert.equal(days[2], old[2]); assert.equal(days[3], old[3]);
  assert.deepEqual([...changed], ['2026-11-18']);
  assert.equal(D.dayAfter('2026-12-31'), '2027-01-01'); assert.equal(D.dayAfter('2028-02-28'), '2028-02-29');
});

test('读到底的说明：最早的一天和总张数', () => {
  assert.deepEqual(D.summary([day('2026-11-18', ['a', 'b']), day('2025-03-02', ['c'])]), {earliest: '2025-03-02', total: 3});
  assert.deepEqual(D.summary([]), {earliest: '', total: 0});
});

test('大图：本页期间固定的那张优先，其次后端的 hero，再次第一张', () => {
  const g = day('2026-11-19', ['n', 'a', 'b'], {hero: 'b'});
  assert.deepEqual(D.heroFirst(g, 'a').map(x => x.uuid), ['a', 'n', 'b']);
  assert.deepEqual(D.heroFirst(g, 'gone').map(x => x.uuid), ['b', 'n', 'a']);
  assert.deepEqual(D.heroFirst({...g, hero: null}).map(x => x.uuid), ['n', 'a', 'b']);
  assert.deepEqual(D.heroFirst({day: 'x', items: []}), []);
});

test('芯片行：放得下几个放几个，还有剩下的要给「另外 N 位」留位置', () => {
  // 全放得下：不用留按钮的位置
  assert.equal(D.fitChips([100, 100, 100], 308, 90, 4), 3);
  // 第三个放不下：前两个加按钮 100 + 4 + 100 + 4 + 90 = 298
  assert.equal(D.fitChips([100, 100, 100], 300, 90, 4), 2);
  // 后端没画出来的角色（hidden）也要留按钮的位置
  assert.equal(D.fitChips([100, 100], 208, 90, 4, 3), 1);
  assert.equal(D.fitChips([400], 300, 90, 4), 0);
  assert.equal(D.fitChips([], 300, 90, 4), 0);
});

test('一天多高、各天的框：没有角色的天没有芯片行；展开的天用量到的高度；dayAt 找视口顶部那天', () => {
  const sizes = {head: 56, chips: 40, rowHeight: 160, gap: 6, after: 36};
  const a = day('2026-11-19', ['a'], {characters: [{id: 'c1', name: 'A', count: 1}]}), b = day('2026-11-18', ['b']);
  assert.equal(D.dayHeight(a, sizes), 56 + 40 + 326 + 36);
  assert.equal(D.dayHeight(b, sizes), 56 + 326 + 36);
  const {boxes, height} = D.dayBoxes([a, b], {width: 900, top: 100, sizes, opened: new Map([['2026-11-19', 90]])});
  assert.deepEqual(boxes.map(x => [x.key, x.y, x.h, x.w]), [['2026-11-19', 100, 508, 900], ['2026-11-18', 608, 418, 900]]);
  assert.equal(height, 1026); assert.equal(boxes[0].g, a);
  assert.equal(D.dayAt(boxes, 0), '2026-11-19');
  assert.equal(D.dayAt(boxes, 607), '2026-11-19');
  assert.equal(D.dayAt(boxes, 608), '2026-11-18');
  assert.equal(D.dayAt([], 10), '');
});

test('去网格：带着现在的筛选和这一天；点角色再加一个 char 条件', () => {
  const route = {page: 'days', q: 'x', has: ['fav'], sort: 'oldest'};
  assert.deepEqual(D.gridRoute(route, '2026-11-19'), {page: 'library', day: '2026-11-19', q: 'x', has: ['fav']});
  assert.deepEqual(D.gridRoute(route, '2026-11-19', 'e1'), {page: 'library', day: '2026-11-19', q: 'x', has: ['fav', 'char:e1']});
  assert.deepEqual(D.gridRoute({page: 'days', has: '["char:e1"]'}, '2026-11-19', 'e1').has, ['char:e1']);
});

test('认一认的请求体：都对、其余是、都不是、点掉的是另一个角色', () => {
  const b = {entity: 'e', kind: 'hint', items: [item('a'), item('b'), item('c')]};
  assert.deepEqual(D.askAnswer(b, 'ok', new Set()), {entity: 'e', kind: 'hint', yes: ['a', 'b', 'c'], no: []});
  assert.deepEqual(D.askAnswer(b, 'ok', new Set(['b'])), {entity: 'e', kind: 'hint', yes: ['a', 'c'], no: ['b']});
  assert.deepEqual(D.askAnswer(b, 'none', new Set(['b'])), {entity: 'e', kind: 'hint', yes: [], no: ['a', 'b', 'c']});
  // 后端的 switch 要求 yes 为空：只提交点掉的那几张，其余留在卡上
  assert.deepEqual(D.askAnswer(b, 'other', new Set(['c', 'a']), 'f'), {entity: 'e', kind: 'hint', yes: [], no: ['a', 'c'], switch: 'f'});
});

test('展开一天的全部角色：分页汇总 memberships，按张数排', async () => {
  const pages = [
    {items: [item('a', {memberships: [{id: 'x', name: 'X'}]}), item('b', {memberships: [{id: 'y', name: 'Y'}, {id: 'x', name: 'X'}]})]},
    {items: [item('c', {memberships: [{id: 'z', name: 'Z'}]})]},
  ];
  const seen = [];
  globalThis.fetch = async path => { seen.push(new URL(path, 'http://x').searchParams); return {status: 200, ok: true, json: async () => pages[seen.length - 1]}; };
  const people = await D.readDayPeople({q: 'k'}, '2026-11-19', {limit: 2});
  assert.deepEqual(people.map(p => [p.id, p.count]), [['x', 2], ['y', 1], ['z', 1]]);
  assert.equal(seen.length, 2);
  assert.equal(seen[0].get('day'), '2026-11-19'); assert.equal(seen[0].get('q'), 'k'); assert.equal(seen[1].get('offset'), '2');
});
