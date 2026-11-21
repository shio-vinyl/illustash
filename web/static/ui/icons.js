// 界面上的线条图标：图形只在这里写一份。icon(名) 每次给一个新的 <svg> 节点，放进模板里留的空位（refs.heart.append(icon('heart'))）
// 或换掉占位节点；大小、描边、填充由所在部件的样式定（css/ 下各文件的 `… > svg`）。名字不在表里时抛错。
//
//   icon('heart')        收藏的心形（网格卡片、按天视图、详情卡片、活页条、画廊卡片）
//   icon('pin')          图钉：钉到活页条的按钮、手选画廊
//   icon('funnel')       按条件的画廊
//   icon('search')       搜索框的放大镜（24 × 24），空状态里「没有找到」也用它
//   整理各队列（活页条）：unmatched、doubts、duplicates、excluded、audit、identity；没有封面的画廊：gallery
//   画廊的展示方式（展示页的分段切换）：collage、magazine、masonry、hall、color；别的展示方式用 square
//
// 空状态的大图标（24 × 24、细描边）只有 ui/empty.js 用，按 emptyIcon 的键取，留在那里。还拼 HTML 字符串的工具栏
// （ui/shell.js 的搜索框、ui/catalog.js 的作品浮层）用 searchHTML()，迁成节点以后删掉。
import {tpl} from './el.js';

const ICONS = {
  heart: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 16.4S3.4 12.5 3.4 7.9A3.3 3.3 0 0 1 10 6.3a3.3 3.3 0 0 1 6.6 1.6c0 4.6-6.6 8.5-6.6 8.5Z"/></svg>`,
  pin: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M12.4 3.1 16.9 7.6M13.9 4.6 10.6 7.6 6.9 8.1 11.9 13.1 12.4 9.4 15.4 6.1M9.4 10.6 4 16"/></svg>`,
  funnel: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3.5 4.5h13l-5 6v5l-3-1.5v-3.5z"/></svg>`,
  search: tpl`<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="10" cy="10" r="6.5"/><path d="m15 15 5 5"/></svg>`,
  unmatched: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="7.2"/><path d="M8 7.8a2.2 2.2 0 1 1 3 2c-.6.3-1 .8-1 1.4v.4M10 14.2v.1"/></svg>`,
  // 可能认错（#787）：两个叠在一起的圆，是 A 还是 B。
  doubts: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="7.6" cy="10" r="4.8"/><circle cx="12.4" cy="10" r="4.8"/></svg>`,
  duplicates: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3.5" y="6" width="9.5" height="11" rx="2"/><path d="M7 3h7.5A2.5 2.5 0 0 1 17 5.5V13"/></svg>`,
  excluded: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="7.2"/><path d="M5 15 15 5"/></svg>`,
  audit: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="4" y="3.5" width="12" height="14" rx="2"/><path d="M7.5 9.8 9.3 11.6 12.8 8"/></svg>`,
  identity: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 7h11l-3-3M16 13H5l3 3"/></svg>`,
  gallery: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="4.5" width="14" height="11" rx="2"/><path d="m3.5 13.5 4-4 3 3 2-2 4 4"/></svg>`,
  collage: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="4" width="8" height="7" rx="1" transform="rotate(-8 7 7.5)"/><rect x="9" y="9"
    width="8" height="7" rx="1" transform="rotate(7 13 12.5)"/></svg>`,
  magazine: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="3" width="8" height="14" rx="1"/><path d="M13.5 4.5h3.5M13.5 8h3.5M13.5 11.5h3.5M13.5 15h3.5"/></svg>`,
  masonry: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="3" width="4" height="8" rx="1"/><rect x="8" y="3" width="4" height="5" rx="1"/><rect
    x="13" y="3" width="4" height="9" rx="1"/><rect x="3" y="12" width="4" height="5" rx="1"/><rect x="8" y="9" width="4" height="8" rx="1"/><rect x="13" y="13"
    width="4" height="4" rx="1"/></svg>`,
  hall: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M2 16.5h16"/><rect x="3.5" y="6" width="4" height="6" rx=".6"/><rect x="9" y="5" width="3"
    height="7" rx=".6"/><rect x="13.5" y="6.5" width="3.5" height="5" rx=".6"/></svg>`,
  color: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="7" cy="7.5" r="3.5"/><circle cx="13" cy="7.5" r="3.5"/><circle cx="10" cy="12.5" r="3.5"/></svg>`,
  square: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="3" width="14" height="14" rx="2"/></svg>`,
};

/** 名字在表里没有：调用方要先判断时用。 */
export const hasIcon = name => Object.hasOwn(ICONS, name);
/** 一个新的图标节点。 */
export function icon(name) {
  if (!hasIcon(name)) throw new Error(`ui/icons.js: 没有叫 ${name} 的图标`);
  return ICONS[name]().el;
}
// 拼 HTML 字符串的工具栏里的放大镜（和上面的 search 同一个图形；那里没有样式定大小和描边，写在特性上）。
export const searchHTML = () => '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><circle cx="10" '
  + 'cy="10" r="6.5"/><path d="m15 15 5 5"/></svg>';
