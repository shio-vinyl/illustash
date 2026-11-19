// ui/day.js、ui/day_cards.js（#707）：按天视图的一天和顶上两张卡。同步进来新图时已有的图块不重建、地址不变；「+N 张」；
// 芯片行放不下的收成「另外 N 位」，展开、收起；认一认的点掉、主按钮文字和三种回答。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

let Day, Cards, frame, document, frames;
const flush = () => { while (frames.length) frames.shift()(); };
test.before(async () => {
  await web.ready(); document = dom.install();
  frame = await web.load('ui/frame.js'); Day = await web.load('ui/day.js'); Cards = await web.load('ui/day_cards.js');
});
test.after(() => dom.uninstall());
test.beforeEach(() => { frames = []; frame.setScheduler(fn => frames.push(fn)); });

const SIZES = {head: 56, chips: 40, rowHeight: 160, gap: 6, after: 36};
const item = (uuid, extra = {}) => ({uuid, width: 300, height: 400, memberships: [], ...extra});
const chars = n => Array.from({length: n}, (_, i) => ({id: 'c' + i, name: 'C' + i, count: n - i}));
function context(over = {}) {
  const heroes = new Map(), log = {toggle: [], resized: []};
  const ctx = {sizes: () => SIZES, hero: d => heroes.get(d) || '', hold: (d, u) => { if (!heroes.has(d)) heroes.set(d, u); },
    href: (d, c) => `#/library?day=${d}${c ? '&char=' + c : ''}`, cover: () => '', toggle: d => log.toggle.push(d), resized: (d, h) => log.resized.push([d, h]),
    marks: () => true, people: () => false, ...over};
  return {ctx, log};
}
const box = (g, w = 1200, extra = {}) => ({key: g.day, x: 0, y: 0, w, h: 500, g, ...extra});
const tiles = d => [...d.el.querySelectorAll('button')].filter(b => b.dataset.id && !b.inert);

test('一天：大图在前、带角色名；新图进来时已有的图块原样留着，地址不变，新图插进来', () => {
  const {ctx} = context();
  const hero = item('h', {memberships: [{id: 'c0', name: 'Alice'}]});
  const g = {day: '2026-11-19', count: 3, people: 1, characters: chars(1), hero: 'h', items: [item('a'), hero, item('b')]};
  const d = Day.dayWidget(box(g), ctx); flush();
  assert.deepEqual(tiles(d).map(b => b.dataset.id), ['h', 'a', 'b']);
  const first = new Map(tiles(d).map(b => [b.dataset.id, b])), src = new Map(tiles(d).map(b => [b.dataset.id, b.querySelector('img').getAttribute('src')]));
  assert.equal(first.get('h').querySelector('b').textContent, 'Alice');
  // 看图器（#709）按 <img> 上的 data-thumb 找缩略图
  for (const [id, el] of first) assert.equal(el.querySelector('img').dataset.thumb, id);
  // 同步进来一张新图 n（后端这时把 n 选成 hero）：大图仍是 h，旧的三个节点和地址都没变
  d.update(box({...g, count: 4, hero: 'n', items: [item('n'), item('a'), hero, item('b')]})); flush();
  assert.deepEqual(tiles(d).map(b => b.dataset.id), ['h', 'n', 'a', 'b']);
  for (const [id, el] of first) { assert.equal(tiles(d).find(b => b.dataset.id === id), el); assert.equal(el.querySelector('img').getAttribute('src'), src.get(id)); }
  d.dispose();
});

test('一天：后端没给全的张数算进最后一格的「+N 张」', () => {
  const {ctx} = context();
  const g = {day: '2026-11-18', count: 40, people: 0, characters: [], hero: 'a', items: ['a', 'b', 'c'].map(u => item(u))};
  const d = Day.dayWidget(box(g, 2000), ctx); flush();
  const last = tiles(d).at(-1);
  assert.equal(last.dataset.more, String(1 + 37));
  assert.match(last.getAttribute('aria-label'), /38/);
  d.dispose();
});

test('芯片行：一行放不下的收起来，按钮写「另外 N 位」；展开、收起交给页面', () => {
  const {ctx, log} = context();
  const g = {day: '2026-11-17', count: 1, people: 12, characters: chars(8), hero: 'a', items: [item('a')]};
  const d = Day.dayWidget(box(g, 420), ctx);
  // 量之前给芯片和按钮定宽：每个芯片 100，按钮 90，缝 4 → 放下 3 个（100×3 + 4×3 + 90 = 402）
  const row = d.el.querySelector('div'), chipEls = () => [...d.el.querySelectorAll('a')].filter(a => a.dataset.char);
  for (const el of chipEls()) el.offsetWidth = 100;
  const more = [...d.el.querySelectorAll('button')].find(b => b.getAttribute('aria-expanded') !== null);
  more.offsetWidth = 90;
  flush();
  assert.ok(row);
  assert.deepEqual(chipEls().map(el => el.classList.contains('is-cut')), [false, false, false, true, true, true, true, true]);
  assert.equal(more.hidden, false); assert.match(more.textContent, /9/);
  more.dispatchEvent(dom.event('click'));
  assert.deepEqual(log.toggle, ['2026-11-17']);
  // 页面带着全部角色和 open 再 update：都显示，按钮是「收起」，量到的高度报给页面
  const all = chars(12);
  d.update(box(g, 420, {open: true, people: all}));
  assert.equal(chipEls().length, 12);
  assert.ok(chipEls().every(el => !el.classList.contains('is-cut')));
  assert.equal(more.textContent, '收起'); assert.equal(more.getAttribute('aria-expanded'), 'true');
  d.el.querySelectorAll('div')[0].offsetHeight = 96;
  for (const el of [...d.el.querySelectorAll('div')]) if (el.classList.contains('day-chars')) el.offsetHeight = 96;
  flush();
  assert.deepEqual(log.resized.at(-1), ['2026-11-17', 96]);
  d.dispose();
});

test('认一认：收起时「不全是」「都对」；展开后点掉一张，主按钮变「其余 N 张是」，「点掉的是…」交出点掉的那几张', () => {
  const calls = [];
  const card = Cards.askCard({answer: (how, off) => calls.push(['answer', how, [...off]]), other: off => calls.push(['other', [...off]]), later: () => calls.push(['later'])});
  const b = {entity: 'e1', name: 'Alice', kind: 'hint', items: ['a', 'b', 'c'].map(u => item(u))};
  card.update({batch: b});
  const btn = text => [...card.el.querySelectorAll('button')].find(x => !x.dataset.peek && x.textContent === text);
  const tile = u => [...card.el.querySelectorAll('button')].find(x => x.dataset.peek === u);
  assert.equal(card.el.hidden, false);
  assert.equal(btn('不全是').hidden, false); assert.equal(btn('都对').hidden, false); assert.equal(btn('点掉的是…').hidden, true);
  assert.match(card.el.querySelector('[id]').textContent, /Alice/);
  assert.deepEqual([...card.el.querySelectorAll('img')].map(img => img.dataset.thumb), ['a', 'b', 'c', 'a', 'b', 'c']);
  btn('不全是').dispatchEvent(dom.event('click'));
  assert.equal(btn('不全是').hidden, true); assert.equal(btn('都不是').hidden, false);
  tile('b').dispatchEvent(dom.event('click'));
  assert.equal(tile('b').getAttribute('aria-pressed'), 'true');
  assert.ok(btn('其余 2 张是')); assert.equal(btn('点掉的是…').hidden, false);
  btn('点掉的是…').dispatchEvent(dom.event('click'));
  btn('其余 2 张是').dispatchEvent(dom.event('click'));
  assert.deepEqual(calls, [['other', ['b']], ['answer', 'ok', ['b']]]);
  // 「点掉的是…」提交后同一个角色少了那一张：仍展开，点掉的清空
  card.update({batch: {...b, items: [item('a'), item('c')]}});
  assert.equal(btn('不全是').hidden, true); assert.ok(btn('都对'));
  // 全部点掉：主按钮是「都不是」，常驻的「都不是」不再画
  tile('a').dispatchEvent(dom.event('click')); tile('c').dispatchEvent(dom.event('click'));
  const all = [...card.el.querySelectorAll('button')].filter(x => !x.dataset.peek && x.textContent === '都不是');
  assert.deepEqual(all.map(x => x.hidden), [true, false]);
  card.update({batch: null});
  assert.equal(card.el.hidden, true);
  card.dispose();
});

test('同步卡：进行中写进度；完成后写新加入几张、今天认出几位；没开过同步时藏起来', () => {
  const opened = [];
  const card = Cards.syncCard({open: () => opened.push(1)});
  card.update({sync: {status: 'off'}});
  assert.equal(card.el.hidden, true);
  card.update({sync: {status: 'importing', import_done: 5, import_total: 20}, today: null});
  assert.equal(card.el.hidden, false);
  assert.equal(card.el.querySelector('b').textContent, '正在同步');
  assert.equal(card.el.querySelector('span').style.getPropertyValue('--p'), '25');
  card.update({sync: {status: 'done', imported: 83, finished_at: '2026-11-19T20:25:00'}, today: {people: 14}});
  assert.equal(card.el.querySelector('b').textContent, '同步完成');
  assert.match(card.el.textContent, /新加入 83 张/); assert.match(card.el.textContent, /今天认出 14 位角色/);
  card.el.dispatchEvent(dom.event('click'));
  assert.equal(opened.length, 1);
  card.dispose();
});
