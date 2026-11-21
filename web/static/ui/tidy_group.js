// 整理页（#713）共用的部件：浅底分组（css/base.css 的 .group）、一排方图、收起动效、看大图的列表，以及各页共用的底栏清理和
// 读失败时的「重试」。样式在 css/tidy.css。按组整理的一组、身份整理的一对用 tidyGroup()：组头一行写标题、张数和一句话，操作
// 收在组头右边；按组整理下面是一排方图，身份整理把方图换成左右两边的样图。画师抽查的一张图只借用 .group.tidy-group 的样子。
//
//   const g = tidyGroup();                      // {el, refs: {title, meta, q, acts, strip, note}}
//   const strip = thumbStrip(g.refs.strip, {scope, max: 16});
//   strip.update(ids, {off, rest: '#/g/3'});    // 方图按 uuid 复用；off 里的标为不算；rest 给了时超出的收成最后一格「+N」
//   setPeekList(g.el, () => ids);               // 长按或 Space 看大图时翻这一组（ui/viewer.js 的 wirePeek，见 wireTidyPeek）
//
// 一组处理完：先 leaving(el) 记下它，再从列表里拿掉；ui/keyed.js 的 exit 钩子 collapse(el) 只对记下的那组播收起——向右淡出，
// 视口附近、在它下面的组从原来的位置滑上来（FLIP，只动 transform 和 opacity），其余直接到位；没记下的（重新分组后整表换掉）
// 直接摘掉。系统开了「减少动态效果」时组直接消失。
import {imageUrl, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {readError} from '../core/session.js';
import {$} from './dom.js';
import {attr, cls, make, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';
import {exit, flip} from './motion.js';
import {wirePeek} from './viewer.js';

const GROUP = tpl`<section class="group tidy-group">
  <header class="tidy-head">
    <div class="tidy-title"><b class="tidy-name" data-ref="title"></b><span class="text-meta" data-ref="meta"></span><span class="tidy-q" data-ref="q"></span></div>
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
 * 带 data-gimg 的方图点一下由页面处理（按组整理：算不算在这组里），身份整理的方图（gimg: false）点一下看大图。
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
      attr(more, 'href', rest); text(more, '+' + number(ids.length - max));
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

// 刚处理完、要播收起的组。
const marked = new WeakSet();
/** 记下这组是刚处理完的：下一次从列表里拿掉它时播收起。 */
export const leaving = el => { if (el) marked.add(el); };
// 视口上下各多量这么多屏：再远的组看不见，直接到位。
const NEAR = 1;
/** keyed 的 exit 钩子（见文件头）。返回的 promise 落定后 keyed 摘掉节点。 */
export async function collapse(el) {
  if (!marked.has(el)) return;
  marked.delete(el);
  const view = globalThis.innerHeight || 0, below = [];
  for (let n = el.nextElementSibling; n; n = n.nextElementSibling) {
    if (n.inert || n.hidden) continue;
    const r = n.getBoundingClientRect();
    if (view && r.top > view * (1 + NEAR)) break;
    below.push([n, r]);
  }
  await exit(el, {to: [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'translateX(28px)'}], dur: 1, remove: false});
  for (const [n, from] of below) flip(n, from, n.getBoundingClientRect(), {dur: 2});
}

/** 面板页刚进来时清掉底栏（#resultCount、#hint）：别让上一页的张数和提示留着。和别的模块一样直接写 textContent。 */
export function clearFooter() {
  for (const id of ['resultCount', 'hint']) { const el = $(id); if (el) el.textContent = ''; }
}
/** 第一次读取就失败、页面上还什么都没有时：空状态的位置写原因和「重试」（data-empty="retry"，由页面接）。 */
export function drawFailure(box, e) {
  drawEmpty(box, {icon: 'image', fact: readError(e), next: [{label: t('重试'), act: 'retry'}]});
}
