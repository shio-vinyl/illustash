// core/layout.js（#704）：固定输入比对输出的框；等高行和 ui/grid.js 挪过来之前的分行逐项相同。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');

let L;
test.before(async () => { L = await web.load('core/layout.js'); });

// ui/grid.js 在 #704 之前的 packRows 原文（只把 state 换成参数），作为「分行规则原样挪」的对照。
function previousPackRows(ratios, width, target, gap, extra) {
  const n = ratios.length, rowStarts = new Int32Array(n + 2), rowTops = new Float64Array(n + 2), rowHeights = new Float64Array(n + 1);
  let row = 0, i = 0;
  rowTops[0] = 0; rowStarts[0] = 0;
  while (i < n) {
    const start = i; let total = 0;
    for (;;) {
      const r = ratios[i], count = i - start + 1;
      if (count > 1 && (total + r) * target + gap * (count - 1) > width) {
        const shrunk = (width - gap * (count - 1)) / (total + r), grown = (width - gap * (count - 2)) / total;
        if (target - shrunk < grown - target) { total += r; i++; }
        break;
      }
      total += r; i++;
      if (i >= n) break;
    }
    const natural = (width - gap * (i - start - 1)) / total;
    rowStarts[row] = start; rowHeights[row] = (i >= n ? Math.min(natural, target) : natural) + extra;
    rowTops[row + 1] = rowTops[row] + rowHeights[row]; row++;
  }
  rowStarts[row] = n;
  return {rows: row, starts: [...rowStarts.slice(0, row + 1)], tops: [...rowTops.slice(0, row + 1)], heights: [...rowHeights.slice(0, row)]};
}

test('等高行：选更接近目标行高的分法，末行不拉满', () => {
  const packed = L.justifiedRows(6, () => 1, {width: 1000, target: 200, gap: 10});
  assert.equal(packed.rows, 2);
  assert.deepEqual([...packed.starts.slice(0, 3)], [0, 5, 6]);
  // 第五张放进来时行高 192，比不放（242.5）更接近 200，所以收进来；末行一张，取目标行高 200，不拉到 1000。
  assert.deepEqual([...packed.heights.slice(0, 2)], [192, 200]);
  assert.deepEqual([...packed.tops.slice(0, 3)], [0, 192, 392]);
  const {boxes, height} = L.justified([1, 1, 1, 1, 1, 1], {width: 1000, target: 200, gap: 10});
  assert.equal(height, 392);
  assert.deepEqual(boxes.map(b => [b.i, b.x, b.y, b.w, b.h]),
    [[0, 0, 0, 192, 192], [1, 202, 0, 192, 192], [2, 404, 0, 192, 192], [3, 606, 0, 192, 192], [4, 808, 0, 192, 192], [5, 0, 192, 200, 200]]);
});

test('等高行：行高含 extra（标题条和行距），框的高度不含', () => {
  const {boxes, height} = L.justified([2, 2], {width: 810, target: 200, gap: 10, extra: 38});
  assert.equal(height, 200 + 38);
  assert.deepEqual(boxes.map(b => [b.x, b.w, b.h]), [[0, 400, 200], [410, 400, 200]]);
});

test('等高行：和网格原来的分行逐项相同', () => {
  let seed = 7;
  const random = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
  for (let k = 0; k < 300; k++) {
    const n = 1 + Math.floor(random() * 80), ratios = Array.from({length: n}, () => L.clampRatio(.3 + random() * 2.5));
    const width = 300 + random() * 1700, target = 80 + random() * 400, gap = random() < .5 ? 6 : 12, extra = random() < .5 ? 0 : 40;
    const want = previousPackRows(ratios, width, target, gap, extra);
    const got = L.justifiedRows(n, i => ratios[i], {width, target, gap, extra});
    assert.equal(got.rows, want.rows);
    assert.deepEqual([...got.starts.slice(0, got.rows + 1)], want.starts);
    assert.deepEqual([...got.tops.slice(0, got.rows + 1)], want.tops);
    assert.deepEqual([...got.heights.slice(0, got.rows)], want.heights);
  }
});

test('等高行：传入上一次的结果时复用数组，放不下才换大的', () => {
  const first = L.justifiedRows(10, () => 1, {width: 500, target: 100});
  const again = L.justifiedRows(8, () => 1, {width: 500, target: 100}, first);
  assert.equal(again.starts, first.starts);
  const bigger = L.justifiedRows(50, () => 1, {width: 500, target: 100}, first);
  assert.notEqual(bigger.starts, first.starts);
  assert.equal(L.justifiedRows(0, () => 1, {width: 500, target: 100}).rows, 0);
});

test('宽高比：上下限 0.5–2.4，未知按已知的平均估，都不知道按 3:4', () => {
  assert.equal(L.clampRatio(.2), .5);
  assert.equal(L.clampRatio(5), 2.4);
  assert.equal(L.aspect({width: 300, height: 400}), .75);
  assert.equal(L.aspect({width: 0, height: 400}), 0);
  assert.equal(L.aspect(undefined), 0);
  assert.equal(L.estimateRatio(3, i => [0, 1, 5][i]), (1 + 2.4) / 2);
  assert.equal(L.estimateRatio(2, () => 0), .75);
});

test('瀑布：放进最矮的一列，一样矮时取靠左的', () => {
  const {boxes, height, columns, columnWidth} = L.masonry([1, .5, 2, 1], {width: 310, column: 100, gap: 5});
  assert.equal(columns, 3);
  assert.equal(columnWidth, 100);
  assert.deepEqual(boxes.map(b => [b.i, b.x, b.y, b.w, b.h]), [[0, 0, 0, 100, 100], [1, 105, 0, 100, 200], [2, 210, 0, 100, 50], [3, 210, 55, 100, 100]]);
  assert.equal(height, 200);
  assert.equal(L.masonry([], {width: 50, column: 100}).columns, 1);
});

test('按天的马赛克：一张大图加两行，放不下的收成最后一格的 +N', () => {
  const close = (a, b) => assert.ok(Math.abs(a - b) < 1e-9, `${a} ≠ ${b}`);
  const many = L.dayMosaic(Array(20).fill(1), {width: 1000, rowHeight: 100, gap: 10});
  assert.equal(many.height, 210);
  assert.deepEqual([many.boxes[0].x, many.boxes[0].w, many.boxes[0].h], [0, 210, 210]);
  // 右边 780 宽：每行放 7 张 100 宽的，拉满到 102.857…；第 15 张起放不下，第 15 张那格写 +6（它自己和后面 5 张）。
  assert.equal(many.shown, 15);
  assert.deepEqual(many.boxes.slice(1).map(b => b.y), [...Array(7).fill(0), ...Array(7).fill(110)]);
  close(many.boxes[1].x, 220);
  close(many.boxes[7].x + many.boxes[7].w, 1000);
  assert.deepEqual(many.boxes.map(b => b.more).filter(Boolean), [6]);
  assert.equal(many.boxes[14].i, 14);
  // 图用完了：末行不放大，靠左。
  const few = L.dayMosaic([.75, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1], {width: 1000, rowHeight: 100, gap: 10});
  assert.deepEqual(few.boxes.slice(-3).map(b => [b.i, b.x, b.y, b.w]), [[8, 167.5, 110, 100], [9, 277.5, 110, 100], [10, 387.5, 110, 100]]);
  assert.ok(few.boxes.every(b => !b.more));
  // 一张：只有大图；右边放不下小图：大图带上全部张数。
  assert.deepEqual(L.dayMosaic([2], {width: 300, rowHeight: 100, gap: 10}).boxes, [{i: 0, x: 0, y: 0, w: 300, h: 210}]);
  const narrow = L.dayMosaic([1, 1, 1], {width: 80, rowHeight: 100, gap: 10});
  assert.deepEqual(narrow.boxes.map(b => [b.w, b.more]), [[40, 3]]);
  assert.deepEqual(L.dayMosaic([], {width: 300, rowHeight: 100}), {boxes: [], height: 0, shown: 0});
});

test('日历月格：按周首补齐前后月，日期不受时区影响', () => {
  const nov = L.monthGrid(2026, 11);   // 2026-11-01 是周日
  assert.equal(nov.weeks, 6);
  assert.deepEqual(nov.cells[0], {date: '2026-10-26', day: 26, inMonth: false, row: 0, col: 0});
  assert.deepEqual(nov.cells[6], {date: '2026-11-01', day: 1, inMonth: true, row: 0, col: 6});
  assert.deepEqual(nov.cells.at(-1), {date: '2026-12-06', day: 6, inMonth: false, row: 5, col: 6});
  assert.equal(nov.cells.filter(c => c.inMonth).length, 30);
  const feb = L.monthGrid(2026, 2, {weekStart: 0});   // 周日开头时二月正好四周
  assert.equal(feb.weeks, 4);
  assert.ok(feb.cells.every(c => c.inMonth));
  assert.equal(L.monthGrid(2024, 2).cells.filter(c => c.inMonth).length, 29);
});

test('日历格里 1 / 2 / 4 张的拼法', () => {
  const box = {width: 100, height: 80, gap: 4};
  assert.deepEqual(L.cellTiles(0, box), []);
  assert.deepEqual(L.cellTiles(1, box), [{i: 0, x: 0, y: 0, w: 100, h: 80}]);
  assert.deepEqual(L.cellTiles(3, box), [{i: 0, x: 0, y: 0, w: 48, h: 80}, {i: 1, x: 52, y: 0, w: 48, h: 80}]);
  assert.deepEqual(L.cellTiles(9, box).map(b => [b.x, b.y, b.w, b.h]), [[0, 0, 48, 38], [52, 0, 48, 38], [0, 42, 48, 38], [52, 42, 48, 38]]);
});
