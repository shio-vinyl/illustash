// 角色页横幅和资料的取数（#712）：横幅用的宽图、最近加入的几张、收集曲线（最近 12 个月，core/detail.js 的 readCharStats，
// 和详情卡片同一份）、最近 30 天进了多少张、常一起出现的角色（同框次数）。不碰 DOM，横幅和资料在 ui/character.js、
// ui/character_facts.js。按角色、NSFW 设置和数据版本缓存一份，同一角色重画不再请求。
// 常一起出现只存 id 和次数，名字和头像在画的时候按当时的角色目录取（peopleOf）：目录比这次读取晚到时，头像跟着新目录。
import {catalog, loadActivity} from './catalog.js';
import {readCharStats} from './detail.js';
import {aspect} from './layout.js';
import {api} from './session.js';
import {state} from './state.js';

// 横幅从最近加入的这么多张里挑；够宽的图（宽高比至少 WIDE）优先，都不够宽就用最近的一张。
const LOOK = 24, WIDE = 1.25;
// 最近加入显示几张、常一起出现的角色显示几个。
export const RECENT = 3, PEERS = 6;

// 横幅的图：items 里第一张够宽的，没有就第一张；都没有是 null。
export function pickBanner(items) {
  return items.find(x => aspect(x) >= WIDE) || items[0] || null;
}
// 同作品的角色：作品相同、不是自己，只当标签的不算；按张数从多到少取前 PEERS 个。原创角色、还没填作品的角色没有同作品。
export function peersOf(c, chars) {
  if (!c?.work_id || c.kind === 'oc') return [];
  return chars.filter(x => x.id !== c.id && x.work_id === c.work_id && !x.mascot && x.count > 0)
    .sort((a, b) => b.count - a.count || String(a.name).localeCompare(String(b.name))).slice(0, PEERS);
}
// 资料区第三组画谁：常一起出现的（together 是 [{id, count}]，按角色目录取名字和头像，目录里没有的、只当标签的不画）；
// 一个都没有时退回同作品的角色。返回 {kind: 'together' | 'work', people: [{c, count}]}。
export function peopleOf(c, together, chars, charMap) {
  const met = (together || []).map(x => ({c: charMap.get(x.id), count: x.count})).filter(x => x.c && !x.c.mascot);
  if (met.length) return {kind: 'together', people: met.slice(0, PEERS)};
  return {kind: 'work', people: peersOf(c, chars).map(x => ({c: x, count: x.count}))};
}
// 最近 30 天进了多少张：资料库也用的动态（core/catalog.js 的 loadActivity，按 NSFW 和数据版本缓存）。缓存的不是这次的设置
// （读失败了）时是 null（不显示），免得写上一种设置下的数。
const recentDays = id => catalog.activityKey === state.nsfw + '|' + state.readEpoch ? catalog.activity.get(id)?.recent || 0 : null;

let cached = {key: '', data: null};
// 一个角色的横幅和资料：{banner, recent, months, recent30, together}。
export async function loadCharacter(c, {signal} = {}) {
  const key = [c.id, state.nsfw, state.readEpoch].join('|');
  if (cached.key === key) return cached.data;
  const nsfw = state.nsfw ? {nsfw: state.nsfw} : {};
  const [list, stats, together] = await Promise.all([
    api('/api/assets?' + new URLSearchParams({entity: c.id, order: 'newest', limit: LOOK, marks: 'color', count: 0, ...nsfw}), {signal}),
    readCharStats(c.id),
    // 同框次数读不到（接口出错）时当作没有，资料区退回同作品。
    api('/api/character/together?' + new URLSearchParams({id: c.id, nsfw: state.nsfw || '', limit: PEERS * 2}), {signal}).catch(e => { if (e.name === 'AbortError') throw e; return []; }),
    loadActivity().catch(() => false),
  ]);
  const items = list?.items || [];
  const data = {banner: pickBanner(items), recent: items.slice(0, RECENT), months: stats?.months || [], recent30: recentDays(c.id),
    together: Array.isArray(together) ? together : []};
  cached = {key, data};
  return data;
}
// 下一次 loadCharacter 重新请求（只有测试用：写入以后数据版本变了，键自然不同）。
export function forgetCharacter() { cached = {key: '', data: null}; }
