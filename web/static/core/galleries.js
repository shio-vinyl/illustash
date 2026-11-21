// 画廊（#445、#710）：列表、封面、预览样图、张数，以及写入。不碰 DOM，Node 里能直接测。
//
// 两种画廊（用户 2026-11-19 定，#703 第 1 问）：
//   按条件（kind 不写）  保存的一组筛选条件，params 就是条件
//   手选（kind: 'manual'）用户挑的一组图（#727），后端给的 params 是 {has: '["gallery:<id>"]'}，成员用 add / remove 改
// 收藏是内置的手选画廊：id 'fav'，条件是 has=["fav"]，写入走 /api/favorite（core/favorites.js）。它不在后端的列表里，
// galleryOf('fav') 和 withFav() 把它补上。
//
// 接口：GET /api/galleries 回 [{id, name, params, kind?, cover_manual, position?}]，按保存的先后；封面按可见的画廊批量取
// GET /api/galleries/covers?ids=[…]&nsfw=（#590，回复里另附封面主色 color，#717）。写入是 POST /api/galleries，每一步都可以撤销
// （回复里的 undo）。放进、拿出的选区和 /api/change 一样二选一：{ids} 或 {filter, exclude}，超过 500 张带 confirm。
import {FAV} from './favorites.js';
import {t} from './i18n.js';
import {api, post} from './session.js';
import {forgetShow, pageFor, showFor} from './gallery_shows.js';
import {pageOf, pathFor} from './pages.js';
import {state} from './state.js';

// 收藏画廊的 id（和收藏的筛选条件 core/favorites.js 的 FAV 恰好同名）。
export const FAV_GALLERY = 'fav';
// 内置的收藏画廊。名字跟着界面语言，所以每次现做。
export const favGallery = () => ({id: FAV_GALLERY, name: t('收藏'), kind: 'manual', builtin: true, params: {has: JSON.stringify([FAV])}, cover_manual: false});
export const isManual = g => g?.kind === 'manual';
export const isFavGallery = id => String(id) === FAV_GALLERY;
// 地址上定义这个画廊的那一个条件（手选画廊的 gallery:<id>、收藏的 fav）：筛选条上不画它，改了别的条件也不算「改过」。
export const ownCondition = g => isFavGallery(g?.id) ? FAV : isManual(g) ? `gallery:${g.id}` : '';
// 地址 r 上定义它那个画廊的条件；不在画廊页上是 ''。
export const ownConditionOf = r => pageOf(r).gallery ? ownCondition(galleryOf(r.id)) : '';

let listRead = null;
// 读一次画廊列表，放进 state.galleries（不含收藏）。
export function loadGalleries() {
  listRead ||= api('/api/galleries').then(list => { state.galleries = list; return list; }).finally(() => { listRead = null; });
  return listRead;
}
export const galleryOf = id => isFavGallery(id) ? favGallery() : (state.galleries || []).find(g => String(g.id) === String(id)) || null;
// 去这个画廊：按它记着的展示方式落在网格页（#/gallery/<id>）或展示页（#/showcase/<id>）。不带查询：网格页按保存的条件打开。
export const galleryHref = (id, show = showFor(id)) => pathFor({page: pageFor(show), id: String(id)});
// 收藏在最前，后面是各画廊。
export const withFav = (list = state.galleries || []) => [favGallery(), ...list];
// 能把图放进去的：收藏和手选画廊。
export const droppable = g => isFavGallery(g.id) || isManual(g);
// 写入之后的列表：回复里的 gallery 替换（或加上）同 id 的那一项；删除时去掉。列表随后由 loadGalleries 重读。
export function setGallery(id, gallery) {
  const list = (state.galleries || []).filter(g => String(g.id) !== String(id));
  if (gallery) { const old = galleryOf(id), at = old ? state.galleries.indexOf(old) : list.length; list.splice(at, 0, gallery); }
  state.galleries = list;
}

// 封面：{id: uuid 或 null}。按当前的 NSFW 筛选取；同一组 id、同一筛选只取一次。
// #590 合入之前 main 上没有封面接口，列表自己带 cover：那时退回去读列表。
const covers = new Map();
export async function coversFor(ids, nsfw = state.nsfw) {
  const key = `${nsfw}|${ids.join(',')}`;
  if (!covers.has(key)) covers.set(key, readCovers(ids, nsfw).catch(e => { covers.delete(key); throw e; }));
  return covers.get(key);
}
async function readCovers(ids, nsfw) {
  if (!ids.length) return {};
  try {
    const rows = await api('/api/galleries/covers?' + new URLSearchParams({ids: JSON.stringify(ids.map(Number)), nsfw}));
    return Object.fromEntries(rows.map(r => [r.id, r.cover]));
  } catch (e) {
    const rows = await api('/api/galleries?' + new URLSearchParams({nsfw})).catch(() => null);
    if (!rows?.every(r => 'cover' in r)) throw e;
    return Object.fromEntries(rows.filter(r => ids.map(String).includes(String(r.id))).map(r => [r.id, r.cover]));
  }
}
// 写入或撤销之后封面、样图可能变了。
export function forgetCovers() { covers.clear(); previews.clear(); }

// 一个画廊的图（/api/assets 的参数）：保存的条件，加 NSFW 设置。
const query = (g, nsfw, extra = {}) => new URLSearchParams({...g.params, ...(nsfw ? {nsfw} : {}), ...extra});

// 画廊列表卡片上的预览：前 n 张（带主色）和张数，一次请求。有手动封面时它排第一张（不在前 n 张里也补上）。
// 回 {items, total} 或 {error}（条件失效）。同一画廊、同一 NSFW 设置只读一次，写入后 forgetCovers() 清掉。
// 几个调用方共用一个请求（画廊列表页的卡片、活页条的收藏封面），所以请求不带任何调用方的 signal：signal 只让那个调用方
// 不再等（收到 AbortError），请求照常读完、留在缓存里，别的调用方照样拿到结果。
export const PREVIEW = 5;
const previews = new Map();
export function previewOf(g, {nsfw = state.nsfw, cover = null, signal} = {}) {
  const key = `${g.id}|${nsfw}|${cover || ''}`;
  if (!previews.has(key)) previews.set(key, readPreview(g, nsfw, cover).catch(e => { previews.delete(key); throw e; }));
  return waitFor(previews.get(key), signal);
}
// 等一个共用的 promise；signal 中止时只是这一处不再等。
function waitFor(promise, signal) {
  if (!signal) return promise;
  const stopped = () => signal.reason ?? Object.assign(new Error('aborted'), {name: 'AbortError'});
  if (signal.aborted) { promise.catch(() => {}); return Promise.reject(stopped()); }   // 没人等的失败不算未处理
  return new Promise((resolve, reject) => {
    const stop = () => reject(stopped());
    signal.addEventListener('abort', stop, {once: true});
    promise.then(resolve, reject).finally(() => signal.removeEventListener('abort', stop));
  });
}
async function readPreview(g, nsfw, cover) {
  let r;
  try { r = await api('/api/assets?' + query(g, nsfw, {limit: PREVIEW, offset: 0, marks: 'color'})); } catch (e) {
    if (e.status === 400) return {error: e.message};
    throw e;
  }
  let items = r.items || [];
  if (cover) { const at = items.findIndex(x => x.uuid === cover); items = at >= 0 ? [items[at], ...items.filter((_, k) => k !== at)] : [{uuid: cover, color: null}, ...items].slice(0, PREVIEW); }
  return {items, total: r.total ?? items.length};
}
// 展示页（拼贴、杂志、展厅）的图：按加入时间从新到旧（order 可以另给）取前 take 张，带收藏和主色。回 {items, total} 或 {error}。
export async function showcaseOf(g, take, {nsfw = state.nsfw, order = 'newest', signal} = {}) {
  try {
    const r = await api('/api/assets?' + query(g, nsfw, {limit: take, offset: 0, order, marks: 'fav,color'}), {signal});
    return {items: r.items || [], total: r.total ?? (r.items || []).length};
  } catch (e) {
    if (e.status === 400) return {error: e.message};
    throw e;
  }
}

// ---------- 写入：都返回 {undo, gallery?} ----------
export const createGallery = (params, name) => post('/api/galleries', {action: 'create', params, ...(name ? {name} : {})});
// 手选画廊：名字必填；可以顺带放进一批图（选区同 /api/change）。
export const createManual = (name, selection = {}) => post('/api/galleries', {action: 'create', kind: 'manual', name, ...selection});
export const updateGallery = (id, patch) => post('/api/galleries', {action: 'update', id: Number(id), ...patch});
export async function deleteGallery(id) {
  const r = await post('/api/galleries', {action: 'delete', id: Number(id)});
  forgetShow(id);
  return r;
}
// 手选画廊放进、拿出一批图（选区同 /api/change）。收藏不走这里：它是 /api/favorite，由网格的收藏写入做（ui/grid.js 的 favorite）。
// 拿出要在网格里淡出，由 ui/gallery_bar.js 交给 grid.js 的 mutate，所以这里给的是请求体。
export const addTo = (id, selection) => post('/api/galleries', {action: 'add', id: Number(id), ...selection});
export const removeBody = (id, selection) => ({action: 'remove', id: Number(id), ...selection});
