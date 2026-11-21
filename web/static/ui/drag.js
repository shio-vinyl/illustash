// 拖图进画廊（#710）：在页面里按住图拖动，「拿起」一张跟手的缩略图，活页条的二级层换成画廊封面（ui/rail.js 的 mode）。
// 松手在收藏或手选画廊上：放进去，缩略图飞进封面，封面弹一下、冒出 +N；按条件的画廊和这一页自己的画廊变淡、不接收；松手在
// 「新建手选画廊」上：先问名字，取消就什么也不建；松手在别处：缩略图飞回去。系统开了「减少动态效果」时不飞、不弹。
// 取消：Esc（拿起时向浮层管理器 ui/overlay.js 登记一层，Esc 不落到网格的兜底）、窗口失焦、收不到松手（指针移动时左键已经
// 不在按着）。
// 拖出窗口：指针坐标出了窗口（按住鼠标时窗口外的 pointermove 照样会来），或者窗口收到 relatedTarget 为空的 mouseout，就交给
// 拖到别的应用（#450，ui/files.js 的 startDrag，只有桌面 App 有；网页形态里拖出去什么也不做，回到窗口里还能接着拖）。
// 窄窗口（活页条收进抽屉）时二级层不在，拿起以后只能拖出窗口；放进画廊用选择栏的「放进画廊…」。
//
// 用指针事件自己做，不用 HTML5 拖放：后者在桌面 App 里会和拖到别的应用的系统拖放打架。接住图的只有活页条上换出来的那组项
// （键 'fav'、'g:<id>'、'new'）。
//
// 给各视图的接口（和 ui/files.js 的 wireDragOut 同一个写法，手势都在 ui/press.js）：
//   wireLift(el, ids, when)  el 里按住拖动超过 6px 时拿起 ids(down) 给的那组图（uuid 数组，空的不拖），之后吞掉那次点击。down 是按下时的
//                            pointerdown 事件：在容器上委托时用 down.target.closest(…) 找按的是哪张；when(down) 为假时不拖。返回 off()，
//                            交给作用域（scope.add(wireLift(…))）。展示页（ui/gallery_stage.js）、按天视图、详情卡片的预览图用它
//   lift(ids, e)             已经判断好要拖的（网格里按住已选中的图拖动）：从指针事件 e 的位置拿起
// 看图器里不拿起（它盖住活页条，没处放），照旧只拖到别的应用（ui/viewer.js 的 wireDragOut）。
import {imageUrl, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {droppable, galleryOf} from '../core/galleries.js';
import {pageOf} from '../core/pages.js';
import {galleryCovers, galleryItems} from '../core/rail.js';
import {state} from '../core/state.js';
import {canDrag} from '../core/files.js';
import {cls, css, make, text} from './el.js';
import {startDrag} from './files.js';
import {key, read, write} from './frame.js';
import {newManual, putInto} from './gallery_actions.js';
import {animate, reduced} from './motion.js';
import {createLayer} from './overlay.js';
import {handedOff, onPress, wirePress} from './press.js';
import {mode} from './rail.js';

// 网格的按下也要 mousedown 补位（ui/press.js），从这里转出，grid.js 不另导入一个模块。
export {onPress};
const GHOST = 64, OFFSET = 14;
// 帧任务的令牌（#751）：同一时间只拖一次，在模块顶层取。
const PLACE = key('lift:place'), HIT = key('lift:hit'), OVER = key('lift:over');
let live = null;   // 正在拖：{ids, ghost, x, y, from, target}
// Esc 取消拖动：拿起时打开这一层，结束时关掉。
const escape = createLayer({kind: 'overlay', id: 'lift', el: () => live?.ghost, onDismiss: () => { if (live) flyBack(true); }});

export function wireLift(el, ids, when = () => true) {
  return wirePress(el, {when, start: (down, move) => {
    const got = ids(down);
    if (!got?.length) return false;
    lift(got, move);
    return true;
  }});
}

// 拿起：缩略图跟着指针，活页条换成画廊封面。拖着的时候一直在窗口上监听指针，松手、取消或拖出窗口时结束。
export function lift(ids, e) {
  if (live || !ids?.length) return;
  document.getSelection?.()?.removeAllRanges();   // 按住拖动时浏览器可能已经开始选文字
  const ghost = make('div', 'lift');
  ghost.setAttribute('aria-hidden', 'true');
  for (const id of ids.slice(0, 3)) { const img = make('img'); img.alt = ''; img.src = imageUrl(id); ghost.append(img); }
  if (ids.length > 1) ghost.append(make('b', 'drop-badge', number(ids.length)));
  document.body.append(ghost);
  live = {ids, ghost, x: e.clientX, y: e.clientY, from: {x: e.clientX, y: e.clientY}, target: null, focus: document.activeElement};
  place();
  document.body.classList.add('lifting');
  mode('drop', dropItems(), t('放进画廊'));
  escape.open();
  for (const [type, fn] of Object.entries(LISTENERS)) addEventListener(type, fn);
}
// 这一页自己是哪个画廊（网格页、展示页）：拖到它上面不算。
const hereId = () => { const p = pageOf(state.route); return p.gallery || p.stage ? String(state.route.id) : ''; };
const accepts = g => !!g && droppable(g) && String(g.id) !== hereId();
// 活页条上换出来的那组：收藏和各画廊（按条件的、这一页自己的变淡、不接收），最后一项是新建手选画廊。
function dropItems() {
  const items = galleryItems(galleryCovers.byId).map(x => ({...x, off: !accepts(x.gallery)}));
  return [...items, {key: 'new', kind: 'gallery', label: t('新建手选画廊'), cover: null, href: '#/galleries', meta: t('松手新建一个手选画廊，放进这些图')}];
}
function place() {
  const {ghost, x, y} = live;
  // 缩略图在指针右下方一点，不挡住指针下面要放进去的那一项。
  write(() => css(ghost, 'transform', `translate(${Math.round(x + OFFSET)}px,${Math.round(y + OFFSET)}px)`), PLACE);
}
// 指针下面接得住图的地方：{key, el}。key 是 'new' 或画廊 id；接不住（按条件的画廊）时是 null。
function targetAt(x, y) {
  const hit = document.elementFromPoint?.(x, y);
  const rail = hit?.closest?.('.rail-item[data-key]');
  if (rail) {
    const key = rail.dataset.key;
    if (key === 'new') return {key: 'new', el: rail};
    const g = rail.railItem?.gallery;
    return accepts(g) ? {key: String(g.id), el: rail} : null;
  }
  return null;
}
// 指针出了窗口（按住鼠标时窗口外的移动照样派过来，坐标在窗口外）。
const outside = e => e.clientX < 0 || e.clientY < 0 || e.clientX >= innerWidth || e.clientY >= innerHeight;
function onMove(e) {
  if (!live) return;
  if (!(e.buttons & 1)) { flyBack(); return; }   // 没收到松手（例如切走以后才松开）：当取消
  if (outside(e)) { handOff(); return; }
  live.x = e.clientX; live.y = e.clientY;
  place();
  read(() => {
    if (!live) return;
    const next = targetAt(live.x, live.y);
    if (next?.el === live.target?.el) return;
    const before = live.target; live.target = next;
    write(() => { if (before) cls(before.el, 'is-drop', false); if (next) cls(next.el, 'is-drop', true); }, OVER);
  }, HIT);
}
// 拖出窗口：交给拖到别的应用（只有桌面 App 有；网页形态里什么也不做，回到窗口里还能接着拖）。
function handOff() {
  if (!live || !canDrag()) return;
  const ids = live.ids;
  finish();
  handedOff();   // 松手被系统拖放接走：清选区、页面暂时不可选，下一次按下由 mousedown 补上（ui/press.js）
  startDrag(ids);
}
function onOut(e) { if (!e.relatedTarget) handOff(); }
function onCancel() { if (live) flyBack(); }
const LISTENERS = {pointermove: onMove, pointerup: onUp, pointercancel: onCancel, mouseout: onOut, blur: onCancel};
async function onUp(e) {
  if (!live) return;
  live.x = e.clientX; live.y = e.clientY;
  const target = targetAt(live.x, live.y);
  if (!target) { flyBack(); return; }
  const {ids, ghost} = live, n = ids.length;
  // 新建：先问名字（取消就什么也不建），缩略图和活页条先收起来。
  if (target.key === 'new') {
    finish();
    if (await newManual({ids})) pageOf(state.route).refresh?.('edit');
    return;
  }
  detach();
  // 先飞进去、弹一下，同时写入；写入失败时提示原因（写入那里），不再另做动画。
  const flown = flyInto(ghost, target.el, n);
  const ok = await putInto(galleryOf(target.key), {ids}, n);
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
  const plus = make('span', 'drop-badge');
  text(plus, `+${number(n)}`);
  el.append(plus);
  await Promise.all([
    animate(el, [{transform: 'scale(1)'}, {transform: 'scale(1.16)'}, {transform: 'scale(1)'}], {dur: 3, ease: 'spring'}),
    animate(plus, [{opacity: 0, transform: 'translateY(4px)'}, {opacity: 1, transform: 'translateY(-10px)', offset: .3}, {opacity: 0, transform: 'translateY(-22px)'}], {dur: 3}),
  ]);
  plus.remove();
}
// 没放进去：飞回拿起的地方，活页条换回来。
async function flyBack(held = false) {
  const {ghost, from} = live;
  detach(held);
  mode(null);
  await animate(ghost, [{transform: ghost.style.transform, opacity: 1}, {transform: `translate(${from.x + OFFSET}px,${from.y + OFFSET}px) scale(.6)`, opacity: 0}], {dur: 1});
  ghost.remove();
}
// 不再跟着指针（拖动结束），缩略图还在，由调用方收尾。
// held：按着键取消（Esc）。指针可能正停在输入框上，输入框等松手以后才重新接指针，焦点也等松手以后再看（#716 macOS 14 走查）。
function detach(held = false) {
  if (!live) return;
  if (live.target) cls(live.target.el, 'is-drop', false);
  if (held) holdInputs(live.focus); else keepFocus(live.focus);
  live = null;
  document.body.classList.remove('lifting');
  escape.close('silent');
  for (const [type, fn] of Object.entries(LISTENERS)) removeEventListener(type, fn);
}
// 拖动期间松手不该把焦点落进输入框（在搜索框上松手，下一个按键就打进去了）：拖着时输入框不接指针（css/galleries.css 的
// body.lifting），松手以后焦点要是进了一个拿起时不在的输入框，就让它失焦。
const TYPING = 'input,textarea,select,[contenteditable]';
function keepFocus(was) {
  setTimeout(() => { const now = document.activeElement; if (now && now !== was && now.matches?.(TYPING)) now.blur(); }, 0);
}
function holdInputs(was) {
  document.body.classList.add('settling');
  let timer = 0;
  const done = () => {
    clearTimeout(timer); document.body.classList.remove('settling');
    for (const type of ['pointerup', 'mouseup']) removeEventListener(type, done, true);
    keepFocus(was);
  };
  for (const type of ['pointerup', 'mouseup']) addEventListener(type, done, true);
  timer = setTimeout(done, 10000);   // 松手的事件没来（窗口外松手）也不一直挡着
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
