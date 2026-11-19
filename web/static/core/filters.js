// 组合筛选条件（#445）：地址里的 has、not、from、to 怎么读写，旧的单值参数（tag、utag、work、artist）怎么并进来，
// 一组条件存成画廊时是什么样子。不碰 DOM，不发请求；条件的格式见 #445 的接口方案第 1 节。
// 条件是「种类:值」，只按第一个冒号切开；收藏是没有值的 fav。
import {t} from './i18n.js';

// 筛选条上最多几个条件（后端 has 和 not 合计 20 条）。
export const LIMIT = 20;
export const kindOf = c => c === 'fav' ? 'fav' : c.slice(0, c.indexOf(':'));
export const valueOf = c => c === 'fav' ? '' : c.slice(c.indexOf(':') + 1);
export const SOURCES = ['pixiv', 'x', 'other'];
// 地址或保存的条件里的 JSON 数组：不是字符串数组就当没有。
export function parseList(raw) {
  if (Array.isArray(raw)) return raw.filter(x => typeof x === 'string' && x);
  if (!raw) return [];
  try { const list = JSON.parse(raw); return Array.isArray(list) ? list.filter(x => typeof x === 'string' && x) : []; } catch { return []; }
}
// 旧的单值参数和它们对应的条件种类。详情栏点画师、作品、标签仍然带着它们进来。
const LEGACY = ['tag', 'utag', 'work', 'artist'];
const CLEARED = Object.fromEntries(LEGACY.map(k => [k, '']));

// 路由上的全部条件：has、not 加上旧的单值参数（并在 has 的末尾）。
export function conditions(r) {
  const has = parseList(r.has), not = parseList(r.not);
  for (const key of LEGACY) {
    const c = r[key] ? `${key}:${r[key]}` : '';
    if (c && !has.includes(c) && !not.includes(c)) has.push(c);
  }
  return {has, not, from: r.from || '', to: r.to || '', day: r.day || ''};
}
export const count = r => { const x = conditions(r); return x.has.length + x.not.length; };
// 路由上唯一的条件（没有别的条件、日期和角色；搜索词不算）：图库标题按它写（#644）。没有时返回 ''。
export function sole(r) {
  const x = conditions(r);
  return x.has.length === 1 && !x.not.length && !x.from && !x.to && !x.day && !r.char ? x.has[0] : '';
}
// 路由上的条件只有这一条，也没有搜索词：侧栏「收藏」（#447）按 only(r, 'fav') 判断亮不亮。
export const only = (r, c) => sole(r) === c && !r.q;
// 一张图有没有条件 c 改成 present 以后，是不是离开这一页（#644）：去掉的正是「含」的条件，或加上的正是「不含」的条件。
// 两种写法（旧的单值参数、条件数组）都按 conditions(r) 算。
export function leaves(r, c, present) { const x = conditions(r); return present ? x.not.includes(c) : x.has.includes(c); }
// 换成这组条件以后的路由：旧的单值参数整体改写成 has。
function rewrite(r, x) { return {...r, ...CLEARED, has: x.has, not: x.not, from: x.from, to: x.to, day: x.day}; }
const without = (list, c) => list.filter(y => y !== c);
export function addCondition(r, c) {
  const x = conditions(r);
  if (x.has.includes(c)) return rewrite(r, x);
  return rewrite(r, {...x, has: [...without(x.has, c), c], not: without(x.not, c)});
}
// 在「含」和「不含」之间切换，位置不变。
export function flipCondition(r, c) {
  const x = conditions(r);
  if (x.has.includes(c)) return rewrite(r, {...x, has: without(x.has, c), not: [...x.not, c]});
  if (x.not.includes(c)) return rewrite(r, {...x, not: without(x.not, c), has: [...x.has, c]});
  return rewrite(r, x);
}
export function dropCondition(r, c) { const x = conditions(r); return rewrite(r, {...x, has: without(x.has, c), not: without(x.not, c)}); }
// 日期：起止都可以空着；day 是旧的「某一天加入」，换日期时一并去掉。
export function setDates(r, from = '', to = '') { return rewrite(r, {...conditions(r), from, to, day: ''}); }
export const yearDates = y => [`${y}-01-01`, `${y}-12-31`];
const DATE = /^\d{4}-\d{2}-\d{2}$/;
export const validDates = (from, to) => (!from || DATE.test(from)) && (!to || DATE.test(to)) && !(from && to && from > to) && !!(from || to);

// 发给 /api/assets 的条件参数（各值都是字符串）。
export function queryParams(r) {
  const f = {}, has = parseList(r.has), not = parseList(r.not);
  if (has.length) f.has = JSON.stringify(has);
  if (not.length) f.not = JSON.stringify(not);
  if (r.from) f.from = r.from;
  if (r.to) f.to = r.to;
  return f;
}

// 存成画廊的条件：has、not、from、to、day、q。在角色页上存，角色（有造型时是造型）也是一个条件。排序和 NSFW 不存。
export function savedParams(r) {
  const x = conditions(r), has = [...x.has];
  const char = r.char ? `char:${r.variant || r.char}` : '';
  if (char && !has.includes(char) && !x.not.includes(char)) has.unshift(char);
  const p = {};
  if (has.length) p.has = JSON.stringify(has);
  if (x.not.length) p.not = JSON.stringify(x.not);
  for (const key of ['from', 'to', 'day']) if (x[key]) p[key] = x[key];
  if (r.q) p.q = r.q;
  return p;
}
export const savable = r => Object.keys(savedParams(r)).length > 0;
// 保存的条件 → 路由上的字段（画廊页打开时用）。
export function routeFields(params = {}) {
  const v = key => typeof params[key] === 'string' ? params[key] : '';
  return {...Object.fromEntries(LEGACY.map(k => [k, v(k)])), has: parseList(params.has), not: parseList(params.not), from: v('from'), to: v('to'), day: v('day'), q: v('q')};
}
// 两组条件是否相同：has、not 各按集合比，其余逐项比。
export function sameParams(a, b) {
  const x = conditions(routeFields(a)), y = conditions(routeFields(b));
  const same = (p, q) => p.length === q.length && p.every(c => q.includes(c));
  return same(x.has, y.has) && same(x.not, y.not) && x.from === y.from && x.to === y.to && x.day === y.day && (a.q || '') === (b.q || '');
}

// 日期条件的文字。
export function datesLabel(from, to) {
  if (from && to && from.slice(0, 4) === to.slice(0, 4) && from.endsWith('-01-01') && to.endsWith('-12-31')) return t('{year} 年', {year: from.slice(0, 4)});
  if (from && to) return from === to ? from : t('{from} 至 {to}', {from, to});
  return from ? t('{date} 起', {date: from}) : t('{date} 之前', {date: to});
}
// 新画廊的默认名：条件的文字用「 · 」连起来，最多 60 字。label(c) 给出一个条件的文字。
export function defaultName(r, label) {
  const x = conditions(r), parts = [];
  if (r.char) parts.push(label(`char:${r.variant || r.char}`));
  parts.push(...x.has.map(label), ...x.not.map(c => t('不含 {name}', {name: label(c)})));
  if (x.from || x.to) parts.push(datesLabel(x.from, x.to));
  if (x.day) parts.push(x.day);
  if (r.q) parts.push(`“${r.q}”`);
  const name = [...new Set(parts.filter(Boolean))].join(' · ') || t('画廊');
  return [...name].length > 60 ? [...name].slice(0, 59).join('') + '…' : name;
}
