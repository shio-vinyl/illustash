// 数字和日期跟随页面语言；中文保留自己的措辞（core/format.js；#182、#199）。
// 英文页面上的同样检查在 tests/english_page.test.cjs。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
before(web.ready);

test('a Chinese page writes dates its own way', () => {
  const {number, dateText, monthText, esc, work} = web.core();
  assert.equal(number(12345), '12,345');
  assert.equal(dateText(2025, 5, 3, true) + '|' + dateText(2026, 5, 3) + '|' + dateText('2026', '05', '03') + '|' + monthText('2026', '05'),
    '2025 年 5 月 3 日|5 月 3 日|05 月 03 日|2026 年 05 月');
  assert.equal(esc(`<a href="x">'&'</a>`), '&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;');
  assert.equal(esc(null), '');
  assert.equal(work('fate_(series)'), 'fate (series)');
});

// 后端的 UTC 时间戳按本地时区的日期显示（#386 的 2-15）：东八区凌晨点的「标为已检查」不写成前一天。
test('a UTC timestamp shows as the local date', () => {
  const {localDate, dateText} = web.core();
  const local = new Date('2026-11-13T18:30:00Z');
  assert.equal(localDate('2026-11-13 18:30:00'), dateText(local.getFullYear(), local.getMonth() + 1, local.getDate()));
  assert.equal(localDate('2026-11-13T18:30:00Z'), localDate('2026-11-13 18:30:00'));
  assert.equal(localDate('2026-11-14T02:30:00+08:00'), localDate('2026-11-13 18:30:00'));
  assert.equal(localDate('2026-11-01'), '11 月 1 日');
  assert.equal(localDate(''), '');
});

// 缩略图按物理像素选档（#452）：CSS 尺寸乘屏幕倍率和 260 比；倍率按 1 封底，1 倍屏的结果和只看 CSS 像素一样。
test('thumbnail tier follows physical pixels', () => {
  const {thumbSize, screenScale} = web.core();
  assert.deepEqual([180, 260, 261].map(px => thumbSize(px, 1)), ['thumb', 'thumb', 'medium']);
  assert.deepEqual([130, 131, 180].map(px => thumbSize(px, 2)), ['thumb', 'medium', 'medium']);
  assert.deepEqual([180, 261].map(px => thumbSize(px, .5)), ['thumb', 'medium']);
  assert.deepEqual([undefined, 0, NaN, .5, 1.5, 2].map(screenScale), [1, 1, 1, 1, 1.5, 2]);
});

// 日期的键（本机日期的「YYYY-MM-DD」「YYYY-MM」）和按键写的日期：读键按年月日构造，不经字符串解析（不受时区、夏令时影响）。
test('date keys use the local date; formatted dates read the key as that local day', () => {
  const {dayKey, monthKey, parseDay, weekdayShort, dayLong, monthName, monthShort, dayWithWeekday, monthLong} = web.core();
  assert.equal(dayKey(new Date(2026, 0, 5, 23, 59)), '2026-01-05');
  assert.equal(monthKey(new Date(2026, 0, 31, 23, 59)), '2026-01');
  assert.deepEqual([parseDay('2026-03-08').getDate(), parseDay('2026-03-08').getHours(), parseDay('2026-03').getDate()], [8, 0, 1]);
  assert.equal(weekdayShort('2024-01-01'), new Intl.DateTimeFormat('zh-CN', {weekday: 'short'}).format(new Date(2024, 0, 1)));
  assert.equal(dayLong('2026-11-03'), '2026年11月3日');
  assert.equal(monthName('2026-11'), '11 月');
  assert.equal(monthShort('2026-03'), new Intl.DateTimeFormat('zh-CN', {month: 'short'}).format(new Date(2026, 2, 1)));
  assert.equal(monthLong('2026-03'), '2026年3月');
  assert.match(dayWithWeekday('2026-11-03'), /11月3日/);
});
