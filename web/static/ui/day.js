// 按天视图的一天（#707，0.4 第 C1 步）：页头（大字日期、星期和张数、「在网格里看 ›」）、当天的角色芯片行（一行放得下几个放
// 几个，其余收成「另外 N 位」，点开换行显示全部）、马赛克（一张大图 + 两行小图，放不下的收成最后一格上的「+N 张」）。
// 部件写法照 docs/WEB_V2.md「渲染底座」：{el, update(box), dispose()}，由 ui/virtual.js 按框挂和摘（ui/days.js）。
//
//   const day = dayWidget(box, ctx);   // box：{key: 日期, x, y, w, h, g: 这一天的数据, open: 芯片行展开着, people: 展开后的全部角色}
//   day.update(box);                   // 数据或宽度变了；图块按 uuid 复用，已有的图不重建、不换地址，新图淡入，挪了位置的滑过去
//   day.mark(uuid);                    // 详情卡片开着的那张
//
// ctx 由页面给（这里不发请求、不读路由）：
//   sizes()            一天各部分的尺寸 {head, chips, rowHeight, gap, after}（core/days.js 的 dayHeight 用同一份）
//   hero(day)          本页期间这一天固定的大图 uuid；hold(day, uuid) 记下第一次用的大图
//   href(day, char?)   「在网格里看」、点角色去的地址
//   cover(char)        角色头像的图片 uuid
//   toggle(day)        点了「另外 N 位」或「收起」
//   resized(day, h)    展开后量到的芯片行高度（收起时是 null），页面据此重排各天的框
//   marks()            图上显示副本数（视图选项）
//   people()           页头写角色数（视图选项）
// 点图、双击图、点「+N 张」由页面在外层统一接（data-id、data-more），这里不挂。
import {dayMosaic} from '../core/layout.js';
import {fitChips, heroFirst, mosaicHeight} from '../core/days.js';
import {dayLabel, number, thumbSize} from '../core/format.js';
import {I18N, t} from '../core/i18n.js';
import {nameWithWork, workLabel} from '../core/labels.js';
import {attr, cls, css, show, text, tpl} from './el.js';
import {keyed} from './keyed.js';
import {enter, exit, flip, thumbs} from './motion.js';
import {scope} from './scope.js';
import {setTint} from './tint.js';
import {releaseImages} from './virtual.js';

const DAY = tpl`<section class="day"><header class="day-head"><h2 class="title-page day-title" data-ref="title"></h2><span class="day-meta" data-ref="meta"></span>
  <a class="day-grid" data-ref="grid"></a></header><div class="day-chars" data-ref="chars"><div class="day-chips" data-ref="chips"></div><button class="chip day-more"
  type="button" data-ref="more" aria-expanded="false"></button></div><div class="day-mosaic" data-ref="mosaic"></div></section>`;
const TILE = tpl`<button class="day-tile" type="button"><img data-ref="img" alt="" draggable="false" decoding="async"><span class="day-cap" data-ref="cap"><b data-ref="name"></b><small
  data-ref="work"></small></span><span class="day-heart" data-ref="heart" aria-hidden="true"><svg viewBox="0 0 20 20"><path d="M10 16.4S3.4 12.5 3.4 7.9A3.3 3.3 0 0 1 10 6.3a3.3
  3.3 0 0 1 6.6 1.6c0 4.6-6.6 8.5-6.6 8.5Z"/></svg></span><span class="day-dup" data-ref="dup"></span><span class="day-plus" data-ref="plus"></span></button>`;
const CHIP = tpl`<a class="chip day-chip"><img class="avatar" data-ref="img" alt="" loading="lazy" decoding="async"><span data-ref="name"></span><span class="chip-count" data-ref="n"></span></a>`;
// 芯片之间的缝（css/library.css .day-chips 的 gap，--space-4）；「另外 N 位」还没量到宽度时按这么宽估。
const CHIP_GAP = 4, MORE_GUESS = 96;

const weekday = day => {
  const [y, m, d] = day.split('-').map(Number);
  return new Intl.DateTimeFormat(I18N.locale, {weekday: 'short'}).format(new Date(y, m - 1, d));
};
const thumbUrl = (uuid, size) => `/image?id=${encodeURIComponent(uuid)}&size=${size}`;
const SIZES = ['thumb', 'medium'];

// ---------- 图块 ----------
// 一张图：主色占位，读完淡入；大图下面压着角色名；收藏的心形、副本数；最后一格盖「+N 张」。
// 缩略图档位按这张图画过的最大高度选，只升不降：图从大图挪成小图时地址不变，不重新读、不闪。
function tileWidget(cell, ctx, s) {
  const {el, refs} = TILE(), ts = s.child('tile');
  let box = null, size = -1;
  ts.on(refs.img, 'load', () => cls(el, 'is-in', true));
  function update(next) {
    const {item, hero, more} = next, b = next.box;
    el.dataset.id = item.uuid; thumbs.mark(el, item.uuid);
    setTint(el, item.color);
    const want = SIZES.indexOf(thumbSize(b.h));
    if (want > size) { size = want; cls(el, 'is-in', false); attr(refs.img, 'src', thumbUrl(item.uuid, SIZES[want])); }
    const top = hero ? item.memberships?.[0] : null;
    cls(el, 'is-hero', !!hero); show(refs.cap, !!top); text(refs.name, top?.name || ''); text(refs.work, top ? workLabel(top) : '');
    attr(el, 'title', (item.memberships || []).map(nameWithWork).join(t('、')) || null);
    attr(refs.heart, 'data-fav', item.uuid); cls(refs.heart, 'on', !!item.fav);
    const dup = ctx.marks() && item.dup ? item.dup.size : 0;
    show(refs.dup, dup > 1); text(refs.dup, dup > 1 ? '×' + dup : '');
    attr(el, 'data-more', more ? String(more) : null); show(refs.plus, !!more);
    text(refs.plus, more ? t('+{n} 张', {n: number(more)}) : '');
    attr(el, 'aria-label', more ? t('还有 {n} 张，在网格里看', {n: number(more)}) : null);
    css(el, 'left', b.x + 'px'); css(el, 'top', b.y + 'px'); css(el, 'width', b.w + 'px'); css(el, 'height', b.h + 'px');
    if (box && (box.x !== b.x || box.y !== b.y || box.w !== b.w || box.h !== b.h)) flip(el, box, b);
    box = b;
  }
  update(cell);
  return {el, update, dispose: () => ts.dispose()};
}

// ---------- 芯片 ----------
function chipWidget(c, ctx, day) {
  const {el, refs} = CHIP();
  el.dataset.char = c.id;
  const update = x => {
    attr(el, 'href', ctx.href(day, x.id));
    const cover = ctx.cover(x.id);
    attr(refs.img, 'src', cover ? thumbUrl(cover, 'thumb') : null);
    text(refs.name, x.name); text(refs.n, number(x.count));
    attr(el, 'title', nameWithWork(x));
  };
  update(c);
  return {el, update, dispose() {}};
}

// ---------- 一天 ----------
export function dayWidget(box, ctx, parent = null) {
  const s = parent ? parent.child('day') : scope('day'), {el, refs} = DAY();
  let current = box, widths = new Map(), moreWidth = 0, shown = Infinity;
  const day = box.key;
  el.dataset.day = day;
  text(refs.title, dayLabel(day));
  text(refs.grid, t('在网格里看') + ' ›');

  const tiles = keyed(refs.mosaic, {key: x => x.item.uuid, create: x => tileWidget(x, ctx, s), scope: s,
    enter: el => enter(el, {from: 'fade'}),
    exit: el => exit(el, {to: 'fade'}).then(() => releaseImages(el))});
  const chips = keyed(refs.chips, {key: x => x.id, create: x => chipWidget(x, ctx, day), scope: s,
    enter: el => enter(el, {from: 'scale', dur: 1})});
  s.on(refs.more, 'click', () => ctx.toggle(day));

  // 芯片行收起时：量还没量过的芯片宽度和「另外 N 位」的宽度，再定放几个。宽度和这一行多宽无关，量一次就记着，窗口变宽变窄
  // 时只重算。展开时不裁，量整行的高度报给页面。
  function measure() {
    for (const el of refs.chips.children) if (!widths.has(el.dataset.char) && !el.classList.contains('is-cut')) widths.set(el.dataset.char, el.offsetWidth || 0);
    if (!refs.more.hidden && refs.more.offsetWidth) moreWidth = refs.more.offsetWidth;
    if (current.open) { const h = refs.chars.offsetHeight; if (h) ctx.resized(day, h); return; }
    s.write(fit, 'day:fit');
  }
  function fit() {
    const g = current.g, list = g.characters || [], hidden = Math.max(0, (g.people || list.length) - list.length);
    shown = fitChips(list.map(c => widths.get(c.id) || 0), current.w, moreWidth || MORE_GUESS, CHIP_GAP, hidden);
    for (const el of refs.chips.children) cls(el, 'is-cut', list.findIndex(c => c.id === el.dataset.char) >= shown);
    const rest = (g.people || list.length) - shown;
    show(refs.more, rest > 0);
    text(refs.more, t('另外 {n} 位', {n: number(rest)}));
  }

  function update(next) {
    current = next;
    const g = next.g, sizes = ctx.sizes(), people = g.people || (g.characters || []).length;
    // 页头
    text(refs.meta, [weekday(day), t('{n} 张', {n: number(g.count)}), ...(ctx.people() && people ? [t('{n} 位角色', {n: number(people)})] : [])].join(' · '));
    attr(refs.grid, 'href', ctx.href(day));
    // 芯片行
    const list = next.open && next.people ? next.people : g.characters || [];
    show(refs.chars, list.length > 0);
    cls(refs.chars, 'is-open', !!next.open);
    css(refs.chars, 'height', next.open ? null : sizes.chips + 'px');
    chips.set(list);
    attr(refs.more, 'aria-expanded', String(!!next.open));
    if (next.open) {
      for (const el of refs.chips.children) cls(el, 'is-cut', false);
      show(refs.more, true); text(refs.more, t('收起'));
    }
    // 每次更新后量一次（只量新出现的芯片，已量过的记着），再定放几个；展开时量整行的高度。
    s.read(measure, 'day:measure');
    // 马赛克：大图本页期间固定（ctx.hero），其余按后端的次序
    const items = heroFirst(g, ctx.hero(day));
    if (items[0]) ctx.hold(day, items[0].uuid);
    const laid = dayMosaic(items.map(x => x.width > 0 && x.height > 0 ? x.width / x.height : 0), {width: next.w, rowHeight: sizes.rowHeight, gap: sizes.gap});
    css(refs.mosaic, 'height', mosaicHeight(sizes) + 'px');
    // 「+N」盖在最后一格上：马赛克没画的（b.more 已含这一格自己）加上这一天后端没给的张数。
    const unsent = Math.max(0, (g.count || 0) - items.length), last = laid.boxes.length - 1;
    tiles.set(laid.boxes.map((b, k) => ({item: items[b.i], box: b, hero: k === 0,
      more: k === last && (b.more || unsent) ? (b.more || 1) + unsent : 0})));
  }
  update(box);

  return {
    el,
    update,
    mark(uuid) { for (const el of refs.mosaic.children) if (!el.inert) attr(el, 'aria-current', el.dataset.id === uuid ? 'true' : null); },
    dispose: () => s.dispose(),
  };
}
