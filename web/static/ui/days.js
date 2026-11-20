// 图库的「按天」视图（#707，0.4 第 C1 步；首页并进图库，#677）：页面模块，照 docs/WEB_V2.md「页面样板」写。
// 顶上两张卡（同步状态 ui/day_cards.js，认一认 ui/day_ask.js），下面一天一段（ui/day.js），只挂视口附近的几天（ui/virtual.js），
// 接近底部接着读更早的几天，读完写「最早的一张在 X 加入 · 共 N 张」。数据在 core/days.js。
//
// - 部件在进这一页时建一次（daysView）；筛选、搜索、at 变了收到 render({samePage: true})，refresh(why) 也只更新已有的部件。
// - 读取的先后（#757 交叉审查）：
//   · 从头读（进页面、换了筛选、搜索或 NSFW）换一个代号（gen），取消还在路上的请求；每个回复按代号和完整的查询条件（key，含
//     NSFW）核对，对不上就丢掉，不看路由对象（切 NSFW 时路由对象不换）。
//   · 读更早的几天：同一个游标同一时间只读一次（paging），由发起的那一次收尾；回复到的时候游标已经变了（刷新换了范围）就丢掉。
//   · 跳到某一天（at=、点活页条）只认最后一次选的（target），读失败时「重试」接着往那一天读。
//   · 刷新（同步、改动、撤销）排队：读取期间来的刷新记下来，读完再补一次；后来的刷新总在先来的读完以后才发，旧回复画不回去。
//     同步之后那一次重读失败了，过一会儿再试。
// - 同步：只重读最新的两天并进来（core/days.js 的 mergeDays），没变的天不动；变了的那一天里图块按 uuid 复用，新图淡入（#683）。
//   改动、撤销之后：重读视野附近的几天；重读的范围里没了的天（筛选下最后一张被移走）去掉，游标、看图的列表和总张数跟着变。
// - 滚动时告诉活页条现在看到哪一天（selectDay）；在这一页里点活页条的某一天或某个月，直接滚过去（onPick），不换地址。
// - 点图打开详情卡片，双击看大图，点「+N 张」进网格看这一天；认一认的图 Space 或长按看大图（wireDays 登记一次）。
import {FIRST_DAYS, MAX_DAYS, MORE_DAYS, dayAfter, dayAt, dayBoxes, dayCells, dayQuery, gridRoute, growCells, heroFirst, mergeDays, readDayPeople,
  readDays, summary, todayKey} from '../core/days.js';
import {dayLabel, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {onJob} from '../core/jobs.js';
import {PAGES, implement, libraryFilters, onPage} from '../core/pages.js';
import {go, hashFor, historyEntry} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {indexOf, inspect} from './detail.js';
import {openViewer, wirePeek} from './viewer.js';
import {$, toast} from './dom.js';
import {daySizes, dayWidget} from './day.js';
import {dayAsk} from './day_ask.js';
import {syncCard} from './day_cards.js';
import {cls, css, show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {showList, viewOpts} from './grid.js';
import {enter} from './motion.js';
import {onPick, selectDay} from './rail.js';
import {openTasks} from './tasks.js';
import {virtual} from './virtual.js';

const PAGE = tpl`<div class="days-page"><div class="day-cards" data-ref="cards"></div><div class="days" data-ref="days"></div><div data-ref="empty" hidden></div>
  <div class="day-load" data-ref="load" aria-hidden="true"><i></i><span data-ref="shape"><i></i><i></i><i></i><i></i></span></div>
  <button class="btn is-small day-retry" type="button" data-ref="retry" hidden data-t>读取失败，重试</button><p class="day-end" data-ref="end" hidden></p></div>`;
// 一天各部分的尺寸（ui/day.js 的 daySizes，唯一的出处）；小图的行高随视图选项的缩略图大小。
const sizes = () => daySizes(viewOpts.homeSize);
// 同步之后那一次重读失败时，过多久再试、最多试几次（都失败了留「重试」）。
const RETRY_MS = 5000, RETRIES = 3;
// 后退、前进回到这一页时按那一条历史记录上次读到的天数读，好让页面框架滚回原来的位置（#526）：按历史条目的编号记
// （core/route.js 的 historyEntry，和 ui/shell.js 记滚动位置的 panelScroll 同一个编号），{key: 查询条件, count: 天数}。
const depth = new Map();
let view = null;

// 这一页的部件：进页面时在页面作用域上建一次。
function daysView(scope) {
  const {el, refs} = PAGE(), viewport = $('viewport');
  $('homePage').replaceChildren(el);
  // failed：从头读或接着读失败了（挡住接着读）；stale：同步、改动之后的重读失败了（'newest' 或 'around'，画面留着，「重试」重做它）。
  // jump：这一次跳转的凭据（target 是要去的那天，读失败后「重试」接着往那里读）。peopleRead：每天正在读的名单（它的 AbortController）。
  const v = {route: null, query: null, key: '', gen: 0, abort: null, days: [], next: '', reading: false, failed: false, stale: '', paging: null,
    target: '', jump: null, pending: new Set(), refreshing: false, retries: 0, boxes: [], height: 0, width: 0, day: '',
    heroes: new Map(), open: new Set(), opened: new Map(), people: new Map(), peopleRead: new Map(), fresh: new Map(), grow: new Map(), widgets: new Set()};
  // 这一轮读取的凭据：回来的时候代号和查询条件都没变、这一页也还在，回复才算数。
  const ticket = () => ({gen: v.gen, key: v.key});
  const valid = tk => scope.alive && tk.gen === v.gen && tk.key === v.key;
  const signal = () => v.abort.signal;
  scope.add(() => { v.abort?.abort(); dropPeopleReads(); });

  // ---------- 两张卡 ----------
  const sync = scope.add(syncCard({open: () => openTasks()}));
  const ask = dayAsk(scope);
  refs.cards.append(sync.el, ask.el);
  // 「今天认出几位」只在最新一天是今天、而且没开筛选（NSFW 设置也算）时写：开着筛选时那是筛选结果的数，不是这次同步的。
  const today = () => v.days[0]?.day === todayKey() && !Object.keys(libraryFilters(v.route || {})).length && !state.nsfw ? v.days[0] : null;
  const paintSync = () => sync.update({sync: state.sync, today: today(), lost: state.syncLost});

  // ---------- 各天 ----------
  const ctx = {
    sizes, marks: () => viewOpts.marks, people: () => viewOpts.people,
    hero: day => v.heroes.get(day) || '', hold: (day, uuid) => { if (!v.heroes.has(day)) v.heroes.set(day, uuid); },
    href: (day, char) => hashFor(gridRoute(v.route, day, char)), cover: id => state.charMap.get(id)?.cover,
    toggle, resized: (day, h) => { if (v.opened.get(day) !== h) { v.opened.set(day, h); layout(); } },
  };
  // 挂着的天：v.widgets 记着它们（详情卡片开着哪张时逐个标），摘掉时从里面去掉。从头读（筛选、搜索、NSFW 换了）时整个列表
  // 释放、重建：旧的天连同图块直接卸掉，不在新结果上播退场（#757 复核第 6 条：收紧 NSFW 时不该显示的图不能再露一下）。
  const makeList = () => virtual(viewport, refs.days, {scope, create: box => {
    const day = dayWidget(box, ctx, scope), delay = v.fresh.get(box.key);
    const comp = {el: day.el, update: day.update, mark: day.mark, dispose() { v.widgets.delete(comp); day.dispose(); }};
    v.widgets.add(comp);
    // 新读到的几天依次淡入（只动透明度：根节点的位移归 ui/virtual.js 管）；之后滚回来重挂时不再播。
    if (delay !== undefined) { v.fresh.delete(box.key); enter(day.el, {from: 'fade', delay}); }
    day.mark(state.focused);
    return comp;
  }});
  let list = makeList();

  // 数据或尺寸变了：排各天的框（宽度量到以后），以及末尾的占位、到底的说明、空状态、看图的列表和同步卡。
  // 末尾三种状态分开：在读（占位，读的时候呼吸）、读失败（重试按钮）、读完了（到底的说明，或者空状态）。
  function layout() {
    if (v.width) {
      const mosaics = new Map([...v.grow].map(([day, x]) => [day, x.height]));
      const {boxes, height} = dayBoxes(v.days, {width: v.width, sizes: sizes(), opened: v.opened, mosaics});
      v.boxes = boxes.map(b => { const grown = v.grow.get(b.key);
        return {...b, open: v.open.has(b.key), people: v.people.get(b.key), cells: grown?.cells, mosaic: grown?.height, hidden: grown?.hidden}; });
      v.height = height;
      list.set(v.boxes, height);
    }
    const s = sizes(), busy = v.reading || !!v.paging, done = !v.reading && !v.failed && !v.next && !v.stale;
    show(refs.load, !v.failed && (v.reading || !!v.next)); cls(refs.load, 'is-busy', busy);
    css(refs.shape, 'height', s.rowHeight * 2 + s.gap + 'px');
    show(refs.retry, v.failed || !!v.stale);
    const {earliest, total} = summary(v.days), empty = done && !v.days.length && !!v.route;
    show(refs.end, done && v.days.length > 0);
    if (done && earliest) text(refs.end, t('最早的一张在 {day}加入 · 共 {n} 张', {day: dayLabel(earliest), n: number(total)}));
    show(refs.empty, empty);
    if (empty) drawEmpty(refs.empty, {icon: 'image', fact: PAGES.library.empty(v.route, {nsfw: state.nsfw})});
    showList(viewList());
    if (!v.reading) depth.set(historyEntry(), {key: v.key, count: v.days.length});
    paintSync();
  }
  // 看图的列表（详情翻页、大图都按它走）：长了行的天按画着的顺序（后端这次没给回来、但还画着的图也在），其余按大图在前；
  // 按 uuid 去重（#757 复核第 4 条）。
  function viewList() {
    const seen = new Set(), out = [];
    for (const g of v.days) {
      const items = v.grow.get(g.day)?.cells.map(c => c.item) || heroFirst(g, v.heroes.get(g.day));
      for (const x of items) if (!seen.has(x.uuid)) { seen.add(x.uuid); out.push(x); }
    }
    return out;
  }
  // 宽度量一次，变了才重排（一天的高度和宽度无关，变宽变窄时只重排马赛克和芯片行）。同步时长了行的天按标准的马赛克重排。
  const measure = () => {
    const w = refs.days.clientWidth;
    if (w && Math.abs(w - v.width) >= 1) { v.width = w; v.grow.clear(); scope.write(layout, 'days:layout'); }
  };
  if (typeof ResizeObserver === 'function') scope.observe(new ResizeObserver(() => scope.read(measure, 'days:width'))).observe(refs.days);
  // 等 ui/virtual.js 这一轮的读和写都跑完（内容的高度已经写上），再量、再滚。
  const settled = () => new Promise(done => scope.read(() => scope.write(done)));
  const offsetOf = () => refs.days.getBoundingClientRect().top - viewport.getBoundingClientRect().top + viewport.scrollTop;
  function failed(e) { v.failed = true; layout(); toast(readError(e)); }
  // 不再往哪一天跳了（地址去掉了 at、换了筛选）：还在路上的跳转到了也不滚。
  function cancelJump() { v.target = ''; v.jump = null; }

  // ---------- 滚动：活页条跟着选中、接近底部接着读 ----------
  // 这里在读阶段（量滚动位置），活页条的选中和接着读（会改 DOM）挪到写阶段。
  function onScroll() {
    const top = viewport.scrollTop - offsetOf(), day = dayAt(v.boxes, top + 1);
    if (day && day !== v.day) { v.day = day; scope.write(() => selectDay(day), 'days:select'); }
    if (v.next && !v.failed && top + viewport.clientHeight * 2.5 > v.height) scope.write(() => more(), 'days:more');
  }
  scope.on(viewport, 'scroll', () => scope.read(onScroll, 'days:scroll'), {passive: true});

  // ---------- 从头读 ----------
  async function load(route, {restore = false, force = false} = {}) {
    v.route = route;
    const query = dayQuery(route, state.nsfw), key = JSON.stringify(query);
    if (!force && key === v.key && (v.days.length || v.reading)) { if (route.at) goto(route.at); else cancelJump(); return; }
    // 后退回来：按这一条历史记录上次读到的天数读（一次最多 MAX_DAYS 天，不够就接着读），页面框架随后滚回原来的位置。先取出来再清空。
    const saved = restore ? depth.get(historyEntry()) : null, count = saved?.key === key ? Math.max(FIRST_DAYS, saved.count) : FIRST_DAYS;
    v.abort?.abort(); v.abort = new AbortController();
    cancelJump();
    Object.assign(v, {query, key, gen: v.gen + 1, days: [], next: '', day: '', reading: true, failed: false, stale: '', paging: null, target: route.at || ''});
    dropPeopleReads();
    for (const m of [v.heroes, v.open, v.opened, v.people, v.fresh, v.grow]) m.clear();
    list.dispose(); list = makeList();
    layout();
    const tk = ticket(), days = [];
    let next = '';
    do {
      let r;
      try { r = await readDays(query, {before: next, days: Math.min(MAX_DAYS, count - days.length), signal: signal()}); }
      catch (e) { if (valid(tk)) { v.reading = false; failed(e); } return; }
      if (!valid(tk)) return;
      days.push(...r.days.filter(g => !days.some(x => x.day === g.day))); next = r.next;
    } while (next && days.length < count);
    Object.assign(v, {days, next, reading: false});
    scope.read(measure, 'days:width'); layout();
    await settled();
    if (!valid(tk)) return;
    scope.read(onScroll, 'days:scroll');
    if (v.target) goto(v.target);
    drain();
  }
  // 读失败后的「重试」重做失败的那一步：同步、改动之后的重读再读一次；一天都没读到就从头读；在往某一天跳就接着跳；否则接着往下读。
  scope.on(refs.retry, 'click', () => {
    const stale = v.stale;
    v.failed = false; v.stale = ''; layout();
    if (stale) { v.retries = 0; refresh(stale); } else if (!v.days.length) load(v.route, {force: true}); else if (v.target) goto(v.target); else more();
  });

  // ---------- 读更早的几天：同一个游标只读一次 ----------
  function more(days = MORE_DAYS) {
    if (v.paging || !v.next || v.reading || v.failed) return v.paging;
    const tk = ticket(), before = v.next;
    const p = (async () => {
      let r;
      try { r = await readDays(v.query, {before, days, signal: signal()}); } catch (e) { if (valid(tk) && v.next === before) failed(e); return false; }
      if (!valid(tk) || v.next !== before) return false;
      const known = new Set(v.days.map(g => g.day)), added = r.days.filter(g => !known.has(g.day));
      added.forEach((g, i) => v.fresh.set(g.day, i * 90));
      Object.assign(v, {days: [...v.days, ...added], next: r.next});
      return true;
    })();
    v.paging = p;
    p.finally(async () => {
      if (v.paging !== p) return;
      v.paging = null;
      if (!scope.alive) return;
      layout();
      await settled();
      if (valid(tk)) scope.read(onScroll, 'days:scroll');
    });
    layout();
    return p;
  }
  // 滚到某一天（或比它早的、最近的一天）：还没读到就一次多读几天，读到那里为止。只认最后一次选的那一天。
  async function goto(day) {
    const jump = {};
    v.target = day; v.jump = jump;
    const mine = () => v.jump === jump && scope.alive;
    while (mine() && !v.reading && v.next && !v.days.some(g => g.day <= day)) {
      await (v.paging || more(MAX_DAYS));
      if (v.failed) return;   // 「重试」接着往这一天读（v.target 留着）
    }
    if (!mine() || v.reading) return;
    await (v.paging || Promise.resolve());
    await settled();
    if (!mine()) return;
    const box = v.boxes.find(b => b.key <= day) || v.boxes.at(-1);
    // 真正写滚动位置之前再核对一次：这期间地址去掉了 at、换了目标，就不滚。
    if (box) scope.read(() => { const top = offsetOf() + box.y; scope.write(() => { if (!mine()) return; viewport.scrollTop = top; cancelJump(); }); });
  }

  // ---------- 刷新：排队，读完再补 ----------
  // kind：'newest'（同步进来新图：最新两天）、'around'（改动、撤销：视野附近几天）。
  function refresh(kind) { v.pending.add(kind); drain(); }
  async function drain() {
    if (v.refreshing || v.reading || !scope.alive) return;
    v.refreshing = true;
    try {
      while (v.pending.size && !v.reading && scope.alive) {
        const kind = v.pending.has('around') ? 'around' : 'newest';
        v.pending.delete(kind);
        if (!v.days.length && !v.next) { v.pending.clear(); load(v.route, {force: true}); return; }   // 空图库第一次同步进图：从头读
        const ok = await reread(kind);
        if (ok) { v.retries = 0; if (v.stale) { v.stale = ''; layout(); } }   // 后来的一次重读成功了：「重试」收起
        else if (kind === 'newest' && v.retries < RETRIES) { v.retries++; scope.timeout(() => refresh('newest'), RETRY_MS); }
        else { v.stale = kind; layout(); }   // 自动重试也用完了（或改动之后的重读失败）：画面留着，给「重试」
      }
    } finally { v.refreshing = false; }
  }
  // 重读一段并进来：成功（或这次读取已经作废）返回 true。
  // 'newest' 从最新读起；一次进来好几天（重读到的还没接上原来最新的那天）时沿游标接着读，补到接上为止，再算到底和总数
  // （#757 复核第 5 条）。'around' 从视野里那天的前一天起往下读一段；在最上面时和 'newest' 一样从最新读起。
  async function reread(kind) {
    const tk = ticket(), top = v.days[0];
    const i = kind === 'around' ? Math.max(0, v.days.findIndex(g => g.day === v.day) - 1) : 0;
    const newest = i > 0 ? dayAfter(v.days[i].day) : '', fresh = [];
    let before = newest, next = '';
    for (;;) {
      let r;
      try { r = await readDays(v.query, {before, days: kind === 'around' ? 6 : 2, signal: signal()}); } catch {
        return !valid(tk);
      }
      if (!valid(tk)) return true;
      fresh.push(...r.days.filter(g => !fresh.some(x => x.day === g.day))); next = r.next;
      if (kind !== 'newest' || !next || !top || fresh.at(-1).day <= top.day) break;
      before = next;
    }
    const was = new Map(v.days.map(g => [g.day, g])), merged = mergeDays(v.days, fresh, {newest, next});
    v.days = merged.days;
    if (merged.end) v.next = next;
    regrow(kind, top, was, merged.changed);
    // 重读过的这几天，展开的角色名单作废（每天一次）：开着的重读，收着的下次展开时再读。
    for (const day of new Set([...fresh.map(g => g.day), ...merged.changed])) forgetPeople(day);
    layout();
    return true;
  }
  // 同步时「今天」长行（core/days.js 的 growCells）：已经长了行的天按「新来的、被移走的、只改了元数据」分别处理，被移走了
  // 或这一天没了就改按标准的马赛克排；同步进来新图时，最上面那一天（今天）开始长。改动、撤销之后变了的天同样处理。
  function regrow(kind, top, was, changed) {
    const s = sizes(), opt = {width: v.width, rowHeight: s.rowHeight, gap: s.gap};
    if (!v.width) { v.grow.clear(); return; }
    for (const [day, state] of [...v.grow]) {
      if (!changed.has(day)) continue;
      const g = v.days.find(x => x.day === day), next = g && growCells(state, was.get(day), g, opt);
      if (next) v.grow.set(day, next); else v.grow.delete(day);
    }
    if (kind === 'newest' && top && changed.has(top.day) && v.days[0]?.day === top.day && !v.grow.has(top.day)) {
      const next = growCells(dayCells(top, opt, v.heroes.get(top.day)), top, v.days[0], opt);
      if (next) v.grow.set(top.day, next);
    }
  }
  // 这一天的展开名单作废，还在路上的那次读取中止（#757 复核第 8 条，不把整天分页读完）；开着的重读。
  function forgetPeople(day) {
    v.peopleRead.get(day)?.abort(); v.peopleRead.delete(day);
    v.people.delete(day);
    if (v.open.has(day)) loadPeople(day);
  }
  function dropPeopleReads() { for (const c of v.peopleRead.values()) c.abort(); v.peopleRead.clear(); }

  // ---------- 展开或收起一天的全部角色 ----------
  // 展开时先用已有的前八位撑着，读到全部再换。
  function toggle(day) {
    if (v.open.has(day)) { v.open.delete(day); v.opened.delete(day); layout(); return; }
    v.open.add(day); layout();
    if (!v.people.has(day)) loadPeople(day);
  }
  // 每天的名单读取各有自己的 AbortController：同一天同一时间只读一次，作废时中止，只收最后一次的回复。
  async function loadPeople(day) {
    if (v.peopleRead.has(day)) return;
    const tk = ticket(), read = new AbortController(), mine = () => valid(tk) && v.peopleRead.get(day) === read;
    v.peopleRead.set(day, read);
    try {
      const people = await readDayPeople(v.query, day, {signal: read.signal});
      if (mine() && v.open.has(day)) { v.people.set(day, people); layout(); }
    } catch (e) { if (mine()) toast(readError(e)); } finally { if (v.peopleRead.get(day) === read) v.peopleRead.delete(day); }
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
    if (change.arrived) refresh('newest');
  }));

  return {
    scope, load, goto, refresh, layout, ask,
    // 视图选项变了（缩略图大小改了行高）：长了行的天按新行高、标准的马赛克重排。
    restyle() { v.grow.clear(); layout(); },
    mark: uuid => { for (const day of v.widgets) day.mark(uuid); },
    // 把这张图所在的那一天滚进视野。
    reveal(uuid) {
      const has = g => g.items.some(i => i.uuid === uuid) || v.grow.get(g.day)?.cells.some(c => c.item.uuid === uuid);
      const g = v.days.find(has), box = g && v.boxes.find(b => b.key === g.day);
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
  wirePeek($('homePage'), () => view?.ask.batch?.items.map(x => x.uuid) || []);
  onPick('library', item => {
    if (!onPage(state.route, 'days') || !view?.scope.alive || !item?.first) return false;
    view.goto(item.first);
    return true;
  });
}

implement({days: {
  // 刚进这一页（或作用域已经释放）才建；同一页又画一次（筛选、搜索、at 变了）只按新地址更新。认一认不随筛选变，只在进页面时读。
  render: ({samePage, restore, scope}) => {
    if (!samePage || !view?.scope.alive) { view = daysView(scope); view.ask.next(); }
    $('selectionBar').hidden = true; $('resultCount').textContent = ''; $('hint').textContent = t('点图片看详情 · 点角色名只看 TA');
    return view.load(state.route, {restore});
  },
  reveal: uuid => view?.reveal(uuid),
  // 详情卡片开着的那张（ui/detail.js 打开、关掉详情卡片时调）。
  focus: uuid => view?.mark(uuid),
  head: () => ({title: t('图库'), sub: state.recentCount ? t('最近一次同步新加入 {n} 张', {n: number(state.recentCount)}) : ''}),
  help: () => [t('按加入日期分组。')],
  refresh: why => {
    if (!view?.scope.alive) return;
    if (why === 'view') view.restyle();
    else if (why === 'ask') view.ask.reset();
    else if (why === 'sync') view.refresh('newest');
    else if (why === 'nsfw') { view.load(state.route); view.ask.reset(); }
    else view.refresh('around');
  },
}});
