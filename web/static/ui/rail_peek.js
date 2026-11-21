// 活页条的悬停预览（#706）：悬停或键盘焦点到二级项时，旁边滑出一张玻璃卡，上面一行图、下面是标题和张数。
// 一行图怎么排在 core/rail.js 的 peekRow（等高、按宽高比放到放不下，core/layout.js 的 strip）；这里只画和动。
// 卡片是 index.html 里的 #railPeek（画在二级层外面，position: fixed，不被滚动区裁掉），建一次，之后只换内容；图的节点按位置复用。
//
// 给 ui/rail.js 的接口：
//   buildPeek(scope)        启动时调一次（活页条接上以后）
//   hover(el, {now, dwell}) 悬停或焦点到了一项（el 是 .rail-item，带 railItem）；null 是离开：稍等 PEEK_LINGER 再收，now 时立刻收。
//                           dwell（指针悬停时传）：卡片还没出来时要在这一项上停够 PEEK_DWELL 才出，从活页条上划过去不出；
//                           卡片已经出来（含离开后还没收起）时换到别的项直接滑过去，不再等。键盘焦点不传，立刻出
//                           二级层可见的内容一变（换入口、换模式、收起抽屉、项被摘掉）也要 hover(null, {now: true})
//   followScroll()          二级层滚动了：键盘焦点就在悬停的那一项上（Tab 走到头时二级层跟着滚）就把卡片挪到它的新位置，
//                           这一项滚出了二级层或是指针悬停时（滚轮）收起
//   hoveredItem()           现在悬停的项（没有是 null）
//
// 两种帧任务，各用一个令牌（buildPeek 里在活页条的作用域上取，#751；同一帧里同一个令牌只跑最后一个，混用会互相顶掉）：
//   keys.show    换了内容：量、写位置，卡片还没进过场就放进场（卡片从活页条那边滑出、图一张接一张发出来），
//                已经开着就沿 translate 滑到新位置、图淡入。要不要滑，写的时候按「卡片是不是已经进过场」定。
//   keys.place   只挪位置（滚动）：量、写 translate，不动内容，不放动画。
// 量和写都是延后的，到时悬停的已经不是这一项就不写（不然收起的预览会被它又显示出来）。
// 颜色：这一项的主色经 ui/tint.js 的 setTint 写到卡片上，变成彩色投影和很淡的环境光（css/rail.css）。
// 宽高比：项里没带原图宽高的图（角色、画廊封面），读到以后按 id 记下比例，重排这一行；下一次悬停时就当作已知的。
import {PEEK, peekRow} from '../core/rail.js';
import {attr, cls, css, make, show, text, tpl} from './el.js';
import {cancelExit, enter, exit} from './motion.js';
import {setTint} from './tint.js';
import {$} from './dom.js';

const CARD = tpl`<div class="rail-peek-in"><span class="rail-peek-pics" data-ref="pics"></span><span class="rail-peek-cap"><b data-ref="title"></b><small
  data-ref="meta"></small></span></div>`;
const IN = [{opacity: 0, transform: 'translateX(-10px) scale(.96)'}, {opacity: 1, transform: 'none'}];
const OUT = [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'translateX(-6px) scale(.98)'}];
const DEAL = [{opacity: 0, transform: 'translateX(-10px) scale(.92)'}, {opacity: 1, transform: 'none'}];
// 指针离开以后过多久收起（毫秒）：划过两项之间的空隙时不闪。
export const PEEK_LINGER = 120;
// 停留判定（毫秒）：指针在一项上停这么久才出第一张卡，只是从活页条上划过去不出（用户 2026-11-20 定）。
export const PEEK_DWELL = 300;
const thumb = id => `/image?id=${encodeURIComponent(id)}&size=thumb`;

let s = null, card = null, keys = null;   // keys：两种帧任务的令牌 {show, place}
let hovered = null;   // 悬停（或焦点所在）的项
let on = false;       // 卡片该显示（收起时置假）
let entered = false;  // 卡片已经放过进场、正显示着：再换项时滑过去，不重新弹出
let hideAt = 0, timer = 0;      // 离开以后：到 hideAt 收起
let dwellAt = 0, dwellTimer = 0; // 停留判定：到 dwellAt 还停在同一项上就出卡片
const ratios = new Map();   // 图片 id → 读到的宽高比

export function buildPeek(scope) {
  s = scope;
  keys = {show: s.key('rail:peek-show'), place: s.key('rail:peek-place')};
  const {el, refs} = CARD();
  $('railPeek').replaceChildren(el);
  card = {refs, imgs: [], ids: []};
}
export const hoveredItem = () => hovered;

// 图的节点按位置复用；第 i 个节点现在放的是哪张图记在 card.ids[i]，读到尺寸时按它记比例。
function img(i) {
  if (!card.imgs[i]) {
    const x = make('img'); x.alt = ''; x.decoding = 'async'; x.draggable = false;
    s.on(x, 'load', () => learn(i));
    card.refs.pics.append(x); card.imgs[i] = x;
  }
  return card.imgs[i];
}
function learn(i) {
  const x = card.imgs[i], id = card.ids[i];
  if (!id || ratios.has(id) || !x.naturalWidth || !x.naturalHeight) return;
  ratios.set(id, x.naturalWidth / x.naturalHeight);
  if (hovered) fillPics(hovered.railItem);   // 按读到的比例重排这一行（宽度变了，行高和位置不变）
}
function fillPics(item) {
  const row = peekRow(item, id => ratios.get(id) || 0), {refs, imgs} = card;
  show(refs.pics, row.pics.length > 0); cls(refs.pics, 'is-round', !!item.peekRound);
  row.pics.forEach((p, i) => {
    const im = img(i);
    card.ids[i] = p.id;
    css(im, 'width', `${p.w}px`); css(im, 'height', `${row.h}px`); attr(im, 'src', thumb(p.id)); show(im, true);
    if (im.complete) learn(i);   // 已经读到过（src 没变不会再来 load）
  });
  imgs.slice(row.pics.length).forEach((im, i) => { show(im, false); card.ids[row.pics.length + i] = ''; });
  return row.pics.length;
}

// 位置：左边压进二级层 PEEK.tuck（抽屉打开时贴着抽屉的右边，不压进去），上下以这一项的中线为准、夹在窗口里。
// 这一项不在二级层的滚动区（#railScroll）看得见的范围里时返回 null。
function measure(el) {
  const peek = $('railPeek'), r = el.getBoundingClientRect(), box = $('railScroll').getBoundingClientRect(), h = peek.offsetHeight;
  if (r.bottom < box.top || r.top > box.bottom) return null;
  const drawer = $('rail').classList.contains('is-drawer');
  const y = Math.max(PEEK.edge, Math.min(innerHeight - h - PEEK.edge, r.top + r.height / 2 - h / 2));
  return {x: Math.round(box.right + (drawer ? PEEK.gap : -PEEK.tuck)), y: Math.round(y)};
}
const place = (peek, at) => css(peek, 'translate', `${at.x}px ${at.y}px`);

export function hover(el, {now = false, dwell = false} = {}) {
  el = el && !el.inert && el.railItem ? el : null;
  if (hovered === el) {
    if (!el && now) hide(true);                   // 已经离开、正等着收起时，要立刻收的照样立刻收
    else if (el && !dwell && dwellAt) { dwellAt = 0; showPeek(el); }   // 指针停着时焦点也到了这一项：不再等
    return;
  }
  if (hovered) for (const x of [hovered, hovered.previousElementSibling, hovered.nextElementSibling]) x?.classList.remove('is-hover', 'is-near');
  hovered = el;
  if (!el) { dwellAt = 0; hide(now); return; }
  hideAt = 0;
  el.classList.add('is-hover');
  el.previousElementSibling?.classList.add('is-near'); el.nextElementSibling?.classList.add('is-near');
  if (dwell && !on) { waitDwell(); return; }   // 卡片还没出来：等停留判定
  dwellAt = 0;
  showPeek(el);
}
// 停留判定：换到别的项就从那时起重新算。和离开的计时器一样只排一个，到点时没到期就再等剩下的。
function waitDwell() {
  dwellAt = Date.now() + PEEK_DWELL;
  if (!dwellTimer) armDwell(PEEK_DWELL);
}
function armDwell(ms) {
  dwellTimer = s.timeout(() => {
    dwellTimer = 0;
    if (!dwellAt || !hovered) return;
    const left = dwellAt - Date.now();
    if (left > 1) { armDwell(left); return; }
    dwellAt = 0; showPeek(hovered);
  }, ms);
}
function showPeek(el) {
  if (!card) return;
  const item = el.railItem, peek = $('railPeek'), {refs} = card;
  text(refs.title, item.label); text(refs.meta, item.meta || ''); show(refs.meta, !!item.meta);
  const n = fillPics(item);
  setTint(peek, item.color);
  if (!on) { cancelExit(peek); entered = false; }
  on = true;
  peek.hidden = false;   // 要量它的高；位置和进场在下一帧画之前写好
  s.read(() => {
    if (hovered !== el) return;
    const at = measure(el);
    if (!at) { hover(null, {now: true}); return; }
    s.write(() => {
      if (hovered !== el) return;
      const pics = card.imgs.slice(0, n);
      cls(peek, 'is-glide', entered); place(peek, at);
      if (entered) { pics.forEach(im => enter(im, {from: 'fade', dur: 1})); return; }
      entered = true;
      enter(peek, {from: IN, dur: 2});
      pics.forEach((im, i) => enter(im, {from: DEAL, dur: 2, delay: 40 + i * 35}));
    }, keys.show);
  }, keys.show);
}
export function followScroll() {
  const el = hovered;
  if (!el || el !== document.activeElement) { hover(null, {now: true}); return; }
  s.read(() => {
    if (hovered !== el || !on) return;
    const at = measure(el);
    if (!at) { hover(null, {now: true}); return; }
    s.write(() => { if (hovered === el && on) place($('railPeek'), at); }, keys.place);
  }, keys.place);
}

// 收起。不立刻收时等 PEEK_LINGER：期间回到某一项就不收；又离开就从那时起重新算（只排一个计时器，到点时没到期就再等剩下的，
// 不在 app 作用域上越积越多）。
function hide(now) {
  if (now) { hideAt = 0; close(); return; }
  hideAt = Date.now() + PEEK_LINGER;
  if (!timer) arm(PEEK_LINGER);
}
function arm(ms) {
  timer = s.timeout(() => {
    timer = 0;
    if (hovered || !hideAt) return;
    const left = hideAt - Date.now();
    if (left > 1) { arm(left); return; }
    hideAt = 0; close();
  }, ms);
}
function close() {
  if (!on) return;
  on = false; entered = false;
  exit($('railPeek'), {to: OUT, remove: false});
}
