// 布局纯函数（0.4 渲染底座，#704）：输入宽高比和宽度，输出框。不碰 DOM、不量尺寸，Node 里能直接测。
//
// 框的形状统一是 {i, x, y, w, h}：i 是输入里的序号，x、y 相对布局的左上角，单位 px。调用方给框配上自己的键
// （通常是图的 uuid）交给 ui/virtual.js 去挂节点；FLIP 的起止位置也直接用这里算出的框，不量 DOM。
//
// 现在有这几种，各页步骤按需在这里加自己的（画廊的拼贴、杂志、展厅由 #710 加）：
//   justifiedRows / justified  等高行：图库网格的原始比例模式（规则见 docs/WEB_V2.md「浏览界面」），按天、画廊也用
//   masonry                    瀑布：固定列宽，每张放进当前最矮的一列
//   dayMosaic                  按天的马赛克：左边一张大图，右边两行小图，放不下的收成最后一格上的「+N」
//   strip                      一行：等高、按宽高比从前往后放到放不下，不拉满（活页条的悬停预览，#706）
//   monthGrid / cellTiles      日历：某月的格子（含前后月补齐的日子），以及格内 1 / 2 / 4 张图的拼法
//   masonryRows                瀑布按行：和 justifiedRows 一样的行数组，给图库网格那一套（画廊的瀑布，#710）
//   collage / magazine / hall  画廊的展示：拼贴、杂志、展厅（#710）；seeded、hashSeed 是它们用的固定种子随机数
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
// ratios 截不截上下限由调用方定（瀑布通常保留长图的原比例）。返回 {boxes, height, columns, columnWidth}；框另带 col（第几列）。
export function masonry(ratios, {width, column, gap = 0}) {
  const columns = Math.max(1, Math.round((width + gap) / (Math.max(1, column) + Math.max(0, gap))));
  const w = Math.max(1, (width - gap * (columns - 1)) / columns), heights = new Array(columns).fill(0), boxes = [];
  for (let i = 0; i < ratios.length; i++) {
    let c = 0; for (let k = 1; k < columns; k++) if (heights[k] < heights[c]) c = k;
    const h = w / usable(ratios[i]);
    boxes.push({i, x: c * (w + gap), y: heights[c], w, h, col: c}); heights[c] += h + gap;
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

// ---------- 一行 ----------
// 一样高的一行，宽度按宽高比（截上下限，未知的按 3:4），从前往后放，再放一张会超出 max 时停下：放几张由排得下多少
// 决定。不拉满，图宽取整。第一张自己就超出 max 时限成 max（总有一张）。返回 {boxes, width}，width 是这一行实际的宽。
export function strip(ratios, {height, max, gap = 0}) {
  const boxes = [];
  let x = 0;
  for (let i = 0; i < ratios.length; i++) {
    const w = Math.min(max, Math.round(clampRatio(usable(ratios[i])) * height)), at = boxes.length ? x + gap : 0;
    if (boxes.length && at + w > max) break;
    boxes.push({i, x: at, y: 0, w, h: height}); x = at + w;
  }
  return {boxes, width: x};
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

// ---------- 画廊的展示布局（#710） ----------
// 画廊有几种展示方式（core/gallery_shows.js 登记）。瀑布、色彩显示全部图，走图库网格那一套（masonryRows、justifiedRows）；
// 拼贴、杂志、展厅只放前几张，框在这里算。随机的部分用固定种子：同一个画廊每次排得一样。

// 字符串的 32 位哈希（FNV-1a），当种子用：画廊的 id 进、种子出。
export function hashSeed(text) {
  let h = 0x811c9dc5;
  for (const ch of String(text)) { h ^= ch.codePointAt(0); h = Math.imul(h, 0x01000193) >>> 0; }
  return h;
}
// 固定种子的随机数（mulberry32）：返回一个函数，每次调用给 [0, 1) 里的下一个数。
export function seeded(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let x = Math.imul(a ^ (a >>> 15), 1 | a);
    x = (x + Math.imul(x ^ (x >>> 7), 61 | x)) ^ x;
    return ((x ^ (x >>> 14)) >>> 0) / 4294967296;
  };
}

// 瀑布按行（给 ui/grid.js 的虚拟网格）：就是上面的 masonry，把它的框改写成网格那一套「行」的形状，不另算一遍。
// 每张图的顶部随序号不减（总是放进最矮的一列），所以可以把每 columns 张连着的图算一「行」，网格按行挂卡片的窗口照用：
//   {rows, starts, tops, heights}  同 justifiedRows：starts[r] 是第 r 行第一张，tops[r] 是那一张的顶，tops[rows] 是总高
//   x, y, w, h, col                每张图的框和它在第几列（类型化数组，传入上一次的结果就复用）
//   reach                          最高的一张有多高：挂卡片的窗口上沿要往上多挂这么多，前几行的长图才不会缺
//   columns, columnWidth           列数和列宽
export function masonryRows(n, ratioAt, {width, column, gap = 0}, reuse = null) {
  const ratios = new Array(n);
  for (let i = 0; i < n; i++) ratios[i] = ratioAt(i);
  const laid = masonry(ratios, {width, column, gap});
  let {x, y, w, h, col, starts, tops, heights} = reuse || {};
  if (!x || x.length < n) { x = new Float64Array(n); y = new Float64Array(n); w = new Float64Array(n); h = new Float64Array(n); col = new Int32Array(n); }
  if (!starts || starts.length < n + 2) { starts = new Int32Array(n + 2); tops = new Float64Array(n + 2); heights = new Float64Array(n + 1); }
  let reach = 0;
  for (const b of laid.boxes) { x[b.i] = b.x; y[b.i] = b.y; w[b.i] = b.w; h[b.i] = b.h; col[b.i] = b.col; if (b.h > reach) reach = b.h; }
  const rows = Math.ceil(n / laid.columns);
  for (let r = 0; r < rows; r++) { starts[r] = r * laid.columns; tops[r] = y[r * laid.columns]; }
  starts[rows] = n; tops[rows] = n ? laid.height : 0;
  for (let r = 0; r < rows; r++) heights[r] = tops[r + 1] - tops[r];
  return {rows, starts, tops, heights, x, y, w, h, col, reach, columns: laid.columns, columnWidth: laid.columnWidth};
}
// 瀑布里 ↑↓：同一列的上一张、下一张；到头了停在原处。
export function masonryStep(packed, n, i, dir) {
  for (let j = i + dir; j >= 0 && j < n; j += dir) if (packed.col[j] === packed.col[i]) return j;
  return i;
}

// 拼贴：前几张图像拍立得一样散放。cols 列（按宽度 2–6 列）、一行一行往下排，每张宽度、偏移、角度（±7°）、叠放次序和
// 胶带都由 seed 定；行与行稍微叠上一点。框是整张拍立得：图在里面，四周留 pad，下边再留 caption 写名字。图框的宽高比限在
// 0.62–1.5（拍立得裁过的样子），长图不会把一行撑得很高。
// 返回 {boxes, height}；框带 rot（度）、z（叠放次序 1–9）、tape（null 或 {at: 0–1 横向位置, rot}）。
export function collage(ratios, {width, seed = 1, pad = 8, caption = 22, columns = 0}) {
  const n = ratios.length, boxes = [];
  if (!n) return {boxes, height: 0};
  const cols = columns || Math.max(2, Math.min(6, Math.round(width / 210)));
  const cell = width / cols, rnd = seeded(seed);
  let top = 0, bottom = 0;
  for (let start = 0; start < n; start += cols) {
    const row = [];
    for (let c = 0; c < cols && start + c < n; c++) {
      const i = start + c, fw = cell * (.74 + rnd() * .2), fh = (fw - pad * 2) / Math.min(1.5, Math.max(.62, usable(ratios[i]))) + pad + caption;
      const x = Math.min(width - fw, Math.max(0, c * cell + (cell - fw) / 2 + (rnd() - .5) * cell * .18));
      const lift = (rnd() - .5) * cell * .14, rot = Math.round((rnd() - .5) * 140) / 10, z = 1 + Math.floor(rnd() * 9);
      const tape = rnd() > .55 ? {at: Math.round((.3 + rnd() * .4) * 100) / 100, rot: Math.round((rnd() - .5) * 200) / 10} : null;
      row.push({i, x, w: fw, h: fh, lift, rot, z, tape});
    }
    const tall = Math.max(...row.map(b => b.h));
    for (const b of row) {
      const yy = Math.max(0, top + (tall - b.h) / 2 + b.lift);
      boxes.push({i: b.i, x: b.x, y: yy, w: b.w, h: b.h, rot: b.rot, z: b.z, tape: b.tape});
      bottom = Math.max(bottom, yy + b.h);
    }
    top += tall * .84;
  }
  return {boxes, height: bottom};
}

// 杂志：封面一张大图（宽的时候占左边约 3/5，高按封面宽 × 1.15，不超过 maxCover），右边上面一段导语（lead 高），下面两张；
// 再下面一排带编号的图（一行等高行，放得下几张放几张）。窄的时候（< 720）封面占满宽，导语在下，两张并排，再一排。
// 返回 {boxes, lead, height}：框带 role（'cover'、'side'、'row'）和 no（编号，封面没有，从 1 起）；lead 是导语的框。
// 杂志的图在框里裁切（铺满），所以宽高比只影响下面那一排的分法。
export function magazine(ratios, {width, gap = 16, lead = 96, maxCover = 600, rowTarget = 200}) {
  const n = ratios.length, boxes = [];
  if (!n) return {boxes, lead: null, height: 0};
  const wide = width >= 720;
  const coverW = wide ? Math.round((width - gap) * .6) : width, coverH = Math.min(maxCover, Math.round(coverW * (wide ? 1.15 : .8)));
  boxes.push({i: 0, x: 0, y: 0, w: coverW, h: coverH, role: 'cover'});
  let no = 1, next = 1, bottom = coverH, leadBox;
  if (wide) {
    const sx = coverW + gap, sw = width - sx, sides = Math.min(2, n - 1);
    leadBox = {x: sx, y: 0, w: sw, h: lead};
    const sh = sides ? (coverH - lead - gap * sides) / sides : 0;
    for (let k = 0; k < sides; k++) boxes.push({i: next++, x: sx, y: lead + gap + k * (sh + gap), w: sw, h: sh, role: 'side', no: no++});
  } else {
    leadBox = {x: 0, y: coverH + gap, w: width, h: lead};
    bottom = coverH + gap + lead;
    const sides = Math.min(2, n - 1), sw = (width - gap) / 2, sh = Math.round(sw * 1.2);
    for (let k = 0; k < sides; k++) boxes.push({i: next++, x: k * (sw + gap), y: bottom + gap, w: sw, h: sh, role: 'side', no: no++});
    if (sides) bottom += gap + sh;
  }
  const rest = ratios.slice(next).map(r => clampRatio(usable(r)));
  if (rest.length) {
    const packed = justifiedRows(rest.length, k => rest[k], {width, target: rowTarget, gap});
    const end = packed.starts[1], hh = packed.heights[0], y = bottom + gap * 2;
    let x = 0;
    for (let k = 0; k < end; k++) { const w = rest[k] * hh; boxes.push({i: next + k, x, y, w, h: hh, role: 'row', no: no++}); x += w + gap; }
    bottom = y + hh;
  }
  return {boxes, lead: leadBox, height: bottom};
}

// 展厅：一面墙上挂一排画框，墙横向延伸（宽度不限，界面横向滚动）。画框里的图高按 HALL_STEPS 轮换（墙高 height 的比例），
// 宽按宽高比；框外再加 frame 宽的画框；画框中心对齐视线高度（墙高的 42%）。左右各留 pad，画框之间留 gap。
// 返回 {boxes, width}：框是画框的外沿。
export const HALL_STEPS = [.46, .36, .5, .4, .44, .34, .42];
export function hall(ratios, {height, gap = 72, frame = 12, pad = 64}) {
  const boxes = [], eye = height * .42;
  let x = pad;
  ratios.forEach((r, i) => {
    const ih = height * HALL_STEPS[i % HALL_STEPS.length], iw = ih * clampRatio(usable(r));
    const w = iw + frame * 2, h = ih + frame * 2;
    boxes.push({i, x, y: Math.max(0, eye - h / 2), w, h}); x += w + gap;
  });
  return {boxes, width: boxes.length ? x - gap + pad : 0};
}
