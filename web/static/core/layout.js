// 布局纯函数（0.4 渲染底座，#704）：输入宽高比和宽度，输出框。不碰 DOM、不量尺寸，Node 里能直接测。
//
// 框的形状统一是 {i, x, y, w, h}：i 是输入里的序号，x、y 相对布局的左上角，单位 px。调用方给框配上自己的键
// （通常是图的 uuid）交给 ui/virtual.js 去挂节点；FLIP 的起止位置也直接用这里算出的框，不量 DOM。
//
// 现在有这几种，各页步骤按需在这里加自己的（画廊的拼贴、杂志、展厅由 #710 加）：
//   justifiedRows / justified  等高行：图库网格的原始比例模式（规则见 docs/WEB_V2.md「浏览界面」），按天、画廊也用
//   masonry                    瀑布：固定列宽，每张放进当前最矮的一列
//   dayMosaic                  按天的马赛克：左边一张大图，右边两行小图，放不下的收成最后一格上的「+N」
//   monthGrid / cellTiles      日历：某月的格子（含前后月补齐的日子），以及格内 1 / 2 / 4 张图的拼法
//
// 宽高比：尺寸还不知道的图，调用方先用 estimateRatio 估（照网格的做法），再交进来。瀑布和马赛克另外兜底：0、负数、
// NaN 这类不是正的有限数的宽高比按 3:4 算，不会算出 NaN 的框或一张窄条。
//
// 新加一种布局：写成「数据进、框出」的纯函数，固定输入在 tests/layout.test.cjs 里比对输出的框；不要在 ui/ 里另算一份。

// ---------- 宽高比 ----------
// 方框的宽高比有上下限，全景图或长条图不会把它那一行压扁或撑大；这样的图在方框里整张显示（留底色），不裁切。
export const FIT_MIN = .5, FIT_MAX = 2.4;
// 还不知道尺寸、也没有别的图可以参考时按 3:4 估。
export const DEFAULT_RATIO = .75;
// 图本身的宽高比；尺寸未知时是 0。
export const aspect = item => item?.width > 0 && item?.height > 0 ? item.width / item.height : 0;
export const clampRatio = r => Math.min(FIT_MAX, Math.max(FIT_MIN, r));
// 不是正的有限数（0 表示尺寸未知，或者算坏了的 NaN）按 3:4 算。
const usable = r => Number.isFinite(r) && r > 0 ? r : DEFAULT_RATIO;
// 已知尺寸的图（ratioAt(i) 非 0）的平均宽高比（按上下限截过），用来估还没读到尺寸的图；一张都没有时是 3:4。
export function estimateRatio(n, ratioAt) {
  let sum = 0, known = 0;
  for (let i = 0; i < n; i++) { const r = ratioAt(i); if (r) { sum += clampRatio(r); known++; } }
  return known ? sum / known : DEFAULT_RATIO;
}

// ---------- 等高行 ----------
// 每行依次放图，直到再放一张会超出宽度时，选更接近目标行高的分法（收进这一张或不收），然后整行等高拉满宽度；
// 末行不拉满，行高取自然高度和目标行高里较小的，靠左。
//
// n 张图，ratioAt(i) 给第 i 张的宽高比（调用方自己截上下限、估未知的）。选项：width 可用宽度，target 目标行高，
// gap 图之间的缝，extra 每行额外加的高度（标题条和行间距；行高 = 图高 + extra）。
// 结果写进可复用的类型化数组（十万张图时不为每张新建对象），传入上一次的结果就复用它的数组：
//   {rows, starts, tops, heights}：starts[r] 是第 r 行第一张的序号，starts[rows] = n；tops[r] 是行顶，
//   tops[rows] 是总高；heights[r] 是行高（含 extra）。
export function justifiedRows(n, ratioAt, {width, target, gap = 0, extra = 0}, reuse = null) {
  let {starts, tops, heights} = reuse || {};
  if (!starts || starts.length < n + 2) { starts = new Int32Array(n + 2); tops = new Float64Array(n + 2); heights = new Float64Array(n + 1); }
  let row = 0, i = 0;
  tops[0] = 0; starts[0] = 0;
  while (i < n) {
    const start = i; let total = 0;
    for (;;) {
      const r = ratioAt(i), count = i - start + 1;
      if (count > 1 && (total + r) * target + gap * (count - 1) > width) {
        // 只有这样更接近目标高度时，才把溢出的那张图收进来。
        const shrunk = (width - gap * (count - 1)) / (total + r), grown = (width - gap * (count - 2)) / total;
        if (target - shrunk < grown - target) { total += r; i++; }
        break;
      }
      total += r; i++;
      if (i >= n) break;
    }
    const natural = (width - gap * (i - start - 1)) / total;
    starts[row] = start; heights[row] = (i >= n ? Math.min(natural, target) : natural) + extra;
    tops[row + 1] = tops[row] + heights[row]; row++;
  }
  starts[row] = n;
  return {rows: row, starts, tops, heights};
}

// 等高行的框：ratios 是宽高比数组（已截过上下限），选项同 justifiedRows。返回 {boxes, height}；框的 h 是图高（不含 extra）。
// 图少（一天、一个画廊）时用它；整个图库的网格走 justifiedRows，自己按行算框。
export function justified(ratios, options) {
  const {gap = 0, extra = 0} = options, packed = justifiedRows(ratios.length, i => ratios[i], options);
  const boxes = [];
  for (let row = 0; row < packed.rows; row++) {
    const h = packed.heights[row] - extra;
    let x = 0;
    for (let i = packed.starts[row]; i < packed.starts[row + 1]; i++) {
      const w = ratios[i] * h; boxes.push({i, x, y: packed.tops[row], w, h}); x += w + gap;
    }
  }
  return {boxes, height: packed.tops[packed.rows]};
}

// ---------- 瀑布 ----------
// 列数由列宽目标 column 推出（至少一列），每张图按宽高比放进当前最矮的一列（一样矮时取靠左的）。
// ratios 截不截上下限由调用方定（瀑布通常保留长图的原比例）。返回 {boxes, height, columns, columnWidth}。
export function masonry(ratios, {width, column, gap = 0}) {
  const columns = Math.max(1, Math.round((width + gap) / (Math.max(1, column) + Math.max(0, gap))));
  const w = Math.max(1, (width - gap * (columns - 1)) / columns), heights = new Array(columns).fill(0), boxes = [];
  for (let i = 0; i < ratios.length; i++) {
    let c = 0; for (let k = 1; k < columns; k++) if (heights[k] < heights[c]) c = k;
    const h = w / usable(ratios[i]);
    boxes.push({i, x: c * (w + gap), y: heights[c], w, h}); heights[c] += h + gap;
  }
  return {boxes, height: Math.max(0, Math.max(...heights) - gap), columns, columnWidth: w};
}

// ---------- 按天的马赛克 ----------
// 一天的图：第一张是大图，高两行（2 × rowHeight + gap），宽按它的宽高比、最多占一半宽度；右边剩下的宽度里放两行小图，
// 每行等高 rowHeight，按宽高比依次放到放不下为止，再把这一行的宽度按比例拉满（图在框里裁切，高度不变）；图用完时
// 最后一行不放大、靠左。
// 两行放不下的图不画：最后一格的框带 more = N（N 是从这一格起没展示的张数，包括这一格自己的图），界面在这格图上盖「+N」。
// 右边窄得放不下一张小图时（< rowHeight × FIT_MIN）只画大图，它带上 more。
// 返回 {boxes, height, shown}：shown 是画出来的张数（最后一格带 more 时也算一张）。
export function dayMosaic(input, {width, rowHeight, gap = 0}) {
  const ratios = input.map(usable), n = ratios.length;
  if (!n) return {boxes: [], height: 0, shown: 0};
  const bigH = rowHeight * 2 + gap;
  if (n === 1) {
    const w = Math.min(width, clampRatio(ratios[0]) * bigH);
    return {boxes: [{i: 0, x: 0, y: 0, w, h: bigH}], height: bigH, shown: 1};
  }
  const bigW = Math.min(width / 2, clampRatio(ratios[0]) * bigH), boxes = [{i: 0, x: 0, y: 0, w: bigW, h: bigH}];
  const left = bigW + gap, room = width - left;
  if (room < rowHeight * FIT_MIN) { boxes[0].more = n; return {boxes, height: bigH, shown: 1}; }
  let i = 1;
  for (let line = 0; line < 2 && i < n; line++) {
    const row = [], y = line * (rowHeight + gap);
    let used = 0;
    while (i < n) {
      const w = clampRatio(ratios[i]) * rowHeight, need = used + (row.length ? gap : 0) + w;
      if (row.length && need > room) break;
      row.push({i, w}); used = need; i++;
      if (used >= room) break;
    }
    // 拉满这一行：缝不动，图宽按比例放大或缩小到正好填满。图用完了、这一行没放满时不放大（同等高行的末行），靠左。
    const fill = (room - gap * (row.length - 1)) / (used - gap * (row.length - 1)), scale = i >= n ? Math.min(1, fill) : fill;
    let x = left;
    for (const cell of row) { const w = cell.w * scale; boxes.push({i: cell.i, x, y, w, h: rowHeight}); x += w + gap; }
  }
  if (i < n) boxes[boxes.length - 1].more = n - i + 1;
  return {boxes, height: bigH, shown: boxes.length};
}

// ---------- 日历 ----------
// year 年 month 月（1–12）的月格：从包含 1 号的那一周排到包含月末的那一周，每周 7 格，周首由 weekStart 定（0 = 周日，
// 1 = 周一，默认周一）。返回 {weeks, cells}，cells 依次是 {date: 'YYYY-MM-DD', day, inMonth, row, col}。
// 日期按公历纯算（UTC），不受时区影响。
export function monthGrid(year, month, {weekStart = 1} = {}) {
  const first = Date.UTC(year, month - 1, 1), days = new Date(Date.UTC(year, month, 0)).getUTCDate();
  const lead = (new Date(first).getUTCDay() - weekStart + 7) % 7, weeks = Math.ceil((lead + days) / 7), cells = [];
  for (let k = 0; k < weeks * 7; k++) {
    const d = new Date(first + (k - lead) * 864e5);
    cells.push({date: d.toISOString().slice(0, 10), day: d.getUTCDate(), inMonth: d.getUTCMonth() === month - 1, row: Math.floor(k / 7), col: k % 7});
  }
  return {weeks, cells};
}

// 日历格里的图：n 张时画几张、怎么拼。0 张不画；1 张占满；2、3 张左右各一半；4 张及以上 2 × 2。
// 返回框（相对格子左上角），i 是第几张图。
export function cellTiles(n, {width, height, gap = 0}) {
  if (n <= 0) return [];
  if (n === 1) return [{i: 0, x: 0, y: 0, w: width, h: height}];
  const w = (width - gap) / 2;
  if (n < 4) return [0, 1].map(i => ({i, x: i * (w + gap), y: 0, w, h: height}));
  const h = (height - gap) / 2;
  return [0, 1, 2, 3].map(i => ({i, x: (i % 2) * (w + gap), y: Math.floor(i / 2) * (h + gap), w, h}));
}
