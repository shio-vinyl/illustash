// 「更多像这张的」（#449）：从 1–20 张例图出发、按已存特征找相似的图。结果是图库的一个视图（core/pages.js 的 like），
// 地址 #/library?like=["uuid",…]&by=ccip，by 省略时按画面（WD 画面标签），ccip 按角色。后端参数是 /api/assets 的
// like、like_by（契约见 docs/WEB_V2.md「从例图找相似」）。这里只有读写这两个地址参数的小函数，不碰 DOM；
// 结果页的标题在 ui/like.js，入口按钮在 ui/detail.js（详情栏、大图）和 ui/grid.js（选择栏）。

// 后端一次最多收的例图张数（native/terms/src/http_example_search.rs 的 MAX_EXAMPLES）。
export const LIKE_MAX = 20;
// 选了几张时能找相似：全选和超过上限时不能。
export const canLike = n => n >= 1 && n <= LIKE_MAX;
// 地址里的例图：认不出的写法当作没有。
export function likeIds(r) {
  if (!r?.like) return [];
  let ids; try { ids = JSON.parse(r.like); } catch { return []; }
  return Array.isArray(ids) ? [...new Set(ids.filter(id => typeof id === 'string' && id))].slice(0, LIKE_MAX) : [];
}
const encode = ids => JSON.stringify([...new Set(ids)].slice(0, LIKE_MAX));
// 去找像这几张的图：从图库的空白条件开始，NSFW 开关是全局的，照旧。
export const likeRoute = (ids, by = '') => ({page: 'library', like: encode(ids), by: by === 'ccip' ? 'ccip' : ''});
// 换一组例图，其余筛选和比较方式不变；一张都不剩时回到同样筛选的图库。
export const withExamples = (r, ids) => ids.length ? {...r, like: encode(ids)} : {...r, like: '', by: '', sort: r.sort === 'similar' ? '' : r.sort};
// 地址里的这两个参数读成路由字段：只有图库（不是角色页）认，例图读不出就都清掉。
export function readLike(q, library) {
  const like = library ? q.get('like') || '' : '';
  if (!likeIds({like}).length) return {like: '', by: ''};
  return {like, by: q.get('by') === 'ccip' ? 'ccip' : ''};
}
// 这个路由对应的后端参数。
export const likeParams = r => !likeIds(r).length ? {} : r.by ? {like: encode(likeIds(r)), like_by: r.by} : {like: encode(likeIds(r))};
