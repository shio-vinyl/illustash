// 资料库的角色海报和圆头像卡（#711，接 #293 的海报小查看器）：满版图 + 压暗渐变 + 白字（css/base.css 的 .poster），左上排名数字、右上
// 「+N」（最近 30 天新进的张数；和张数相同时不写，只写张数）、钉到活页条的图钉，图的主色做读图前的底。资料库的角色页签和作品页共用。
//
//   const make = posters(s);                 // s：页面或页签的作用域；返回给 ui/keyed.js 的 create
//   const list = keyed(box, {key: x => x.c.id, create: make, scope: s});
//   list.set([{c: 角色, rank: 1, big: true, sub: '某作品 · 120 张'}, …]);
//
// 海报接近屏幕时（提前 300px）抽一次这个角色的图（core/catalog.js 的 posterPictures，每个角色、每种筛选、每个数据版本一次），
// 读好第一张以后变成小查看器：指针横向移动在抽到的图里挑，纵向移动上下看这张图（看下半部分时名字淡出），两端各有一条进入
// 即到头的带子，右边的小地图显示看到哪里；有焦点时 ← → 翻图、↑ ↓ 上下看；触摸屏上横向拖动翻图（#300）。
// 比海报宽的图整张显示在主色底上（原来垫一张模糊的副本，每张海报一块显存，#683 去掉）。
// 位置的缓动每帧一次，走作用域的帧任务（ui/frame.js），不直接调 requestAnimationFrame。
import {posterPictures, recentOf} from '../core/catalog.js';
import {imageUrl, number} from '../core/format.js';
import {composing} from '../core/keys.js';
import {nameTip} from '../core/labels.js';
import {charHash} from '../core/route.js';
import {state} from '../core/state.js';
import {attr, cls, css, make, show, text, tpl} from './el.js';
import {reduced} from './motion.js';
import {pinButton} from './pin_button.js';
import {setTint} from './tint.js';

// 图钉放在链接外面（按钮不嵌在链接里，读屏时海报的名字不带「钉到活页条」），外面包一层定位。
const POSTER = tpl`<div class="cat-poster-wrap"><a class="poster cat-poster" data-ref="link"><img data-ref="img" alt="" loading="lazy" decoding="async"
  draggable="false"><img class="cat-next" data-ref="next" alt="" decoding="async" draggable="false"><span class="cat-rank" data-ref="rank"></span><span class="cat-plus"
  data-ref="plus"></span><span class="poster-cap" data-ref="cap"><b data-ref="name"></b><small data-ref="sub"></small></span><span class="cat-ticks" data-ref="ticks"></span><span
  class="cat-mini" data-ref="mini" hidden><i data-ref="thumb"></i></span></a></div>`;
const EDGE = .14;   // 顶部和底部那条进入即到头的带子
const STEP = 36;    // 触摸时横向拖多少 px 翻一张
const clamp = x => Math.min(1, Math.max(0, x));
// 抽到的图：键里带着筛选和数据版本，换了就重抽；换了筛选或数据版本时旧的整批丢掉。
const picked = new Map();
let pickedFor = '';
const pickKey = c => `${c.id}|${state.nsfw}|${state.readEpoch}`;
function pictures(c) {
  const epoch = `${state.nsfw}|${state.readEpoch}`;
  if (epoch !== pickedFor) { picked.clear(); pickedFor = epoch; }
  const key = pickKey(c);
  if (!picked.has(key)) picked.set(key, c.count > 1 ? posterPictures(c) : Promise.resolve([c.cover]));
  return picked.get(key);
}
// 海报接近屏幕时通知：一个页签一个观察器，登记在 s 上。
function nearing(s) {
  const seen = new WeakMap();
  const io = typeof IntersectionObserver === 'function' ? s.observe(new IntersectionObserver(es => {
    for (const e of es) if (e.isIntersecting) { io.unobserve(e.target); seen.get(e.target)?.(); }
  }, {rootMargin: '300px'})) : null;
  return (el, fn) => { if (!io) return; seen.set(el, fn); io.unobserve(el); io.observe(el); };
}

export function posters(parent) {
  const watch = nearing(parent);
  return item => posterWidget(item, parent.child('poster'), watch);
}

function posterWidget(first, s, watch) {
  // 帧任务的令牌在这张海报自己的作用域上取（#751）：同一帧里好几张海报一起缓动时各跑各的。
  const {el, refs} = POSTER(), a = refs.link, LOOK = s.key('poster:look');
  const pinned = pinButton(s, {onChange: on => cls(el, 'is-pinned', on)});
  el.append(pinned.el);
  let c = null, big = false, list = [], look = null, drag = null, key = '';
  const src = uuid => imageUrl(uuid, big ? 'medium' : 'thumb');

  function update(next) {
    const wasBig = big;
    c = next.c; big = !!next.big;
    attr(a, 'href', charHash(c.id));
    attr(a, 'data-char', c.id);
    cls(el, 'is-big', big);
    setTint(a, c.color);
    // 换了筛选、数据版本或封面：小查看器作废，换回封面，接近屏幕时按新的条件重抽。
    const want = pickKey(c) + '|' + c.cover;
    if (want !== key) { key = want; reset(); }
    else if (big !== wasBig) attr(refs.img, 'src', look ? src(list[look.i]) : c.cover ? src(c.cover) : null);
    show(refs.rank, !!next.rank); text(refs.rank, next.rank ? String(next.rank) : '');
    const plus = recentOf(c);
    show(refs.plus, plus > 0 && plus !== c.count); text(refs.plus, plus ? '+' + number(plus) : '');
    text(refs.name, c.name); attr(refs.name, 'title', nameTip(c));
    text(refs.sub, next.sub || '');
    pinned.update('c:' + c.id);
  }
  function reset() {
    look = null; list = [];
    refs.ticks.replaceChildren(); show(refs.mini, false); css(refs.cap, 'opacity', null); css(refs.img, 'object-position', null);
    cls(a, 'is-whole', false);
    attr(refs.img, 'src', c.cover ? src(c.cover) : null);
    const want = key;
    watch(a, () => { if (c?.cover) pictures(c).then(pics => { if (key === want) start(pics); }); });
  }

  // ---------- 小查看器 ----------
  // 第一张先在上层读好再换下来，封面不会闪一下变成空白。
  function start(pics) {
    if (!s.alive || look || !pics.length) return;
    const first = pics[0], url = src(first), go = () => {
      if (!s.alive || look) return;
      list = pics;
      look = {i: 0, target: .22, at: .22, cap: 1, capTarget: 1, over: false};
      attr(refs.img, 'src', url); cls(refs.next, 'on', false);
      refs.ticks.replaceChildren(...(list.length > 1 ? list.map((_, k) => make('i', k ? '' : 'on')) : []));
      kick();
    };
    if (refs.img.getAttribute('src') === url) { go(); return; }
    attr(refs.next, 'src', url);
    if (refs.next.complete && refs.next.naturalWidth) go(); else refs.next.onload = refs.next.onerror = go;
  }
  // 显示的图在海报里怎么放：高的（往下看）、宽的（整张显示）或正好合适。
  function shape() {
    const img = refs.img;
    if (!img.naturalWidth) return {kind: 'fit', seen: 1};
    const r = img.naturalWidth / img.naturalHeight, rb = a.clientWidth / a.clientHeight;
    return r < rb * .94 ? {kind: 'tall', seen: r / rb} : r > rb * 1.06 ? {kind: 'wide', seen: 1} : {kind: 'fit', seen: 1};
  }
  function step() {
    if (!look || !s.alive) return;
    const L = look, sh = shape(), now = reduced();
    L.at += now ? L.target - L.at : (L.target - L.at) * .2;
    L.cap += now ? L.capTarget - L.cap : (L.capTarget - L.cap) * .2;
    cls(a, 'is-whole', L.over && sh.kind === 'wide');
    css(refs.img, 'object-position', sh.kind === 'tall' ? `50% ${(L.at * 100).toFixed(2)}%` : '');
    const mini = L.over && sh.kind === 'tall' && sh.seen < .9;
    show(refs.mini, mini);
    if (mini) { css(refs.thumb, 'height', `${sh.seen * 100}%`); css(refs.thumb, 'top', `${L.at * (1 - sh.seen) * 100}%`); }
    css(refs.cap, 'opacity', L.cap.toFixed(3));
    if (Math.abs(L.target - L.at) > .001 || Math.abs(L.capTarget - L.cap) > .002) kick();
  }
  const kick = () => s.write(step, LOOK);
  // 换一张：下一张先在上层读好，再淡入换下来，不闪白。
  function showPic(i) {
    if (!look || i === look.i) return;
    look.i = i;
    const want = src(list[i]), next = refs.next;
    const swap = () => {
      if (look?.i !== i) return;
      attr(refs.img, 'src', want); cls(next, 'on', false);
      kick();
    };
    attr(next, 'src', want);
    if (next.complete && next.naturalWidth) swap(); else { cls(next, 'on', true); next.onload = swap; next.onerror = null; }
    [...refs.ticks.children].forEach((x, k) => cls(x, 'on', k === i));
    for (const n of [i + 1, i + 2]) if (list[n]) new Image().src = src(list[n]);
  }
  function aim(e) {
    const r = a.getBoundingClientRect(), fy = (e.clientY - r.top) / r.height;
    look.over = true;
    look.target = clamp((fy - EDGE) / (1 - 2 * EDGE));
    look.capTarget = 1 - .88 * clamp((fy - .5) / .3);
    return r;
  }
  s.on(a, 'mousemove', e => {
    if (!look) return;
    const r = aim(e);
    showPic(Math.min(list.length - 1, Math.max(0, Math.floor((e.clientX - r.left) / r.width * list.length))));
    kick();
  });
  s.on(a, 'mouseleave', () => { if (!look) return; look.over = false; look.target = .22; look.capTarget = 1; showPic(0); kick(); });
  s.on(a, 'keydown', e => {
    if (!look || composing(e) || !['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) return;
    e.preventDefault();
    look.over = true;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') showPic(Math.min(list.length - 1, Math.max(0, look.i + (e.key === 'ArrowLeft' ? -1 : 1))));
    else { look.target = clamp(look.target + (e.key === 'ArrowUp' ? -.25 : .25)); look.capTarget = look.target > .5 ? .15 : 1; }
    kick();
  });
  // 触摸：横向开始的拖动翻图，上下开始的拖动照常滚动页面（touch-action: pan-y）；拖完的那次点击不打开角色。
  s.on(a, 'pointerdown', e => { drag = e.pointerType !== 'mouse' && look ? {id: e.pointerId, x: e.clientX, y: e.clientY, i: look.i, sideways: false} : null; });
  s.on(a, 'pointermove', e => {
    const d = drag; if (!d || e.pointerId !== d.id) return;
    const dx = e.clientX - d.x, dy = e.clientY - d.y;
    if (!d.sideways) {
      if (Math.abs(dx) < 10 || Math.abs(dx) < Math.abs(dy)) { if (Math.abs(dy) > 10) drag = null; return; }
      d.sideways = true; cls(a, 'is-dragging', true); a.setPointerCapture?.(e.pointerId);
    }
    aim(e);
    showPic(Math.min(list.length - 1, Math.max(0, d.i + Math.round(-dx / STEP))));
    kick();
  });
  const lift = e => {
    const d = drag; if (!d || e.pointerId !== d.id) return;
    drag = null;
    if (!d.sideways || !look) return;
    look.over = false; look.capTarget = 1; cls(a, 'is-dragging', false); a.dataset.dragged = String(performance.now()); kick();
  };
  s.on(a, 'pointerup', lift); s.on(a, 'pointercancel', lift);
  s.on(a, 'click', e => { if (performance.now() - Number(a.dataset.dragged || 0) < 500) { e.preventDefault(); e.stopPropagation(); } }, true);

  update(first);
  return {el, update, dispose: () => s.dispose()};
}

// ---------- 圆头像卡 ----------
// 圆形代表人：资料库「常见」一档的角色、画师页签的画师。{href, cover, color, name, sub, title}；点了按链接走。
const PERSON = tpl`<a class="cat-person"><img class="avatar is-large" data-ref="img" alt="" loading="lazy" decoding="async" draggable="false"><b data-ref="name"></b><small
  data-ref="sub"></small></a>`;
export function personCard(first) {
  const {el, refs} = PERSON();
  function update(x) {
    attr(el, 'href', x.href);
    attr(el, 'title', x.title || null);
    attr(refs.img, 'src', x.cover ? imageUrl(x.cover) : null);
    setTint(refs.img, x.color);
    text(refs.name, x.name); text(refs.sub, x.sub || '');
  }
  update(first);
  return {el, update};
}
