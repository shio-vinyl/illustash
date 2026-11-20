// 图库「按天」视图的数据（#707，0.4 第 C1 步）：读哪几天、同步后怎么并进已读到的天、展开一天的全部角色、「认一认」的
// 读取和「以后再说」，以及几个排版要用的纯计算（芯片行放得下几个、一天多高、各天的框）。不碰 DOM，Node 里能直接测
// （tests/days.test.cjs）。画在哪在 ui/days.js（页面）、ui/day.js（一天）、ui/day_cards.js（顶上两张卡）。
//
// 接口（#718、#717）：/api/home 和图库网格接受同样的筛选，按日期从新到旧给几天，每天 {day, count, people, characters（前八位）,
// items（最多 18 张）, hero}；before 是排他的游标（上一批的 next），next 为空表示读到底了。marks=fav,color 让每张图带收藏和主色。
import {addCondition} from './filters.js';
import {I18N} from './i18n.js';
import {FIT_MAX, FIT_MIN, dayMosaic} from './layout.js';
import {libraryFilters} from './pages.js';
import {viewRoute} from './rail.js';
import {api, post} from './session.js';

// 每张图带上收藏和主色（缩略图读完之前的占位色，#717）。
export const DAY_MARKS = 'fav,color';
// 第一次读几天；接近底部时每次再读几天。
export const FIRST_DAYS = 5, MORE_DAYS = 4;
// 后端一次最多给 20 天（/api/home 的 days）、一页最多 200 张（/api/assets 的 limit）。
export const MAX_DAYS = 20, MAX_PAGE = 200;

// ---------- 读 ----------
// 按天读取用的参数：图库的筛选和搜索（三种视图共用，#703 第 2 问；旧的单值参数 day 在三种视图里都是「只看这一天加入的」）
// 加 NSFW 设置。看哪一天（at）不是筛选，不在里面。
export function dayQuery(route, nsfw) {
  const f = libraryFilters(route);
  if (nsfw) f.nsfw = nsfw;
  return f;
}
// 读几天：before 为空从最新的一天读起。返回 {days, next}。
export async function readDays(query, {before = '', days = FIRST_DAYS, signal} = {}) {
  const r = await api('/api/home?' + new URLSearchParams({...query, days, ...(before ? {before} : {}), marks: DAY_MARKS}), {signal});
  return {days: r.days || [], next: r.next || ''};
}

// 一天的全部角色（展开「另外 N 位」时）：/api/home 只给前八位，其余按同样的筛选和这一天分页读 /api/assets 直到读完，汇总 memberships。
// 按当天张数从多到少排，一样多时按名字。
export async function readDayPeople(query, day, {limit = MAX_PAGE, signal} = {}) {
  const counts = new Map();
  for (let offset = 0; ; offset += limit) {
    const r = await api('/api/assets?' + new URLSearchParams({...query, day, order: 'newest', offset, limit, count: 0}), {signal});
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
// 两次读到的同一天是不是一样：部件画出来的东西（张数、角色数、前几位角色、大图、每张图的尺寸、收藏、主色、角色和副本数）都没变。
// 一样时沿用旧对象，部件不用更新。
const shown = g => JSON.stringify([g.count, g.people, g.hero, (g.characters || []).map(c => [c.id, c.name, c.tag, c.count]),
  (g.items || []).map(i => [i.uuid, i.width, i.height, !!i.fav, i.color || '', i.dup?.size || 0, (i.memberships || []).map(m => [m.id, m.name])])]);
export function sameDay(a, b) {
  return !!a && !!b && a.day === b.day && shown(a) === shown(b);
}
// 已读到的天 list，并进重读的一段 fresh（从新到旧，读回来的 next 是它的游标）。重读的是从 newest（不含这一天，before 游标）
// 往前的几天；newest 为空是从最新的一天读起。这一段覆盖的日期范围：上界是 newest（为空时没有上界），下界是 fresh 最早的一天；
// next 为空说明读到底了，下界一直到最早。范围里以 fresh 为准：没变的天沿用旧对象，没了的天去掉（筛选下最后一张被移走），
// 新的一天插进来；范围以外的天原样留着。
// 返回 {days, changed, end}：changed 是换了对象或去掉了的日期；end 为真时范围已经到了已读到的最早一天以下（调用方把游标换成 next）。
export function mergeDays(list, fresh, {newest = '', next = ''} = {}) {
  const floor = next ? fresh[fresh.length - 1]?.day : '';
  if (next && !floor) return {days: list, changed: new Set(), end: false};
  const inRange = day => (!newest || day < newest) && (!floor || day >= floor);
  const before = new Map(list.map(g => [g.day, g])), changed = new Set();
  const merged = fresh.map(g => {
    const old = before.get(g.day);
    if (old && sameDay(old, g)) return old;
    changed.add(g.day);
    return g;
  });
  for (const g of list) if (inRange(g.day) && !fresh.some(x => x.day === g.day)) changed.add(g.day);
  const above = list.filter(g => newest && g.day >= newest), below = list.filter(g => !inRange(g.day) && !(newest && g.day >= newest));
  const end = !list.length || !floor || floor <= list[list.length - 1].day;
  return {days: [...above, ...merged, ...below], changed, end};
}
// 某一天的下一天（'YYYY-MM-DD'）：要从这一天起往前读时当 before 游标（游标不含它自己）。
export function dayAfter(day) {
  const [y, m, d] = day.split('-').map(Number), next = new Date(Date.UTC(y, m - 1, d + 1));
  return next.toISOString().slice(0, 10);
}
// 今天（本地时间）是哪一天，'YYYY-MM-DD'。
export function todayKey(now = new Date()) {
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
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
const ratioOf = x => x.width > 0 && x.height > 0 ? x.width / x.height : 0;
// 一天的格子：标准的马赛克（core/layout.js 的 dayMosaic）。返回 {cells, height, rows: 0}；每格 {item, box, hero, index, more}，
// more 是盖在这一格上的「+N 张」：马赛克没画的（dayMosaic 的 more 已含这一格自己）加上这一天后端没给的张数。
export function dayCells(g, {width, rowHeight, gap}, hero = '') {
  const items = heroFirst(g, hero), laid = dayMosaic(items.map(ratioOf), {width, rowHeight, gap});
  const unsent = Math.max(0, (g.count || 0) - items.length), last = laid.boxes.length - 1;
  const cells = laid.boxes.map((box, k) => ({item: items[box.i], box, hero: k === 0, index: k,
    more: k === last && (box.more || unsent) ? (box.more || 1) + unsent : 0}));
  return {cells, height: laid.height, rows: 0};
}
// 同步进行中「今天」那一段怎么长（用户 2026-11-20 定，#707）：已经显示的图不动，也不被挤进「+N」；这一批新来的图接在下面
// 多长一行（行高同小图，按宽高比从左往右放，放不下的算进没画的张数）；最多多长 maxRows 行，再来的新图只加大没画的张数。
// 没画的张数（hidden）不盖在哪一张图上（盖上去那张图就点不开了），由一天的部件写在页头「在网格里看」上（#757 复核第 3 条）。
//
// prev 是这一天现在的格子（第一次是标准的 dayCells），old 是上一次读到的这一天，g 是这次重读到的。后端每天只给最新的
// 18 张（items，按新到旧），所以要分清三种情况：
//   新来的图：排在样本里第一张以前见过的图前面、以前没见过的（以前排在「+N」里、这次因为别的图走了才进样本的旧图不算）
//   被移走的图（撤销、同步失败回滚、不再符合筛选）：已显示的图不在新样本里，而且样本是全的（张数等于 count），或者样本里
//     出现了上一次排在它后面的图——这时返回 null，页面改按标准的马赛克排
//   只改了元数据（收藏、主色、角色）：没有新来的也没有被移走的，格子原地换成新数据，不挪、不长行
// 返回新的 {cells, height, rows, hidden}，或者 null。
export function growCells(prev, old, g, {width, rowHeight, gap}, maxRows = 2) {
  const items = g.items || [], byId = new Map(items.map(x => [x.uuid, x]));
  const complete = items.length >= (g.count || 0), before = (old?.items || []).map(x => x.uuid);
  for (const c of prev.cells) {
    const u = c.item.uuid;
    if (byId.has(u)) continue;
    const at = before.indexOf(u);
    if (complete || (at >= 0 && before.slice(at + 1).some(x => byId.has(x)))) return null;
  }
  const known = new Set([...before, ...prev.cells.map(c => c.item.uuid)]), first = items.findIndex(x => known.has(x.uuid));
  const fresh = (first < 0 ? items : items.slice(0, first)).filter(x => !known.has(x.uuid));
  const cells = prev.cells.map(c => ({...c, item: byId.get(c.item.uuid) || c.item, more: 0}));
  let {height, rows} = prev;
  if (fresh.length && rows < maxRows) {
    const y = height + gap;
    let x = 0;
    for (const item of fresh) {
      const w = Math.min(width, Math.min(FIT_MAX, Math.max(FIT_MIN, ratioOf(item) || .75)) * rowHeight);
      if (x && x + w > width) break;
      cells.push({item, box: {x, y, w, h: rowHeight}, hero: false, index: cells.length, more: 0});
      x += w + gap;
    }
    height = y + rowHeight; rows++;
  }
  return {cells, height, rows, hidden: Math.max(0, (g.count || 0) - cells.length)};
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
// 一天由上到下：页头、角色芯片行（当天有角色时）、马赛克（大图高两行；同步时长出来的行另算）。芯片行收起时一行高；展开后的
// 高度由界面量了报上来。sizes：{head, chips, rowHeight, gap, after}（ui/day.js 的 daySizes，唯一的出处），after 是两天之间的距离。
export const mosaicHeight = ({rowHeight, gap}) => rowHeight * 2 + gap;
export function dayHeight(g, sizes, chipsHeight = sizes.chips, mosaic = mosaicHeight(sizes)) {
  const chips = (g.characters || []).length ? chipsHeight : 0;
  return sizes.head + chips + mosaic + sizes.after;
}
// 各天的框（交给 ui/virtual.js）：一列，从 top 往下排。opened 是展开了芯片行的天 -> 量到的芯片行高度；mosaics 是同步时
// 长了行的天 -> 马赛克的高度。返回 {boxes, height}；框带上这一天的数据 g。
export function dayBoxes(days, {width, top = 0, sizes, opened = new Map(), mosaics = new Map()}) {
  let y = top;
  const boxes = days.map(g => {
    const h = dayHeight(g, sizes, opened.get(g.day), mosaics.get(g.day));
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
// 「在网格里看」和点当天的角色：带着现在的筛选和搜索去网格（core/rail.js 的 viewRoute，和切视图同一套），只看这一天；
// 点角色时再加一个 char 条件（core/filters.js 的 addCondition：不合法的地址参数照常读，「不含」里的同一个角色拿掉）。
export function gridRoute(route, day, char = '') {
  const r = {...viewRoute(route, 'grid'), day};
  return char ? addCondition(r, 'char:' + char) : r;
}

// ---------- 认一认 ----------
// 下一批要问的：一个角色，几张机器认出的图；没有可问的是 null，读失败时抛错（调用方分得清「没题」和「没读到」）。
// skip 是这一轮已经问过的角色；后端最多收 ASK_SKIP 个，只带最近的那些。
export const ASK_SKIP = 50;
export async function readAsk(nsfw, skip = [], {signal} = {}) {
  return (await api('/api/ask?' + new URLSearchParams({nsfw, skip: skip.slice(-ASK_SKIP).join(',')}), {signal})).batch || null;
}
// 回答一批（一步、一个撤销）：返回后端的回复（带 undo）。失败时抛错：带 status 的是后端明确拒绝了（锁库、参数不对），
// 不带的是没收到回复（断连），这时用 askLanded 核对这一步是不是已经写进去了。
export const answerAsk = body => post('/api/ask/answer', body);
// 断连以后核对：这一批要写的每条人工结论（「是」的图对原角色 confirmed、「不是」的图对原角色 rejected、「点掉的是…」的图对
// 选中的角色 confirmed）是不是都已经在详情的 assertions 里。都在才算写进去了；读不到也算没有。
export async function askLanded(body, {signal} = {}) {
  const want = [...body.yes.map(u => [u, body.entity, 'confirmed']), ...body.no.map(u => [u, body.entity, 'rejected']),
    ...(body.switch ? body.no.map(u => [u, body.switch, 'confirmed']) : [])];
  try {
    for (const [uuid, entity, status] of want) {
      const d = await api('/api/detail?id=' + encodeURIComponent(uuid), {signal});
      if (!(d.assertions || []).some(x => x.id === entity && x.status === status)) return false;
    }
    return true;
  } catch { return false; }
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
