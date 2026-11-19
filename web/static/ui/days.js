// 图库的「按天」视图（#707，0.4 第 C1 步；首页并进图库，#677）：页面模块，照 docs/WEB_V2.md「页面样板」写。
// 顶上两张卡（同步状态、认一认，ui/day_cards.js），下面一天一段（ui/day.js），只挂视口附近的几天（ui/virtual.js），
// 接近底部接着读更早的几天，读完写「最早的一张在 X 加入 · 共 N 张」。数据在 core/days.js。
//
// - 部件在进这一页时建一次（daysView）；筛选、搜索、at 变了收到 render({samePage: true})，refresh(why) 也只更新已有的部件。
// - 同步：只重读最新的两天并进来（core/days.js 的 mergeDays），没变的天不动；变了的那一天里图块按 uuid 复用，新图淡入（#683）。
// - 改动、撤销之后：重读视野附近的几天。切换 NSFW：从头读。视图选项：只重排。
// - 滚动时告诉活页条现在看到哪一天（selectDay）；在这一页里点活页条的某一天或某个月，直接滚过去（onPick），不换地址。
//   地址带 at=YYYY-MM-DD 时（从网格、日历点活页条过来）滚到那一天，没读到先读到那里。
// - 点图打开详情卡片，双击看大图，点「+N 张」进网格看这一天；认一认的图 Space 或长按看大图（wireDays 登记一次）。
import {FIRST_DAYS, MAX_DAYS, MORE_DAYS, askAnswer, askLater, dayAfter, dayAt, dayBoxes, dayQuery, gridRoute, heroFirst, mergeDays, readAsk, readDayPeople,
  readDays, summary} from '../core/days.js';
import {edits} from '../core/edits.js';
import {dayLabel, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {onJob} from '../core/jobs.js';
import {PAGES, implement, onPage} from '../core/pages.js';
import {go, hashFor} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {indexOf, inspect, openViewer, wirePeek} from './detail.js';
import {$, toast} from './dom.js';
import {dayWidget} from './day.js';
import {askCard, syncCard} from './day_cards.js';
import {cls, css, show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {mutate, showList, viewOpts} from './grid.js';
import {enter} from './motion.js';
import {openPicker} from './palette.js';
import {onPick, selectDay} from './rail.js';
import {openTasks} from './tasks.js';
import {virtual} from './virtual.js';

const PAGE = tpl`<div class="days-page"><div class="day-cards" data-ref="cards"></div><div class="days" data-ref="days"></div><div data-ref="empty" hidden></div>
  <div class="day-load" data-ref="load" aria-hidden="true"><i></i><span data-ref="shape"><i></i><i></i><i></i><i></i></span></div>
  <button class="btn is-small day-retry" type="button" data-ref="retry" hidden data-t>读取失败，重试</button><p class="day-end" data-ref="end" hidden></p></div>`;
// 一天各部分的尺寸，和 css/library.css「按天」一段一致（页头 56、收起的芯片行 40）；行高随视图选项的缩略图大小。
const ROWS = [130, 160, 200];
const sizes = () => ({head: 56, chips: 40, rowHeight: ROWS[viewOpts.homeSize] ?? ROWS[1], gap: 6, after: 36});
// 后退回到这一页时按上次读到的天数读，好让页面框架滚回原来的位置（#526）。
let remembered = {key: '', count: 0};
let view = null;

function todayKey() {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

// 这一页的部件：进页面时在页面作用域上建一次。
function daysView(scope) {
  const {el, refs} = PAGE(), viewport = $('viewport');
  $('homePage').replaceChildren(el);
  const v = {scope, route: null, query: null, key: '', days: [], next: '', loading: false, boxes: [], height: 0, width: 0, day: '',
    heroes: new Map(), open: new Set(), opened: new Map(), people: new Map(), fresh: new Map(), widgets: new Set(), batch: null, skip: []};
  const alive = route => scope.alive && state.route === route;

  // ---------- 两张卡 ----------
  const sync = scope.add(syncCard({open: () => openTasks()}));
  const ask = scope.add(askCard({answer, other, later}));
  refs.cards.append(sync.el, ask.el);
  const today = () => v.days[0]?.day === todayKey() ? v.days[0] : null;
  const paintSync = () => sync.update({sync: state.sync, today: today()});

  // ---------- 各天 ----------
  const ctx = {
    sizes, marks: () => viewOpts.marks, people: () => viewOpts.people,
    hero: day => v.heroes.get(day) || '', hold: (day, uuid) => { if (!v.heroes.has(day)) v.heroes.set(day, uuid); },
    href: (day, char) => hashFor(gridRoute(v.route, day, char)), cover: id => state.charMap.get(id)?.cover,
    toggle, resized: (day, h) => { if (v.opened.get(day) !== h) { v.opened.set(day, h); layout(); } },
  };
  const list = virtual(viewport, refs.days, {scope, create: box => {
    const day = dayWidget(box, ctx, scope), dispose = day.dispose, delay = v.fresh.get(box.key);
    v.widgets.add(day); day.dispose = () => { v.widgets.delete(day); dispose(); };
    // 新读到的几天依次淡入（只动透明度：根节点的位移归 ui/virtual.js 管）；之后滚回来重挂时不再播。
    if (delay !== undefined) { v.fresh.delete(box.key); enter(day.el, {from: 'fade', delay}); }
    day.mark(state.focused);
    return day;
  }});

  // 数据或尺寸变了：排各天的框（宽度量到以后），以及末尾的占位、到底的说明、空状态、看图的列表和同步卡。
  function layout() {
    if (v.width) {
      const {boxes, height} = dayBoxes(v.days, {width: v.width, sizes: sizes(), opened: v.opened});
      v.boxes = boxes.map(b => ({...b, open: v.open.has(b.key), people: v.people.get(b.key)}));
      v.height = height;
      list.set(v.boxes, height);
    }
    const s = sizes(), empty = !v.days.length && !v.loading && v.route;
    show(refs.load, !!v.next); cls(refs.load, 'is-busy', v.loading);
    css(refs.shape, 'height', s.rowHeight * 2 + s.gap + 'px');
    const {earliest, total} = summary(v.days);
    show(refs.end, !v.next && v.days.length > 0);
    if (!v.next && earliest) text(refs.end, t('最早的一张在 {day}加入 · 共 {n} 张', {day: dayLabel(earliest), n: number(total)}));
    show(refs.empty, empty);
    if (empty) drawEmpty(refs.empty, {icon: 'image', fact: PAGES.library.empty(v.route, {nsfw: state.nsfw})});
    showList(v.days.flatMap(g => heroFirst(g, v.heroes.get(g.day))));
    remembered = {key: v.key, count: v.days.length};
    paintSync();
  }
  // 宽度量一次，变了才重排（一天的高度和宽度无关，变宽变窄时只重排马赛克和芯片行）。
  const measure = () => {
    const w = refs.days.clientWidth;
    if (w && Math.abs(w - v.width) >= 1) { v.width = w; layout(); }
  };
  if (typeof ResizeObserver === 'function') scope.observe(new ResizeObserver(() => scope.read(measure, 'days:width'))).observe(refs.days);
  // 等 ui/virtual.js 这一轮的读和写都跑完（内容的高度已经写上），再量、再滚。
  const settled = () => new Promise(done => scope.read(() => scope.write(done)));
  const offsetOf = () => refs.days.getBoundingClientRect().top - viewport.getBoundingClientRect().top + viewport.scrollTop;

  // ---------- 滚动：活页条跟着选中、接近底部接着读 ----------
  function onScroll() {
    const top = viewport.scrollTop - offsetOf(), day = dayAt(v.boxes, top + 1);
    if (day && day !== v.day) { v.day = day; selectDay(day); }
    if (v.next && !v.loading && refs.retry.hidden && top + viewport.clientHeight * 2.5 > v.height) more();
  }
  scope.on(viewport, 'scroll', () => scope.read(onScroll, 'days:scroll'), {passive: true});

  // ---------- 读 ----------
  async function load(route, {restore = false} = {}) {
    v.route = route;
    const query = dayQuery(route, state.nsfw), key = JSON.stringify(query);
    if (key === v.key && v.days.length) { if (route.at) await goto(route.at); return; }
    Object.assign(v, {query, key, days: [], next: '', day: '', loading: true});
    for (const m of [v.heroes, v.open, v.opened, v.people, v.fresh]) m.clear();
    show(refs.retry, false); layout();
    // 后退回来：按上次读到的天数读（一次最多 MAX_DAYS 天，不够就接着读），页面框架随后滚回原来的位置。
    const count = restore && remembered.key === key ? Math.max(FIRST_DAYS, remembered.count) : FIRST_DAYS;
    const days = [];
    let next = '';
    do {
      let r;
      try { r = await readDays(query, {before: next, days: Math.min(MAX_DAYS, count - days.length), signal: scope.signal}); }
      catch (e) { if (alive(route)) { v.loading = false; failed(e); } return; }
      if (!alive(route)) return;
      days.push(...r.days.filter(g => !days.some(x => x.day === g.day))); next = r.next;
    } while (next && days.length < count);
    Object.assign(v, {days, next, loading: false});
    scope.read(measure, 'days:width'); layout();
    await settled();
    if (alive(route)) scope.read(onScroll, 'days:scroll');
    if (route.at && alive(route)) await goto(route.at);
  }
  function failed(e) { show(refs.retry, true); layout(); toast(readError(e)); }
  scope.on(refs.retry, 'click', () => { show(refs.retry, false); if (v.days.length) more(); else load(v.route); });

  // 接近底部：再读几天，接在后面依次淡入。
  async function more() {
    const route = v.route, query = v.query;
    v.loading = true; cls(refs.load, 'is-busy', true);
    let r;
    try { r = await readDays(query, {before: v.next, days: MORE_DAYS, signal: scope.signal}); } catch (e) { if (alive(route)) { v.loading = false; failed(e); } return; }
    if (!alive(route) || query !== v.query) return;
    const known = new Set(v.days.map(g => g.day)), added = r.days.filter(g => !known.has(g.day));
    added.forEach((g, i) => v.fresh.set(g.day, i * 90));
    Object.assign(v, {days: [...v.days, ...added], next: r.next, loading: false});
    layout();
    await settled();
    if (alive(route)) scope.read(onScroll, 'days:scroll');
  }
  // 滚到某一天（或比它早的、最近的一天）：还没读到就先一次多读几天，读到那里为止。
  async function goto(day) {
    const route = v.route;
    while (v.next && !v.days.some(g => g.day <= day)) {
      let r;
      try { r = await readDays(v.query, {before: v.next, days: MAX_DAYS, signal: scope.signal}); } catch (e) { if (alive(route)) failed(e); return; }
      if (!alive(route)) return;
      const known = new Set(v.days.map(g => g.day));
      Object.assign(v, {days: [...v.days, ...r.days.filter(g => !known.has(g.day))], next: r.next});
    }
    layout();
    await settled();
    const box = v.boxes.find(b => b.key <= day) || v.boxes.at(-1);
    if (!box || !alive(route)) return;
    scope.read(() => { const top = offsetOf() + box.y; scope.write(() => { viewport.scrollTop = top; }); });
  }
  // 同步进来新图：只重读最新的两天并进来。
  let syncing = false;
  async function newest() {
    if (syncing || !v.days.length) return;
    syncing = true;
    const route = v.route, query = v.query;
    try {
      const r = await readDays(query, {days: 2, signal: scope.signal});
      if (!alive(route) || query !== v.query) return;
      const {days, changed} = mergeDays(v.days, r.days);
      if (changed.size) { v.days = days; layout(); }
    } catch { /* 同步时读失败：下一次再读 */ } finally { syncing = false; }
  }
  // 改动、撤销之后：重读视野附近（上一天起往下几天），其余的天不动。
  async function around() {
    const route = v.route, query = v.query, i = Math.max(0, v.days.findIndex(g => g.day === v.day) - 1);
    const before = i > 0 ? dayAfter(v.days[i].day) : '';
    try {
      const r = await readDays(query, {before, days: 6, signal: scope.signal});
      if (!alive(route) || query !== v.query) return;
      v.days = mergeDays(v.days, r.days, before).days; layout();
    } catch (e) { if (alive(route)) toast(readError(e)); }
  }
  // 展开或收起一天的全部角色：展开时先用已有的前八位撑着，读到全部再换。
  async function toggle(day) {
    if (v.open.has(day)) { v.open.delete(day); v.opened.delete(day); layout(); return; }
    v.open.add(day); layout();
    if (v.people.has(day)) return;
    const route = v.route;
    try {
      const people = await readDayPeople(v.query, day, {signal: scope.signal});
      if (alive(route)) { v.people.set(day, people); layout(); }
    } catch (e) { if (alive(route)) toast(readError(e)); }
  }

  // ---------- 认一认 ----------
  async function nextAsk() {
    const route = state.route;
    const batch = viewOpts.ask ? await readAsk(state.nsfw, v.skip) : null;
    if (scope.alive && state.route === route) { v.batch = batch; ask.update({batch}); }
  }
  async function answer(how, off) {
    const b = v.batch; if (!b || edits.busy) return;
    v.skip.push(b.entity);
    const following = viewOpts.ask ? readAsk(state.nsfw, v.skip) : Promise.resolve(null);   // 卡片换之前下一个角色已经准备好
    await mutate('/api/ask/answer', askAnswer(b, how, off), false);
    const batch = await following;
    if (scope.alive) { v.batch = batch; ask.update({batch}); }
  }
  // 「点掉的是…」（#675 第二步）：选一个角色，点掉的那几张一步记成「不是 TA、是选中的角色」，一个撤销；其余几张留在卡上。
  function other(off) {
    const b = v.batch; if (!b || !off.size || edits.busy) return;
    const ids = b.items.filter(x => off.has(x.uuid)).map(x => x.uuid);
    openPicker({title: t('点掉的是…'), subject: ids, exclude: [b.entity], suggest: {ids},
      sentence: e => t('把 {n} 张从 {from} 换成 {name}', {n: number(ids.length), from: b.name, name: e.name}),
      pick: async e => {
        await mutate('/api/ask/answer', askAnswer(b, 'other', off, e.id), false);
        if (!scope.alive || v.batch !== b) return;
        const rest = b.items.filter(x => !off.has(x.uuid));
        if (rest.length) { v.batch = {...b, items: rest}; ask.update({batch: v.batch}); } else { v.skip.push(b.entity); await nextAsk(); }
      }});
  }
  async function later() {
    const b = v.batch; if (!b) return;
    v.skip.push(b.entity);
    askLater(b).catch(e => toast(e.message));
    await nextAsk();
  }

  // ---------- 点图 ----------
  scope.on(el, 'click', e => {
    const tile = e.target.closest('.day-tile'); if (!tile) return;
    if (tile.dataset.more) go(gridRoute(v.route, tile.closest('.day').dataset.day)); else inspect(tile.dataset.id);
  });
  scope.on(el, 'dblclick', e => { const tile = e.target.closest('.day-tile'); if (tile && !tile.dataset.more) openViewer(indexOf(tile.dataset.id)); });
  // 同步的进度和新图：卡片跟着变；进来了新图就把最新的两天并进来（同步结束时 ui/effects.js 还会调一次 refresh('sync')）。
  scope.add(onJob((job, change) => {
    if (job !== 'sync') return;
    paintSync();
    if (change.arrived) newest();
  }));

  return {
    scope, load, goto, newest, around, nextAsk, layout,
    get batch() { return v.batch; },
    mark: uuid => { for (const day of v.widgets) day.mark(uuid); },
    // 把这张图所在的那一天滚进视野。
    reveal(uuid) {
      const g = v.days.find(x => x.items.some(i => i.uuid === uuid)), box = g && v.boxes.find(b => b.key === g.day);
      if (!box) return;
      scope.read(() => {
        const top = offsetOf() + box.y, now = viewport.scrollTop, h = viewport.clientHeight;
        if (top < now || top + box.h > now + h) scope.write(() => { viewport.scrollTop = Math.max(0, top - 16); });
      });
    },
  };
}

// 启动时调一次：认一认的图 Space 或长按看大图（ui/detail.js 的 wirePeek 不能撤销，所以挂在常驻的主区上，只挂一次）；
// 活页条里点某一天、某个月：已经在按天视图里时直接滚过去，不换地址（WEB_V2.md「页面框架与活页条」）。
export function wireDays() {
  wirePeek($('homePage'), () => view?.batch?.items.map(x => x.uuid) || []);
  onPick('library', item => {
    if (!onPage(state.route, 'days') || !view?.scope.alive || !item?.first) return false;
    view.goto(item.first);
    return true;
  });
}

implement({days: {
  // 刚进这一页（或作用域已经释放）才建；同一页又画一次（筛选、搜索、at 变了）只按新地址更新。
  render: ({samePage, restore, scope}) => {
    if (!samePage || !view?.scope.alive) view = daysView(scope);
    $('selectionBar').hidden = true; $('resultCount').textContent = ''; $('hint').textContent = t('点图片看详情 · 点角色名只看 TA');
    if (!samePage) view.nextAsk();
    return view.load(state.route, {restore});
  },
  reveal: uuid => view?.reveal(uuid),
  // 详情卡片开着的那张（ui/detail.js 打开、关掉详情卡片时调）。
  focus: uuid => view?.mark(uuid),
  head: () => ({title: t('图库'), sub: state.recentCount ? t('最近一次同步新加入 {n} 张', {n: number(state.recentCount)}) : ''}),
  help: () => [t('按加入日期分组。')],
  refresh: why => {
    if (!view?.scope.alive) return;
    if (why === 'view') view.layout();
    else if (why === 'ask') view.nextAsk();
    else if (why === 'sync') view.newest();
    else if (why === 'nsfw') { view.load(state.route); view.nextAsk(); }
    else view.around();
  },
}});
