// 页面在哪（#199）：把地址（#/page/section?query）读成路由再写回去，以及一个路由是哪类页面。不碰 DOM：它读写
// location.hash 和 history。
import {readLike} from './like.js';
import {prefs} from './prefs.js';
import {matchPath, pageOf, pathFor, viewOf} from './pages.js';
import {state} from './state.js';
import {parseList} from './filters.js';

// ---------- routing ----------
export function parseHash() {
  const [path, query = ''] = location.hash.replace(/^#/, '').split('?');
  const parts = path.split('/').filter(Boolean), q = new URLSearchParams(query);
  const route = {page: 'library', q: q.get('q') || '', tag: q.get('tag') || '', utag: q.get('utag') || '', dup: q.get('dup') || '', reason: q.get('reason') || '', kind: q.get('kind') || '', seen: q.get('seen') || '', maybe: q.get('maybe') || '', sort: q.get('sort') || '',
    work: q.get('work') || '', day: q.get('day') || '', artist: q.get('artist') || '', char: q.get('char') || '', variant: q.get('variant') || '', check: q.get('check') || '', doubt: q.get('doubt') || '',
    // 组合条件（#445）：has、not 是条件的 JSON 数组，from、to 是加入日期。
    has: parseList(q.get('has')), not: parseList(q.get('not')), from: q.get('from') || '', to: q.get('to') || ''};
  // 打开 App 落在首页；只有查询的地址（#/?artist=… 和其他旧链接）仍表示图库；#/c/id/skin 是旧的角色页，现在是按这个角色
  // 筛选的图库；资料库和设置有各自地址的子视图。哪个路径对应哪个页面由 core/pages.js 登记。
  Object.assign(route, matchPath(parts, !!query));
  // 角色和检查筛选只属于有图库工具栏的页面（网格、画廊）；画师和组合条件另外属于按天、日历（注册表的 filters，#706）。
  if (pageOf(route).toolbar !== 'library') Object.assign(route, {char: '', variant: '', check: '', doubt: ''});
  if (!pageOf(route).filters) Object.assign(route, {artist: '', has: [], not: [], from: '', to: ''});
  // 「更多像这张的」（#449）只属于图库本身，不和角色页叠在一起。
  Object.assign(route, readLike(q, pageOf(route).toolbar === 'library' && !route.char));
  if (!route.like && route.sort === 'similar') route.sort = '';   // 「最相似」离开例图就没有意义
  return route;
}
export function hashFor(r) {
  const path = pathFor(r);
  const q = new URLSearchParams();
  if (r.like) { q.set('like', r.like); if (r.by) q.set('by', r.by); }
  if (r.work) q.set('work', r.work); if (r.day) q.set('day', r.day); if (r.artist) q.set('artist', r.artist);
  if (r.char) q.set('char', r.char); if (r.variant) q.set('variant', r.variant); if (r.check) q.set('check', r.check); if (r.doubt) q.set('doubt', r.doubt);
  if (r.q) q.set('q', r.q); if (r.dup) q.set('dup', r.dup); if (r.reason) q.set('reason', r.reason); if (r.kind) q.set('kind', r.kind); if (r.seen) q.set('seen', r.seen); if (r.maybe) q.set('maybe', r.maybe); if (r.tag) q.set('tag', r.tag); if (r.utag) q.set('utag', r.utag); if (r.sort && r.sort !== defaultSort(r)) q.set('sort', r.sort);
  // 画廊页总是写上 has（可以是 []）：不带查询的 #/gallery/<id> 表示按保存的条件打开（ui/galleries.js）。
  if (r.has?.length || pageOf(r).gallery) q.set('has', JSON.stringify(r.has || []));
  if (r.not?.length) q.set('not', JSON.stringify(r.not)); if (r.from) q.set('from', r.from); if (r.to) q.set('to', r.to);
  return path + (q.toString() ? '?' + q : '');
}
// 排序记在哪个偏好里（#386 的 2-26）：每一类页面一个键，下拉改了就写进去，离开再回来还是它。图库和角色页的键就是
// 设置 › 通用里那两项默认排序，所以在页面上改和在设置里改是同一回事；未识别一类共用一个键，其余各页各一个。键由
// core/pages.js 的 sortPref 登记。
// 没改过排序的页面跟着设置里的图库默认排序；注册表写了 sortDefault 的（相似结果的「最相似」）跟着它。
export const sortKey = r => viewOf(r).sortPref || 'sort.' + pageOf(r).name;
export const defaultSort = r => r.char ? prefs.get('sortCharacter', 'suspicious') : prefs.get(sortKey(r), viewOf(r).sortDefault || prefs.get('sortLibrary', 'newest'));
export function go(route) { location.hash = hashFor(route); }
// 加一个图库筛选（标签、我的标签）要去的地址：在有图库工具栏的页面（图库、角色页）上叠加到现有的筛选上，在别的
// 页面去图库只按它筛选（#386 的 2-30）。工具栏和详情栏都用它。
export const filterRoute = patch => ({...(pageOf(state.route).toolbar === 'library' ? state.route : {page: 'library'}), ...patch});
// 角色是图库的一个筛选，不是单独的页面。
export const charRoute = (id, variant = '') => ({page: 'library', char: id, variant});
export const charHash = (id, variant = '') => hashFor(charRoute(id, variant));
// 换到新解析出的路由（只有 ui/shell.js applyRoute 调用）；replaceRoute 只改当前路由的参数、不触发换页。
export function setRoute(route) { state.route = route; }
export function replaceRoute(patch) { Object.assign(state.route, patch); history.replaceState(history.state, '', hashFor(state.route)); }
// ---------- 后退／前进（#402） ----------
// 桌面 App 菜单里的「后退／前进」（⌘[、⌘]）经 window.illustashHistory 调 stepHistory。只在这个页面自己的地址之间走：
// 页面打开之前的那一条是启动页或上一个图库（带一次性登录码的地址），退过去就离开了图库。所以每一条地址在
// history.state 里记着它是这个页面打开后的第几条（0 是第一条），退到 0 就不再退；前进到头浏览器自己就不动。
let entry = 0, noted = false;
// 每次地址变了都调用一次（ui/shell.js applyRoute）：新建的那条还没有序号，记成上一条的下一个；后退、前进到的那条
// 已经有。url 是要就地改写成的地址（旧地址换成新的），不给就是现在的地址。返回 true 表示回到的是一条已有的记录
// （后退、前进或刷新），false 是新的一条。
export function noteHistory(url) {
  const seq = history.state?.illustash;
  if (Number.isInteger(seq) && !url) { entry = seq; noted = true; return true; }
  const known = Number.isInteger(seq);
  if (!known) entry = noted ? entry + 1 : 0; else entry = seq;
  noted = true;
  history.replaceState({...history.state, illustash: entry}, '', url ?? location.hash);
  return known;
}
// 现在在第几条（0 是页面打开后的第一条）；还能不能后退。标题旁的「‹」按它显示（#526）。
export const historyEntry = () => entry;
export const canGoBack = () => entry > 0;
// 后退时先把序号减掉：连按两下时第二下可能早于第一下的 hashchange，不能按还没更新的序号再退一次。前进不预先加：
// 已经在最后一条时浏览器不动，预先加了就会让下一次后退越过第一条。
export function stepHistory(dir) {
  if (dir > 0) history.go(1);
  else if (entry > 0) { entry--; history.go(-1); }
}
// 整理视图（按角色筛选，或复核池）：点击即选择。
export const cleanup = () => viewOf(state.route).click === 'cleanup';
// 浏览页：点击打开详情，选择要主动开启。
export const browsing = () => viewOf(state.route).click === 'browse';
