// 画廊的展示方式（#710）：登记表和「每个画廊用哪一种」。不碰 DOM，Node 里能直接测（tests/gallery_shows.test.cjs）。
//
// 每种一项：
//   id      键：存进偏好，样式类名 .show-<id>，图标在 ui/gallery_head.js
//   label   名字
//   all     true：显示全部图，就是图库网格那一套（选择、选择栏、键盘、详情、大图），页面是 gallery（#/gallery/<id>）；
//           false：展示页，只放前 take 张，页面是 showcase（#/showcase/<id>），末尾「全部 N 张」去 ALL_SHOW
//   take    展示页放几张（一屏放得下，展示不靠滚动）
//   grid    网格那一种的分行，给 ui/grid.js（注册表 gallery 页的 gridLayout）：
//           {kind: 'masonry', scale}  瀑布，列宽 = 网格缩放档位的格宽 × scale（core/layout.js 的 masonryRows）
//           {kind: 'justified', scale} 等高行，强制原始比例，目标行高 = 原来的 × scale
//   params  这一种加给 /api/assets 的参数（色彩按颜色排：#717 的 order=color）
//   order   这一种的排序在排序下拉里叫什么（有 params.order 时下拉只有这一项）
//
// 加一种展示方式：在 SHOWS 里加一项。全部图的那一类给 grid（要新的分行就在 core/layout.js 加布局函数，ui/grid.js 按 kind 接）；
// 展示页那一类在 ui/gallery_stage.js 的 STAGES 登记画法（布局函数同样写进 core/layout.js）。分段控件、偏好、地址都从这张表来。
import {t} from './i18n.js';

export const SHOWS = [
  {id: 'collage', label: () => t('拼贴'), all: false, take: 18},
  {id: 'magazine', label: () => t('杂志'), all: false, take: 9},
  {id: 'masonry', label: () => t('瀑布'), all: true, grid: {kind: 'masonry', scale: 1.25}},
  {id: 'hall', label: () => t('展厅'), all: false, take: 12},
  {id: 'color', label: () => t('色彩'), all: true, grid: {kind: 'justified', scale: .75}, params: {order: 'color'}, order: () => t('按颜色')},
];
// 没选过时用哪一种：瀑布（几千张也能全看，选择、批量都在）；展示页的「全部」也去它。
export const DEFAULT_SHOW = 'masonry', ALL_SHOW = 'masonry';
export const showOf = id => SHOWS.find(s => s.id === id) || SHOWS.find(s => s.id === DEFAULT_SHOW);
// 这一种在哪个页面上：全部图的在网格页 gallery，展示页在 showcase。
export const pageFor = show => showOf(show).all ? 'gallery' : 'showcase';

// 每个画廊用哪一种：{画廊 id: 展示方式 id}，按图库存在浏览器本地（core/galleries.js 读写，键 'galleryShow'）。
// 这里只放读到的那份，同步可读（注册表在换页时要用）；还没读到时都是默认。
let chosen = {};
export const showFor = id => showOf(chosen[String(id)]).id;
export function setShows(map) { chosen = map && typeof map === 'object' ? {...map} : {}; }
export function chooseShow(id, show) { chosen = {...chosen, [String(id)]: showOf(show).id}; return chosen; }
// 删掉的画廊不再记着。
export function forgetShow(id) { const next = {...chosen}; delete next[String(id)]; chosen = next; return chosen; }
