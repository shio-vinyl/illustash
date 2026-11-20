// 图库的日历视图（#708，0.4 第 C2 步）：一个月一页。数据在 core/calendar.js（按月读接口、汇总当月的角色），这里只画。
// 文件分两段：日历部件 calendarView（下面的用法），和文件末尾按页面样板登记的日历页（docs/WEB_V2.md「页面样板」）。
//
//   const view = calendarView({scope, dayHref: day => '#/library?day=' + day, onMonth: month => …, empty: (box, data) => …});
//   box.append(view.el);
//   view.update({month: '2026-11', data: null});      // 读的时候：格子按月份先排好，标题写月份，翻页按钮先禁用
//   view.update({month: '2026-11', data: shaped});    // 数据到了（core/calendar.js 的 readMonth）
//
// - 月份标题：月、年、张数、几天有新图；背后是这个月的环境光（当月第一个有颜色的日子，ui/tint.js），旁边一排这个月出现的
//   角色头像（点头像去角色页）。前后翻月：‹ › 去筛选后最近有图的月份（接口的 prev / next，空的月份跳过），中间回到本月。
// - 每天一格：按张数拼 1、2 或 4 张图（core/layout.js 的 cellTiles），底色是当天的颜色；没有图的格子空着；今天有标记。
//   有图的格子是链接，点进去由 dayHref 决定去哪。前后月补齐的格子只写日期。
// - 只看不选：格子没有选择状态。键盘：Tab 走有图的格子，方向键在格子之间按日期挪（跳过没有图的），Enter 照链接打开。
// - 格子按位置复用（ui/keyed.js，键是第几格）：翻月时节点不重建，只换内容。图按格子的实际宽度取缩略图档位。
// - 没有图的月份：格子收起来，空状态交给 empty(box, data) 画（页面用 ui/empty.js）。
import {charHash, go, hashFor} from '../core/route.js';
import {I18N, t} from '../core/i18n.js';
import {composing} from '../core/keys.js';
import {number, thumbSize} from '../core/format.js';
import {localDay, monthOf, parseMonth, readMonth} from '../core/calendar.js';
import {cellTiles, monthGrid} from '../core/layout.js';
import {implement, libraryFilters} from '../core/pages.js';
import {viewRoute} from '../core/rail.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {$, error} from './dom.js';
import {attr, cls, css, show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';
import {enter, thumbs} from './motion.js';
import {setTint} from './tint.js';
import {releaseImages} from './virtual.js';

const VIEW = tpl`<section class="cal">
  <header class="cal-head" data-ref="head">
    <hgroup class="cal-title"><h2 class="title-page" data-ref="title"></h2><p class="cal-sub" data-ref="sub"></p></hgroup>
    <div class="cal-people" data-ref="people"></div>
    <nav class="seg cal-nav" aria-label="翻月" data-t-attr="aria-label">
      <button type="button" data-ref="prev" title="上一个有图的月份" aria-label="上一个有图的月份" data-t-attr="title aria-label">
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M12 4.5 6.5 10l5.5 5.5"/></svg></button>
      <button type="button" data-ref="now" data-t>本月</button>
      <button type="button" data-ref="next" title="下一个有图的月份" aria-label="下一个有图的月份" data-t-attr="title aria-label">
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m8 4.5 5.5 5.5L8 15.5"/></svg></button>
    </nav>
  </header>
  <div class="cal-dow" data-ref="dow" aria-hidden="true"></div>
  <div class="cal-grid" data-ref="grid"></div>
  <div class="cal-empty" data-ref="empty" hidden></div>
</section>`;
const DAY = tpl`<a class="cal-day"><span class="cal-tiles" data-ref="tiles"><img alt="" decoding="async" loading="lazy" draggable="false"><img alt="" decoding="async"
  loading="lazy" draggable="false"><img alt="" decoding="async" loading="lazy" draggable="false"><img alt="" decoding="async" loading="lazy"
  draggable="false"></span><b class="cal-num" data-ref="num"></b><small class="cal-count" data-ref="count"></small></a>`;
const FACE = tpl`<a class="cal-face"><img class="avatar is-small" data-ref="img" alt="" decoding="async" loading="lazy" draggable="false"></a>`;
const DOW = tpl`<span></span>`;

// 格内图片之间的缝，按格子边长的百分比（cellTiles 用 100 × 100 的格子算，结果直接当百分比用）。
const TILE_GAP = 2.5;
// 月份标题旁最多几个头像。
const FACES = 8;

const imageUrl = (uuid, size) => `/image?id=${encodeURIComponent(uuid)}&size=${size}`;
const monthTitle = (year, month) => I18N.language === 'zh-CN' ? t('{m} 月', {m: month})
  : new Intl.DateTimeFormat(I18N.locale, {month: 'long'}).format(new Date(year, month - 1, 1));
const dayLabel = date => new Intl.DateTimeFormat(I18N.locale, {year: 'numeric', month: 'long', day: 'numeric'}).format(new Date(`${date}T12:00:00`));
// 一周七天的短名，从 weekStart 开始（1 = 周一）。2024-01-01 是周一。
const weekdays = weekStart => Array.from({length: 7}, (_, k) =>
  new Intl.DateTimeFormat(I18N.locale, {weekday: 'short'}).format(new Date(2024, 0, 1 + (k + weekStart + 6) % 7)));

// 一个日子的格子。props：{cell（monthGrid 的格子）, day（接口的那一天，或 null）, today, href, width（格宽 px）}。
function dayCell(s) {
  const {el, refs} = DAY(), imgs = [...refs.tiles.children];
  // 图读坏了就记下（样式把它藏起来，露出格子的底色）；换一张图时去掉记号。
  for (const img of imgs) s.on(img, 'error', () => attr(img, 'data-broken', true));
  return {el, update({cell, day, today, href, width}) {
    const n = cell.inMonth ? day?.count || 0 : 0, samples = n ? day.samples || [] : [];
    const tiles = cellTiles(Math.min(n, samples.length), {width: 100, height: 100, gap: TILE_GAP});
    text(refs.num, cell.day);
    text(refs.count, n ? number(n) : '');
    show(refs.count, n > 0);
    cls(el, 'is-out', !cell.inMonth); cls(el, 'has', tiles.length > 0);
    attr(el, 'aria-current', cell.inMonth && cell.date === today ? 'date' : null);
    attr(el, 'href', tiles.length ? href : null);
    attr(el, 'aria-label', tiles.length ? t('{date}，{n} 张', {date: dayLabel(cell.date), n: number(n)}) : null);
    setTint(el, tiles.length ? day.color : null);
    imgs.forEach((img, k) => {
      const box = tiles[k], item = box && samples[box.i];
      // 不用的图去掉地址：放掉解码后的位图（#683）。
      if (!item) { attr(img, 'src', null); show(img, false); return; }
      const src = imageUrl(item.uuid, thumbSize(width * box.w / 100));
      if (img.getAttribute('src') !== src) attr(img, 'data-broken', null);
      attr(img, 'src', src);
      show(img, true);
      css(img, 'left', `${box.x}%`); css(img, 'top', `${box.y}%`); css(img, 'width', `${box.w}%`); css(img, 'height', `${box.h}%`);
      setTint(img, item.color);
      thumbs.mark(img, item.uuid);
    });
  }, dispose() { releaseImages(el); s.dispose(); }};
}

// 月份标题旁的一个角色头像。
function face(s) {
  const {el, refs} = FACE();
  s.on(refs.img, 'error', () => attr(refs.img, 'data-broken', true));
  return {el, update(p) {
    attr(el, 'href', charHash(p.id));
    attr(el, 'title', p.name);
    attr(el, 'aria-label', p.name);
    const src = imageUrl(p.cover, 'thumb');
    if (refs.img.getAttribute('src') !== src) attr(refs.img, 'data-broken', null);
    attr(refs.img, 'src', src);
    setTint(refs.img, p.color);
  }, dispose() { releaseImages(el); s.dispose(); }};
}

/**
 * 日历视图部件。
 * @param {{scope: object, dayHref: (day: string) => string, onMonth: (month: string) => void,
 *          empty?: (box: HTMLElement, data: object) => void, weekStart?: number}} options
 *   scope 是页面作用域（ui/scope.js）；dayHref 给有图的格子的链接；onMonth 是点了翻月按钮（页面改地址、重读）；
 *   empty 在这个月没有图时画空状态；weekStart 是一周从哪天开始（默认周一）。
 * @returns {{el: HTMLElement, update: (props: {month: string, data: object|null}) => void, dispose: () => void}}
 */
export function calendarView({scope: parent, dayHref, onMonth, empty, weekStart = 1}) {
  const s = parent.child('calendar'), {el, refs} = VIEW();
  let props = {month: '', data: null}, width = 0, shown = '';
  // keyed 只给复用的部件调 update，新建的在这里先画一次。
  const made = make => item => { const comp = make(); comp.update(item); return comp; };
  const days = keyed(refs.grid, {key: x => x.cell.row * 7 + x.cell.col, create: made(() => dayCell(s.child('day'))), scope: s});
  const faces = keyed(refs.people, {key: p => p.id, create: made(() => face(s.child('face'))), scope: s});
  for (const name of weekdays(weekStart)) { const d = DOW(); text(d.el, name); refs.dow.append(d.el); }

  const go = month => { if (month && month !== props.month) onMonth(month); };
  s.on(refs.prev, 'click', () => go(props.data?.prev));
  s.on(refs.next, 'click', () => go(props.data?.next));
  s.on(refs.now, 'click', () => go(monthOf(localDay())));
  s.on(refs.grid, 'keydown', e => {
    if (composing(e)) return;
    const step = {ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7}[e.key];
    if (!step || e.metaKey || e.ctrlKey || e.altKey) return;
    const cells = [...refs.grid.children], from = cells.findIndex(c => c.contains(e.target));
    if (from < 0) return;
    // 按日期挪：先到 ±1 / ±7 那一格，那一格没有图就沿着同一个方向找下一个有图的格子。
    for (let k = from + step; k >= 0 && k < cells.length; k += Math.sign(step)) {
      if (cells[k].hasAttribute('href')) { e.preventDefault(); cells[k].focus(); return; }
    }
  });
  // 格宽决定缩略图的档位：量一次，变了再画。
  const measure = () => {
    const w = refs.grid.clientWidth / 7;
    if (Math.abs(w - width) >= 1) { width = w; s.write(draw, 'calendar:draw'); }
  };
  if (typeof ResizeObserver === 'function') s.observe(new ResizeObserver(() => s.read(measure, 'calendar:measure'))).observe(refs.grid);

  function draw() {
    const {month, data} = props, p = parseMonth(month);
    if (!p) return;
    const today = localDay(), ready = data?.month === month ? data : null;
    text(refs.title, monthTitle(p.year, p.month));
    const facts = [String(p.year)];
    if (ready?.total) facts.push(t('{n} 张', {n: number(ready.total)}), t('{n} 天有新图', {n: number(ready.active)}));
    text(refs.sub, facts.join(' · '));
    setTint(refs.head, ready?.color);
    faces.set((ready?.people || []).slice(0, FACES));
    attr(refs.prev, 'disabled', !ready?.prev);
    attr(refs.next, 'disabled', !ready?.next);
    attr(refs.now, 'aria-pressed', month === monthOf(today) ? 'true' : 'false');
    days.set(monthGrid(p.year, p.month, {weekStart}).cells.map(cell => {
      const day = cell.inMonth && ready?.days.get(cell.date) || null;
      return {cell, day, today, href: day ? dayHref(cell.date) : null, width: width || 120};
    }));
    // 这个月没有图：格子收起来，只写空状态。
    const none = !!ready && !ready.total;
    show(refs.empty, none); show(refs.grid, !none); show(refs.dow, !none);
    if (none) empty?.(refs.empty, ready);
    // 换了月份：格子淡入（数据到的那一次，不是先排空格子的那一次）。
    if (ready && shown && shown !== month) enter(refs.grid, {from: 'fade'});
    if (ready) shown = month;
  }

  return {el, update(next) { props = {...props, ...next}; draw(); }, dispose() { s.dispose(); }};
}

// ---------- 日历页（页面样板） ----------
// 看哪个月由地址的 at（YYYY-MM-DD，docs/WEB_V2.md「看哪一天：at」）定，没有时是本月；翻月是换 at，后退能回到上一个月。
// 点一天去按天视图的那一天（#/days?at=…），带着现在的筛选，和活页条的时间轴一样。筛选条和搜索照常生效（#703 第 2 问）。
const monthAt = route => parseMonth(monthOf(route.at)) ? monthOf(route.at) : monthOf(localDay());
const atHref = (route, view, day) => hashFor({...viewRoute(route, view), at: day});
// 这个月没有图：有别的月份就给去处，整个图库（按现在的筛选）都没有图就照实说。
function drawMonthEmpty(box, data) {
  const month = m => ({href: atHref(state.route, 'calendar', `${m}-01`)});
  const next = [data.prev && {label: t('上一个有图的月份'), ...month(data.prev)}, data.next && {label: t('下一个有图的月份'), ...month(data.next)}].filter(Boolean);
  const filtered = Object.keys(libraryFilters(state.route)).length > 0 || state.nsfw === 'only';
  drawEmpty(box, {icon: 'calendar', next, fact: next.length ? t('这个月没有图片。') : filtered ? t('没有符合条件的图片。') : t('图库里还没有图片。')});
}
// 这一页的部件：进页面时在页面作用域上建一次，之后只 load。
function calendarPage(scope) {
  const view = calendarView({scope, empty: drawMonthEmpty,
    dayHref: day => atHref(state.route, 'days', day),
    onMonth: month => go({...viewRoute(state.route, 'calendar'), at: `${month}-01`})});
  $('calendarPage').replaceChildren(view.el);
  // 上一次读到的 {key, data}：只是刷新（同步、写入之后）时先留着它，读完再换，格子不先闪成空的。
  let last = null;
  return {scope, el: view.el, async load(route) {
    const month = monthAt(route), params = {...libraryFilters(route), ...(state.nsfw ? {nsfw: state.nsfw} : {})}, key = JSON.stringify([month, params]);
    view.update({month, data: last?.key === key ? last.data : null});
    let data;
    try { data = await readMonth(month, params, {signal: scope.signal}); } catch (e) { if (scope.alive && state.route === route) error(readError(e)); return; }
    if (!scope.alive || state.route !== route) return;
    last = {key, data};
    view.update({month, data});
  }};
}
let page = null;
implement({calendar: {
  // 刚进这一页（或作用域已经释放）才建，内容淡入；同一页又画一次（翻月、筛选变了）只按新地址重读、更新格子。
  render: ({samePage, scope}) => {
    if (!samePage || !page?.scope.alive) { page = calendarPage(scope); enter(page.el, {from: 'fade'}); }
    return page.load(state.route);
  },
  // 同步、写入、撤销、NSFW 设置变了：重读这个月。
  refresh: () => page?.scope.alive ? page.load(state.route) : undefined,
  head: () => ({title: t('图库')}),
  help: () => [t('每天一格，格子里是那天加入的图。点一天去按天看那一天。')],
}});
