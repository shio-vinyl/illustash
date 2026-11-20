// 视图选项的值（右上「视图」浮层里的开关，按浏览器保存；#708 审核从 ui/grid.js 拆出）。网格、首页、详情栏读；浮层
// （ui/grid_view.js）改了以后调 saveView() 存下。这里的一切都是可选的细节：默认保持安静。
import {prefs} from './prefs.js';

export const VIEW_DEFAULTS = {marks: true, hints: true, footer: false, chart: true, same: true, meta: true, homeSize: 1, hero: true, people: false, ask: true};
// 只认现有的键：旧版本存过、后来去掉的选项（如「图片下方显示角色名」captions，#708）读的时候丢掉，不再写回去。
const saved = prefs.get('view', {}) || {};
export const viewOpts = {...VIEW_DEFAULTS, ...Object.fromEntries(Object.entries(saved).filter(([k]) => k in VIEW_DEFAULTS))};
/** 存下当前的视图选项。 */
export function saveView() { prefs.set('view', viewOpts); }
/** 恢复默认并存下。 */
export function resetView() { Object.assign(viewOpts, VIEW_DEFAULTS); saveView(); }
