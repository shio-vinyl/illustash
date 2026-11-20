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

test('瀑布和马赛克：列宽为 0、尺寸未知、NaN 都不出错', () => {
  const tiny = L.masonry([1, 1], {width: 10, column: 0, gap: 0});
  assert.equal(tiny.columns, 10);
  assert.ok(tiny.boxes.every(b => Number.isFinite(b.h)));
  assert.equal(L.masonry([0, NaN], {width: 100, column: 100}).boxes[1].h, 100 / .75);
  const mosaic = L.dayMosaic([0, NaN, 1], {width: 1000, rowHeight: 100, gap: 10});
  assert.ok(mosaic.boxes.every(b => [b.x, b.y, b.w, b.h].every(Number.isFinite)));
  assert.equal(mosaic.boxes[0].w, .75 * 210, 'unknown size counts as 3:4, not a 0.5 sliver');
});

// ---------- 画廊的展示布局（#710） ----------
test('固定种子：同一个种子每次一样，不同的种子不一样；哈希对同一个字符串不变', () => {
  const a = L.seeded(7), b = L.seeded(7), c = L.seeded(8);
  const take = f => Array.from({length: 5}, f);
  assert.deepEqual(take(a), take(b));
  assert.notDeepEqual(take(L.seeded(7)), take(c));
  assert.ok(take(L.seeded(1)).every(x => x >= 0 && x < 1));
  assert.equal(L.hashSeed('fav'), L.hashSeed('fav'));
  assert.notEqual(L.hashSeed('3'), L.hashSeed('4'));
});

test('瀑布按行：放法和 masonry 一样；顶部随序号不减，每 columns 张一行，reach 是最高一张', () => {
  const ratios = [1, .5, 2, 1, .75, 1.5, .6];
  const packed = L.masonryRows(ratios.length, i => ratios[i], {width: 310, column: 100, gap: 5});
  const plain = L.masonry(ratios, {width: 310, column: 100, gap: 5});
  assert.equal(packed.columns, 3);
  plain.boxes.forEach(b => assert.deepEqual([packed.x[b.i], packed.y[b.i], packed.w[b.i], packed.h[b.i]], [b.x, b.y, b.w, b.h]));
  for (let i = 1; i < ratios.length; i++) assert.ok(packed.y[i] >= packed.y[i - 1]);
  assert.equal(packed.rows, 3);
  assert.deepEqual([...packed.starts.slice(0, 4)], [0, 3, 6, 7]);
  assert.deepEqual([...packed.tops.slice(0, 4)], [0, 55, packed.y[6], plain.height]);
  assert.equal(packed.reach, 200);
  // 同一列的上一张、下一张；到头了停在原处。
  assert.equal(L.masonryStep(packed, ratios.length, 0, 1), [...packed.col].findIndex((c, j) => j > 0 && c === packed.col[0]));
  assert.equal(L.masonryStep(packed, ratios.length, 0, -1), 0);
  // 复用上一次的数组；0 张时总高 0。
  const again = L.masonryRows(2, () => 1, {width: 310, column: 100, gap: 5}, packed);
  assert.equal(again.x, packed.x);
  assert.equal(again.tops[again.rows], 100);
  assert.equal(L.masonryRows(0, () => 1, {width: 310, column: 100}).tops[0], 0);
});

test('拼贴：同一个种子排得一样；角度在 ±7° 里，框不出左右边，行与行稍微叠上', () => {
  const ratios = Array.from({length: 18}, (_, i) => [.75, 1, 1.4, .6][i % 4]);
  const one = L.collage(ratios, {width: 1200, seed: 42}), two = L.collage(ratios, {width: 1200, seed: 42});
  assert.deepEqual(one, two);
  assert.notDeepEqual(L.collage(ratios, {width: 1200, seed: 43}).boxes, one.boxes);
  assert.equal(one.boxes.length, 18);
  assert.ok(one.boxes.every(b => Math.abs(b.rot) <= 7 && b.x >= 0 && b.x + b.w <= 1200 + 1e-9 && b.z >= 1 && b.z <= 9));
  assert.ok(one.boxes.every(b => b.tape === null || (b.tape.at >= .3 && b.tape.at <= .7)));
  // 1200 宽是 6 列；第二行的图从第一行最高那张的 84% 附近开始，所以会叠上一点。
  const firstRowBottom = Math.max(...one.boxes.slice(0, 6).map(b => b.y + b.h));
  assert.ok(Math.min(...one.boxes.slice(6, 12).map(b => b.y)) < firstRowBottom);
  assert.ok(one.height >= Math.max(...one.boxes.map(b => b.y + b.h)) - 1e-9);
  assert.deepEqual(L.collage([], {width: 800}), {boxes: [], height: 0});
  assert.equal(L.collage([1, 1, 1], {width: 300, seed: 1}).boxes.length, 3);
});

test('杂志：宽时封面在左、导语和两张在右、下面一排带编号；窄时竖着排', () => {
  const ratios = Array(9).fill(.75);
  const wide = L.magazine(ratios, {width: 1200, gap: 16, lead: 96});
  const roles = wide.boxes.map(b => b.role);
  assert.deepEqual(roles.slice(0, 3), ['cover', 'side', 'side']);
  assert.ok(roles.slice(3).every(r => r === 'row'));
  const [cover, s1, s2] = wide.boxes;
  assert.equal(cover.w, Math.round((1200 - 16) * .6));
  assert.equal(s1.x, cover.w + 16);
  assert.equal(s2.y + s2.h, cover.h);
  assert.deepEqual(wide.lead, {x: cover.w + 16, y: 0, w: 1200 - cover.w - 16, h: 96});
  assert.deepEqual(wide.boxes.slice(1).map(b => b.no), wide.boxes.slice(1).map((_, k) => k + 1));
  const row = wide.boxes.filter(b => b.role === 'row');
  assert.ok(row.every(b => b.y === row[0].y && b.h === row[0].h));
  assert.ok(row.at(-1).x + row.at(-1).w <= 1200 + 1e-6);
  const narrow = L.magazine(ratios, {width: 600, gap: 16, lead: 96});
  assert.equal(narrow.boxes[0].w, 600);
  assert.equal(narrow.lead.y, narrow.boxes[0].h + 16);
  assert.equal(narrow.boxes[1].y, narrow.boxes[2].y);
  assert.deepEqual(L.magazine([], {width: 600}), {boxes: [], lead: null, height: 0});
  assert.deepEqual(L.magazine([1], {width: 1200}).boxes.map(b => b.role), ['cover']);
});

test('展厅：一排画框，高按档位轮换、宽按比例，中心在视线高度；墙宽算上两边留白', () => {
  const {boxes, width} = L.hall([1, .5, 3], {height: 500, gap: 72, frame: 12, pad: 64});
  assert.deepEqual(boxes.map(b => b.h), L.HALL_STEPS.slice(0, 3).map(s => 500 * s + 24));
  assert.equal(boxes[0].w, 500 * L.HALL_STEPS[0] + 24);
  assert.equal(boxes[1].w, 500 * L.HALL_STEPS[1] * .5 + 24);
  assert.equal(boxes[2].w, 500 * L.HALL_STEPS[2] * 2.4 + 24, '全景图按上限 2.4 算');
  boxes.forEach(b => assert.ok(Math.abs(b.y + b.h / 2 - 210) < 1e-9));
  assert.equal(boxes[1].x, boxes[0].x + boxes[0].w + 72);
  assert.equal(width, boxes[2].x + boxes[2].w + 64);
  assert.deepEqual(L.hall([], {height: 500}), {boxes: [], width: 0});
});

test('一行（strip）：等高、按宽高比放到放不下为止，不拉满；未知按 3:4，超宽的截上限', () => {
  const four = L.strip([.7, .7, .7, .7], {height: 84, max: 288, gap: 4});
  assert.deepEqual(four.boxes.map(b => [b.i, b.x, b.w, b.h]), [[0, 0, 59, 84], [1, 63, 59, 84], [2, 126, 59, 84], [3, 189, 59, 84]]);
  assert.equal(four.width, 248);
  assert.deepEqual(L.strip([4 / 3, 4 / 3, 4 / 3], {height: 84, max: 288, gap: 4}).boxes.map(b => b.w), [112, 112]);   // 第三张放不下
  assert.deepEqual(L.strip([0, NaN, 10, .1], {height: 100, max: 1000}).boxes.map(b => b.w), [75, 75, 240, 50]);     // 未知 3:4；截在 0.5–2.4
  assert.deepEqual(L.strip([3], {height: 128, max: 288}).boxes, [{i: 0, x: 0, y: 0, w: 288, h: 128}]);             // 一张也超出：限成 max
  assert.deepEqual(L.strip([], {height: 84, max: 288}), {boxes: [], width: 0});
});
