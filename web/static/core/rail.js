// 活页条的数据（#706）：一级入口、地址落在哪个入口和哪一项、图库二级层的时间轴、资料库二级层钉住的项。不碰 DOM，
// 可以在 Node 里单独测试（tests/rail.test.cjs）。画在哪、动效在 ui/rail.js。
//
// 二级层每一项是 {key, label, meta, href, kind, cover?, count?, round?, first?}：
//   key    在这个入口里唯一，选中、指示条、动画都按它（'d:2026-11-17'、'm:2026-10'、'y:2025'、'fav'、'g:3'、'c:<角色>'、
//          'w:<作品>'、'unmatched'……）
//   kind   'day' | 'month' | 'year' | 'gallery' | 'char' | 'work' | 'queue'，决定画成什么样
//   cover  封面图的 uuid（没有时画占位色）；count 是数字角标（整理的队列）；first 是月、年里最新的那一天（点它滚到那里）
//   peek   悬停预览里排的图 [{id, w?, h?}]（w、h 是原图的宽高，用来按真实宽高比排；没有时按图读到以后的样子排）。
//          不给时预览只放封面（有的话）；peekRound 为真时画成圆头像（作品的角色）
//   color  这一项的主色（十六进制），悬停预览拿它做很淡的环境光和彩色投影；没有时用中性色
import {castWorks} from './catalog.js';
import {listeners} from './events.js';
import {favoritesOnly} from './favorites.js';
import {t} from './i18n.js';
import {number} from './format.js';
import {libraryPrefs} from './prefs.js';
import {CHARACTER, libraryFilters, pageOf, viewOf} from './pages.js';
import {api} from './session.js';
import {state} from './state.js';
import {currentLibraryId} from './settings.js';
import {hashFor} from './route.js';

// ---------- 一级入口 ----------
// 顺序就是 ⌘1–4（Windows 上 Ctrl）。设置不在这里：它是底部的齿轮。
export const ENTRIES = [
  {id: 'library', label: () => t('图库')},
  {id: 'catalog', label: () => t('资料库')},
  {id: 'galleries', label: () => t('画廊')},
  {id: 'tidy', label: () => t('整理')},
];

// 地址落在哪个入口、二级层里选中哪一项。char 默认取路由的 char（角色还没读到时也认得出角色页）。
export function railAt(route, char = route.char) {
  if (favoritesOnly(route)) return {entry: 'galleries', item: 'fav'};
  const view = viewOf(route, char);
  if (view.gallery) return {entry: 'galleries', item: 'g:' + route.id};
  if (view === CHARACTER) return {entry: 'catalog', item: 'c:' + route.char};
  // 只按一部作品筛选的图库就是作品页（F #711 另做作品页之前），在资料库下，钉住了就选中它。
  if (workOnly(route)) return {entry: 'catalog', item: 'w:' + route.work};
  // 按天、日历带着 at（滚到哪一天、看哪个月）时，时间轴选中那一天（或它的月、年）。
  if (view.entry === 'library' && route.at) return {entry: 'library', item: itemForDay(timeline.items, route.at)};
  return {entry: view.entry || '', item: view.item || ''};
}
// 只按一部作品筛选、没有别的条件的图库。
const OTHER = ['q', 'tag', 'utag', 'day', 'artist', 'char', 'from', 'to', 'like', 'has', 'not'];
export const workOnly = route => pageOf(route).view === 'grid' && !!route.work && !OTHER.some(k => Array.isArray(route[k]) ? route[k].length : route[k]);

// 图库：本次运行里切过的视图，再点「图库」时沿用（#703）；筛选条件不带过去。
let libraryView = 'days';
export function noteLibraryView(route) {
  const v = pageOf(route).view;
  if (v && pageOf(route).entry === 'library') libraryView = v;
}
const VIEW_PAGE = {days: 'days', grid: 'library', calendar: 'calendar'};
export const viewPage = v => VIEW_PAGE[v] || 'days';
// 图库三种视图之间切换：带着筛选和搜索（包括旧的单值参数 day，它在三种视图里都是「只看这一天加入的」），换页面（和视图）；
// 页头的分段控件、时间轴的项用它。排序、角色、例图不带过去；at 由调用方另给。
export function viewRoute(route, v) {
  const keep = {};
  for (const k of ['q', 'tag', 'utag', 'work', 'artist', 'day', 'has', 'not', 'from', 'to']) if (route[k]?.length) keep[k] = route[k];
  return {page: viewPage(v), ...keep};
}
// 时间轴的一项点了去哪（WEB_V2.md「页面框架与活页条」）：在日历里看那一天所在的月；在按天、网格里去按天视图的那一天。
// 都带着现在的筛选。按天视图自己能滚过去时（C1 登记 onPick）不换地址。
export const timelineHref = (route, day) => hashFor({...viewRoute(route, pageOf(route).view === 'calendar' ? 'calendar' : 'days'), at: day});
// 一级入口点了去哪：整理去第一个有待处理的队列，没有就去未识别。
export function entryHref(id) {
  if (id === 'library') return hashFor({page: viewPage(libraryView)});
  if (id === 'catalog') return '#/characters';
  if (id === 'galleries') return '#/galleries';
  if (id === 'tidy') return queues().find(q => q.count)?.href || '#/groups';
  return '';
}
// 整理有没有待处理的（一级入口上亮一个点）。
export const tidyWaiting = () => queues().some(q => q.count);

// ---------- 整理：三个队列，另有题时随机抽查、身份整理 ----------
// 数字和点进去落在哪个页签与原来侧栏的计数一致（docs/WEB_UI_MAP.md「侧栏计数」）。route 给出现在在哪：在随机抽查、身份整理上
// 时，即使没题了也留着它们这一项。
export function queues(route = state.route) {
  const at = pageOf(route).item;
  const q = [
    {key: 'unmatched', kind: 'queue', label: t('未识别'), count: state.pools.unmatched || 0, href: '#/groups', meta: t('还没认出角色的图')},
    {key: 'duplicates', kind: 'queue', label: t('重复'), count: state.dupInfo.mixed || 0, href: state.dupInfo.mixed ? '#/duplicates?dup=mixed' : '#/duplicates',
      meta: t('角色不一致的副本组')},
    {key: 'excluded', kind: 'queue', label: t('已排除'), count: state.pendingCount || 0, href: state.pendingCount ? '#/pending' : '#/excluded', meta: t('像截图、待确认的图')},
  ];
  if (state.pools.audit || at === 'audit') q.push({key: 'audit', kind: 'queue', label: t('随机抽查'), count: state.pools.audit || 0, href: '#/audit', meta: t('随机抽查还剩几张')});
  if (state.identityCount || at === 'identity') {
    q.push({key: 'identity', kind: 'queue', label: t('身份整理'), count: state.identityCount || 0, href: '#/identity', meta: t('长得像、可能是同一人或同一人另一个造型的角色对')});
  }
  for (const x of q) x.meta = x.count ? `${x.meta} · ${number(x.count)}` : x.meta;
  return q;
}

// ---------- 画廊：收藏在最前，后面是保存的画廊 ----------
export function galleryItems(covers = {}) {
  const fav = {key: 'fav', kind: 'gallery', label: t('收藏'), heart: true, href: hashFor({page: 'library', has: ['fav']}), meta: t('在详情面板或大图里点心形，或者按 F 收藏。')};
  return [fav, ...(state.galleries || []).map(g => ({key: 'g:' + g.id, kind: 'gallery', label: g.name, cover: covers[g.id] || null,
    href: hashFor({page: 'gallery', id: String(g.id), has: []}), meta: ''}))];
}

// ---------- 图库：时间轴 ----------
// 最近 RECENT 天各是一项（日期封面）；更早的、和最新那天同一年的按月（叠成一摞的封面，写月份）；再早的按年。
// 用户 2026-11-19 看概念稿时定（#706）。days 是按日期从新到旧的 [{day: 'YYYY-MM-DD', count, cover}]。
export const RECENT = 5;
export function groupTimeline(days) {
  if (!days.length) return [];
  const year = days[0].day.slice(0, 4), items = [], buckets = new Map();
  days.forEach((d, i) => {
    const pics = d.samples?.length ? d.samples : d.cover ? [{id: d.cover}] : [];
    if (i < RECENT) { items.push({key: 'd:' + d.day, kind: 'day', day: d.day, first: d.day, count: d.count, cover: d.cover, color: d.color || null, peek: pics, label: d.day}); return; }
    const sameYear = d.day.slice(0, 4) === year, key = sameYear ? 'm:' + d.day.slice(0, 7) : 'y:' + d.day.slice(0, 4);
    let b = buckets.get(key);
    if (!b) {
      b = {key, kind: sameYear ? 'month' : 'year', first: d.day, last: d.day, days: 0, count: 0, cover: d.cover, cover2: null, color: d.color || null, peek: [], label: key.slice(2)};
      buckets.set(key, b); items.push(b);
    }
    b.last = d.day; b.days++; b.count += d.count;
    if (!b.cover2 && d.cover && d.cover !== b.cover) b.cover2 = d.cover;
    // 月、年的预览：每天的头一张，从新到旧，最多 PEEK_POOL 张（放几张由预览排得下多少决定，见 peekRow）
    if (b.peek.length < PEEK_POOL && pics[0]) b.peek.push(pics[0]);
    b.color ||= d.color || null;
  });
  return items;
}
// 一项的预览最多带几张图：一行最宽 PEEK.max，竖图大约放得下五六张，再多也排不下。
export const PEEK_POOL = 8;

// ---------- 悬停预览：一行图按真实宽高比排 ----------
// 预览里的图排成一行、一样高，宽度按原图的宽高比（太窄、太宽的限在 PEEK.ratio 之间，裁掉一点），从前往后放，放到一行
// 放不下为止：放几张由排得下多少决定，不写死。只有一张时排得高一些（角色、画廊的封面）。圆头像（作品的角色）是正圆，小一些。
// 尺寸和 css/rail.css 的 .rail-peek 一致：gap 是 --space-4。返回 {h, width, pics: [{id, w}]}，没有图时 pics 是空的。
export const PEEK = {h: 84, solo: 128, face: 56, max: 288, gap: 4, ratio: [0.6, 2]};
export function peekRow(pics = [], {round = false} = {}) {
  const list = pics.filter(p => p?.id), h = round ? PEEK.face : list.length === 1 ? PEEK.solo : PEEK.h, out = [];
  let width = 0;
  for (const p of list) {
    const r = round ? 1 : p.w > 0 && p.h > 0 ? Math.min(PEEK.ratio[1], Math.max(PEEK.ratio[0], p.w / p.h)) : 1;
    const w = Math.min(PEEK.max, Math.round(r * h)), next = width + (out.length ? PEEK.gap : 0) + w;
    if (out.length && next > PEEK.max) break;
    out.push({id: p.id, w, known: round || (p.w > 0 && p.h > 0)}); width = next;
  }
  return {h, width, pics: out};
}

// 按天视图滚到某一天时选中哪一项（C1 调 ui/rail.js 的 selectDay）：那一天在时间轴里就是它，否则是它所在的月，再否则是年。
export function itemForDay(items, day) {
  if (!day) return '';
  for (const key of ['d:' + day, 'm:' + day.slice(0, 7), 'y:' + day.slice(0, 4)]) if (items.some(x => x.key === key)) return key;
  return '';
}

// 读时间轴：从这个月起按 /api/calendar 的 prev 一个月一个月往前读（#718：每天的张数和最多四张样图；没有按月汇总的接口，
// 几百天的图库要读十几次）。参数是图库的筛选（libraryFilters，三种视图共用）和 NSFW 设置。
// - 参数没变、也没有写入时不重读（换页、改视图不会触发）；同步到了新图（newest）只重读最新的那个月，更早的沿用。
// - 写入、撤销之后（forgetTimeline）整条重读：排除、删图会改旧月份的张数和封面。
// - 换了参数时边读边通知（先看到最近的几个月）；同一组参数重读时读完才换。读失败保留上一次的结果。新的一轮开始时取消上一轮。
export const timeline = {items: [], params: '', months: new Map(), dirty: true, changed: listeners()};
let timelineRun = null;
const samples = list => (list || []).filter(x => x?.uuid).slice(0, 4).map(x => ({id: x.uuid, w: Number(x.width) || 0, h: Number(x.height) || 0}));
const thisMonth = (now = new Date()) => `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}`;
export function forgetTimeline() { timeline.dirty = true; }
function daysOf(months) {
  const days = new Map();
  for (const page of months.values()) for (const d of page.days || []) {
    const pics = samples(d.samples);
    days.set(d.day, {day: d.day, count: d.count, cover: pics[0]?.id || null, samples: pics, color: d.color || null});
  }
  return groupTimeline([...days.values()].sort((a, b) => b.day.localeCompare(a.day)));
}
export async function loadTimeline(route = state.route, {newest = false, month: start = thisMonth()} = {}) {
  const base = {...libraryFilters(route), ...(state.nsfw ? {nsfw: state.nsfw} : {})}, key = JSON.stringify(base);
  const same = key === timeline.params;
  if (same && !timeline.dirty && !newest) return;
  timelineRun?.abort();
  const run = timelineRun = new AbortController(), keep = same && !timeline.dirty ? timeline.months : new Map(), months = new Map();
  let month = start, first = true;
  try {
    while (month) {
      const cached = !first && keep.get(month);
      const page = cached || await api('/api/calendar?' + new URLSearchParams({...base, month}), {signal: run.signal});
      if (run.signal.aborted) return;
      months.set(month, page); first = false;
      if (!same) { timeline.items = daysOf(months); timeline.params = key; await timeline.changed.emit(); }
      month = page.prev || '';
    }
    Object.assign(timeline, {items: daysOf(months), months, params: key, dirty: false});
    await timeline.changed.emit();
  } catch (e) {
    if (run.signal.aborted) return;
    timeline.dirty = true;   // 下次再读；这一次的结果留着
  } finally {
    if (timelineRun === run) timelineRun = null;
  }
}

// ---------- 资料库：钉住的作品和角色（#684） ----------
// 存在浏览器本地，按图库分开（core/prefs.js 的 libraryPrefs，#523），键 'railPins'，值是有序的键列表（'c:<角色 id>'、'w:<作品键>'）。
// 钉住、取消钉住的按钮在资料库的海报和作品卡上（#711）；这里管存取和顺序，ui/rail.js 画、拖动排序。
export const pins = {keys: [], ready: null, library: '', changed: listeners()};
// 读不到图库 id（/api/libraries 失败）时不缓存这次读出的空列表、也不写，免得拿到 id 以后用只含新那一项的列表盖掉存着的。
export function loadPins() {
  pins.ready ||= currentLibraryId().then(id => {
    if (!id) { pins.ready = null; return pins.keys; }
    const v = libraryPrefs(id).get('railPins', []);
    pins.library = id; pins.keys = Array.isArray(v) ? v.filter(k => typeof k === 'string') : [];
    return pins.keys;
  });
  return pins.ready;
}
async function savePins(keys, change) {
  await loadPins();
  if (!pins.library) return;
  pins.keys = keys;
  libraryPrefs(pins.library).set('railPins', keys);
  await pins.changed.emit(change);
}
export const isPinned = key => pins.keys.includes(key);
// 钉住：放到末尾。取消：去掉。move：挪到第 to 个（拖动排序）。
export async function pin(key) { await loadPins(); if (!isPinned(key)) await savePins([...pins.keys, key], {added: key}); }
export async function unpin(key) { await loadPins(); if (isPinned(key)) await savePins(pins.keys.filter(k => k !== key), {removed: key}); }
export async function movePin(key, to) {
  await loadPins();
  const keys = pins.keys.filter(k => k !== key);
  if (keys.length === pins.keys.length) return;
  keys.splice(Math.max(0, Math.min(keys.length, to)), 0, key);
  await savePins(keys, {moved: key});
}
// 钉住的键 → 二级层的项。角色和作品不在了（被合并、删掉）的不画，存着的键留着（换回来时还在）。
export function pinItems(keys = pins.keys) {
  const works = new Map(castWorks().map(w => [String(w.key), w]));
  return keys.map(key => {
    const id = key.slice(2);
    if (key.startsWith('c:')) {
      const c = state.charMap.get(id); if (!c) return null;
      return {key, kind: 'char', label: c.name, round: true, cover: c.cover || null, color: c.color || null, href: hashFor({page: 'library', char: id}),
        meta: t('{n} 张', {n: number(c.count || 0)})};
    }
    const w = works.get(id); if (!w || w.pseudo) return null;
    // 作品的预览是它的角色：一排圆头像（圆形是人）。
    return {key, kind: 'work', label: w.name, cover: w.chars?.[0]?.cover || null, href: hashFor({page: 'library', work: id}),
      peek: (w.chars || []).filter(c => c.cover).slice(0, PEEK_POOL).map(c => ({id: c.cover})), peekRound: true,
      meta: t('{n} 位角色', {n: number(w.chars?.length || 0)})};
  }).filter(Boolean);
}
