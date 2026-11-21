// 整理页（#713）共用的部件：浅底分组（css/base.css 的 .group）、一排方图、收起动效、看大图的列表，以及各页共用的底栏清理和
// 读失败时的「重试」。样式在 css/tidy.css。按组整理的一组、身份整理的一对用 tidyGroup()：组头一行写标题、张数和一句话，操作
// 收在组头右边；按组整理下面是一排可以点掉的方图（共用的判断卡 ui/judge.js，#787），身份整理把方图换成左右两边的样图。
// 画师抽查的一张图只借用 .group.tidy-group 的样子。
//
//   const g = tidyGroup();                      // {el, refs: {title, meta, q, acts, strip, note}}
//   const strip = thumbStrip(g.refs.strip, {scope, max: 6});
//   strip.update(ids);                          // 只看的方图（身份整理）：按 uuid 复用，点一下看大图
//   setPeekList(g.el, () => ids);               // 长按或 Space 看大图时翻这一组（ui/viewer.js 的 wirePeek，见 wireTidyPeek）
//
// 一组处理完：先 leaving(el) 记下它，再从列表里拿掉；ui/keyed.js 的 exit 钩子 collapse(el) 只对记下的那组播收起——向右淡出，
// 视口附近、在它下面的组从原来的位置滑上来（FLIP，只动 transform 和 opacity），其余直接到位；没记下的（重新分组后整表换掉）
// 直接摘掉。系统开了「减少动态效果」时组直接消失。
import {imageUrl} from '../core/format.js';
import {t} from '../core/i18n.js';
import {readError} from '../core/session.js';
import {$} from './dom.js';
import {attr, make, tpl} from './el.js';
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
const THUMB = tpl`<button type="button" class="tidy-thumb"><img alt="" decoding="async" loading="lazy" draggable="false"></button>`;

/** 一个空的分组，内容由各页填。 */
export const tidyGroup = () => GROUP();

/** 一排只看的方图（身份整理）：ids 是 uuid，只画前 max 张；点一下看大图。要点掉的方图用 ui/judge.js 的判断卡。 */
export function thumbStrip(box, {scope, max = 16} = {}) {
  // keyed 管 thumbs 里的全部子节点（.tidy-thumbs 是 display: contents，方图排在 box 的网格里）。
  const thumbs = make('span', 'tidy-thumbs');
  box.replaceChildren(thumbs);
  const list = keyed(thumbs, {key: x => x, scope, create: id => {
    const {el} = THUMB(), img = el.firstElementChild;
    attr(el, 'data-peek', id); attr(el, 'title', t('看大图'));
    attr(img, 'src', imageUrl(id, 'thumb'));
    return {el, update() {}};
  }});
  return {update(ids) { list.set(ids.slice(0, max)); }};
}

// 各组看大图时翻哪些图：节点 → 返回 uuid 列表的函数。
const peekLists = new WeakMap();
export const setPeekList = (el, list) => peekLists.set(el, list);
function peekList(el) {
  for (let node = el; node; node = node.parentElement) { const f = peekLists.get(node); if (f) return f(el); }
  return [];
}
/** 大图关掉时把最后看的那张缩略图滚进视野（整理各页登记的 reveal，主区都是 #groupsPage）。 */
export function revealThumb(uuid) { [...$('groupsPage').querySelectorAll('[data-peek]')].find(x => x.dataset.peek === uuid)?.scrollIntoView({block: 'nearest'}); }
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
