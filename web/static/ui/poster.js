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
import {t} from '../core/i18n.js';
import {composing} from '../core/keys.js';
import {nameTip} from '../core/labels.js';
import {charHash} from '../core/route.js';
import {state} from '../core/state.js';
import {attr, cls, css, make, show, text, tpl} from './el.js';
import {reduced} from './motion.js';
import {pinButton} from './pin_button.js';
import {setTint} from './tint.js';

const POSTER = tpl`<a class="poster cat-poster"><img data-ref="img" alt="" loading="lazy" decoding="async" draggable="false"><img class="cat-next" data-ref="next" alt=""
  decoding="async" draggable="false"><span class="cat-rank" data-ref="rank"></span><span class="cat-plus" data-ref="plus"></span><span class="poster-cap" data-ref="cap"><b
  data-ref="name"></b><small data-ref="sub"></small></span><span class="cat-ticks" data-ref="ticks"></span><span class="cat-mini" data-ref="mini" hidden><i
  data-ref="thumb"></i></span></a>`;
const EDGE = .14;   // 顶部和底部那条进入即到头的带子
const STEP = 36;    // 触摸时横向拖多少 px 翻一张
const clamp = x => Math.min(1, Math.max(0, x));
// 抽到的图：键里带着筛选和数据版本，换了就重抽。
const picked = new Map();
const pickKey = c => `${c.id}|${state.nsfw}|${state.readEpoch}`;

// 海报接近屏幕时通知：一个页签一个观察器，登记在 s 上。
function nearing(s) {
  const seen = new WeakMap();
  const io = typeof IntersectionObserver === 'function' ? s.observe(new IntersectionObserver(es => {
    for (const e of es) if (e.isIntersecting) { io.unobserve(e.target); seen.get(e.target)?.(); }
  }, {rootMargin: '300px'})) : null;
  return (el, fn) => { if (!io) return; seen.set(el, fn); io.observe(el); };
}

export function posters(parent) {
  const watch = nearing(parent);
  return item => posterWidget(item, parent.child('poster'), watch);
}

function posterWidget(first, s, watch) {
  const {el, refs} = POSTER(), pinned = pinButton(s);
  el.append(pinned.el);
  let c = null, list = [], look = null, drag = null;
  const src = (uuid, big) => imageUrl(uuid, big ? 'medium' : 'thumb');

  function update({c: next, rank = 0, big = false, sub = ''}) {
    const changed = c?.id !== next.id || c?.cover !== next.cover;
    c = next;
    attr(el, 'href', charHash(c.id));
    attr(el, 'data-char', c.id);
    cls(el, 'is-big', big);
    setTint(el, c.color);
    if (changed && !look) attr(refs.img, 'src', c.cover ? src(c.cover, big) : null);
    show(refs.rank, !!rank); text(refs.rank, rank ? String(rank) : '');
    const plus = recentOf(c);
    show(refs.plus, plus > 0 && plus !== c.count); text(refs.plus, plus ? '+' + number(plus) : '');
    text(refs.name, c.name); attr(refs.name, 'title', nameTip(c));
    text(refs.sub, sub);
    pinned.update('c:' + c.id);
    big0 = big;
  }
  let big0 = !!first.big;

  // ---------- 小查看器 ----------
  function start(pics) {
    if (!s.alive || look || !pics.length) return;
    list = pics;
    look = {i: 0, target: .22, at: .22, cap: 1, capTarget: 1, over: false};
    attr(refs.img, 'src', src(list[0], big0));
    refill();
    kick();
  }
  function refill() {
    refs.ticks.replaceChildren(...(list.length > 1 ? list.map((_, k) => make('i', k ? '' : 'on')) : []));
  }
  // 显示的图在海报里怎么放：高的（往下看）、宽的（整张显示）或正好合适。
  function shape() {
    const img = refs.img;
    if (!img.naturalWidth) return {kind: 'fit', seen: 1};
    const r = img.naturalWidth / img.naturalHeight, rb = el.clientWidth / el.clientHeight;
    return r < rb * .94 ? {kind: 'tall', seen: r / rb} : r > rb * 1.06 ? {kind: 'wide', seen: 1} : {kind: 'fit', seen: 1};
  }
  function step() {
    if (!look || !s.alive) return;
    const L = look, sh = shape(), now = reduced();
    L.at += now ? L.target - L.at : (L.target - L.at) * .2;
    L.cap += now ? L.capTarget - L.cap : (L.capTarget - L.cap) * .2;
    cls(el, 'is-whole', L.over && sh.kind === 'wide');
    css(refs.img, 'object-position', sh.kind === 'tall' ? `50% ${(L.at * 100).toFixed(2)}%` : '');
    const mini = L.over && sh.kind === 'tall' && sh.seen < .9;
    show(refs.mini, mini);
    if (mini) { css(refs.thumb, 'height', `${sh.seen * 100}%`); css(refs.thumb, 'top', `${L.at * (1 - sh.seen) * 100}%`); }
    css(refs.cap, 'opacity', L.cap.toFixed(3));
    if (Math.abs(L.target - L.at) > .001 || Math.abs(L.capTarget - L.cap) > .002) kick();
  }
  const kick = () => s.write(step, 'look');
  // 换一张：下一张先在上层读好，再淡入换下来，不闪白。
  function showPic(i) {
    if (!look || i === look.i) return;
    look.i = i;
    const want = src(list[i], big0), next = refs.next;
    const swap = () => {
      if (look?.i !== i) return;
      attr(refs.img, 'src', want); cls(next, 'on', false);
      kick();
    };
    attr(next, 'src', want);
    if (next.complete && next.naturalWidth) swap(); else { cls(next, 'on', true); next.onload = swap; }
    [...refs.ticks.children].forEach((x, k) => cls(x, 'on', k === i));
    for (const n of [i + 1, i + 2]) if (list[n]) new Image().src = src(list[n], big0);
  }
  function aim(e) {
    const r = el.getBoundingClientRect(), fy = (e.clientY - r.top) / r.height;
    look.over = true;
    look.target = clamp((fy - EDGE) / (1 - 2 * EDGE));
    look.capTarget = 1 - .88 * clamp((fy - .5) / .3);
    return r;
  }
  s.on(el, 'mousemove', e => {
    if (!look) return;
    const r = aim(e);
    showPic(Math.min(list.length - 1, Math.max(0, Math.floor((e.clientX - r.left) / r.width * list.length))));
    kick();
  });
  s.on(el, 'mouseleave', () => { if (!look) return; look.over = false; look.target = .22; look.capTarget = 1; showPic(0); kick(); });
  s.on(el, 'keydown', e => {
    if (!look || composing(e) || !['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) return;
    e.preventDefault();
    look.over = true;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') showPic(Math.min(list.length - 1, Math.max(0, look.i + (e.key === 'ArrowLeft' ? -1 : 1))));
    else { look.target = clamp(look.target + (e.key === 'ArrowUp' ? -.25 : .25)); look.capTarget = look.target > .5 ? .15 : 1; }
    kick();
  });
  // 触摸：横向开始的拖动翻图，上下开始的拖动照常滚动页面（touch-action: pan-y）；拖完的那次点击不打开角色。
  s.on(el, 'pointerdown', e => { drag = e.pointerType !== 'mouse' && look ? {id: e.pointerId, x: e.clientX, y: e.clientY, i: look.i, sideways: false} : null; });
  s.on(el, 'pointermove', e => {
    const d = drag; if (!d || e.pointerId !== d.id) return;
    const dx = e.clientX - d.x, dy = e.clientY - d.y;
    if (!d.sideways) {
      if (Math.abs(dx) < 10 || Math.abs(dx) < Math.abs(dy)) { if (Math.abs(dy) > 10) drag = null; return; }
      d.sideways = true; cls(el, 'is-dragging', true); el.setPointerCapture?.(e.pointerId);
    }
    aim(e);
    showPic(Math.min(list.length - 1, Math.max(0, d.i + Math.round(-dx / STEP))));
    kick();
  });
  const lift = e => {
    const d = drag; if (!d || e.pointerId !== d.id) return;
    drag = null;
    if (!d.sideways) return;
    look.over = false; look.capTarget = 1; cls(el, 'is-dragging', false); el.dataset.dragged = String(performance.now()); kick();
  };
  s.on(el, 'pointerup', lift); s.on(el, 'pointercancel', lift);
  s.on(el, 'click', e => { if (performance.now() - Number(el.dataset.dragged || 0) < 500) { e.preventDefault(); e.stopPropagation(); } }, true);

  update(first);
  watch(el, () => {
    if (!c?.cover) return;
    const key = pickKey(c);
    if (!picked.has(key)) picked.set(key, c.count > 1 ? posterPictures(c) : Promise.resolve([c.cover]));
    picked.get(key).then(start);
  });
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
