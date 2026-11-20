// 整理页（#713）共用的分组：按组整理的一组、身份整理的一对、画师抽查的一张都画成同一种浅底组（css/base.css 的 .group），
// 组头一行写标题、张数和一句话，操作收在组头右边；下面是一排方图，占满一行宽。样式在 css/tidy.css。
//
//   const g = tidyGroup();                      // {el, refs: {title, meta, q, acts, strip, note}}
//   const strip = thumbStrip(g.refs.strip, {scope, max: 16});
//   strip.update(ids, {off, rest: '#/g/3'});    // 方图按 uuid 复用；off 里的标为不算；rest 给了时超出的收成最后一格「+N」
//   setPeekList(g.el, () => ids);               // 长按或 Space 看大图时翻这一组（ui/viewer.js 的 wirePeek，见 wireTidyPeek）
//
// 一组处理完：交给 ui/keyed.js 的 exit 钩子 collapse(el)——这组向右淡出，下面的组从原来的位置滑上来（FLIP，只动 transform 和
// opacity）。系统开了「减少动态效果」时组直接消失。
import {imageUrl} from '../core/format.js';
import {t} from '../core/i18n.js';
import {$} from './dom.js';
import {attr, cls, make, text, tpl} from './el.js';
import {keyed} from './keyed.js';
import {exit, flip} from './motion.js';
import {wirePeek} from './viewer.js';

const GROUP = tpl`<section class="group tidy-group">
  <header class="tidy-head">
    <div class="tidy-title"><b class="tidy-name" data-ref="title"></b><span class="tidy-meta" data-ref="meta"></span><span class="tidy-q" data-ref="q"></span></div>
    <div class="tidy-acts" data-ref="acts"></div>
  </header>
  <div class="tidy-strip" data-ref="strip"></div>
  <p class="tidy-note" data-ref="note" hidden></p>
</section>`;
const THUMB = tpl`<button type="button" class="tidy-thumb"><img alt="" decoding="async" loading="lazy" draggable="false"><i aria-hidden="true">✕</i></button>`;
const MORE = tpl`<a class="tidy-thumb is-more"></a>`;

/** 一个空的分组，内容由各页填。 */
export const tidyGroup = () => GROUP();

/**
 * 一排方图。ids 是这一组的 uuid，只画前 max 张；off（Set）里的标为「不算在这组里」；rest 给了时超出的收成最后一格「+N」链接。
 * 带 data-gimg 的方图点一下由页面处理（按组整理：算不算在这组里），身份整理、画师抽查的方图（gimg: false）点一下看大图。
 */
export function thumbStrip(box, {scope, max = 16, gimg = true} = {}) {
  // keyed 管 thumbs 里的全部子节点，「+N」放在它后面（.tidy-thumbs 是 display: contents，方图和「+N」排在同一个网格里）。
  const thumbs = make('span', 'tidy-thumbs'), more = MORE().el;
  box.replaceChildren(thumbs);
  const list = keyed(thumbs, {key: x => x.id, scope, create: x => {
    const {el} = THUMB(), img = el.firstElementChild;
    attr(el, 'data-peek', x.id);
    if (gimg) attr(el, 'data-gimg', x.id);
    attr(img, 'src', imageUrl(x.id, 'thumb'));
    const update = y => {
      cls(el, 'is-off', !!y.off);
      attr(el, 'aria-pressed', gimg ? String(!!y.off) : null);
      attr(el, 'title', gimg ? (y.off ? t('点一下：算回这组') : t('点一下：不算在这组里')) + ' · ' + t('长按或 Space 看大图') : t('看大图'));
    };
    update(x);
    return {el, update};
  }});
  return {update(ids, {off = null, rest = null} = {}) {
    list.set(ids.slice(0, max).map(id => ({id, off: !!off?.has(id)})));
    if (rest && ids.length > max) {
      attr(more, 'href', rest); text(more, '+' + (ids.length - max));
      if (more.parentNode !== box) box.append(more);
    } else more.remove();
  }};
}

// 各组看大图时翻哪些图：节点 → 返回 uuid 列表的函数。
const peekLists = new WeakMap();
export const setPeekList = (el, list) => peekLists.set(el, list);
function peekList(el) {
  for (let node = el; node; node = node.parentElement) { const f = peekLists.get(node); if (f) return f(el); }
  return [];
}
/** 整理各页的主区共用一个 #groupsPage：看大图在它上面登记一次（页面启动时，app.js）。 */
export function wireTidyPeek() { wirePeek($('groupsPage'), peekList); }

/**
 * keyed 的 exit 钩子：这组向右淡出，之后同一个列表里它下面的组从原来的位置滑上来。返回的 promise 落定后 keyed 摘掉节点。
 * 「减少动态效果」时 motion.js 直接到终态：组立刻藏起来，其余的组不滑。
 */
export async function collapse(el) {
  const below = [];
  for (let n = el.nextElementSibling; n; n = n.nextElementSibling) if (!n.inert && !n.hidden) below.push(n);
  const before = below.map(n => n.getBoundingClientRect());
  await exit(el, {to: [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'translateX(28px)'}], dur: 1, remove: false});
  below.forEach((n, i) => flip(n, before[i], n.getBoundingClientRect(), {dur: 2}));
}
/** 一组处理完时，页面上的这个计数先按本地算的减掉（活页条的数字滚动由 ui/rail.js 的 count 管，元数据回来再写真值）。 */
export const minus = (n, by) => Math.max(0, (Number(n) || 0) - by);
