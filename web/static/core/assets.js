// 图片列表的取数（#706、#708）：页面样板里 ui/ 不直接发请求（tools/check_frontend.py 规则 22），要读图片列表的地方经这里。
// 网格（ui/grid.js）的查询参数、分页读取、计数和按筛选取 id 都在这里；相似组页「判断关系」要的前几张图的 id 也是。
import {MARKS} from './favorites.js';
import {pageOf} from './pages.js';
import {defaultSort} from './route.js';
import {api} from './session.js';
import {state} from './state.js';

// 网格一页读几张。
export const PAGE = 100;
// 网格现在这一页的查询参数：排序、页面登记的 pool 和参数（core/pages.js；图库的筛选在 params 里）、角色页的角色和状态、NSFW 设置。
// 选择栏按筛选写入（{filter: …}）时也用它，所以写入和列表是同一个选区。
export function gridFilters(r = state.route) {
  const sort = r.sort || defaultSort(r), f = {order: sort === 'suspicious' && r.char && state.char?.mascot ? 'newest' : sort};   // 标签页没有「可疑优先」
  const page = pageOf(r);
  if (!page.pool) Object.assign(f, page.params?.(r));
  if (r.char) { f.entity = r.char; if (r.maybe) f.status = 'maybe'; else if (r.doubt) f.status = 'ambiguous'; else { if (r.variant) f.variant = r.variant; if (r.check) f.status = 'machine'; } }
  if (page.pool) { f.pool = page.pool; Object.assign(f, page.params?.(r)); }
  if (state.nsfw) f.nsfw = state.nsfw;
  return f;
}
// 网格的一页：从第 offset 张起 PAGE 张。count 为假时后端不重新数总数（翻页时用）。卡片要收藏标记和主色（主色是图读完之前的
// 占位、悬停时的投影，#717）。
export const readPage = (offset, {count = true, signal} = {}) =>
  api('/api/assets?' + new URLSearchParams({...gridFilters(), offset, limit: PAGE, marks: `${MARKS.marks},color`, ...(count ? {} : {count: 0})}), {signal});
// 符合这组查询参数（/api/assets 的筛选和 pool）的图有几张。
export const countAssets = params => api('/api/assets/count?' + new URLSearchParams(params)).then(x => x.total);
// 符合这组查询参数的前 limit 张图的 id（params 里可以带 offset）。
export const listIds = (params, limit) => api('/api/ids?' + new URLSearchParams({...params, limit}));
