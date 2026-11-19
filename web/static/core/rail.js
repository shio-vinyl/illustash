// 活页条的数据（#706）：一级入口、地址落在哪个入口和哪一项、图库二级层的时间轴、资料库二级层钉住的项。不碰 DOM，
// 可以在 Node 里单独测试（tests/rail.test.cjs）。画在哪、动效在 ui/rail.js。
//
// 二级层每一项是 {key, label, meta, href, kind, cover?, count?, round?, first?}：
//   key    在这个入口里唯一，选中、指示条、动画都按它（'d:2026-11-17'、'm:2026-10'、'y:2025'、'fav'、'g:3'、'c:<角色>'、
//          'w:<作品>'、'unmatched'……）
//   kind   'day' | 'month' | 'year' | 'gallery' | 'char' | 'work' | 'queue'，决定画成什么样
//   cover  封面图的 uuid（没有时画占位色）；count 是数字角标（整理的队列）；first 是月、年里最新的那一天（点它滚到那里）
import {castWorks} from './catalog.js';
import {listeners} from './events.js';
import {favoritesOnly} from './favorites.js';
import {t} from './i18n.js';
import {number} from './format.js';
import {libraryPrefs} from './prefs.js';
import {CHARACTER, PAGES, pageOf, viewOf} from './pages.js';
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
  return {entry: view.entry || '', item: view.item || ''};
}

// 图库：本次运行里切过的视图，再点「图库」时沿用（#703）；筛选条件不带过去。
let libraryView = 'days';
export function noteLibraryView(route) {
  const v = pageOf(route).view;
  if (v && pageOf(route).entry === 'library') libraryView = v;
}
const VIEW_PAGE = {days: 'days', grid: 'library', calendar: 'calendar'};
export const viewPage = v => VIEW_PAGE[v] || 'days';
// 图库三种视图之间切换：带着筛选和搜索，换页面（和视图）；页头的分段控件用它。
export function viewRoute(route, v) {
  const keep = {};
  for (const k of ['q', 'tag', 'utag', 'work', 'artist', 'has', 'not', 'from', 'to']) if (route[k]?.length) keep[k] = route[k];
  return {page: viewPage(v), ...keep};
}
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
    if (i < RECENT) { items.push({key: 'd:' + d.day, kind: 'day', day: d.day, first: d.day, count: d.count, cover: d.cover, peek: (d.samples || []).slice(1, 4), label: d.day}); return; }
    const sameYear = d.day.slice(0, 4) === year, key = sameYear ? 'm:' + d.day.slice(0, 7) : 'y:' + d.day.slice(0, 4);
    let b = buckets.get(key);
    if (!b) {
      b = {key, kind: sameYear ? 'month' : 'year', first: d.day, last: d.day, days: 0, count: 0, cover: d.cover, cover2: null, peek: [], label: key.slice(2)};
      buckets.set(key, b); items.push(b);
    }
    b.last = d.day; b.days++; b.count += d.count;
    if (!b.cover2 && d.cover && d.cover !== b.cover) b.cover2 = d.cover;
    if (b.peek.length < 3 && d.cover && d.cover !== b.cover) b.peek.push(d.cover);
  });
  return items;
}
// 按天视图滚到某一天时选中哪一项（C1 调 ui/rail.js 的 selectDay）：那一天在时间轴里就是它，否则是它所在的月，再否则是年。
export function itemForDay(items, day) {
  if (!day) return '';
  for (const key of ['d:' + day, 'm:' + day.slice(0, 7), 'y:' + day.slice(0, 4)]) if (items.some(x => x.key === key)) return key;
  return '';
}

// 读时间轴要的天：最近几天从 /api/home 来（带封面），更早的按 /api/calendar 的 prev 一个月一个月往前读（#718；没有按月汇总的
// 接口，几百天的图库要读十几次，读到一批就通知一次）。参数是图库的筛选（按天、日历和网格共用，#703 第 2 问）和 R18。
// 同一组参数重读时只重读最新的那个月（同步到了新图），更早的月份沿用。
export const timeline = {items: [], params: '', loading: false, months: new Map(), changed: listeners()};
let timelineRun = 0;
const samples = list => (list || []).map(x => x?.uuid).filter(Boolean).slice(0, 4);
export async function loadTimeline(route = state.route) {
  const filters = {...(PAGES.library.params(route) || {})};
  delete filters.like; delete filters.by;
  const base = {...filters, ...(state.nsfw ? {nsfw: state.nsfw} : {})}, key = JSON.stringify(base);
  const run = ++timelineRun, same = key === timeline.params;
  if (!same) { timeline.months = new Map(); timeline.params = key; }
  timeline.loading = true;
  const days = new Map();
  try {
    const home = await api('/api/home?' + new URLSearchParams({...base, days: RECENT}));
    if (run !== timelineRun) return;
    for (const g of home.days || []) { const pics = samples(g.items); days.set(g.day, {day: g.day, count: g.count ?? g.items?.length ?? 0, cover: pics[0] || null, samples: pics}); }
    const newest = [...days.keys()].sort().pop();
    let month = newest ? newest.slice(0, 7) : '', first = true;
    while (month) {
      const cached = same && !first && timeline.months.get(month);
      // 读不到某个月（旧宿主没有这个接口、读的时候出错）就停在这里：最近几天照样有。
      const page = cached || await api('/api/calendar?' + new URLSearchParams({...base, month})).catch(() => null);
      if (run !== timelineRun || !page) return;
      timeline.months.set(month, page);
      first = false;
      for (const d of page.days || []) if (!days.has(d.day)) { const pics = samples(d.samples); days.set(d.day, {day: d.day, count: d.count, cover: pics[0] || null, samples: pics}); }
      timeline.items = groupTimeline([...days.values()].sort((a, b) => b.day.localeCompare(a.day)));
      if (!cached) await timeline.changed.emit();
      month = page.prev || '';
    }
  } finally {
    if (run === timelineRun) { timeline.loading = false; timeline.items = groupTimeline([...days.values()].sort((a, b) => b.day.localeCompare(a.day))); await timeline.changed.emit(); }
  }
}

// ---------- 资料库：钉住的作品和角色（#684） ----------
// 存在浏览器本地，按图库分开（core/prefs.js 的 libraryPrefs，#523），键 'railPins'，值是有序的键列表（'c:<角色 id>'、'w:<作品键>'）。
// 钉住、取消钉住的按钮在资料库的海报和作品卡上（#711）；这里管存取和顺序，ui/rail.js 画、拖动排序。
export const pins = {keys: [], ready: null, changed: listeners()};
const store = async () => libraryPrefs(await currentLibraryId());
export function loadPins() {
  pins.ready ||= store().then(s => { const v = s.get('railPins', []); pins.keys = Array.isArray(v) ? v.filter(k => typeof k === 'string') : []; return pins.keys; });
  return pins.ready;
}
async function savePins(keys, change) {
  await loadPins();
  pins.keys = keys;
  (await store()).set('railPins', keys);
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
      return {key, kind: 'char', label: c.name, round: true, cover: c.cover || null, href: hashFor({page: 'library', char: id}), meta: t('{n} 张', {n: number(c.count || 0)})};
    }
    const w = works.get(id); if (!w || w.pseudo) return null;
    return {key, kind: 'work', label: w.name, cover: w.chars?.[0]?.cover || null, href: hashFor({page: 'library', work: id}),
      meta: t('{n} 位角色', {n: number(w.chars?.length || 0)})};
  }).filter(Boolean);
}
