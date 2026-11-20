// 整理页（#713）的数据：按组整理、相似的图、相似组 N 的「看过了」、身份整理、画师抽查的读取和写入，以及排组、相近一组的说明、
// 身份整理的建议和写入内容、画师抽查的统计这些纯函数。不碰 DOM，Node 里能直接测；画在 ui/groups.js、ui/similar.js、
// ui/identity.js、ui/artist_audit.js。读取都带 signal：换页或换了子视图时由页面作用域取消。
import {t} from './i18n.js';
import {api, post} from './session.js';

// ---------- 按组整理（未识别的相似分组） ----------
/** 未识别图的相似分组：{clusters: [{cid, size, members, lead, traits, near, near_kind, built_at}], learner}。 */
export const loadClusters = (nsfw, signal) => api('/api/clusters?nsfw=' + nsfw, {signal});
/** 用现在的未识别图重新分组（后台进行）。 */
export const regroup = () => post('/api/regroup', {});
// 各组的先后：一键就能确认的组排在前面（大的在前），然后是还要起名字的组。组页空了时的「下一组」也按它走。
export const groupOrder = clusters => [...clusters].sort((a, b) => !!b.lead - !!a.lead || b.size - a.size || a.cid - b.cid);
// 'look'：中心接近；'trait'：离得更远，但有相同的独特 WD 特征（较弱的线索）。
export const nearText = g => g.near_kind === 'trait' ? t('外观相似：第 {n} 组', {n: g.near}) : t('可能和第 {n} 组是同一人', {n: g.near});
/** 「下一组」：原来排在 at 后面、还在的第一组；后面都没了取剩下的第一组；一组都不剩是 null。 */
export function nextGroupId(order, at, clusters) {
  const left = new Set(clusters.map(x => String(x.cid))); left.delete(at);
  return order.slice(order.indexOf(at) + 1).find(id => left.has(id)) || groupOrder(clusters).map(x => String(x.cid)).find(id => id !== at) || null;
}

// ---------- 相似的图（整个图库，native/terms/src/research_compute/similar.rs） ----------
/** 相似组列表（含看过的组，由调用方按 seen 分开）：{groups: [{gid, size, covers, kinds, seen}], state}。 */
export const loadSimilar = ({kind = '', nsfw}, signal) => api('/api/similar?' + new URLSearchParams({kind, nsfw, seen: '1'}), {signal});
/** 比较新加入的图和还没比较过的候选。 */
export const scanSimilar = () => post('/api/similar', {});
/** 把一组标为看过或没看过（#427）。size 和读的时候对不上（组刚变过）时后端返回 409。 */
export const setSimilarSeen = ({gid, size, nsfw, seen}) => post('/api/similar/seen', {gid, size, nsfw, seen});
/** 相似组 gid 的看过状态：{seen, size, nsfw}；组不在了是 null。 */
export async function readSimilarSeen(gid, nsfw, signal) {
  const result = await api('/api/similar?' + new URLSearchParams({nsfw, seen: '1'}), {signal});
  const g = result.groups.find(x => String(x.gid) === String(gid));
  return g ? {seen: !!g.seen, size: g.size, nsfw} : null;
}

// ---------- 身份整理（/api/identity/queue，native/terms/src/http_characters.rs） ----------
/** 一页长得像的角色对：{items: [{a, b, hint, distance}], total}。 */
export const loadIdentity = ({limit, offset = 0, nsfw}, signal) => api('/api/identity/queue?' + new URLSearchParams({limit, offset, nsfw}), {signal});
// 一对角色的键：判断、撤销以后按它比对（#452）。
export const pairKey = x => `${x.a.id}|${x.b.id}`;
/** 外部资料建议的做法：'same'、'b_of_a'、'a_of_b' 或 ''。 */
export function identitySuggestion(x) {
  const h = x.hint; if (!h) return '';
  if (h.kind === 'duplicate_of') return 'same';
  if (h.kind === 'form_of') return h.subject && h.subject === x.b.tag ? 'b_of_a' : h.subject && h.subject === x.a.tag ? 'a_of_b' : '';
  return '';
}
/** 这一对为什么排进来，一句纯文字。 */
export function identityEvidence(x) {
  const h = x.hint, name = tag => tag === x.a.tag ? x.a.name : tag === x.b.tag ? x.b.name : tag.replaceAll('_', ' ');
  const parts = [];
  if (h?.kind === 'duplicate_of') parts.push(t('{source}：同一个 tag 的别名', {source: h.source || t('外部资料')}));
  else if (h?.kind === 'form_of') parts.push(t('{source}：「{subject}」是「{object}」的一个造型', {source: h.source || t('外部资料'), subject: name(h.subject), object: name(h.object)}));
  if (x.distance !== null && x.distance !== undefined) parts.push(t('参考图外观距离 {d}', {d: x.distance.toFixed(3)}) + (x.distance <= 0.01 ? t('（几乎分不开）') : ''));
  return parts.join(' · ') || t('参考图外观很接近');
}
/**
 * 判断一对：act 是 'same'、'b_of_a'、'a_of_b'、'different' 或 'skip'。返回写入的结果（带 undo）和提示条上的一句话。
 * 记为同一角色时，重复的角色显示在图多的那一边下面。
 */
export async function decideIdentity(x, act) {
  if (act === 'skip') return {result: await post('/api/identity/skip', {a: x.a.id, b: x.b.id}), message: t('已暂不判断')};
  const [small, big] = x.a.count <= x.b.count ? [x.a, x.b] : [x.b, x.a];
  const rel = {same: {a: small.id, b: big.id, kind: 'duplicate_of'}, b_of_a: {a: x.b.id, b: x.a.id, kind: 'form_of'},
    a_of_b: {a: x.a.id, b: x.b.id, kind: 'form_of'}, different: {a: x.a.id, b: x.b.id, kind: 'different_from'}}[act];
  if (!rel) throw new Error('unknown identity act: ' + act);
  const message = {same: t('已把 {a} 和 {b} 记为同一角色', {a: small.name, b: big.name}), b_of_a: t('{a} 已作为 {b} 的一个造型', {a: x.b.name, b: x.a.name}),
    a_of_b: t('{a} 已作为 {b} 的一个造型', {a: x.a.name, b: x.b.name}), different: t('已记下 {a} 和 {b} 是不同角色', {a: x.a.name, b: x.b.name})}[act];
  return {result: await post('/api/identity/decision', {status: 'accepted', reason: '身份整理', ...rel}), message};
}

// ---------- 画师抽查（docs/PROVENANCE.md 第 8 节） ----------
/** 固定的一份随机样本的前 n 张：{items: [{uuid, artists, posts}], pool}。 */
export const loadArtistAudit = (n, signal) => api('/api/artist-audit?n=' + n, {signal});
/** 对一张图上的一个画师下结论：verdict 是 'yes'、'no' 或 null（改回没判断）。 */
export const artistVerdict = ({uuid, artist, verdict, name}) => post('/api/artist/verdict', {ids: [uuid], artist, verdict: verdict || null, name});
export const AUDIT_METHODS = ['pixel_hash', 'gallery_match', 'ocr'];
/** 这个画师是怎么找回来的：像素哈希优先，否则第一种方法。 */
export const auditMethod = a => a.methods?.includes('pixel_hash') ? 'pixel_hash' : a.methods?.[0] || '';
/** 统计：全部、按方法各一份 {yes, no, done, all}；accuracy 没判断过时是 null。 */
export function auditTally(items) {
  const pairs = items.flatMap(it => it.artists.map(a => ({...a, method: auditMethod(a)})));
  const tally = list => { const yes = list.filter(a => a.confirmed).length, no = list.filter(a => a.rejected).length; return {yes, no, done: yes + no, all: list.length}; };
  const all = tally(pairs);
  return {all, accuracy: all.done ? all.yes / all.done : null,
    methods: AUDIT_METHODS.map(m => [m, tally(pairs.filter(a => a.method === m))]).filter(([, x]) => x.all)};
}
