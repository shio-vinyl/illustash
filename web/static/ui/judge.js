// 判断卡（#787）：一组图问一句「是不是」。点一张是「点掉」（再点算回），长按或 Space 看大图（data-peek，页面用 ui/viewer.js
// 的 wirePeek 接）；回答按钮随点掉的张数变。认一认（ui/day_cards.js 的 askCard）、按组整理（ui/groups.js）、可能认错
// （ui/doubts.js）三处共用。部件只画、不发请求：问句、头像这些卡头由各页自己画，这里只管图、点掉和按钮，回答交给调用方。
//
//   const j = judgeCard({scope, look: 'strip', tip, actions, status, onOff});
//   box.append(j.el); foot.append(j.acts);          // 图是一个节点、按钮是一个节点，各页放在自己的位置
//   j.update(items, {off, rest});                   // items [{uuid, width, height}]；off 是点掉的 uuid 集合（部件就地改它，
//                                                   //   不给就是一个空的）；rest 给了时超出 max 的收成最后一格「+N」链接
//   j.paint();                                      // 调用方自己的状态变了（认一认展开、没读到）：按钮按现在的状态重算一遍
//   j.busy(on);                                     // 写入的时候锁住：按钮和图都点不动
//   j.off                                           // 现在点掉的集合
//
//   look     'tiles' 按宽高比排成一样高（认一认）；'strip' 方图占满一行（按组整理、可能认错）
//   max      最多画几张（默认 16）；items 里没画出来的照样算在 n、kept 里（按组整理的回答对整组生效）
//   tip      {off, on}：图上的提示，off 是还没点掉时（「点一下：不是 TA」），on 是点掉以后（「点一下：改回「是」」），后面接
//            「长按或 Space 看大图」；可以是值，也可以是 () => 值（可能认错的提示里写着 A 的名字）
//   status   ({n, k}) => 文字：按钮最前面的一句（认一认的「N 张不是 TA」）；不给就不画
//   actions  回答按钮，按次序画，每个 {key, label, when, disabled, primary, quiet, title, href, run}：
//            label、title、when、disabled、primary 可以是值，也可以是 ({n, k, kept}) => 值（n 图的张数，k 点掉的张数，kept 没点掉的
//            uuid）；when 为假时藏起来；href 给了画成链接（不调 run）；run(off) 点了调，off 是点掉的集合的一份拷贝。
//            按钮带 data-ref，值是 key（页面和走查按它找按钮）
//   onOff(off)  点掉的变了（按组整理把它记进页面，重读以后还在）
//   shrink   拿掉的图缩小淡出（认一认、可能认错：「点掉的是…」写完，那几张从卡上走掉）；不给就直接摘掉
//
// 「点掉」的画法只有一种（css/base.css 的 .judge-tile）：图变淡、盖一个 ✕，aria-pressed 为 true。
import {imageUrl, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {attr, cls, css, prop, show, text, tpl} from './el.js';
import {keyed} from './keyed.js';
import {exit, thumbs} from './motion.js';
import {releaseImages} from './virtual.js';

const TILE = tpl`<button class="judge-tile" type="button"><img data-ref="img" alt="" draggable="false" loading="lazy" decoding="async"><i aria-hidden="true">✕</i></button>`;
const MORE = tpl`<a class="judge-tile is-more"></a>`;
const BOX = tpl`<div class="judge-tiles"><span class="judge-list" data-ref="list"></span></div>`;
const ACTS = tpl`<div class="judge-acts"></div>`;
const STATE = tpl`<span class="judge-state" role="status"></span>`;
const BUTTON = tpl`<button class="btn is-small" type="button"></button>`;
const LINK = tpl`<a class="btn is-small"></a>`;
const ratio = x => x.width > 0 && x.height > 0 ? Math.min(2.4, Math.max(.4, x.width / x.height)) : .75;
const val = (v, ctx) => typeof v === 'function' ? v(ctx) : v;

function tile(x, look) {
  const {el, refs} = TILE();
  el.dataset.peek = x.uuid;
  attr(refs.img, 'src', imageUrl(x.uuid)); thumbs.mark(refs.img, x.uuid);   // 看图器从这张 <img> 展开、缩回（#709）
  if (look === 'tiles') css(el, 'aspect-ratio', String(ratio(x)));
  return {el, update() {}, dispose() { releaseImages(el); }};
}

export function judgeCard({scope: parent, look = 'strip', max = 16, tip, status = null, actions = [], onOff = () => {}, shrink = false}) {
  const s = parent.child('judge');
  const {el, refs: {list}} = BOX(), acts = ACTS().el, more = MORE().el;
  cls(el, 'is-' + look, true);
  const say = status ? STATE().el : null;
  if (say) acts.append(say);
  const buttons = actions.map(a => {
    const b = (a.href ? LINK : BUTTON)().el;
    attr(b, 'data-ref', a.key);
    acts.append(b);
    if (!a.href) s.on(b, 'click', () => { if (!locked) a.run?.(new Set(off)); });
    return [a, b];
  });
  let items = [], off = new Set(), locked = false;
  const tiles = keyed(list, {key: x => x.uuid, create: x => tile(x, look), scope: s, ...(shrink ? {exit: node => exit(node, {to: 'scale'})} : {})});
  // 点一张图：在「点掉」和「算回」之间切换（在图的容器上接，按 data-peek 找是哪张）。
  s.on(list, 'click', e => {
    let node = e.target;
    while (node && node !== list && !node.dataset?.peek) node = node.parentNode;
    if (!node || node === list || node.inert || node.disabled || locked) return;
    const uuid = node.dataset.peek;
    if (off.has(uuid)) off.delete(uuid); else off.add(uuid);
    onOff(off);
    paint();
  });

  // 每次写全部状态；值没变的不动 DOM（ui/el.js）。
  function paint() {
    const n = items.length, k = items.filter(x => off.has(x.uuid)).length;
    const ctx = {n, k, kept: items.filter(x => !off.has(x.uuid)).map(x => x.uuid)};
    if (say) text(say, status(ctx));
    for (const [a, b] of buttons) {
      show(b, a.when === undefined || !!val(a.when, ctx));
      text(b, val(a.label, ctx) || '');
      attr(b, 'title', val(a.title, ctx) || null);
      if (a.href) attr(b, 'href', val(a.href, ctx));
      else prop(b, 'disabled', locked || !!val(a.disabled, ctx));
      const primary = !!val(a.primary, ctx);
      cls(b, 'is-primary', primary); cls(b, 'is-quiet', !primary && !!val(a.quiet, ctx));
    }
    for (const node of list.children) {
      if (node.inert) continue;
      const no = off.has(node.dataset.peek);
      attr(node, 'aria-pressed', String(no));
      attr(node, 'title', val(no ? tip.on : tip.off, ctx) + ' · ' + t('长按或 Space 看大图'));
      prop(node, 'disabled', locked);
    }
  }
  function update(next, {off: given = null, rest = null} = {}) {
    items = next || [];
    off = given || new Set();
    tiles.set(items.slice(0, max));
    if (rest && items.length > max) {
      attr(more, 'href', rest); text(more, '+' + number(items.length - max));
      if (more.parentNode !== el) el.append(more);
    } else more.remove();
    paint();
  }
  function busy(on) { locked = on; paint(); }
  return {el, acts, update, paint, busy, get off() { return off; }, dispose: () => s.dispose()};
}
