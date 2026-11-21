// 图库的日历视图（#708）：core/calendar.js 的月份计算、请求参数和回复整理，ui/calendar.js 的部件（格子的拼法、底色、
// 今天、链接、翻月、按位置复用、键盘）。图片和角色都是合成数据。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

let C, V, S, document;
test.before(async () => {
  await web.ready();
  document = dom.install();
  C = await web.load('core/calendar.js');
  V = await web.load('ui/calendar.js');
  S = await web.load('ui/scope.js');
});
test.after(() => dom.uninstall());

const item = (uuid, chars = [], color = null) => ({uuid, width: 600, height: 800, memberships: chars.map(([id, name]) => ({id, name, status: 'confirmed'})), color});
const reply = {
  days: [
    {day: '2026-11-03', count: 1, color: '#336699', samples: [item('a', [['c1', '合成甲']], '#336699')]},
    {day: '2026-11-10', count: 3, color: null, samples: [item('b', [['c1', '合成甲'], ['c2', '合成乙']]), item('c', [['c2', '合成乙']]), item('d')]},
    {day: '2026-11-19', count: 9, color: '#aa3344', samples: [item('e', [['c2', '合成乙']]), item('f'), item('g'), item('h')]},
  ],
  total: 13, prev: '2026-09', next: null,
};

test('月份：解析、格式、前后挪', () => {
  assert.deepEqual(C.parseMonth('2026-11'), {year: 2026, month: 11});
  for (const bad of ['2026-13', '2026-1', '26-11', '', null]) assert.equal(C.parseMonth(bad), null);
  assert.equal(C.shiftMonth('2026-11', 2), '2027-01');
  assert.equal(C.shiftMonth('2026-01', -1), '2025-12');
  assert.equal(C.shiftMonth('2026-05', -17), '2024-12');
  assert.equal(C.localDay(new Date(2026, 0, 5, 23, 59)), '2026-01-05');
  assert.equal(C.monthOf('2026-11-19'), '2026-11');
});

test('请求地址：筛选原样带上，空的不带，数组写成 JSON，样图要主色', () => {
  const url = new URL(C.calendarUrl('2026-11', {q: '合成', has: ['fav'], not: [], from: '', nsfw: 'hide'}), 'http://x');
  assert.equal(url.pathname, '/api/calendar');
  assert.deepEqual(Object.fromEntries(url.searchParams), {q: '合成', has: '["fav"]', nsfw: 'hide', month: '2026-11', marks: 'color'});
});

test('回复整理：按日期放进 Map，有图的天数、环境光的颜色、当月角色按出现次数排', () => {
  const m = C.shapeMonth('2026-11', reply);
  assert.equal(m.days.get('2026-11-10').count, 3);
  assert.equal(m.active, 3);
  assert.equal(m.total, 13);
  assert.equal(m.color, '#336699');
  assert.equal(m.prev, '2026-09');
  assert.equal(m.next, null);
  assert.deepEqual(m.people.map(p => p.id), ['c2', 'c1']);   // 乙出现在三张图上，甲两张
  assert.equal(m.people[1].cover, 'a');
  // 坏的回复不抛错，当作空月
  const empty = C.shapeMonth('2026-11', {days: null, total: 'x', prev: 'nope'});
  assert.equal(empty.total, 0); assert.equal(empty.active, 0); assert.equal(empty.prev, null); assert.deepEqual(empty.people, []);
});

test('readMonth 发一次请求并整理；月份不对时不发请求', async () => {
  const seen = [];
  globalThis.fetch = async path => { seen.push(path); return {ok: true, status: 200, json: async () => reply}; };
  const m = await C.readMonth('2026-11', {q: 'x'});
  assert.equal(seen.length, 1);
  assert.match(seen[0], /month=2026-11/);
  assert.equal(m.month, '2026-11');
  await assert.rejects(C.readMonth('2026-xx'), /月份格式不对/);
  assert.equal(seen.length, 1);
});

function mount() {
  const pageScope = S.scope('test-page'), months = [], empties = [];
  const view = V.calendarView({scope: pageScope, dayHref: day => '#/library?day=' + day, onMonth: m => months.push(m), empty: (box, data) => empties.push(data.month)});
  document.body.append(view.el);
  const cells = () => view.el.querySelectorAll('a').filter(a => a.getAttribute('class')?.split(' ').includes('cal-day'));
  return {view, pageScope, months, empties, cells, cell: date => cells()[dayIndex(date)]};
}
// 2026 年 11 月 1 日是周日：周一开头时前面补 6 天，所以 11 月 n 日是第 n + 5 格。
const dayIndex = date => Number(date.slice(8)) + 5;

test('格子：按张数拼 1 / 2 / 4 张，底色是当天的颜色，没有图的格子空着、不是链接，前后月只写日期', () => {
  const {view, cell, cells, pageScope} = mount();
  view.update({month: '2026-11', data: C.shapeMonth('2026-11', reply)});
  assert.equal(cells().length, 42);   // 1 日是周日：前面补 6 天，共 6 周
  const shown = date => cell(date).querySelectorAll('img').filter(img => !img.hidden).length;
  assert.equal(shown('2026-11-03'), 1);
  assert.equal(shown('2026-11-10'), 2);   // 3 张拼 2 张
  assert.equal(shown('2026-11-19'), 4);
  assert.equal(shown('2026-11-04'), 0);
  assert.equal(cell('2026-11-03').getAttribute('href'), '#/library?day=2026-11-03');
  assert.equal(cell('2026-11-04').getAttribute('href'), null);
  assert.equal(cell('2026-11-03').style.getPropertyValue('--tint'), '#336699');
  assert.ok(cell('2026-11-03').classList.contains('tint'));
  assert.ok(!cell('2026-11-10').classList.contains('tint'));   // 那天没有颜色：中性底
  // 一张图占满，两张各占一半
  const [one] = cell('2026-11-03').querySelectorAll('img');
  assert.equal(one.style.getPropertyValue('width'), '100%');
  const [left, right] = cell('2026-11-10').querySelectorAll('img');
  assert.equal(left.style.getPropertyValue('left'), '0%');
  assert.ok(Number.parseFloat(right.style.getPropertyValue('left')) > 50);
  assert.equal(cell('2026-11-19').querySelectorAll('img')[0].getAttribute('data-thumb'), 'e');   // 缩略图登记（#709 用）
  // 10 月最后几天：只有日期
  const out = cells()[0];
  assert.ok(out.classList.contains('is-out'));
  assert.equal(out.getAttribute('href'), null);
  pageScope.dispose();
});

test('标题、头像、翻月：prev 是筛选后最近有图的月份，next 为 null 时禁用；中间回到本月', () => {
  const {view, months, pageScope} = mount();
  view.update({month: '2026-11', data: null});
  const [prev, now, next] = view.el.querySelectorAll('button');
  assert.ok(prev.hasAttribute('disabled') && next.hasAttribute('disabled'));   // 读的时候先禁用
  view.update({data: C.shapeMonth('2026-11', reply)});
  const [title, sub] = view.el.querySelector('hgroup').children;
  assert.equal(title.textContent, '11 月');
  assert.match(sub.textContent, /2026 · 13 张 · 3 天有新图/);
  assert.equal(view.el.querySelectorAll('img').filter(img => img.classList.contains('avatar')).length, 2);
  assert.ok(!prev.hasAttribute('disabled'));
  assert.ok(next.hasAttribute('disabled'));
  prev.dispatchEvent({type: 'click'});
  assert.deepEqual(months, ['2026-09']);
  now.dispatchEvent({type: 'click'});
  assert.equal(months.length, C.monthOf(C.localDay()) === '2026-11' ? 1 : 2);
  pageScope.dispose();
});

test('翻月时格子按位置复用，不重建；今天有标记；空月交给 empty 画', () => {
  const {view, cells, empties, pageScope} = mount();
  const today = C.localDay(), month = C.monthOf(today);
  view.update({month, data: C.shapeMonth(month, {days: [], total: 0, prev: null, next: null})});
  const first = cells()[0];
  const marked = cells().filter(c => c.getAttribute('aria-current') === 'date');
  assert.equal(marked.length, 1);
  assert.equal(marked[0].querySelector('b').textContent, String(Number(today.slice(8))));
  assert.deepEqual(empties, [month]);
  assert.ok(!view.el.querySelectorAll('div').find(d => d.getAttribute('class') === 'cal-empty').hidden);
  view.update({month: '2026-11', data: C.shapeMonth('2026-11', reply)});
  assert.equal(cells()[0], first);
  assert.ok(view.el.querySelectorAll('div').find(d => d.getAttribute('class') === 'cal-empty').hidden);
  pageScope.dispose();
  assert.equal(view.el.querySelectorAll('a').length, 0);   // 释放后格子都摘掉
});

test('方向键按日期在有图的格子之间挪，跳过空格子；输入法组字时不动', () => {
  const {view, cell, pageScope} = mount();
  view.update({month: '2026-11', data: C.shapeMonth('2026-11', reply)});
  const grid = cell('2026-11-03').parentNode, key = (target, k, extra = {}) => {
    let stopped = false;
    grid.dispatchEvent({type: 'keydown', key: k, target, preventDefault: () => { stopped = true; }, ...extra});
    return stopped;
  };
  assert.ok(key(cell('2026-11-03'), 'ArrowRight'));
  assert.equal(document.activeElement, cell('2026-11-10'));
  assert.ok(key(cell('2026-11-10'), 'ArrowDown'));   // 17 日没有图，往后找到 19 日
  assert.equal(document.activeElement, cell('2026-11-19'));
  assert.ok(!key(cell('2026-11-19'), 'ArrowRight'));   // 后面没有了：不拦
  assert.ok(!key(cell('2026-11-03'), 'ArrowRight', {isComposing: true}));
  pageScope.dispose();
});

// 页面样板第 5 条：同步、写入之后的重读用的是同一个路由对象。先后两次读，先发的那次后回来，不画（只认最后发出的那次）。
test('同一路由先后读两次，先发的那次后回来，不画', async () => {
  const {PAGES} = await web.load('core/pages.js'), {state} = web.core();
  const box = document.createElement('div'); box.setAttribute('id', 'calendarPage'); document.body.append(box);
  const waiting = [];
  globalThis.fetch = () => new Promise(done => waiting.push(body => done({ok: true, status: 200, json: async () => body})));
  state.route = {page: 'calendar', at: '2026-11-10', has: [], not: []};
  const scope = S.page('calendar');
  const first = PAGES.calendar.render({samePage: false, scope});
  const second = PAGES.calendar.refresh('sync');
  assert.equal(waiting.length, 2);
  waiting[1](reply); await second;
  waiting[0]({...reply, total: 99}); await first;
  assert.match(box.querySelector('hgroup').children[1].textContent, /13 张/, '画的是后发的那次');
  S.page('library');
  box.remove();
});
