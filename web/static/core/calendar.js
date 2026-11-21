// 日历视图的数据（#708）：按月读 /api/calendar，月份的加减和格式，当月的角色和颜色汇总。不碰 DOM，Node 里能直接测；
// 画在 ui/calendar.js。
//
//   const month = await readMonth('2026-11', {...筛选, nsfw}, {signal});
//   // month = {month, days: Map('YYYY-MM-DD' → {day, count, samples, color}), total, active, prev, next, color, people}
//
// 接口（#718）：GET /api/calendar?month=YYYY-MM 加图库的筛选，返回 {days, total, prev, next}。days 只有有匹配图的日子，
// 每天最多四张样图（图片对象和图库列表相同，带 memberships），color 是当天的代表色（#717）；prev、next 是筛选后最近的
// 有图月份，到头是 null。日期是本机的加入日。
import {api} from './session.js';

const MONTH = /^(\d{4})-(0[1-9]|1[0-2])$/;

// 'YYYY-MM' → {year, month}（month 是 1–12）；格式不对时是 null。
export function parseMonth(text) {
  const m = MONTH.exec(text || '');
  return m ? {year: Number(m[1]), month: Number(m[2])} : null;
}
// 日期键的月份（「YYYY-MM-DD」→「YYYY-MM」）。今天是 core/format.js 的 dayKey()。
export const monthOf = day => String(day).slice(0, 7);

// 当月出现过的角色：按样图里的角色数张数（同一张图里的同一个角色只算一次），多的在前，一样多时先出现的在前。
// 只看样图（每天最多四张），所以是「这个月常见的人」，不是精确的张数；界面不写这些数。
export function monthPeople(days, limit = 12) {
  const seen = new Map();
  let order = 0;
  for (const day of days) for (const item of day.samples || []) {
    const ids = new Set();
    for (const m of item.memberships || []) {
      if (!m?.id || ids.has(m.id)) continue;
      ids.add(m.id);
      const p = seen.get(m.id) || {id: m.id, name: m.name || '', cover: item.uuid, color: item.color ?? null, n: 0, at: order++};
      p.n++; seen.set(m.id, p);
    }
  }
  return [...seen.values()].sort((a, b) => b.n - a.n || a.at - b.at).slice(0, limit).map(({id, name, cover, color}) => ({id, name, cover, color}));
}

// 整理接口的回复：days 按日期放进 Map；active 是有图的天数；color 是月份标题背后环境光的颜色（当月第一个有颜色的日子）。
export function shapeMonth(month, reply) {
  const list = Array.isArray(reply?.days) ? reply.days : [], days = new Map();
  for (const d of list) if (d?.day) days.set(d.day, {day: d.day, count: Number(d.count) || 0, samples: d.samples || [], color: d.color ?? null});
  const valid = m => parseMonth(m) ? m : null;
  return {month, days, total: Number(reply?.total) || 0, active: days.size, prev: valid(reply?.prev), next: valid(reply?.next),
    color: list.find(d => d?.color)?.color ?? null, people: monthPeople(list)};
}

// 请求的地址：筛选参数原样带上（值是空串、空数组的不带；数组照接口写成 JSON），样图要主色（格子底色、占位）。
export function calendarUrl(month, params = {}) {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === '' || (Array.isArray(v) && !v.length)) continue;
    q.set(k, Array.isArray(v) ? JSON.stringify(v) : String(v));
  }
  q.set('month', month); q.set('marks', 'color');
  return '/api/calendar?' + q;
}

// 读一个月。month 格式不对时抛错（调用方先用 parseMonth 判断）。
export async function readMonth(month, params = {}, {signal} = {}) {
  if (!parseMonth(month)) throw new Error(`core/calendar.js: 月份格式不对：${month}`);
  return shapeMonth(month, await api(calendarUrl(month, params), {signal}));
}
