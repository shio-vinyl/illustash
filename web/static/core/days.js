// 图库「按天」视图的数据（#707，0.4 第 C1 步）：读哪几天、同步后怎么并进已读到的天、展开一天的全部角色、「认一认」的
// 读取和「以后再说」，以及几个排版要用的纯计算（芯片行放得下几个、一天多高、各天的框）。不碰 DOM，Node 里能直接测
// （tests/days.test.cjs）。画在哪在 ui/days.js（页面）、ui/day.js（一天）、ui/day_cards.js（顶上两张卡）。
//
// 接口（#718、#717）：/api/home 和图库网格接受同样的筛选，按日期从新到旧给几天，每天 {day, count, people, characters（前八位）,
// items（最多 18 张）, hero}；before 是排他的游标（上一批的 next），next 为空表示读到底了。marks=fav,color 让每张图带收藏和主色。
import {I18N} from './i18n.js';
import {PAGES} from './pages.js';
import {api, post} from './session.js';

// 每张图带上收藏和主色（缩略图读完之前的占位色，#717）。
export const DAY_MARKS = 'fav,color';
// 第一次读几天；接近底部时每次再读几天。
export const FIRST_DAYS = 5, MORE_DAYS = 4;

// ---------- 读 ----------
// 按天读取用的参数：图库网格的筛选和搜索（三种视图共用，#703 第 2 问）加 R18。按天视图自己按日期分组，所以路由里的
// day 不带；「更多像这张的」（like、by）不是筛选，也不带。
export function dayQuery(route, nsfw) {
  const f = {...(PAGES.library.params(route) || {})};
  delete f.day; delete f.like; delete f.by;
  if (nsfw) f.nsfw = nsfw;
  return f;
}
// 读几天：before 为空从最新的一天读起。返回 {days, next}。
export async function readDays(query, {before = '', days = FIRST_DAYS} = {}) {
  const r = await api('/api/home?' + new URLSearchParams({...query, days, ...(before ? {before} : {}), marks: DAY_MARKS}));
  return {days: r.days || [], next: r.next || ''};
}

// 一天的全部角色（展开「另外 N 位」时）：/api/home 只给前八位，其余按同样的筛选和这一天分页读 /api/assets，汇总 memberships。
// 按当天张数从多到少排，一样多时按名字。图很多的一天最多读 limit × pages 张，读不完的不算进去（芯片行仍以 people 为总数）。
export async function readDayPeople(query, day, {limit = 500, pages = 20} = {}) {
  const counts = new Map();
  for (let page = 0; page < pages; page++) {
    const r = await api('/api/assets?' + new URLSearchParams({...query, day, order: 'newest', offset: page * limit, limit, count: 0}));
    for (const item of r.items || []) for (const m of item.memberships || []) {
      const c = counts.get(m.id) || {id: m.id, name: m.name, tag: m.tag, count: 0};
      c.count++; counts.set(m.id, c);
    }
    if ((r.items || []).length < limit) break;
  }
  const collator = new Intl.Collator(I18N.locale);
  return [...counts.values()].sort((a, b) => b.count - a.count || collator.compare(a.name || '', b.name || ''));
}

// ---------- 同步后并进已读到的天 ----------
// 两次读到的同一天是不是一样：张数、角色、图（次序、收藏、主色）都没变。一样时沿用旧对象，部件不用更新。
export function sameDay(a, b) {
  if (!a || !b || a.day !== b.day || a.count !== b.count || a.people !== b.people || a.hero !== b.hero) return false;
  const chars = x => (x.characters || []).map(c => `${c.id}:${c.count}`).join(' ');
  const items = x => (x.items || []).map(i => `${i.uuid}:${i.fav ? 1 : 0}:${i.color || ''}`).join(' ');
  return chars(a) === chars(b) && items(a) === items(b);
}
// 已读到的天 list，加上重读的最新几天 fresh（从新到旧）：fresh 覆盖的日期范围里以 fresh 为准（没变的天沿用旧对象，
// 没了的天去掉，新的一天插到前面），更早的天原样留着。返回 {days, changed}：changed 是换了对象的日期。
export function mergeDays(list, fresh) {
  if (!fresh.length) return {days: list, changed: new Set()};
  const oldest = fresh[fresh.length - 1].day, before = new Map(list.map(g => [g.day, g])), changed = new Set(), days = [];
  for (const g of fresh) {
    const old = before.get(g.day);
    if (old && sameDay(old, g)) days.push(old); else { days.push(g); changed.add(g.day); }
  }
  for (const g of list) if (g.day < oldest) days.push(g);
  return {days, changed};
}
// 读到的天里最早的一天和总张数（读到底以后写「最早的一张在 X 月 X 日加入 · 共 N 张」）。
export function summary(days) {
  return {earliest: days.length ? days[days.length - 1].day : '', total: days.reduce((n, g) => n + (g.count || 0), 0)};
}

// ---------- 一天里的图 ----------
// 马赛克的大图：本页期间固定（同步进来新图时大图不换，免得整块重排）；原来那张不在了就用后端选的 hero（当天第一位角色
// 最新的一张），再没有就是第一张。返回大图在前、其余按原次序的列表。
export function heroFirst(g, keep = '') {
  const items = g.items || [], pick = items.find(x => x.uuid === keep) || items.find(x => x.uuid === g.hero) || items[0];
  return pick ? [pick, ...items.filter(x => x !== pick)] : [];
}

// ---------- 芯片行 ----------
// 一行放得下几个芯片：widths 是各芯片的宽，room 是这一行的宽，moreWidth 是「另外 N 位」按钮的宽，gap 是缝。还有放不下的
// （包括 hidden 个根本没画出来的）时要给按钮留位置。返回放得下的个数。
export function fitChips(widths, room, moreWidth, gap, hidden = 0) {
  let used = 0, shown = 0;
  for (let i = 0; i < widths.length; i++) {
    const need = used + (i ? gap : 0) + widths[i], rest = widths.length - i - 1 + hidden;
    if (need + (rest ? gap + moreWidth : 0) > room) break;
    used = need; shown++;
  }
  return shown;
}

// ---------- 一天多高、各天的框 ----------
// 一天由上到下：页头、角色芯片行（当天有角色时）、马赛克（大图高两行）。芯片行收起时一行高；展开后的高度由界面量了报上来。
// sizes：{head, chips, rowHeight, gap, after}，after 是两天之间的距离。
export const mosaicHeight = ({rowHeight, gap}) => rowHeight * 2 + gap;
export function dayHeight(g, sizes, chipsHeight = sizes.chips) {
  const chips = (g.characters || []).length ? chipsHeight : 0;
  return sizes.head + chips + mosaicHeight(sizes) + sizes.after;
}
// 各天的框（交给 ui/virtual.js）：一列，从 top 往下排。opened 是展开了芯片行的天 -> 量到的芯片行高度。
// 返回 {boxes, height}；框带上这一天的数据 g。
export function dayBoxes(days, {width, top = 0, sizes, opened = new Map()}) {
  let y = top;
  const boxes = days.map(g => {
    const h = dayHeight(g, sizes, opened.get(g.day));
    const box = {key: g.day, x: 0, y, w: width, h, g};
    y += h;
    return box;
  });
  return {boxes, height: y};
}
// 视口顶部那条线落在哪一天（滚动时告诉活页条选中它）。
export function dayAt(boxes, y) {
  let lo = 0, hi = boxes.length - 1, found = '';
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (boxes[mid].y <= y) { found = boxes[mid].key; lo = mid + 1; } else hi = mid - 1;
  }
  return found || boxes[0]?.key || '';
}

// ---------- 去网格 ----------
// 「在网格里看」和点当天的角色：带着现在的筛选和搜索去网格，加上这一天（和这个角色）。
const KEEP = ['q', 'tag', 'utag', 'work', 'artist', 'has', 'not', 'from', 'to'];
export function gridRoute(route, day, char = '') {
  const r = {page: 'library', day};
  for (const k of KEEP) if (Array.isArray(route[k]) ? route[k].length : route[k]) r[k] = route[k];
  if (char) {
    const has = Array.isArray(route.has) ? route.has : typeof route.has === 'string' && route.has ? JSON.parse(route.has) : [];
    r.has = has.includes('char:' + char) ? has : [...has, 'char:' + char];
  }
  return r;
}

// ---------- 认一认 ----------
// 下一批要问的：一个角色，几张机器认出的图。skip 是这一轮已经问过的角色。没有可问的、或者读失败时是 null。
export async function readAsk(nsfw, skip = []) {
  try { return (await api('/api/ask?' + new URLSearchParams({nsfw, skip: skip.join(',')}))).batch || null; } catch { return null; }
}
// 「以后再说」：记下这一批问过了，不改图库，不进撤销。
export const askLater = batch => post('/api/ask/answer', {entity: batch.entity, kind: batch.kind, later: batch.items.map(x => x.uuid)});
// 回答的请求体（交给 ui/grid.js 的 mutate，一步、一个撤销）：
//   how 'ok'    点掉的（off）记成不是 TA，其余记成是 TA
//   how 'none'  整批都记成不是 TA（#675）
//   how 'other' 点掉的是另一个角色 other：只提交点掉的那几张，一步写两条结论（不是 TA、是 other），其余几张留在卡上（#675 第二步）。
//               后端的 switch 要求 yes 为空，所以这一步不带其余几张
export function askAnswer(batch, how, off, other = '') {
  const ids = batch.items.map(x => x.uuid);
  if (how === 'other') return {entity: batch.entity, kind: batch.kind, yes: [], no: ids.filter(u => off.has(u)), switch: other};
  const no = how === 'none' ? ids : ids.filter(u => off.has(u));
  return {entity: batch.entity, kind: batch.kind, yes: ids.filter(u => !no.includes(u)), no};
}
