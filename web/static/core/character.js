// 角色页横幅和资料的取数（#712）：横幅用的宽图、最近加入的几张、收集曲线（最近 12 个月）、最近 30 天进了多少张、同作品的角色。
// 不碰 DOM，横幅和资料在 ui/character.js、ui/character_facts.js。按角色、NSFW 设置和数据版本缓存一份，同一角色重画不再请求。
import {catalog, loadActivity} from './catalog.js';
import {aspect} from './layout.js';
import {api} from './session.js';
import {state} from './state.js';

// 横幅从最近加入的这么多张里挑；够宽的图（宽高比至少 WIDE）优先，都不够宽就用最近的一张。
const LOOK = 24, WIDE = 1.25;
// 最近加入显示几张、同作品的角色显示几个。
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
// 最近 30 天进了多少张：资料库也用的动态（core/catalog.js 的 loadActivity，按筛选和数据版本缓存）；读不到是 null（不显示）。
const recentDays = id => catalog.activityKey ? catalog.activity.get(id)?.recent || 0 : null;

let cached = {key: '', data: null};
// 一个角色的横幅和资料：{banner, recent, months, first, recent30, peers}。同作品的角色每次按 state.chars 重算（改名、张数变了）。
export async function loadCharacter(c, {signal} = {}) {
  const key = [c.id, state.nsfw, state.readEpoch].join('|');
  if (cached.key === key) return {...cached.data, peers: peersOf(c, state.chars)};
  const nsfw = state.nsfw ? {nsfw: state.nsfw} : {};
  const [list, stats] = await Promise.all([
    api('/api/assets?' + new URLSearchParams({entity: c.id, order: 'newest', limit: LOOK, marks: 'color', count: 0, ...nsfw}), {signal}),
    api('/api/character/stats?' + new URLSearchParams({id: c.id, nsfw: state.nsfw || ''}), {signal}),
    loadActivity().catch(() => false),
  ]);
  const items = list?.items || [];
  const data = {banner: pickBanner(items), recent: items.slice(0, RECENT), months: stats?.months || [], first: stats?.first || '',
    recent30: recentDays(c.id), peers: peersOf(c, state.chars)};
  cached = {key, data};
  return data;
}
// 下一次 loadCharacter 重新请求（测试和写入后用）。
export function forgetCharacter() { cached = {key: '', data: null}; }
