// 动效（#705，0.4 第 A2 步）：界面上所有用脚本做的动画都从这里走，el.animate、startViewTransition 只在这个文件里调
// （tools/check_frontend.py 拦）。CSS 过渡照旧写在样式里，用 var(--dur-N)、var(--ease) 两组令牌。
//
// 怎么用：
//   import {enter, exit, flip, share, shareBack, countTo, thumbs} from './motion.js';
//   await enter(card);                          // 进场：淡入并上移一点，--dur-2
//   await exit(card);                           // 出场：比进场短（--dur-1），结束后把节点摘掉；{remove: false} 改成藏起来
//   flip(el, oldBox, newBox);                   // 位置变了：用 core/layout.js 算出的框（{x, y, w, h}），不量 DOM
//   const off = thumbs.add(uuid, img);          // 各视图登记缩略图，节点换图或摘掉时调 off()
//   await share({from: thumbs.find(uuid), to: bigImg, update: () => showViewer()});  // 缩略图展开成大图
//   await shareBack(bigImg, thumbs.rect(uuid)); // 关掉时缩回缩略图的位置；没有位置（滚走了）就淡出
//   countTo(numberEl, 1234);                    // 数字滚到新值
//
// 时长传 1 / 2 / 3（对应 --dur-1 / -2 / -3），也可以直接传毫秒；缓动传 'ease' 或 'spring'（--ease-spring，只给活页条标签和
// 「弹一下」这类强调动作）。只动 transform 和 opacity（共享元素过渡另动 clip-path）。
//
// 约定：
// - 系统开了「减少动态效果」时一律直接到终态：Web Animations 的时长变成 0（fill 照旧，所以终态和正常播放的终态一样），
//   数字直接写成新值，不用 View Transitions。app.css 的总开关只管 CSS 过渡和动画，管不到 el.animate，所以在这里判断。
// - 结束用 animation.finished，另按时长加计时兜底：动画被别处取消、页面在后台、或者 finished 一直不来（Safari 17 的
//   transitionend 偶尔不来，Web Animations 也按同样的思路兜底）时，最晚在时长之后 FALLBACK_MS 结束，返回的 Promise 不会挂住。
//   Promise 的值：正常结束或兜底结束是 true，被取消是 false。
// - View Transitions 只作增强（Safari 18.0 才有）：没有 startViewTransition 时用 Web Animations 做 FLIP，效果相同。

const DEFAULT_MS = {1: 140, 2: 240, 3: 380};
const DEFAULT_EASE = {ease: 'cubic-bezier(.2,.8,.2,1)', spring: 'cubic-bezier(.34,1.42,.5,1)'};
// 计时兜底比时长多等的毫秒数。
export const FALLBACK_MS = 120;

// ---- 令牌与「减少动态效果」 ----
let query;
/** 系统是否开了「减少动态效果」。每次调用都读当前值，设置改了立刻生效。 */
export function reduced() {
  if (query === undefined) query = typeof matchMedia === 'function' ? matchMedia('(prefers-reduced-motion: reduce)') : null;
  return Boolean(query?.matches);
}

const cache = new Map();
function token(name) {
  if (cache.has(name)) return cache.get(name);
  let value = '';
  try {
    value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  } catch {
    value = '';
  }
  // 「减少动态效果」打开时令牌里的时长是 0，不缓存，免得设置关掉以后还是 0。
  if (value && !/^0m?s$/.test(value)) cache.set(name, value);
  return value;
}
function milliseconds(text) {
  const m = /^(-?[\d.]+)(ms|s)$/.exec(text);
  return m ? Number(m[1]) * (m[2] === 's' ? 1000 : 1) : NaN;
}
/** 时长（毫秒）：1 / 2 / 3 取 --dur-N 令牌，数字原样返回。「减少动态效果」打开时是 0。 */
export function duration(level = 2) {
  if (reduced()) return 0;
  if (!(level in DEFAULT_MS)) return Math.max(0, Number(level) || 0);
  const ms = milliseconds(token(`--dur-${level}`));
  return Number.isFinite(ms) && ms > 0 ? ms : DEFAULT_MS[level];
}
/** 缓动：'ease' 取 --ease，'spring' 取 --ease-spring；别的字符串原样返回（比如 'linear' 给进度这类匀速的东西）。 */
export function easing(name = 'ease') {
  if (!(name in DEFAULT_EASE)) return name;
  return token(name === 'spring' ? '--ease-spring' : '--ease') || DEFAULT_EASE[name];
}

// ---- 结束与兜底 ----
/**
 * 等 `animation` 结束，最晚 `ms + FALLBACK_MS` 毫秒；兜底时把动画推到终点。
 * @returns {Promise<boolean>} 正常或兜底结束为 true，被取消为 false
 */
export function settle(animation, ms) {
  return new Promise(resolve => {
    let done = false;
    const finish = ok => { if (!done) { done = true; clearTimeout(timer); resolve(ok); } };
    const timer = setTimeout(() => {
      try { animation.finish?.(); } catch { /* 无限循环的动画不能 finish，照样结束等待 */ }
      finish(true);
    }, Math.max(0, ms) + FALLBACK_MS);
    Promise.resolve(animation.finished).then(() => finish(true), () => finish(false));
  });
}

/**
 * Web Animations 的唯一入口。
 * @param {Element} el
 * @param {Keyframe[]} keyframes
 * @param {{dur?: number, ease?: string, delay?: number, fill?: FillMode}} [options] dur 是 1 / 2 / 3 或毫秒
 * @returns {Promise<boolean>}
 */
export function animate(el, keyframes, {dur = 2, ease = 'ease', delay = 0, fill = 'none'} = {}) {
  const off = reduced();
  const ms = off ? 0 : duration(dur);
  const wait = off ? 0 : Math.max(0, delay);
  if (typeof el?.animate !== 'function') return Promise.resolve(true);
  const animation = el.animate(keyframes, {duration: ms, delay: wait, easing: easing(ease), fill});
  return settle(animation, ms + wait);
}

// ---- 进场与出场 ----
const ENTER = {
  rise: [{opacity: 0, transform: 'translateY(8px)'}, {opacity: 1, transform: 'none'}],
  fade: [{opacity: 0}, {opacity: 1}],
  scale: [{opacity: 0, transform: 'scale(.96)'}, {opacity: 1, transform: 'none'}],
};
const EXIT = {
  rise: [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'translateY(4px)'}],
  fade: [{opacity: 1}, {opacity: 0}],
  scale: [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'scale(.97)'}],
};
/** 进场：节点已经在页面上、是它最终的样子，从淡、低一点的位置过渡过来。from：'rise'（默认）、'fade'、'scale' 或自己的关键帧。 */
export function enter(el, {from = 'rise', dur = 2, delay = 0, ease = 'ease'} = {}) {
  return animate(el, Array.isArray(from) ? from : ENTER[from] || ENTER.rise, {dur, delay, ease});
}
/**
 * 出场，比进场短。结束后默认把节点摘掉；remove: false 时改成 hidden = true（藏起来以后再撤掉动画，不会闪一下）。
 * 出场途中又要进场（同一个节点），先 cancelExit(el)。
 */
export async function exit(el, {to = 'rise', dur = 1, delay = 0, remove = true} = {}) {
  const keyframes = Array.isArray(to) ? to : EXIT[to] || EXIT.rise;
  const off = reduced();
  if (typeof el?.animate !== 'function') { finishExit(el, remove); return true; }
  const ms = off ? 0 : duration(dur);
  const animation = el.animate(keyframes, {duration: ms, delay: off ? 0 : delay, easing: easing('ease'), fill: 'forwards'});
  exiting.set(el, animation);
  const ok = await settle(animation, ms + (off ? 0 : delay));
  if (exiting.get(el) !== animation) return false;
  exiting.delete(el);
  if (!ok) return false;
  finishExit(el, remove);
  animation.cancel();
  return true;
}
const exiting = new WeakMap();
function finishExit(el, remove) {
  if (remove) el?.remove?.();
  else if (el) el.hidden = true;
}
/** 取消正在进行的出场（节点留在原处，回到自己的样子）。 */
export function cancelExit(el) {
  const animation = exiting.get(el);
  if (!animation) return;
  exiting.delete(el);
  animation.cancel();
}

// ---- FLIP ----
function box(r) {
  if (!r) return null;
  const x = r.x ?? r.left, y = r.y ?? r.top, w = r.w ?? r.width, h = r.h ?? r.height;
  return [x, y, w, h].every(Number.isFinite) ? {x, y, w, h} : null;
}
/**
 * FLIP 的起点：节点已经画在 `to`，要看起来从 `from` 出发，需要的平移和缩放（变换原点在左上角）。
 * 框是 {x, y, w, h}（也接受 DOMRect 的 left/top/width/height）。宽或高为 0 时那个方向不缩放。
 */
export function flipDelta(from, to) {
  const a = box(from), b = box(to);
  if (!a || !b) return {x: 0, y: 0, sx: 1, sy: 1};
  return {x: a.x - b.x, y: a.y - b.y, sx: b.w > 0 ? a.w / b.w : 1, sy: b.h > 0 ? a.h / b.h : 1};
}
function transform({x, y, sx, sy}) {
  return `translate(${round(x)}px, ${round(y)}px) scale(${round(sx, 4)}, ${round(sy, 4)})`;
}
function round(n, places = 2) {
  const f = 10 ** places;
  return Math.round(n * f) / f;
}
/** FLIP 的关键帧：从 `from` 回到节点自己的位置 `to`。起止位置不变时返回 null。 */
export function flipFrames(from, to) {
  const d = flipDelta(from, to);
  if (d.x === 0 && d.y === 0 && d.sx === 1 && d.sy === 1) return null;
  return [{transformOrigin: '0 0', transform: transform(d)}, {transformOrigin: '0 0', transform: 'none'}];
}
/**
 * 位置或大小变了的节点从旧框滑到新框。框用 core/layout.js 算出的，不量 DOM。
 * @returns {Promise<boolean>}
 */
export function flip(el, from, to, {dur = 2, ease = 'ease', delay = 0} = {}) {
  const frames = flipFrames(from, to);
  return frames ? animate(el, frames, {dur, ease, delay}) : Promise.resolve(true);
}

// ---- 共享元素：缩略图到大图 ----
/**
 * 共享元素过渡的关键帧：节点最终在 `to`（比如看图器里按比例放下的大图），开始时看起来正好是 `from`（网格里裁成方块的缩略图）。
 * 等比放大到盖满 `from`，再用 clip-path 裁出 `from` 那一块，所以图不会被拉变形。radius 是起止两端的圆角（px）。
 */
export function shareFrames(from, to, {radius = [8, 0]} = {}) {
  const a = box(from), b = box(to);
  if (!a || !b || b.w <= 0 || b.h <= 0) return null;
  const s = Math.max(a.w / b.w, a.h / b.h);
  const width = b.w * s, height = b.h * s;
  const x = a.x + a.w / 2 - width / 2 - b.x;
  const y = a.y + a.h / 2 - height / 2 - b.y;
  // 裁切在缩放之前，所以按缩放前的尺寸算：盖满的图左右、上下各多出来的一半。
  const insetX = (width - a.w) / 2 / s, insetY = (height - a.h) / 2 / s;
  const [r0, r1] = radius;
  return [
    {transformOrigin: '0 0', transform: transform({x, y, sx: s, sy: s}),
      clipPath: `inset(${round(insetY)}px ${round(insetX)}px round ${round(r0 / s)}px)`},
    {transformOrigin: '0 0', transform: 'none', clipPath: `inset(0px 0px round ${round(r1)}px)`},
  ];
}
/**
 * 缩略图展开成大图。`from` 是缩略图节点（通常来自 thumbs.find）或它的框；`to` 是大图节点；`update` 把大图画出来
 * （在 View Transitions 里调用，或者在 Web Animations 之前调用）。toBox 不传时量 `to` 的位置。
 * 有 document.startViewTransition、from 是节点、没开「减少动态效果」时用 View Transitions，否则用 Web Animations。
 */
export async function share({from, to, update, toBox, dur = 3, radius} = {}) {
  const node = from && typeof from.getBoundingClientRect === 'function' ? from : null;
  if (node && typeof document !== 'undefined' && typeof document.startViewTransition === 'function' && !reduced() && to?.style) {
    node.style.viewTransitionName = 'share';
    const transition = document.startViewTransition(() => {
      node.style.viewTransitionName = '';
      update?.();
      to.style.viewTransitionName = 'share';
    });
    const ok = await settle(transition, duration(dur));
    to.style.viewTransitionName = '';
    return ok;
  }
  const start = node ? node.getBoundingClientRect() : from;
  update?.();
  const frames = shareFrames(start, toBox || to?.getBoundingClientRect?.(), {radius});
  if (!frames) return enter(to, {from: 'scale', dur: 2});
  return animate(to, frames, {dur});
}
/**
 * 大图缩回缩略图的位置（关看图器时）。`el` 停在终点（fill: forwards），调用方在 Promise 之后把它摘掉。
 * 没有目标框（缩略图滚走了、所在的视图没登记）时淡出。
 */
export function shareBack(el, target, {fromBox, dur = 2, radius = [0, 8]} = {}) {
  const frames = target ? shareFrames(target, fromBox || el?.getBoundingClientRect?.(), {radius: [radius[1], radius[0]]}) : null;
  if (!frames) return animate(el, EXIT.scale, {dur: 1, fill: 'forwards'});
  return animate(el, frames.slice().reverse(), {dur, fill: 'forwards'});
}

// ---- 数字滚动 ----
const counting = new WeakMap();
/**
 * 数字从当前值滚到 `to`。format 把数字变成显示的文字（默认按整数）；`from` 不传时取上一次滚到的值，没有就直接写 `to`。
 * 同一个节点又滚时接着新的目标走。
 * @returns {Promise<boolean>}
 */
export function countTo(el, to, {from, dur = 2, format = n => String(Math.round(n))} = {}) {
  const previous = counting.get(el);
  if (previous) previous.stop();
  const start = from ?? previous?.value ?? lastValue.get(el);
  lastValue.set(el, to);
  const ms = duration(dur);
  const raf = typeof requestAnimationFrame === 'function' ? requestAnimationFrame : null;
  if (!raf || !ms || start === undefined || start === to) {
    el.textContent = format(to);
    return Promise.resolve(true);
  }
  return new Promise(resolve => {
    let stopped = false, began;
    const job = {value: start, stop: () => { stopped = true; resolve(false); }};
    counting.set(el, job);
    const step = now => {
      if (stopped) return;
      began ??= now;
      const t = Math.min(1, (now - began) / ms);
      job.value = start + (to - start) * (1 - (1 - t) ** 3);
      el.textContent = format(t === 1 ? to : job.value);
      if (t < 1) raf(step);
      else { counting.delete(el); resolve(true); }
    };
    raf(step);
    // 后台页面不跑 requestAnimationFrame：到时间仍没写完就直接写终值。
    setTimeout(() => {
      if (stopped || counting.get(el) !== job) return;
      stopped = true; counting.delete(el); el.textContent = format(to); resolve(true);
    }, ms + FALLBACK_MS);
  });
}
const lastValue = new WeakMap();

// ---- 缩略图在屏幕上哪里 ----
/**
 * 「这张图的缩略图在屏幕上哪里」的登记表：各视图（网格、按天、画廊…）把画出来的缩略图节点按图片 id 登记，看图器查询，
 * 所以看图器不用导入各个视图。同一张图在几处都画着时，取最后登记、还在页面上、并且在视口里的那一个。
 * 节点被复用来画另一张图时直接登记新的 id，旧的登记跟着去掉。
 */
export const thumbs = {
  /** 登记；返回撤销登记的函数（交给 ui/scope.js 的作用域，换页时一起释放）。 */
  add(id, el) {
    const old = owner.get(el);
    if (old !== undefined && old !== id) places.get(old)?.delete(el);
    owner.set(el, id);
    const set = places.get(id) || new Set();
    set.delete(el);
    set.add(el);
    places.set(id, set);
    return () => thumbs.remove(id, el);
  },
  remove(id, el) {
    const set = places.get(id);
    if (!set) return;
    set.delete(el);
    if (owner.get(el) === id) owner.delete(el);
    if (!set.size) places.delete(id);
  },
  /** 屏幕上看得见的那个缩略图节点；没有时是 null。 */
  find(id) {
    const set = places.get(id);
    if (!set) return null;
    for (const el of [...set].reverse()) {
      if (el.isConnected === false) { set.delete(el); continue; }
      if (visible(el.getBoundingClientRect())) return el;
    }
    return null;
  },
  /** 那个缩略图在视口里的框（DOMRect）；没有时是 null。 */
  rect(id) {
    return thumbs.find(id)?.getBoundingClientRect() || null;
  },
  /** 测试用：清空登记表。 */
  clear() { places.clear(); },
};
const places = new Map();
const owner = new WeakMap();
function visible(r) {
  if (!r || !(r.width > 0) || !(r.height > 0)) return false;
  const width = globalThis.innerWidth ?? Infinity, height = globalThis.innerHeight ?? Infinity;
  return r.right > 0 && r.bottom > 0 && r.left < width && r.top < height;
}
