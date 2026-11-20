// 收藏（#447）：不碰 DOM 的部分。收藏是一个人工标记，后端写在 /api/favorite，筛选条件是 has / not 里的 `fav`，列表、
// 详情、按天视图带 marks=fav 时每张图多一个 `fav` 字段（口径：这张或同组任何一份收藏过）。画在哪、怎么写入在 ui/favorites.js
// 和 ui/grid.js。可以在 Node 里单独测试（tests/favorites.test.cjs）。
import {state} from './state.js';

// 读列表、详情时带上的参数（按天视图带 marks=fav,color，core/days.js）：回复里每张图多一个 fav。
export const MARKS = {marks: 'fav'};
// 网格卡片要的标记：收藏，再加上每张图的主色（读完之前的底色、悬停投影，#717）。首页、详情栏以后要主色时也用它。
export const GRID_MARKS = {marks: 'fav,color'};
export const FAV = 'fav';

// 路由或查询参数里的 has / not：条件数组。地址和 /api/assets 的参数里是 JSON 字符串（#445），读成路由后可能已是数组。
export function conditions(value) {
  if (Array.isArray(value)) return value;
  if (typeof value !== 'string' || !value) return [];
  try { const list = JSON.parse(value); return Array.isArray(list) ? list : []; } catch { return []; }
}
// 写入以后这张图是不是离开本页：「只看收藏」里取消，「不含收藏」里收藏。
export const favLeaves = (add, route = state.route) => add ? conditions(route.not).includes(FAV) : conditions(route.has).includes(FAV);
// 只按收藏筛选的图库（没有别的条件）：判断写在 core/pages.js（注册表要用它，放在这里会和注册表互相导入）。
export {favoritesOnly} from './pages.js';
// 在一组查询参数的 key（has 或 not）里加一个条件，返回新的一组；参数里条件写成 JSON 字符串数组，同 /api/assets。
export function withCondition(params, key, condition) {
  const list = conditions(params[key]);
  return list.includes(condition) ? {...params} : {...params, [key]: JSON.stringify([...list, condition])};
}

// 一张图现在是不是收藏：列表里的 fav，没有时看详情栏读到的那份（面板页借来的列表只有 uuid）。
export function isFav(item, s = state) {
  if (!item) return false;
  if (typeof item.fav === 'boolean') return item.fav;
  return s.detail?.uuid === item.uuid ? !!s.detail.fav : false;
}
// 选中的图该收藏还是取消：只要有一张还没收藏（或有选中的图还没读到）就收藏，否则全部取消。
// items 是读到的选中的图，size 是选中的张数。
export const wantsAdd = (items, size = items.length) => items.length < size || items.some(x => !isFav(x));

// 写入成功后在本地改掉这些图的 fav，不重读列表：选中的图和它们的同组副本（dup.grp 相同）。
// pick(item) 判断一张图是不是这次写入的；返回改到的 uuid。
export function markLocal(list, pick, on) {
  const groups = new Set(), changed = new Set();
  for (const x of list) if (x && pick(x)) { changed.add(x.uuid); if (x.dup?.grp != null) groups.add(x.dup.grp); }
  for (const x of list) {
    if (!x) continue;
    if (!changed.has(x.uuid) && x.dup?.grp != null && groups.has(x.dup.grp)) changed.add(x.uuid);
    if (changed.has(x.uuid)) x.fav = on;
  }
  return changed;
}
