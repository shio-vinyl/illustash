// 活页条（#706）：一级入口（图库、资料库、画廊、整理）、二级层、底部的同步圆环和设置。全应用一直在的部件，挂在 app 作用域上，
// 不随换页释放。数据（入口、地址落在哪一项、时间轴、钉住）在 core/rail.js；这里只画和动。
//
// 给各页的接口：
//   provide(entry, fn)     换这个入口二级层的内容：fn() 返回项的列表（core/rail.js 开头写了项的字段），refresh 时重新调。
//                          B 给四个入口都登记了默认的，各页步骤要接手时再登记一个（例如 E 的画廊封面）
//   refresh(entry?)        数据变了，重画一个入口（不给就是全部）
//   select(key)            在当前入口里选中某项（'' 不选），指示条滑过去
//   selectDay(day)         C1：按天视图滚到某一天时调；那一天在时间轴里就选中它，否则选中它所在的月或年
//   onPick(entry, fn)      点了二级项（或用 ⌥↑↓ 走到）：fn(item, event) 返回 true 表示自己处理了、不换地址（C1：已经在按天视图里时
//                          直接滚到 item.first 那一天），否则按项的链接走
//   count(key, n)          H：改一个队列的数字（数字滚动）
//   mode(name, items, hint) E：临时换成另一组项（拖放时换成画廊封面，hint 是上面的一句提示）；mode(null) 换回
//   railKey(e)             键盘（ui/grid.js 的键盘处理里调）：⌘1–4（Windows 上 Ctrl）切一级，⌥↑↓ 在二级里移动，
//                          ⌥⇧↑↓ 挪动钉住的项；处理了返回 true
//   railRoute(route)       换页时（ui/shell.js applyRoute）：点亮入口和项，收起窄窗口的抽屉，需要时重读时间轴
//
// 动效（都经 ui/motion.js，「减少动态效果」时直接到终态）：切一级时标签带回弹滑过去（CSS 过渡，--dur-3、--ease-spring），
// 旧的二级项依次退场、新的依次进场；悬停二级项时它探出来，相邻的项微微探出，旁边滑出预览卡（画在二级层外面，不被滚动区裁掉，
// 写法见下面「悬停」一节）；
// 指示条跟着选中项滑；时间轴、钉住的项增减时其余平滑让位。
import {ENTRIES, PEEK, entryHref, forgetTimeline, galleryItems, itemForDay, loadPins, loadTimeline, movePin, noteLibraryView, peekRow, pinItems, pins, queues,
  railAt, tidyWaiting, timeline, timelineHref} from '../core/rail.js';
import {coversFor} from '../core/galleries.js';
import {number} from '../core/format.js';
import {I18N, t} from '../core/i18n.js';
import {composing} from '../core/keys.js';
import {onJob} from '../core/jobs.js';
import {pageOf} from '../core/pages.js';
import {onPost} from '../core/session.js';
import {state} from '../core/state.js';
import {attr, cls, css, make, show, text, tpl} from './el.js';
import {keyed} from './keyed.js';
import {app} from './scope.js';
import {animate, cancelExit, countTo, enter, exit, reduced} from './motion.js';
import {setTint} from './tint.js';
import {createLayer} from './overlay.js';
import {$} from './dom.js';

// 二级项是链接（读屏读成链接）；外层的 #railItems 是一组导航，不另设列表的角色。
const ITEM = tpl`<a class="rail-item" draggable="false"><span class="rail-cover" data-ref="cover"><span class="rail-stack" data-ref="stack"><img data-ref="img2" alt=""
  draggable="false"></span><img data-ref="img" alt="" draggable="false" loading="lazy" decoding="async"><span class="rail-icon" data-ref="icon"></span></span><span
  class="rail-mark" data-ref="mark"></span><span class="rail-count" data-ref="count"></span><span class="rail-heart" data-ref="heart" hidden><svg viewBox="0 0 20 20"
  aria-hidden="true"><path d="M10 16.4S3.4 12.5 3.4 7.9A3.3 3.3 0 0 1 10 6.3a3.3 3.3 0 0 1 6.6 1.6c0 4.6-6.6 8.5-6.6 8.5Z"/></svg></span></a>`;
// 整理各队列的图标（和原来侧栏的一样），以及没有封面的画廊（收藏还没有图、画廊的条件现在没有图）。
const ICONS = {
  unmatched: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="7.2"/><path d="M8 7.8a2.2 2.2 0 1 1 3 2c-.6.3-1 .8-1 1.4v.4M10 14.2v.1"/></svg>`,
  duplicates: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3.5" y="6" width="9.5" height="11" rx="2"/><path d="M7 3h7.5A2.5 2.5 0 0 1 17 5.5V13"/></svg>`,
  excluded: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="7.2"/><path d="M5 15 15 5"/></svg>`,
  audit: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="4" y="3.5" width="12" height="14" rx="2"/><path d="M7.5 9.8 9.3 11.6 12.8 8"/></svg>`,
  identity: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 7h11l-3-3M16 13H5l3 3"/></svg>`,
  fav: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 16.4S3.4 12.5 3.4 7.9A3.3 3.3 0 0 1 10 6.3a3.3 3.3 0 0 1 6.6 1.6c0 4.6-6.6 8.5-6.6 8.5Z"/></svg>`,
  gallery: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="4.5" width="14" height="11" rx="2"/><path d="m3.5 13.5 4-4 3 3 2-2 4 4"/></svg>`,
};
const iconOf = x => x.kind === 'queue' ? x.key : x.kind === 'gallery' && !x.cover ? (x.key === 'fav' ? 'fav' : 'gallery') : '';
const PEEK_CARD = tpl`<div class="rail-peek-in"><span class="rail-peek-pics" data-ref="pics"></span><span class="rail-peek-cap"><b data-ref="title"></b><small
  data-ref="meta"></small></span></div>`;

const s = app.child('rail');
const providers = new Map(), pickers = new Map(), lists = new Map();
let current = '', selected = '', temp = null, pitch = 0, wired = false, swapping = 0;
const monthName = new Intl.DateTimeFormat(I18N.locale, {month: 'short'});
const thumb = id => id ? `/image?id=${encodeURIComponent(id)}&size=thumb` : '';
const itemsOf = box => box ? [...box.children].filter(x => !x.inert) : [];
const itemAt = (box, key) => itemsOf(box).find(x => x.dataset.key === key) || null;

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
  el.dataset.key = item.key; el.dataset.kind = item.kind;
  el.railCount = refs.count;
  const update = x => {
    attr(el, 'href', x.href || '#'); attr(el, 'aria-label', x.meta ? `${x.label}，${x.meta}` : x.label);
    attr(el, 'aria-current', x.key === selected ? 'page' : null);
    cls(el, 'is-round', !!x.round);
    attr(refs.img, 'src', thumb(x.cover) || null); show(refs.img, !!x.cover);
    attr(refs.img2, 'src', thumb(x.cover2) || null); show(refs.stack, x.kind === 'month' || x.kind === 'year');
    text(refs.mark, mark(x)); show(refs.mark, !!mark(x));
    show(refs.heart, !!x.heart && !!x.cover);
    const icon = iconOf(x);
    if (refs.icon.dataset.icon !== icon) { refs.icon.dataset.icon = icon; refs.icon.replaceChildren(...(ICONS[icon] ? [ICONS[icon]().el] : [])); }
    cls(el, 'is-icon', !!icon);
    // 数字只给整理的队列；时间轴的张数写在悬停预览里。数字正在滚动（count）时不打断它，滚完由 count 写终值。
    const n = x.kind === 'queue' ? x.count : 0;
    if (!counting.has(refs.count)) text(refs.count, n ? number(n) : '');
    show(refs.count, !!n);
    el.railItem = x;
  };
  update(item);
  return {el, update, dispose() {}};
}
const counting = new WeakSet();

// ---------- 每个入口一组 ----------
// 每个入口一个容器、一个按键复用的列表：切换入口时旧的一组退场、新的一组进场，回到原来的入口时节点还在。
function group(name) {
  const box = make('div', 'rail-group');
  box.dataset.entry = name; box.hidden = true;
  $('railItems').append(box);
  return box;
}
function listFor(entry) {
  if (lists.has(entry)) return lists.get(entry);
  const box = group(entry), list = {box, items: [], leaving: 0};
  list.keyed = keyed(box, {key: x => x.key, create: itemWidget, scope: s,
    enter: el => !box.hidden && enter(el, {from: [{opacity: 0, transform: 'translateX(-20px) scale(.5)'}, {opacity: 1, transform: 'none'}], dur: 3, ease: 'spring'}),
    // 让位：每一项一样高，挪了几格就从几格之外滑回来，不用量。有项正在退场时（它还占着位置）不滑，等它摘掉以后一起滑。
    move: (el, x, from, to) => {
      if (!box.hidden && !list.leaving && pitch) animate(el, [{transform: `translateY(${(from - to) * pitch}px)`}, {transform: 'none'}], {dur: 3, ease: 'spring'});
    },
    exit: async el => {
      if (box.hidden) return;
      list.leaving++;
      await exit(el, {to: [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'translateX(-14px) scale(.6)'}], remove: false});
      list.leaving--;
      const after = itemsOf(box).filter(x => x !== el), before = after.map(x => x.offsetTop);
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
  if (entry === current && !temp) s.read(measure, 'rail:measure');
}
export function provide(entry, fn) { providers.set(entry, fn); fill(entry); }
export function refresh(entry) { for (const e of entry ? [entry] : providers.keys()) fill(e); if (!entry || entry === 'tidy') dot(); }
export function onPick(entry, fn) { pickers.set(entry, fn); }

// ---------- 选中与指示条 ----------
function measure() {
  const box = visibleBox(), el = itemAt(box, selected), all = itemsOf(box);
  if (all.length > 1) pitch = all[1].offsetTop - all[0].offsetTop;
  const y = el ? el.offsetTop + el.offsetHeight / 2 - 13 : null;
  s.write(() => { css($('railInd'), 'transform', y === null ? null : `translateY(${y}px)`); cls($('railInd'), 'is-on', y !== null); }, 'rail:ind');
}
const visibleBox = () => temp?.list.box || lists.get(current)?.box;
export function select(key) {
  selected = key || '';
  for (const list of [...lists.values(), ...(temp ? [temp.list] : [])]) {
    for (const el of itemsOf(list.box)) attr(el, 'aria-current', el.dataset.key === selected ? 'page' : null);
  }
  s.read(measure, 'rail:measure');
  const el = itemAt(visibleBox(), selected);
  if (el) s.read(() => { const box = $('railLayer'), top = el.offsetTop, h = el.offsetHeight;
    if (top < box.scrollTop || top + h > box.scrollTop + box.clientHeight) s.write(() => { box.scrollTop = Math.max(0, top - box.clientHeight / 2); }, 'rail:scroll'); }, 'rail:reveal');
}
export function selectDay(day) { if (current === 'library') select(itemForDay(timeline.items, day)); }
// 队列的数字滚到新值（带千位分隔）；滚完经 ui/el.js 的 text() 写一次终值，好让写入缓存和屏幕上的一致，之后元数据带回的值照常能写进去。
export function count(key, n) {
  const el = itemAt(lists.get('tidy')?.box, key)?.railCount;
  if (!el) return;
  const from = Number(String(el.textContent).replace(/\D/g, '')) || 0;
  show(el, !!n); counting.add(el);
  countTo(el, n, {from, format: x => number(Math.round(x))}).finally(() => { counting.delete(el); text(el, n ? number(n) : ''); show(el, !!n); });
  animate(el, [{transform: 'scale(1)'}, {transform: 'scale(1.35)'}, {transform: 'scale(1)'}], {dur: 2, ease: 'spring'});
}
const dot = () => {
  for (const a of $('railEntries').querySelectorAll('[data-entry]')) {
    const b = [...a.children].find(c => c.classList.contains('rail-dot'));
    if (b) b.hidden = a.dataset.entry !== 'tidy' || !tidyWaiting();
  }
};

// ---------- 切换入口 ----------
// 旧的一组依次退场、新的一组依次进场。连着切了几次时（退场还没放完又换了入口），只有最后一次的那一组显示：每一次切换记一个代号，
// 退场回来发现代号已经变了就不再显示自己要显示的那组；不管走哪条路，最后都把要显示的那组以外的组藏起来。
function settle(toBox) {
  for (const box of $('railItems').children) {
    if (box === toBox) continue;
    if (!box.hidden) box.hidden = true;
    for (const el of box.children) el.getAnimations?.().forEach(a => a.cancel());
  }
}
async function swap(fromBox, toBox) {
  const gen = ++swapping;
  const quick = reduced() || !fromBox || fromBox.hidden || fromBox === toBox;
  cls($('railInd'), 'is-on', false);
  if (!quick) {
    await Promise.all(itemsOf(fromBox).map((el, i) => animate(el, [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'translateX(-16px) scale(.92)'}],
      {dur: 1, delay: i * 16, fill: 'forwards'})));
    if (gen !== swapping) { fromBox.hidden = true; for (const el of fromBox.children) el.getAnimations?.().forEach(a => a.cancel()); return; }
  }
  settle(toBox);
  if (!toBox) return;
  toBox.hidden = false;
  if (!quick) itemsOf(toBox).forEach((el, i) => enter(el, {from: [{opacity: 0, transform: 'translateX(-22px) scale(.86)'}, {opacity: 1, transform: 'none'}],
    dur: 3, delay: i * 38, ease: 'spring'}));
  s.read(measure, 'rail:measure');
}
function showEntry(entry) {
  if (entry === current) return;
  const was = current; current = entry;
  for (const a of $('railEntries').querySelectorAll('[data-entry]')) attr(a, 'aria-current', a.dataset.entry === entry ? 'page' : null);
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
    swap(box, providers.has(current) ? listFor(current).box : null).finally(() => box.remove());
    return;
  }
  if (!temp) {
    const box = group('mode:' + name);
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
  for (const a of $('railEntries').querySelectorAll('[data-entry]')) attr(a, 'href', entryHref(a.dataset.entry));
  attr($('settingsLink'), 'aria-current', at.entry === 'settings' ? 'page' : null);
  showEntry(at.entry);
  if (at.entry === 'library') { refresh('library'); loadTimeline(route).catch(() => {}); }   // 时间轴的链接带着现在的视图和筛选
  select(at.item);
  setDrawer(false, 'silent');
}

// ---------- 悬停：抽卡和预览 ----------
// 预览是一张从这一项旁边滑出来的玻璃卡：上面一行图，下面是标题和张数。图按真实宽高比排成一样高的一行（core/rail.js 的 peekRow），
// 放得下几张放几张，卡片跟着内容变宽；作品的角色画成一排圆头像。这一项的主色经 setTint 变成卡片底下的彩色投影和靠活页条那边
// 一层很淡的光（css/rail.css），不出色块。卡片建一次（wirePeek），之后只换内容，图的节点按位置复用。
// 动效：第一次出现时卡片从活页条那边滑出、图一张接一张发出来；已经开着时换到相邻的项，卡片滑到新的位置（CSS 过渡 translate），
// 不重新弹出；离开二级层稍等一会儿再收起，划过两项之间的空隙时不闪。滚动、拖动时立刻收起。键盘焦点到了项上同样给预览
// （读屏不读它：aria-hidden，项的 aria-label 里已经有标题和张数）。
const PEEK_IN = [{opacity: 0, transform: 'translateX(-10px) scale(.96)'}, {opacity: 1, transform: 'none'}];
const PEEK_OUT = [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'translateX(-6px) scale(.98)'}];
const DEAL = [{opacity: 0, transform: 'translateX(-10px) scale(.92)'}, {opacity: 1, transform: 'none'}];
const PEEK_LINGER = 120;   // 指针离开以后过多久收起（毫秒）
let hovered = null, card = null, peekOn = false, peekTimer = 0;
function wirePeek() {
  const {el, refs} = PEEK_CARD(), imgs = [];
  $('railPeek').replaceChildren(el);
  // 不知道宽高比的图（角色、画廊的封面）先按方的排，读到以后按它自己的比例改宽。
  const fit = img => {
    if (img.dataset.known || !img.naturalWidth || !img.naturalHeight) return;
    const r = Math.min(PEEK.ratio[1], Math.max(PEEK.ratio[0], img.naturalWidth / img.naturalHeight));
    css(img, 'width', `${Math.min(PEEK.max, Math.round(r * Number(img.dataset.h)))}px`);
  };
  const img = i => {
    if (!imgs[i]) {
      const x = make('img'); x.alt = ''; x.decoding = 'async'; x.draggable = false;
      x.addEventListener('load', () => fit(x));
      refs.pics.append(x); imgs[i] = x;
    }
    return imgs[i];
  };
  card = {refs, imgs, img};
}
function hover(el, {now = false} = {}) {
  if (hovered === el) return;
  if (hovered) for (const x of [hovered, hovered.previousElementSibling, hovered.nextElementSibling]) x?.classList.remove('is-hover', 'is-near');
  hovered = el;
  if (!el || el.inert || !el.railItem) { hovered = null; hidePeek(now); return; }
  el.classList.add('is-hover');
  el.previousElementSibling?.classList.add('is-near'); el.nextElementSibling?.classList.add('is-near');
  showPeek(el);
}
function showPeek(el) {
  if (!card) return;
  const x = el.railItem, peek = $('railPeek'), {refs, imgs} = card;
  const row = peekRow(x.peek?.length ? x.peek : x.cover ? [{id: x.cover}] : [], {round: !!x.peekRound});
  text(refs.title, x.label); text(refs.meta, x.meta || ''); show(refs.meta, !!x.meta);
  show(refs.pics, row.pics.length > 0); cls(refs.pics, 'is-round', !!x.peekRound);
  row.pics.forEach((p, i) => {
    const im = card.img(i);
    im.dataset.known = p.known ? '1' : ''; im.dataset.h = String(row.h);
    css(im, 'width', `${p.w}px`); css(im, 'height', `${row.h}px`); attr(im, 'src', thumb(p.id)); show(im, true);
  });
  for (const im of imgs.slice(row.pics.length)) show(im, false);
  setTint(peek, x.color);
  const glide = peekOn;
  peekOn = true;
  if (!glide) cancelExit(peek);
  peek.hidden = false;   // 要量它的高；位置和进场在下一帧画之前写好，不会先在旧位置闪一下
  s.read(() => {
    const r = el.getBoundingClientRect(), layer = $('railLayer').getBoundingClientRect(), h = peek.offsetHeight, edge = 8;
    const y = Math.round(Math.max(edge, Math.min(innerHeight - h - edge, r.top + r.height / 2 - h / 2)));
    const left = Math.round(layer.right - 6);
    s.write(() => {
      cls(peek, 'is-glide', glide);
      css(peek, 'translate', `${left}px ${y}px`);
      const shown = imgs.slice(0, row.pics.length);
      if (glide) { shown.forEach(im => enter(im, {from: 'fade', dur: 1})); return; }
      enter(peek, {from: PEEK_IN, dur: 2});
      shown.forEach((im, i) => enter(im, {from: DEAL, dur: 2, delay: 40 + i * 35}));
    }, 'rail:peek');
  }, 'rail:peek-read');
}
// 收起：now 时立刻（滚动、拖动），否则等 PEEK_LINGER，到时指针还没落到别的项上才收。只排一个计时器（挂在 app 作用域上，
// 不随每次悬停越积越多）。
function hidePeek(now) {
  const close = () => { if (!peekOn) return; peekOn = false; exit($('railPeek'), {to: PEEK_OUT, remove: false}); };
  if (now) { close(); return; }
  if (!peekTimer) peekTimer = s.timeout(() => { peekTimer = 0; if (!hovered) close(); }, PEEK_LINGER);
}

// ---------- 钉住的项拖动排序（#684） ----------
let drag = null;
function startDrag(e, el) {
  if (e.button !== 0 || current !== 'catalog' || temp || !pins.keys.includes(el.dataset.key)) return;
  drag = {el, key: el.dataset.key, y: e.clientY, moved: false, id: e.pointerId};
}
function moveDrag(e) {
  if (!drag || e.pointerId !== drag.id) return;
  const dy = e.clientY - drag.y;
  if (!drag.moved && Math.abs(dy) < 5) return;
  if (!drag.moved) { drag.moved = true; drag.el.setPointerCapture?.(e.pointerId); cls(drag.el, 'is-dragging', true); hover(null, {now: true}); }
  css(drag.el, 'transform', `translateY(${dy}px)`);
}
function endDrag(e) {
  if (!drag || e.pointerId !== drag.id) return;
  const {el, key, moved, y} = drag; drag = null;
  if (!moved) return;
  cls(el, 'is-dragging', false); css(el, 'transform', null);
  // 拖完松手会接着来一次 click：吞掉它（不当成点了这一项）。没有来（pointercancel、松手在别处）时下一轮就摘掉，不吞别的点击。
  const swallow = ev => ev.preventDefault();
  el.addEventListener('click', swallow, {once: true, capture: true});
  s.timeout(() => el.removeEventListener('click', swallow, {capture: true}), 0);
  if (e.type === 'pointercancel') return;
  const steps = pitch ? Math.round((e.clientY - y) / pitch) : 0, from = pins.keys.indexOf(key);
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
// ⌘1–4 认 e.code（Digit1–4）：法语这类布局上数字键的 e.key 不是数字。
const DIGITS = {Digit1: 0, Digit2: 1, Digit3: 2, Digit4: 3};
export function railKey(e) {
  if (composing(e)) return false;
  const mod = e.metaKey || e.ctrlKey;
  if (mod && !e.altKey && !e.shiftKey && e.code in DIGITS) {
    e.preventDefault(); location.hash = entryHref(ENTRIES[DIGITS[e.code]].id); return true;
  }
  if (!e.altKey || mod || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return false;
  if (e.target?.closest?.('input,textarea,select,[contenteditable]')) return false;
  const box = visibleBox(); if (!box) return false;
  e.preventDefault();
  const dir = e.key === 'ArrowDown' ? 1 : -1;
  if (e.shiftKey) {   // 挪动钉住的项：焦点或选中的那项
    const key = document.activeElement?.dataset?.key || selected, at = pins.keys.indexOf(key);
    if (current === 'catalog' && at >= 0) movePin(key, at + dir).then(() => itemAt(box, key)?.focus({preventScroll: true}));
    return true;
  }
  const items = itemsOf(box), i = items.findIndex(x => x.dataset.key === selected);
  const next = items[i < 0 ? (dir > 0 ? 0 : items.length - 1) : Math.max(0, Math.min(items.length - 1, i + dir))];
  if (next && next.dataset.key !== selected) pick(next, e);
  return true;
}
// 点了（或用 ⌥↑↓ 走到）一项：这一页登记了 onPick 且处理了就到此为止；网格视图里用键盘走只移动选中（不把人从网格带到按天视图，
// 回车或点击才去）；其余按项的链接走（点击由 <a> 自己走）。地址没变（已经在那里）时只改选中。
function pick(el, e) {
  const fn = temp ? null : pickers.get(current);
  if (fn?.(el.railItem, e)) { e?.preventDefault?.(); select(el.dataset.key); return; }
  if (e?.type === 'click') return;
  if (current === 'library' && pageOf(state.route).view === 'grid') { select(el.dataset.key); el.focus?.({preventScroll: true}); return; }
  const href = el.getAttribute('href');
  if (location.hash === href) select(el.dataset.key); else location.hash = href;
}

// ---------- 启动 ----------
export function wireRail() {
  wired = true;
  wirePeek();
  provide('tidy', () => queues());
  provide('galleries', () => galleryItems(galleryCovers));
  provide('catalog', () => pinItems());
  provide('library', () => timeline.items.map(x => ({...x, label: timelineLabel(x), meta: timelineMeta(x), href: timelineHref(state.route, x.first)})));
  // 时间轴读完重画：原来选中的那项还在就留着（网格里 ⌥↑↓ 移过去的、按天视图滚动时 selectDay 设的），不在了才回到地址上的那一项。
  timeline.changed.on(() => {
    refresh('library');
    if (current === 'library' && !timeline.items.some(x => x.key === selected)) select(railAt(state.route, state.char).item);
  });
  pins.changed.on(() => { refresh('catalog'); select(railAt(state.route, state.char).item); });
  loadPins().then(() => refresh('catalog'));
  // 写入、撤销之后旧月份的张数和封面可能变了：整条重读。同步到了新图只重读最新的月。
  // 连着写几次（点收藏、改标签）只在停下 0.7 秒后读一次，不在图库里时等下次进来再读。
  let soon = 0;
  s.add(onPost(() => {
    forgetTimeline(); clearTimeout(soon);
    soon = s.timeout(() => { if (current === 'library') loadTimeline(state.route).catch(() => {}); }, 700);
  }));
  s.add(onJob((job, change) => { if (job === 'sync' && change.arrived && current === 'library') loadTimeline(state.route, {newest: true}).catch(() => {}); }));
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
  s.on($('railLayer'), 'scroll', () => hover(null, {now: true}), {passive: true});
  s.on($('sidebarToggle'), 'click', () => setDrawer(!$('rail').classList.contains('is-drawer')));
  // 窗口拉宽到不再是抽屉时，开着的抽屉收起来（不然它一直留在浮层栈里，网格的方向键不响应）。
  const narrow = typeof matchMedia === 'function' ? matchMedia('(max-width: 760px)') : null;
  if (narrow) s.on(narrow, 'change', () => { if (!narrow.matches) setDrawer(false, 'silent'); });
  if (typeof ResizeObserver === 'function') s.observe(new ResizeObserver(() => s.read(measure, 'rail:measure'))).observe($('railLayer'));
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
// 切换 NSFW 之后：时间轴（在图库里时）和画廊封面按新的筛选重读。
export function railNsfw() {
  if (current === 'library') loadTimeline(state.route).catch(() => {});
  loadGalleryCovers();
}
// 画廊的封面按 NSFW 设置批量读（core/galleries.js）；读到以后重画画廊这一组。
let galleryCovers = {};
export async function loadGalleryCovers() {
  const ids = (state.galleries || []).map(g => g.id);
  try { galleryCovers = await coversFor(ids); } catch { galleryCovers = {}; }
  refresh('galleries');
}
