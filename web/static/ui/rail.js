// 活页条（#706）：一级入口（图库、资料库、画廊、整理）、二级层、底部的同步圆环和设置。全应用一直在的部件，挂在 app 作用域上，
// 不随换页释放。数据（入口、地址落在哪一项、时间轴、钉住）在 core/rail.js；这里只画和动。
//
// 给各页的接口：
//   provide(entry, fn)     换这个入口二级层的内容：fn() 返回项的列表（core/rail.js 开头写了项的字段），refresh 时重新调。
//                          B 给四个入口都登记了默认的，各页步骤要接手时再登记一个（例如 E 的画廊封面）
//   refresh(entry?)        数据变了，重画一个入口（不给就是全部）
//   select(key)            在当前入口里选中某项（'' 不选），指示条滑过去
//   selectDay(day)         C1：按天视图滚到某一天时调；那一天在时间轴里就选中它，否则选中它所在的月或年
//   onPick(entry, fn)      点了二级项：fn(item, event) 返回 true 表示自己处理了（图库：点月份滚到那个月的第一天），否则按 href 走
//   count(key, n)          H：改一个队列的数字（数字滚动）
//   mode(name, items, hint) E：临时换成另一组项（拖放时换成画廊封面，hint 是上面的一句提示）；mode(null) 换回
//   railKey(e)             键盘（ui/grid.js 的键盘处理里调）：⌘1–4（Windows 上 Ctrl）切一级，⌥↑↓ 在二级里移动，
//                          ⌥⇧↑↓ 挪动钉住的项；处理了返回 true
//   railRoute(route)       换页时（ui/shell.js applyRoute）：点亮入口和项，收起窄窗口的抽屉，需要时重读时间轴
//
// 动效（都经 ui/motion.js，「减少动态效果」时直接到终态）：切一级时标签带回弹滑过去（CSS 过渡，--dur-3、--ease-spring），
// 旧的二级项依次退场、新的依次进场；悬停二级项时它探出来，相邻的项微微探出，旁边浮出预览（画在二级层外面，不被滚动区裁掉）；
// 指示条跟着选中项滑；时间轴、钉住的项增减时其余平滑让位。
import {ENTRIES, entryHref, galleryItems, itemForDay, loadPins, loadTimeline, movePin, noteLibraryView, pinItems, pins, queues, railAt, tidyWaiting,
  timeline} from '../core/rail.js';
import {coversFor} from '../core/galleries.js';
import {number} from '../core/format.js';
import {I18N, t} from '../core/i18n.js';
import {composing} from '../core/keys.js';
import {onJob} from '../core/jobs.js';
import {state} from '../core/state.js';
import {attr, cls, css, make, show, text, tpl} from './el.js';
import {keyed} from './keyed.js';
import {app} from './scope.js';
import {animate, countTo, enter, exit, reduced} from './motion.js';
import {createLayer} from './overlay.js';
import {$} from './dom.js';

const ITEM = tpl`<a class="rail-item" role="listitem" draggable="false"><span class="rail-cover" data-ref="cover"><span class="rail-stack" data-ref="stack"><img data-ref="img2" alt=""
  draggable="false"></span><img data-ref="img" alt="" draggable="false" loading="lazy" decoding="async"><span class="rail-icon" data-ref="icon"></span></span><b class="rail-mark"
  data-ref="mark"></b><em class="rail-count" data-ref="count"></em><u class="rail-heart" data-ref="heart" hidden><svg viewBox="0 0 20 20" aria-hidden="true"><path
  d="M10 16.4S3.4 12.5 3.4 7.9A3.3 3.3 0 0 1 10 6.3a3.3 3.3 0 0 1 6.6 1.6c0 4.6-6.6 8.5-6.6 8.5Z"/></svg></u></a>`;
// 整理各队列的图标（和原来侧栏的一样）。
const ICONS = {
  unmatched: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="7.2"/><path d="M8 7.8a2.2 2.2 0 1 1 3 2c-.6.3-1 .8-1 1.4v.4M10 14.2v.1"/></svg>`,
  duplicates: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3.5" y="6" width="9.5" height="11" rx="2"/><path d="M7 3h7.5A2.5 2.5 0 0 1 17 5.5V13"/></svg>`,
  excluded: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="7.2"/><path d="M5 15 15 5"/></svg>`,
  audit: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="4" y="3.5" width="12" height="14" rx="2"/><path d="M7.5 9.8 9.3 11.6 12.8 8"/></svg>`,
  identity: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 7h11l-3-3M16 13H5l3 3"/></svg>`,
};
const PEEK = tpl`<div class="rail-peek-in"><b data-ref="title"></b><small data-ref="meta"></small><span class="rail-peek-pics" data-ref="pics"></span></div>`;

const s = app.child('rail');
const providers = new Map(), pickers = new Map(), lists = new Map();
let current = '', selected = '', temp = null, pitch = 0, wired = false;
const monthName = new Intl.DateTimeFormat(I18N.locale, {month: 'short'});
const thumb = id => id ? `/image?id=${encodeURIComponent(id)}&size=thumb` : '';

// ---------- 一项 ----------
function mark(item) {
  if (item.kind === 'day') return String(Number(item.day.slice(8)));
  if (item.kind === 'month') return monthName.format(new Date(Number(item.key.slice(2, 6)), Number(item.key.slice(7)) - 1, 1));
  if (item.kind === 'year') return item.key.slice(2);
  if (item.kind === 'work') return [...(item.label || '')][0] || '';
  return '';
}
function itemWidget(item) {
  const {el, refs} = ITEM();
  if (ICONS[item.key] && item.kind === 'queue') refs.icon.append(ICONS[item.key]().el);
  el.dataset.key = item.key; el.dataset.kind = item.kind;
  const update = x => {
    attr(el, 'href', x.href || '#'); attr(el, 'aria-label', x.meta ? `${x.label}，${x.meta}` : x.label);
    attr(el, 'aria-current', x.key === selected ? 'page' : null);
    cls(el, 'is-round', !!x.round);
    attr(refs.img, 'src', thumb(x.cover) || null); show(refs.img, !!x.cover);
    attr(refs.img2, 'src', thumb(x.cover2) || null); show(refs.stack, x.kind === 'month' || x.kind === 'year');
    text(refs.mark, mark(x)); show(refs.mark, !!mark(x));
    show(refs.heart, !!x.heart);
    if (!counting.has(refs.count)) text(refs.count, x.count ? String(x.count) : '');
    show(refs.count, !!x.count);
    el.railItem = x;
  };
  update(item);
  return {el, update, dispose() {}};
}
const counting = new WeakSet();

// ---------- 每个入口一组 ----------
// 每个入口一个容器、一个按键复用的列表：切换入口时旧的一组退场、新的一组进场，回到原来的入口时节点还在。
function listFor(entry) {
  if (lists.has(entry)) return lists.get(entry);
  const box = document.createElement('div');
  box.className = 'rail-group'; box.dataset.entry = entry; box.hidden = true;
  $('railItems').append(box);
  const list = {box, items: [], leaving: 0};
  list.keyed = keyed(box, {key: x => x.key, create: itemWidget, scope: s,
    enter: el => !box.hidden && enter(el, {from: [{opacity: 0, transform: 'translateX(-20px) scale(.5)'}, {opacity: 1, transform: 'none'}], dur: 3, ease: 'spring'}),
    // 让位：每一项一样高，挪了几格就从几格之外滑回来，不用量。有项正在退场时（它还占着位置）不滑，等它摘掉以后一起滑。
    move: (el, x, from, to) => { if (!box.hidden && !list.leaving && pitch) animate(el, [{transform: `translateY(${(from - to) * pitch}px)`}, {transform: 'none'}], {dur: 3, ease: 'spring'}); },
    exit: async el => {
      if (box.hidden) return;
      list.leaving++;
      await exit(el, {to: [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'translateX(-14px) scale(.6)'}], remove: false});
      list.leaving--;
      const after = [...box.children].filter(x => x !== el && !x.inert), before = after.map(x => x.offsetTop);
      el.remove();
      after.forEach((x, i) => { const d = before[i] - x.offsetTop; if (d) animate(x, [{transform: `translateY(${d}px)`}, {transform: 'none'}], {dur: 3, ease: 'spring'}); });
    }});
  lists.set(entry, list);
  return list;
}
function fill(entry) {
  const fn = providers.get(entry); if (!fn) return;
  const list = listFor(entry), items = fn() || [];
  list.items = items; list.keyed.set(items);
  if (entry === current && !temp) { s.read(measure, 'rail:measure'); }
}
export function provide(entry, fn) { providers.set(entry, fn); fill(entry); }
export function refresh(entry) { for (const e of entry ? [entry] : providers.keys()) fill(e); if (!entry || entry === 'tidy') dot(); }
export function onPick(entry, fn) { pickers.set(entry, fn); }

// ---------- 选中与指示条 ----------
function measure() {
  const box = visibleBox(), el = box?.querySelector(`.rail-item[aria-current="page"]`);
  const first = box?.children[0], second = box?.children[1];
  if (first && second) pitch = second.offsetTop - first.offsetTop;
  const y = el ? el.offsetTop + el.offsetHeight / 2 - 13 : null;
  s.write(() => { css($('railInd'), 'transform', y === null ? null : `translateY(${y}px)`); cls($('railInd'), 'is-on', y !== null); }, 'rail:ind');
}
const visibleBox = () => temp?.list.box || lists.get(current)?.box;
export function select(key) {
  selected = key || '';
  for (const list of lists.values()) for (const el of list.box.children) attr(el, 'aria-current', el.dataset.key === selected ? 'page' : null);
  s.read(measure, 'rail:measure');
  const el = visibleBox()?.querySelector('.rail-item[aria-current="page"]');
  if (el) s.read(() => { const box = $('railLayer'), top = el.offsetTop, h = el.offsetHeight;
    if (top < box.scrollTop || top + h > box.scrollTop + box.clientHeight) s.write(() => { box.scrollTop = Math.max(0, top - box.clientHeight / 2); }, 'rail:scroll'); }, 'rail:reveal');
}
export function selectDay(day) { if (current === 'library') select(itemForDay(timeline.items, day)); }
export function count(key, n) {
  const el = visibleBox()?.querySelector(`.rail-item[data-key="${key}"] .rail-count`) || lists.get('tidy')?.box.querySelector(`.rail-item[data-key="${key}"] .rail-count`);
  if (!el) return;
  show(el, !!n); counting.add(el);
  countTo(el, n).finally(() => counting.delete(el));
  animate(el, [{transform: 'scale(1)'}, {transform: 'scale(1.35)'}, {transform: 'scale(1)'}], {dur: 2, ease: 'spring'});
}
const dot = () => { for (const b of document.querySelectorAll('#railEntries .rail-dot')) b.hidden = b.parentElement.dataset.entry !== 'tidy' || !tidyWaiting(); };

// ---------- 切换入口 ----------
async function swap(fromBox, toBox) {
  if (fromBox === toBox) { if (toBox) toBox.hidden = false; return; }
  const quick = reduced() || !fromBox || fromBox.hidden;
  cls($('railInd'), 'is-on', false);
  if (!quick) {
    const old = [...fromBox.children].filter(x => !x.inert);
    await Promise.all(old.map((el, i) => animate(el, [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'translateX(-16px) scale(.92)'}],
      {dur: 1, delay: i * 16, fill: 'forwards'})));
  }
  if (fromBox) { fromBox.hidden = true; for (const el of fromBox.children) el.getAnimations?.().forEach(a => a.cancel()); }
  if (!toBox) return;
  toBox.hidden = false;
  if (!quick) [...toBox.children].forEach((el, i) => enter(el, {from: [{opacity: 0, transform: 'translateX(-22px) scale(.86)'}, {opacity: 1, transform: 'none'}],
    dur: 3, delay: i * 38, ease: 'spring'}));
  s.read(measure, 'rail:measure');
}
function showEntry(entry) {
  if (entry === current) return;
  const was = current; current = entry;
  for (const a of document.querySelectorAll('#railEntries .rail-entry')) attr(a, 'aria-current', a.dataset.entry === entry ? 'page' : null);
  const index = ENTRIES.findIndex(x => x.id === entry);
  cls($('railTab'), 'is-on', index >= 0);
  if (index >= 0) css($('railTab'), '--at', String(index));
  cls($('rail'), 'is-empty', !providers.has(entry));
  if (temp) return;
  swap(lists.get(was)?.box, providers.has(entry) ? listFor(entry).box : null).catch(() => {});
}
// 二级层临时换成另一组（E 的拖放）。
export function mode(name, items = [], hint = '') {
  if (!name) {
    if (!temp) return;
    const box = temp.list.box; temp.list.keyed.dispose(); temp = null;
    text($('railHint'), ''); cls($('railHint'), 'is-on', false);
    swap(box, listFor(current).box).finally(() => box.remove());
    return;
  }
  if (!temp) {
    const box = document.createElement('div'); box.className = 'rail-group'; box.dataset.entry = 'mode:' + name; box.hidden = true; $('railItems').append(box);
    temp = {name, list: {box, leaving: 0, keyed: keyed(box, {key: x => x.key, create: itemWidget, scope: s})}};
    temp.list.keyed.set(items);
    swap(lists.get(current)?.box, box).catch(() => {});
  } else temp.list.keyed.set(items);
  text($('railHint'), hint); cls($('railHint'), 'is-on', !!hint);
}

// ---------- 地址变了 ----------
export function railRoute(route) {
  noteLibraryView(route);
  if (!wired) return;   // 活页条还没接上（只有测试会这样）
  const at = railAt(route, state.char);
  for (const a of document.querySelectorAll('#railEntries .rail-entry')) attr(a, 'href', entryHref(a.dataset.entry));
  attr($('settingsLink'), 'aria-current', at.entry === 'settings' ? 'page' : null);
  showEntry(at.entry);
  select(at.item);
  setDrawer(false, 'silent');
  if (at.entry === 'library') loadTimeline(route).catch(() => {});
}

// ---------- 悬停：抽卡和预览 ----------
let hovered = null;
function hover(el) {
  if (hovered === el) return;
  if (hovered) for (const x of [hovered, hovered.previousElementSibling, hovered.nextElementSibling]) x?.classList.remove('is-hover', 'is-near');
  hovered = el;
  const peek = $('railPeek');
  if (!el || el.inert) { peek.hidden = true; return; }
  el.classList.add('is-hover');
  el.previousElementSibling?.classList.add('is-near'); el.nextElementSibling?.classList.add('is-near');
  const x = el.railItem, {el: body, refs} = PEEK();
  text(refs.title, x.label); text(refs.meta, x.meta || ''); show(refs.meta, !!x.meta);
  for (const id of (x.peek || []).slice(0, 3)) { const img = make('img'); img.alt = ''; img.src = thumb(id); refs.pics.append(img); }
  show(refs.pics, !!x.peek?.length);
  peek.replaceChildren(body);
  s.read(() => {
    const r = el.getBoundingClientRect();
    s.write(() => { css(peek, 'transform', `translate(${Math.round(r.right + 14)}px,${Math.round(r.top + r.height / 2)}px) translateY(-50%)`); peek.hidden = false; }, 'rail:peek');
  }, 'rail:peek-read');
}

// ---------- 钉住的项拖动排序（#684） ----------
let drag = null;
function startDrag(e, el) {
  if (e.button !== 0 || el.dataset.kind === 'gallery' || current !== 'catalog' || temp) return;
  drag = {el, key: el.dataset.key, y: e.clientY, moved: false, id: e.pointerId};
}
function moveDrag(e) {
  if (!drag || e.pointerId !== drag.id) return;
  const dy = e.clientY - drag.y;
  if (!drag.moved && Math.abs(dy) < 5) return;
  if (!drag.moved) { drag.moved = true; drag.el.setPointerCapture?.(e.pointerId); cls(drag.el, 'is-dragging', true); hover(null); }
  css(drag.el, 'transform', `translateY(${dy}px)`);
}
function endDrag(e) {
  if (!drag || e.pointerId !== drag.id) return;
  const {el, key, moved, y} = drag; drag = null;
  if (!moved) return;
  cls(el, 'is-dragging', false); css(el, 'transform', null);
  const steps = pitch ? Math.round((e.clientY - y) / pitch) : 0, from = pins.keys.indexOf(key);
  el.addEventListener('click', ev => ev.preventDefault(), {once: true, capture: true});
  if (steps && from >= 0) movePin(key, from + steps);
}

// ---------- 窄窗口：二级层收进抽屉 ----------
const drawer = createLayer({kind: 'popover', id: 'rail-drawer', el: () => $('railLayer'), anchor: () => $('sidebarToggle'), focusIn: 'first',
  onDismiss: reason => setDrawer(false, reason)});
function setDrawer(open, reason) {
  cls($('rail'), 'is-drawer', open); attr($('sidebarToggle'), 'aria-expanded', String(open));
  if (open) { drawer.open(); s.read(measure, 'rail:measure'); } else drawer.close(reason);
}

// ---------- 键盘 ----------
export function railKey(e) {
  if (composing(e)) return false;
  const mod = e.metaKey || e.ctrlKey;
  if (mod && !e.altKey && !e.shiftKey && /^[1-4]$/.test(e.key)) {
    e.preventDefault(); location.hash = entryHref(ENTRIES[Number(e.key) - 1].id); return true;
  }
  if (!e.altKey || mod || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return false;
  if (e.target.closest?.('input,textarea,select,[contenteditable]')) return false;
  const box = visibleBox(); if (!box) return false;
  e.preventDefault();
  const dir = e.key === 'ArrowDown' ? 1 : -1;
  if (e.shiftKey) {   // 挪动钉住的项：焦点或选中的那项
    const key = document.activeElement?.closest?.('.rail-item')?.dataset.key || selected, at = pins.keys.indexOf(key);
    if (current === 'catalog' && at >= 0) movePin(key, at + dir).then(() => box.querySelector(`.rail-item[data-key="${key}"]`)?.focus({preventScroll: true}));
    return true;
  }
  const items = [...box.children].filter(x => !x.inert), i = items.findIndex(x => x.dataset.key === selected);
  const next = items[i < 0 ? (dir > 0 ? 0 : items.length - 1) : Math.max(0, Math.min(items.length - 1, i + dir))];
  if (next && next.dataset.key !== selected) pick(next, e);
  return true;
}
// 点了（或用 ⌥↑↓ 走到）一项：这一页登记了 onPick 且处理了就到此为止，否则按链接走（点击由 <a> 自己走）。
function pick(el, e) {
  const fn = temp ? null : pickers.get(current);
  if (fn?.(el.railItem, e)) { e?.preventDefault?.(); select(el.dataset.key); return; }
  if (e?.type !== 'click') location.hash = el.getAttribute('href');
}

// ---------- 启动 ----------
export function wireRail() {
  wired = true;
  provide('tidy', () => queues());
  provide('galleries', () => galleryItems(galleryCovers));
  provide('catalog', () => pinItems());
  provide('library', () => timeline.items.map(x => ({...x, label: timelineLabel(x), meta: timelineMeta(x), href: '#/days'})));
  timeline.changed.on(() => refresh('library'));
  pins.changed.on(() => { refresh('catalog'); select(railAt(state.route, state.char).item); });
  loadPins().then(() => refresh('catalog'));
  onJob((job, change) => { if (job === 'sync' && change.arrived && current === 'library') loadTimeline(state.route).catch(() => {}); });
  const items = $('railItems');
  s.on(items, 'click', e => { const el = e.target.closest('.rail-item'); if (el && !el.inert) pick(el, e); });
  s.on(items, 'pointerover', e => hover(e.target.closest('.rail-item')));
  s.on(items, 'pointerleave', () => hover(null));
  s.on(items, 'focusin', e => hover(e.target.closest('.rail-item')));
  s.on(items, 'focusout', () => hover(null));
  s.on(items, 'pointerdown', e => { const el = e.target.closest('.rail-item'); if (el) startDrag(e, el); });
  s.on(items, 'pointermove', moveDrag);
  s.on(items, 'pointerup', endDrag);
  s.on(items, 'pointercancel', endDrag);
  s.on($('railLayer'), 'scroll', () => hover(null), {passive: true});
  s.on($('sidebarToggle'), 'click', () => setDrawer(!$('rail').classList.contains('is-drawer')));
  s.observe(new ResizeObserver(() => s.read(measure, 'rail:measure'))).observe($('railLayer'));
}
// 时间轴每一项悬停时写什么：哪天（或哪个月、哪一年）、几张。
const dayTitle = new Intl.DateTimeFormat(I18N.locale, {month: 'long', day: 'numeric', weekday: 'short'});
const monthTitle = new Intl.DateTimeFormat(I18N.locale, {year: 'numeric', month: 'long'});
const date = key => new Date(key + 'T00:00:00');
function timelineLabel(x) {
  if (x.kind === 'day') return dayTitle.format(date(x.day));
  return x.kind === 'year' ? t('{year} 年', {year: x.key.slice(2)}) : monthTitle.format(date(x.key.slice(2) + '-01'));
}
function timelineMeta(x) {
  const n = t('{n} 张', {n: number(x.count)});
  return x.kind === 'day' ? n : `${t('{n} 天', {n: number(x.days)})} · ${n}`;
}
// 切换 R18 之后：时间轴（在图库里时）和画廊封面按新的筛选重读。
export function railNsfw() {
  if (current === 'library') loadTimeline(state.route).catch(() => {});
  loadGalleryCovers();
}
// 画廊的封面按 R18 筛选批量读（core/galleries.js）；读到以后重画画廊这一组。
let galleryCovers = {};
export async function loadGalleryCovers() {
  const ids = (state.galleries || []).map(g => g.id);
  try { galleryCovers = await coversFor(ids); } catch { galleryCovers = {}; }
  refresh('galleries');
}
