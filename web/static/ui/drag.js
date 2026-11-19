// 拖图进画廊（#710）：在页面里按住图拖动，「拿起」一张跟手的缩略图，活页条的二级层换成画廊封面（ui/rail.js 的 mode）。
// 松手在收藏或手选画廊上：放进去，缩略图飞进封面，封面弹一下、冒出 +N；按条件的画廊变淡、不接收；松手在别处：缩略图飞回去。
// 拖出窗口边缘：交给拖到别的应用（#450，ui/files.js 的 startDrag，只有桌面 App 有）。系统开了「减少动态效果」时不飞、不弹。
//
// 接住图的地方：活页条上换出来的那组项（键 'fav'、'g:<id>'、'new'），以及页面里带 data-drop 的元素（全部画廊的卡片写画廊 id，
// 「新建」写 new）。用指针事件自己做，不用 HTML5 拖放：后者在桌面 App 里会和拖到别的应用的系统拖放打架。
//
// 给各视图的接口：
//   wireLift(el, pick, scope)  el 里按住拖动超过 6px 时拿起 pick(按下的元素) 给的那组图（uuid 数组；null 不拖），之后吞掉那次点击。
//                              展示页（ui/gallery_stage.js）、按天视图用它
//   lift(ids, e)               已经判断好要拖的（网格里按住已选中的图拖动）：从指针事件 e 的位置拿起
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {droppable, galleryOf} from '../core/galleries.js';
import {pageOf} from '../core/pages.js';
import {galleryItems} from '../core/rail.js';
import {state} from '../core/state.js';
import {canDrag} from '../core/files.js';
import {cls, css, make, text} from './el.js';
import {startDrag} from './files.js';
import {read, write} from './frame.js';
import {newManual, putInto} from './gallery_actions.js';
import {animate, reduced} from './motion.js';
import {itemEl, mode, railCovers} from './rail.js';

const SLOP = 6, GHOST = 64;
let live = null;   // 正在拖：{ids, ghost, x, y, from, target}

export function wireLift(el, pick, scope) {
  let start = null, dragged = false;
  const on = (type, fn, options) => scope ? scope.on(el, type, fn, options) : el.addEventListener(type, fn, options);
  on('pointerdown', e => {
    dragged = false;
    const ids = e.button === 0 && e.pointerType !== 'touch' ? pick(e.target) : null;
    start = ids?.length ? {x: e.clientX, y: e.clientY, ids} : null;
  });
  on('pointermove', e => {
    if (!start || !(e.buttons & 1) || Math.hypot(e.clientX - start.x, e.clientY - start.y) < SLOP) return;
    const ids = start.ids; start = null; dragged = true;
    lift(ids, e);
  });
  for (const type of ['pointerup', 'pointercancel']) on(type, () => { start = null; });
  on('click', e => { if (dragged) { e.preventDefault(); e.stopImmediatePropagation(); } dragged = false; }, true);
}

// 拿起：缩略图跟着指针，活页条换成画廊封面。拖着的时候一直在窗口上监听指针，松手、取消或拖出窗口时结束。
export function lift(ids, e) {
  if (live || !ids?.length) return;
  document.getSelection?.()?.removeAllRanges();   // 按住拖动时浏览器可能已经开始选文字
  const ghost = make('div', 'lift');
  ghost.setAttribute('aria-hidden', 'true');
  for (const id of ids.slice(0, 3)) { const img = make('img'); img.alt = ''; img.src = `/image?id=${encodeURIComponent(id)}&size=thumb`; ghost.append(img); }
  if (ids.length > 1) ghost.append(make('b', 'lift-count', number(ids.length)));
  document.body.append(ghost);
  live = {ids, ghost, x: e.clientX, y: e.clientY, from: {x: e.clientX, y: e.clientY}, target: null};
  place();
  document.body.classList.add('lifting');
  mode('drop', dropItems(), t('放进画廊'));
  addEventListener('pointermove', onMove);
  addEventListener('pointerup', onUp);
  addEventListener('pointercancel', onCancel);
  addEventListener('mouseout', onOut);
}
// 活页条上换出来的那组：收藏和各画廊（按条件的变淡、不接收），最后一项是新建手选画廊。
function dropItems() {
  const items = galleryItems(railCovers()).map(x => ({...x, off: !droppable(x.gallery)}));
  return [...items, {key: 'new', kind: 'gallery', label: t('新建手选画廊'), cover: null, href: '#/galleries', meta: t('松手新建一个手选画廊，放进这些图')}];
}
function place() {
  const {ghost, x, y} = live;
  write(() => css(ghost, 'transform', `translate(${Math.round(x - GHOST / 2)}px,${Math.round(y - GHOST / 2)}px)`), 'lift:place');
}
// 指针下面接得住图的地方：{key, el}。key 是 'new' 或画廊 id；接不住（按条件的画廊）时是 null。
function targetAt(x, y) {
  const hit = document.elementFromPoint?.(x, y);
  const rail = hit?.closest?.('.rail-item[data-key]');
  if (rail) {
    const key = rail.dataset.key;
    if (key === 'new') return {key: 'new', el: rail};
    const g = rail.railItem?.gallery;
    return g && droppable(g) ? {key: String(g.id), el: rail} : null;
  }
  const box = hit?.closest?.('[data-drop]');
  return box ? {key: box.dataset.drop, el: box} : null;
}
function onMove(e) {
  if (!live) return;
  live.x = e.clientX; live.y = e.clientY;
  place();
  read(() => {
    if (!live) return;
    const next = targetAt(live.x, live.y);
    if (next?.el === live.target?.el) return;
    const before = live.target; live.target = next;
    write(() => { if (before) cls(before.el, 'is-drop', false); if (next) cls(next.el, 'is-drop', true); }, 'lift:over');
  }, 'lift:hit');
}
// 拖出窗口：交给拖到别的应用（只有桌面 App 有；网页形态里拖出去什么也不做，回到窗口里还能接着拖）。
function onOut(e) {
  if (!live || e.relatedTarget || !canDrag()) return;
  const ids = live.ids;
  finish();
  startDrag(ids);
}
function onCancel() { if (live) flyBack(); }
async function onUp(e) {
  if (!live) return;
  live.x = e.clientX; live.y = e.clientY;
  const target = targetAt(live.x, live.y);
  if (!target) { flyBack(); return; }
  const {ids, ghost} = live, n = ids.length;
  detach();
  const g = target.key === 'new' ? null : galleryOf(target.key);
  // 先飞进去、弹一下，同时写入；写入失败时提示原因（write 里），不再另做动画。
  const flown = flyInto(ghost, target.el, n);
  const ok = g ? await putInto(g, {ids}, n) : !!await newManual({ids});
  await flown;
  if (ok) pageOf(state.route).refresh?.('edit');
  mode(null);
}
// 飞进封面，封面弹一下、冒出 +N。
async function flyInto(ghost, el, n) {
  cls(el, 'is-drop', false);
  const r = el.getBoundingClientRect(), x = r.left + r.width / 2 - GHOST / 2, y = r.top + r.height / 2 - GHOST / 2;
  await animate(ghost, [{transform: ghost.style.transform, opacity: 1}, {transform: `translate(${x}px,${y}px) scale(.25)`, opacity: .2}], {dur: 2});
  ghost.remove();
  if (reduced()) return;
  const plus = make('span', 'drop-plus');
  text(plus, `+${number(n)}`);
  el.append(plus);
  await Promise.all([
    animate(el, [{transform: 'scale(1)'}, {transform: 'scale(1.16)'}, {transform: 'scale(1)'}], {dur: 3, ease: 'spring'}),
    animate(plus, [{opacity: 0, transform: 'translateY(4px)'}, {opacity: 1, transform: 'translateY(-10px)', offset: .3}, {opacity: 0, transform: 'translateY(-22px)'}], {dur: 3}),
  ]);
  plus.remove();
}
// 没放进去：飞回拿起的地方，活页条换回来。
async function flyBack() {
  const {ghost, from} = live;
  detach();
  mode(null);
  await animate(ghost, [{transform: ghost.style.transform, opacity: 1}, {transform: `translate(${from.x - GHOST / 2}px,${from.y - GHOST / 2}px) scale(.6)`, opacity: 0}], {dur: 1});
  ghost.remove();
}
// 不再跟着指针（拖动结束），缩略图还在，由调用方收尾。
function detach() {
  if (!live) return;
  if (live.target) cls(live.target.el, 'is-drop', false);
  live = null;
  document.body.classList.remove('lifting');
  removeEventListener('pointermove', onMove);
  removeEventListener('pointerup', onUp);
  removeEventListener('pointercancel', onCancel);
  removeEventListener('mouseout', onOut);
}
// 立刻结束：缩略图摘掉，活页条换回来。
function finish() {
  const ghost = live?.ghost;
  detach();
  ghost?.remove();
  mode(null);
}
// 有没有正在拖（测试和走查用）。
export const lifting = () => !!live;
