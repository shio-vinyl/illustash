// 画廊（#445）：保存的一组筛选条件加封面，存在图库里。这里读写列表、取封面和张数，不碰 DOM。
// 接口：GET /api/galleries 回 [{id, name, params, cover_manual, position?}]，按保存的先后；封面按可见的画廊批量取
// GET /api/galleries/covers?ids=[…]&nsfw=（#590）。写入是 POST /api/galleries，每一步都可以撤销。
import {api, post} from './session.js';
import {state} from './state.js';

let listRead = null;
// 读一次画廊列表，放进 state.galleries。
export function loadGalleries() {
  listRead ||= api('/api/galleries').then(list => { state.galleries = list; return list; }).finally(() => { listRead = null; });
  return listRead;
}
export const galleryOf = id => (state.galleries || []).find(g => String(g.id) === String(id)) || null;
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
// 写入或撤销之后封面可能变了。
export function forgetCovers() { covers.clear(); }

// 一个画廊现在有几张：{n} 或 {error}（保存的条件失效时后端回 400，error 是它的原因）。
export async function countOf(params, nsfw = state.nsfw) {
  try {
    const r = await api('/api/assets/count?' + new URLSearchParams({...params, ...(nsfw ? {nsfw} : {})}));
    return {n: r.total};
  } catch (e) {
    if (e.status === 400) return {error: e.message};
    throw e;
  }
}

// 写入：都返回 {undo, gallery?}。
export const createGallery = (params, name) => post('/api/galleries', {action: 'create', params, ...(name ? {name} : {})});
export const updateGallery = (id, patch) => post('/api/galleries', {action: 'update', id: Number(id), ...patch});
export const deleteGallery = id => post('/api/galleries', {action: 'delete', id: Number(id)});
