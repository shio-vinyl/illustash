// 图库网格的行坐标（#452 的成果，#708 从 ui/grid.js 拆出）：按视口宽度和缩放分行，按位置或序号查行，算每张卡片的框。
// 网格的挂卡片、键盘、框选和缩放都用这一套坐标；详情栏的「滚到这张」（ui/detail.js 的 ensureVisible）也用。
//
// - 方格：每行 state.cols 张，等宽等高。原始比例：两端对齐的行（core/layout.js 的 justifiedRows，规则见 docs/WEB_V2.md
//   「浏览界面」）。行起点、行顶和行高放在复用的类型化数组里，按位置或序号查行都是二分查找。
// - 还没读到的图用已读到的图的平均宽高比估，分页到达时各行尽量少挪。
// - 网格顶部可以有一块跟着滚动的头部（角色页的横幅，#712）：它是 #grid 的第一个子节点，高度由 setHead() 告诉这里，
//   所有行顶都往下挪这么多。所以「滚动位置 ↔ 网格坐标」的换算照旧，不用各处另减头部高度。
// - 坐标都相对 #grid 的左上角。
// - 瀑布（画廊，#710）：等宽的几列，每张放进当时最矮的一列（core/layout.js 的 masonryRows，包着 masonry）。按序号每「列数」张
//   算一行，行顶是这一行第一张的顶（各张的顶随序号不减），所以按位置查行照旧是二分查找；但一张图可能比好几行还高，所以从
//   视口顶往上 rowReach()（最高那张的高度）起挂卡片、算框选。
// - 排法由页面定：注册表登记了 gridLayout（core/pages.js，画廊按展示方式给瀑布或色彩的等高行）时照它，工具栏的「方格 /
//   原始比例」开关不起作用（fixedLayout()）；没登记时看那个开关。再加一种排法：在 layout() 里加一个分支，把它的框写进
//   cardRect、verticalStep、indexAt，行数组照上面的口径填。下面这几个导出的函数（rowCount、rowStart、rowOf、rowTop、
//   rowHeight、rowAt、rowReach、cardRect、verticalStep、indexAt）是网格读几何的唯一入口，ui/grid.js 不另算框。
import {DEFAULT_RATIO, aspect, clampRatio, estimateRatio, justifiedRows, masonryRows, masonryStep} from '../core/layout.js';
import {pageOf} from '../core/pages.js';
import {state} from '../core/state.js';
import {$} from './dom.js';

// 浏览器滚过约 33M px 就不再滚动；宁可加列，也不让网格坏掉。
const MAX_HEIGHT = 3e7;
// 这一次的排法：'square' 方格、'justified' 两端对齐的等高行、'masonry' 瀑布。后两种的行放在下面的数组里。
let kind = 'square';
const isFit = () => kind !== 'square';
const isStack = () => kind === 'masonry';
let key = '', fitRows = 0, estimate = DEFAULT_RATIO, packed = null, stacked = null, head = 0;
let rowStarts = new Int32Array(1), rowTops = new Float64Array(1), rowHeights = new Float64Array(0);

// 页面定死的排法 {kind, scale}（scale 乘在缩放滑块的格宽上）；没有时是 null，照「方格 / 原始比例」开关。
export const fixedLayout = (r = state.route) => pageOf(r).gridLayout?.(r) || null;
// 现在是不是按图的比例排（两端对齐或瀑布）：卡片据此对超宽超长的图整张显示。
export const flowing = () => isFit();
// 一张图最多比它所在的行高出多少：从视口顶往上这么多的行里，也可能有伸进视口的图（只有瀑布不是 0）。
export const rowReach = () => isStack() ? stacked.reach : 0;
// 这张图在原始比例里按多宽排：宽高比截过上下限；尺寸还不知道的用估计值。
export const imageRatio = item => { const r = aspect(item); return r ? clampRatio(r) : estimate; };
export const rowCount = () => isFit() ? fitRows : Math.ceil(state.total / state.cols);
export const rowStart = row => isFit() ? rowStarts[Math.max(0, Math.min(row, fitRows))] : Math.max(0, Math.min(row * state.cols, state.total));
export function rowOf(i) {
  if (!isFit()) return Math.floor(i / state.cols);
  let lo = 0, hi = Math.max(0, fitRows - 1);
  while (lo < hi) { const mid = (lo + hi + 1) >>> 1; if (rowStarts[mid] <= i) lo = mid; else hi = mid - 1; }
  return lo;
}
// 行顶（含顶部头部的高度）；rowTop(rowCount()) 是网格的总高。
export function rowTop(row) { return head + (isFit() ? rowTops[Math.max(0, Math.min(row, fitRows))] : row * state.rowH); }
export function rowHeight(row) { return isFit() ? rowHeights[row] || state.rowH : state.rowH; }
// 网格坐标 y 落在哪一行（头部里的算第 0 行）。
export function rowAt(y) {
  y -= head;
  if (!isFit()) return Math.max(0, Math.floor(y / state.rowH));
  let lo = 0, hi = fitRows;
  while (lo < hi) { const mid = (lo + hi) >>> 1; if (rowTops[mid + 1] <= y) lo = mid + 1; else hi = mid; }
  return Math.min(lo, Math.max(0, fitRows - 1));
}
// 第 i 张卡片的框 {x, y, w, h}。
export function cardRect(i) {
  if (isStack()) return {x: stacked.x[i], y: head + stacked.y[i], w: stacked.w[i], h: stacked.h[i]};
  const row = rowOf(i);
  if (!isFit()) return {x: (i % state.cols) * (state.cellW + state.gap), y: rowTop(row), w: state.cellW, h: state.rowH - state.gap};
  const h = rowHeight(row) - state.gap;
  let x = 0; for (let j = rowStart(row); j < i; j++) x += imageRatio(state.items[j]) * h + state.gap;
  return {x, y: rowTop(row), w: imageRatio(state.items[i]) * h, h};
}
// 方向键上 / 下：相邻行里中心在水平方向上最接近的那张图；瀑布是同一列的上一张、下一张。到头了和两端对齐一样去第一张、最后一张。
export function verticalStep(i, dir) {
  if (isStack()) { const j = masonryStep(stacked, state.total, i, dir); return j !== i ? j : dir < 0 ? 0 : state.total - 1; }
  if (!isFit()) return Math.max(0, Math.min(state.total - 1, i + dir * state.cols));
  const row = rowOf(i) + dir; if (row < 0 || row >= fitRows) return dir < 0 ? 0 : state.total - 1;
  const box = cardRect(i), x = box.x + box.w / 2;
  let best = rowStart(row), dist = Infinity;
  for (let j = rowStart(row); j < rowStart(row + 1); j++) { const b = cardRect(j), d = Math.abs(b.x + b.w / 2 - x); if (d < dist) { dist = d; best = j; } }
  return best;
}
// 屏幕上的一点落在第几张（clamp 为假时落在网格外是 -1）。
export function indexAt(clientX, clientY, clamp = true) {
  const rect = $('grid').getBoundingClientRect(), x = clientX - rect.left, y = clientY - rect.top;
  if (isStack()) return stackAt(x, y, clamp);
  const row = rowAt(y), start = rowStart(row), end = rowStart(row + 1);
  let i = start;
  while (i < end - 1) { const box = cardRect(i); if (x < box.x + box.w + state.gap) break; i++; }
  if (!clamp && (x < 0 || y < head || y >= rowTop(rowCount()))) return -1;
  return clamp ? Math.max(0, Math.min(i, state.total - 1)) : i;
}

// 瀑布里一点落在第几张：先按横坐标定列，再在这一列里找竖着盖住它的那张；落在两张之间的缝里算上面那张。
function stackAt(x, y, clamp) {
  const {columns, columnWidth, col, y: top} = stacked, gy = y - head;
  const c = Math.max(0, Math.min(columns - 1, Math.floor(x / (columnWidth + state.gap))));
  // 各张的顶随序号不减：这一列里最后一张顶在 gy 以上的就是盖住它（或它上面缝里）的那张。再往前的盖不到这里。
  let hit = -1;
  for (let i = rowStart(rowAt(y - stacked.reach - state.gap)), end = rowStart(rowAt(y) + 1); i < end; i++) if (col[i] === c && top[i] <= gy) hit = i;
  if (!clamp && (x < 0 || gy < 0 || y >= rowTop(rowCount()) || hit < 0)) return -1;
  return Math.max(0, Math.min(hit < 0 ? rowStart(rowAt(y)) : hit, state.total - 1));
}

// 顶部头部的高度（0 是没有）。变了以后下一次 layout() 重新分行。
export function setHead(height) { height = Math.max(0, Math.round(height || 0)); if (height !== head) { head = height; key = ''; } }
// 网格头部的高度变了（ui/grid.js 的 ResizeObserver 报的边框盒高度）：记进行坐标。视口已经滚过头部时（回到原位置 #402 以后，
// 横幅的资料才读到、变高），滚动位置加上高度差，看着的那几行不动；先撑高网格，滚动位置才不会被截住。
export function headResized(height) {
  height = Math.max(0, Math.round(height || 0));
  const v = $('viewport'), shift = height !== head && v.scrollTop > head ? height - head : 0;
  setHead(height);
  if (shift) { $('grid').style.height = `${rowTop(rowCount())}px`; v.scrollTop += shift; }
}
// 这一次分行的键：宽度、缩放、显示方式、张数、数据版本、头部变了就不同。网格用它判断要不要重新定位、重挂卡片。
export const layoutKey = () => key;

function packRows(width, target) {
  const n = state.total, items = state.items;
  estimate = estimateRatio(n, i => aspect(items[i]));
  packed = justifiedRows(n, i => imageRatio(items[i]), {width, target, gap: state.gap, extra: state.gap}, packed);
  ({starts: rowStarts, tops: rowTops, heights: rowHeights, rows: fitRows} = packed);
}
// 瀑布的宽高比也截在上下限里（0.5–2.4，同原始比例）：长条图在框里留底色，换来 rowReach() 有上限（最高一张不超过列宽的两倍）。
function stackColumns(width) {
  const n = state.total, items = state.items;
  estimate = estimateRatio(n, i => aspect(items[i]));
  stacked = masonryRows(n, i => imageRatio(items[i]), {width, column: state.cellW, gap: state.gap}, stacked);
  ({starts: rowStarts, tops: rowTops, heights: rowHeights, rows: fitRows} = stacked);
}
// 按视口宽度和目标格宽（state.size，乘上页面排法的 scale）分行；和上一次一样就什么也不做。
export function layout() {
  const width = Math.max(1, $('viewport').clientWidth - 2), fixed = fixedLayout();
  const nextKind = fixed?.kind || (state.fit ? 'justified' : 'square'), size = state.size * (fixed?.scale || 1);
  state.gap = state.size < 130 ? 6 : 12;
  let cols = Math.max(1, Math.round((width + state.gap) / (size + state.gap)));
  const next = `${width}:${size}:${nextKind}:${state.total}:${state.dataVersion}:${head}`;
  if (next === key) return;
  key = next; kind = nextKind;
  for (;;) {
    state.cols = cols; state.cellW = Math.max(1, (width - state.gap * (cols - 1)) / cols);
    state.rowH = Math.round(state.cellW + state.gap);
    // 3:4 的插画（常见情况）和方形格子同宽。瀑布的列宽就是格宽。
    if (kind === 'justified') packRows(width, state.cellW * 4 / 3);
    else if (kind === 'masonry') stackColumns(width);
    else fitRows = 0;
    if (rowTop(rowCount()) <= MAX_HEIGHT || cols > 40) break;
    cols++;
  }
}
