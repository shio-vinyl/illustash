// 看图器（0.4 第 D 步，#709；原来在 ui/detail.js 里）：全屏大图、翻页、缩放平移、底栏，以及从缩略图展开、关掉时缩回去。
//
// - 舞台里只放三张：当前一张和左右各一张（.vw-slot），循环复用。邻居的预览图提前设好并 decode()，翻页时只是换哪一张可见，
//   不等网络、不闪白。原图（size=full）只给当前那张，解码好了叠在预览图上面（.vw-high）。
// - 图的框按宽高比用容器单位定（.vw-stage 是 size 容器，.vw-frame 的宽 = min(100cqw, 比例 × 100cqh)），不用脚本量。
// - 开：从缩略图（ui/motion.js 的 thumbs.find：各视图在缩略图上写 data-thumb）展开，先等预览图解码（至少 50ms、至多 250ms），
//   再 share()：支持 View Transitions 的平台用它，其余（Safari 17）用 Web Animations 的 FLIP。没有缩略图时从中间放大淡入。
//   关：把最后看的那张滚进视野，等两帧让视图画出缩略图，再 shareBack() 缩回去；缩略图不在视野里时淡出。
//   「减少动态效果」时 ui/motion.js 直接到终态。
// - 缩放、平移的事件只记数，改 transform 排进 ui/frame.js 的写阶段，一帧一次。
// - 和网格、详情栏的配合（翻页、关掉后详情栏跟到最后看的那张、面板页借来的列表）由 ui/detail.js 在启动时经 bindViewer()
//   传进来：这里不导入 ui/grid.js、ui/detail.js（tools/check_frontend.py 规则 1 不许新增循环导入）。
// - 写 state.viewer、state.viewerItem（core/state.js 的「大图」一组）。
import {t} from '../core/i18n.js';
import {number} from '../core/format.js';
import {aspect} from '../core/layout.js';
import {unreadableText} from '../core/jobs.js';
import {detailAfterViewer} from '../core/modes.js';
import {keyScope, pageOf, viewOf} from '../core/pages.js';
import {state} from '../core/state.js';
import {readDetail, setDetail, setQuestion, shouldAsk} from '../core/detail.js';
import {$} from './dom.js';
import {askBar, identities} from './detail_card.js';
import {attr, cls, css, make, show, text, tpl} from './el.js';
import {wireDragOut} from './files.js';
import {read} from './frame.js';
import {animate, enter, share, shareBack, thumbs} from './motion.js';
import {createLayer} from './overlay.js';
import {scope} from './scope.js';

const img = (uuid, size) => `/image?id=${encodeURIComponent(uuid)}&size=${size}`;
const SLOT = tpl`<div class="vw-slot" hidden><div class="vw-frame" data-ref="frame"><div class="vw-zoom" data-ref="zoom">
  <img class="vw-low" data-ref="low" alt="" draggable="false"><img class="vw-high" data-ref="high" alt="" draggable="false" hidden></div></div>
  <p class="vw-missing" data-ref="missing" hidden></p></div>`;
// 开场前等预览图解码的上下限（#709）。
const OPEN_MIN_MS = 50, OPEN_MAX_MS = 250;
const sleep = ms => new Promise(r => setTimeout(r, ms));
// 等下一帧；后台页面不跑帧，最多等 100ms（只防挂住，关大图不能因为没有帧就一直不藏）。
const nextFrame = () => new Promise(r => { read(r); setTimeout(r, 100); });

let hooks = null;
/**
 * ui/detail.js 在 wireDetail 里调一次：{loadPage, PAGE, setCursor, previewList, endPreview, previewing, kept, ensureVisible, inspect,
 * step, markDetailKeys, askEnabled, act(name, data, event, d)}。act 处理底栏里的点击（和详情卡片同一套）。
 */
export function bindViewer(h) { hooks = h; }

const s = scope('viewer');
const view = {scale: 1, x: 0, y: 0};
let slots = [], current = null, bar = null, closing = 0, backdrop = null;

// 大图是盖满页面的层：Esc 关它（焦点在层外的输入框里时让位给输入框失焦），关闭时焦点还给打开它的地方。
// 开着时焦点在大图上，它盖住的页面、选择栏和任务面板设成 inert，Tab 只在大图的按钮间走（#422）；提示条（撤销）不盖。
const layer = createLayer({kind: 'overlay', id: 'viewer', el: () => $('viewer'), yieldToInput: true, focusIn: 'self',
  inert: () => [document.querySelector('.app'), $('selectionBar'), $('taskPanel')], onDismiss: reason => closeViewer(reason)});

// ---------- 三张轮换 ----------
function makeSlot() {
  const {el, refs} = SLOT();
  const slot = {el, ...refs, uuid: null, ready: Promise.resolve()};
  s.on(refs.low, 'error', () => { if (slot.uuid) { show(refs.frame, false); text(refs.missing, unreadableText()); show(refs.missing, true); } });
  s.on(refs.high, 'load', () => { if (slot === current && refs.high.getAttribute('src')) refs.high.decode?.().catch(() => {}).then(() => { if (slot === current) show(refs.high, true); }); });
  // 没放大时按住大图拖动：把这张图拖到别的应用（#450）；放大后按住拖动是平移。
  wireDragOut(refs.frame, () => [slot.uuid].filter(Boolean), () => view.scale <= 1.01);
  return slot;
}
// 把一张图放进槽里：预览图、按宽高比定框；返回预览图解码的 promise（失败也落定）。
function fill(slot, item) {
  if (slot.uuid === item.uuid) return slot.ready;
  slot.uuid = item.uuid;
  show(slot.frame, true); show(slot.missing, false); show(slot.high, false); attr(slot.high, 'src', null);
  css(slot.zoom, 'transform', null);
  ratio(slot, aspect(item));
  attr(slot.low, 'src', img(item.uuid, 'preview'));
  slot.ready = (slot.low.decode?.() || Promise.resolve()).then(() => { if (!aspect(item) && slot.low.naturalWidth) ratio(slot, slot.low.naturalWidth / slot.low.naturalHeight); }, () => {});
  return slot.ready;
}
function ratio(slot, r) {
  const k = r > 0 && Number.isFinite(r) ? r : 3 / 4;
  css(slot.frame, 'aspect-ratio', String(k));
  css(slot.frame, 'width', `min(100cqw, ${(k * 100).toFixed(3)}cqh)`);
}
// 当前那张换成 item，左右邻居换成 i±1（列表里有的话）；已经在槽里的图不重读。
function arrange(item, i) {
  if (!slots.length) return Promise.resolve();   // 还没 wireViewer（Node 测试里只测状态）
  const wanted = [item, state.items[i - 1], state.items[i + 1]];
  let cur = slots.find(x => x.uuid === item.uuid);
  const used = new Set();
  if (!cur) cur = slots.find(x => !wanted.some(w => w && w.uuid === x.uuid)) || slots[0];
  used.add(cur);
  const ready = fill(cur, item);
  for (const w of wanted.slice(1)) {
    if (!w) continue;
    let slot = slots.find(x => x.uuid === w.uuid && !used.has(x));
    if (!slot) slot = slots.find(x => !used.has(x) && !wanted.some(v => v && v.uuid === x.uuid)) || slots.find(x => !used.has(x));
    if (!slot) continue;
    used.add(slot); fill(slot, w);
  }
  for (const slot of slots) { show(slot.el, slot === cur); cls(slot.el, 'is-current', slot === cur); }
  // 再远一张只预读，不占槽。
  for (const d of [-2, 2]) { const x = state.items[i + d]; if (x && typeof Image === 'function') new Image().src = img(x.uuid, 'preview'); }
  if (current !== cur) { current = cur; resetView(); }
  // 原图只给当前那张，解码好了由 load 处理盖上去。
  if (!cur.high.getAttribute('src')) attr(cur.high, 'src', img(item.uuid, 'full'));
  return ready;
}

// ---------- 缩放与平移 ----------
let stageBox = null;
function resetView() { Object.assign(view, {scale: 1, x: 0, y: 0}); applyView(); }
function applyView() {
  s.write(() => {
    if (!current) return;
    css(current.zoom, 'transform', view.scale === 1 && !view.x && !view.y ? null : `translate(${view.x}px, ${view.y}px) scale(${view.scale})`);
    cls($('viewerStage'), 'zoomed', view.scale > 1.01);
  }, 'viewer:zoom');
}
/** 以 (cx, cy)（视口坐标）为中心缩放 factor 倍。放大上限是原图的两倍像素和 2 倍里大的那个。 */
export function zoomView(factor, cx, cy) {
  const box = stageBox || {left: 0, top: 0, width: innerWidth, height: innerHeight};
  const px = cx - box.left - box.width / 2, py = cy - box.top - box.height / 2;
  const pic = current?.high.naturalWidth ? current.high : current?.low, shown = current?.frame.clientWidth || 0;
  const limit = Math.max(2, pic && shown ? (pic.naturalWidth / shown) * 2 : 2);
  const next = Math.max(1, Math.min(limit, view.scale * factor)), k = next / view.scale;
  view.x = px - (px - view.x) * k; view.y = py - (py - view.y) * k; view.scale = next;
  if (next === 1) { view.x = 0; view.y = 0; }
  applyView();
}

// ---------- 底栏：提问、角色条和几个动作 ----------
const BAR = tpl`<div class="vw-bar-in"><p class="vw-status" data-ref="status" hidden></p><div data-ref="askSlot"></div><div data-ref="whoSlot"></div>
  <div class="vw-acts" data-ref="acts"></div></div>`;
function viewerBar() {
  const {el, refs} = BAR(), ask = askBar(), who = identities({compact: true});
  refs.askSlot.replaceWith(ask.el); refs.whoSlot.replaceWith(who.el); s.add(ask); s.add(who);
  const out = {el, d: null, failed(message) { text(refs.status, message); show(refs.status, true); }, update(d, asking) {
    out.d = d; show(refs.status, false);
    ask.update(asking ? d.question : null); who.update(d);
    const acts = viewerActions(d);
    refill(refs.acts, JSON.stringify(acts), () => acts.map(a => {
      const b = make('button', a.fav ? 'chip fav-act' : 'chip'); b.type = 'button'; b.dataset.act = a.act;
      if (a.key) b.append(make('kbd', '', a.key));
      if (!a.fav) { b.append(a.label); return b; }
      // 收藏：写入后 ui/favorites.js 的 paintHearts 按 data-fav 就地改 .on、aria-pressed、提示和 .fav-label。
      b.dataset.fav = d.uuid; b.classList.toggle('on', a.on); b.setAttribute('aria-pressed', String(a.on));
      b.title = a.on ? t('取消收藏（F）') : t('收藏（F）'); b.append(make('span', 'fav-label', a.label));
      return b;
    }));
  }};
  return out;
}
// 按内容的签名重画一小块：签名没变就不动。
function refill(node, sig, build) { if (node.dataset.sig !== sig) { node.dataset.sig = sig; node.replaceChildren(...build()); } }
/** 大图底栏的动作：「跳过 S」「排除 X」「收藏 F」「信息 I」和「更多像这张的」。只在这一页按这个键做的就是它时才画（#424 的 2-1、2-7）。 */
function viewerActions(d) {
  const out = [];
  if (pageOf(state.route).skip && !d.memberships.length && !d.unknown) out.push({act: 'skip', key: 'S', label: t('跳过')});
  if (!d.excluded) out.push({act: 'exclude', key: 'X', label: t('排除')});
  out.push({act: 'fav', key: 'F', label: d.fav ? t('已收藏') : t('收藏', {context: 'verb'}), fav: true, on: !!d.fav});
  if (keyScope(viewOf(state.route)) && !hooks.previewing()) out.push({act: 'info', key: 'I', label: t('信息')});
  out.push({act: 'like', label: t('更多像这张的')});
  return out;
}
async function refreshBar(item) {
  let d;
  try { d = await readDetail(item.uuid); } catch (e) { if (state.viewerItem?.uuid === item.uuid) bar?.failed(e.message); return; }
  if (!d || state.viewerItem?.uuid !== item.uuid) return;
  setDetail(d);
  const asking = shouldAsk(d, item.uuid, hooks.askEnabled());
  setQuestion(asking ? {q: d.question, ident: item.uuid} : null);
  bar?.update(d, asking);
}

// ---------- 开、关、翻 ----------
const root = () => $('viewer'), chrome = () => [backdrop, root().querySelector('.viewer-top'), $('viewerBar')];
/** 打开第 i 张的大图。 */
export function openViewer(i) {
  if (i < 0 || !state.items[i]) return;
  if (state.viewer >= 0) { showViewer(i); return; }
  const mine = ++closing, item = state.items[i], thumb = thumbs.find(item.uuid);
  state.viewer = i;
  document.body.classList.add('viewing');
  const ready = arrange(item, i);
  showViewer(i);
  // 展开：等预览图解码（有上下限），然后从缩略图的位置长到大图的位置。
  Promise.all([sleep(OPEN_MIN_MS), Promise.race([ready, sleep(OPEN_MAX_MS)])]).then(() => {
    if (mine !== closing || state.viewer < 0) return;
    share({from: thumb, to: () => current?.frame, radius: [8, 0], update: () => {
      if (mine !== closing) return;
      // 上一次缩回还没走完就又打开：先撤掉它停在终点的动画。
      for (const el of [...slots.map(x => x.frame), ...chrome()]) for (const anim of el.getAnimations?.() || []) anim.cancel();
      root().hidden = false; cls(root(), 'is-closing', false); layer.open();
      for (const el of chrome()) enter(el, {from: 'fade', dur: 2});
    }});
  });
}
/**
 * 关大图。详情栏开着时，关大图后它跟到大图里最后看的那张（core/modes.js 的 detailAfterViewer，#399）：光标已经在那张，
 * 不跟的话快捷键改的是详情栏里的旧图，Space、方向键却从光标那张走。
 * 面板页借列表看的大图（previewList）关掉时把列表还回去，光标和详情栏都不动，只把最后看的那张滚进视野。
 * reason 是 'silent' 时（换页）不播动画。
 */
export function closeViewer(reason) {
  layer.close(reason); if (state.viewer < 0) return;
  const follow = detailAfterViewer(), last = state.viewerItem?.uuid, mine = ++closing;
  hooks.kept.hold = null; state.viewerItem = null; document.body.classList.remove('viewing');
  if (hooks.endPreview()) { state.viewer = -1; reveal(last); hooks.markDetailKeys(); }
  else {
    hooks.setCursor(state.viewer); state.viewer = -1; hooks.ensureVisible(state.cursor);
    if (follow) hooks.inspect(follow, true); else hooks.markDetailKeys();   // 选择栏又不被盖住了：详情栏的键标跟着它
  }
  const frameEl = current?.frame;
  const done = () => {
    if (mine !== closing) return;
    root().hidden = true; cls(root(), 'is-closing', false);
    for (const el of [frameEl, ...chrome()]) for (const a of el?.getAnimations?.() || []) a.cancel();
  };
  if (reason === 'silent' || !frameEl || root().hidden) { done(); return; }
  // 缩回去：等视图把滚进视野的那张画出来（两帧），找它的缩略图；退场期间不接点击（.is-closing）。
  cls(root(), 'is-closing', true);
  nextFrame().then(nextFrame).then(() => {
    if (mine !== closing) return;
    resetView();
    // 顶栏、底栏淡出后停在透明，不藏起来：藏了舞台会变高，大图的框跟着变，缩回去的终点就错位了。
    for (const el of chrome()) animate(el, [{opacity: 1}, {opacity: 0}], {dur: 1, fill: 'forwards'});
    return shareBack(frameEl, last ? thumbs.find(last) : null, {radius: [0, 8]});
  }).then(done);
}
/** 显示第 i 张（held 是已经离开列表、但还停在屏幕上的那张）。 */
export async function showViewer(i, held = null) {
  if (i < 0 && !held) { closeViewer(); return; }
  if (!held && !state.items[i]) await hooks.loadPage(Math.floor(i / hooks.PAGE) * hooks.PAGE);
  const item = held || state.items[i]; if (!item) { closeViewer(); return; }
  const same = state.viewerItem?.uuid === item.uuid;
  state.viewer = Math.max(0, i); hooks.setCursor(state.viewer); state.viewerItem = item;
  const at = state.items.findIndex(x => x?.uuid === item.uuid);
  text($('viewerCount'), at >= 0 ? `${number(at + 1)} / ${number(state.total)}` : t('已移出当前列表 · ← → 继续'));
  if (!same) { arrange(item, at >= 0 ? at : i); noteSize(); }
  return refreshBar(item);
}
// 主区不是网格的页面把一张图滚进视野：页面在注册表里登记 reveal(uuid)（首页、按组整理）。
const reveal = uuid => { if (uuid) pageOf(state.route).reveal?.(uuid); };
/** 顶栏的尺寸：原图读好以后写原图的像素，之前空着。 */
function noteSize() {
  const pic = current?.high;
  text($('viewerSize'), pic?.naturalWidth && pic.complete ? t('本地副本 {w} × {h}', {w: pic.naturalWidth, h: pic.naturalHeight}) : '');
}

// ---------- 面板页看大图（#424 的 2-2、2-34） ----------
// 按组整理的缩略图、首页「认一认」的图，点一下是「不算」或「不是 TA」，看大图用长按，或焦点在图上、鼠标停在图上时按
// Space（grid.js 的键盘处理调 peek）；身份整理的样图、画师抽查的图点一下就看大图（页面自己调 peekAt）。这样的图带
// data-peek（uuid），所在的区域用 wirePeek 登记，list(el) 返回它那一组的 uuid：大图里翻的就是这一组（grid.js 的 previewList）。
const LONG_PRESS = 450, peeks = [];
let hovered = null;
export function wirePeek(rootEl, list) {
  peeks.push({root: rootEl, list});
  let timer = 0, fired = false, start = [0, 0];
  const cancel = () => { clearTimeout(timer); timer = 0; };
  rootEl.addEventListener('pointerdown', e => {
    fired = false; cancel();
    const el = e.target.closest('[data-peek]'); if (!el || e.button !== 0) return;
    start = [e.clientX, e.clientY];
    timer = setTimeout(() => { timer = 0; fired = peekAt(el); }, LONG_PRESS);
  });
  rootEl.addEventListener('pointermove', e => { if (timer && Math.hypot(e.clientX - start[0], e.clientY - start[1]) > 8) cancel(); });
  for (const type of ['pointerup', 'pointercancel', 'pointerleave']) rootEl.addEventListener(type, cancel);
  // 长按打开了大图：松手时的这次点击不再算「不算在这组里」。松手常落在大图上，这次点击到不了这里，标志留到下一次按下
  // 才清；所以只吞指针产生的点击（detail > 0），键盘 ↵ 按下按钮产生的点击（detail 为 0）照常。
  rootEl.addEventListener('click', e => { if (fired && e.detail > 0) { e.preventDefault(); e.stopPropagation(); } fired = false; }, true);
  rootEl.addEventListener('contextmenu', e => { if (fired || timer) e.preventDefault(); });
  rootEl.addEventListener('mouseover', e => { hovered = e.target.closest('[data-peek]'); });
  rootEl.addEventListener('mouseleave', () => { hovered = null; });
}
/** Space：焦点所在的图，否则鼠标停着的那张（还在页面上、看得见）。打开了返回 true。 */
export function peek(target) {
  const el = target?.closest?.('[data-peek]') || (hovered?.isConnected && hovered.offsetParent ? hovered : null);
  return el ? peekAt(el) : false;
}
export function peekAt(el) {
  const p = peeks.find(x => x.root.contains(el)), ids = p?.list(el) || [], i = ids.indexOf(el.dataset.peek);
  if (i < 0) return false;
  hooks.previewList(ids, i); return true;
}

// ---------- 启动 ----------
/** 建舞台和底栏，挂上手势。页面启动时调用一次（ui/detail.js 的 wireDetail）。 */
export function wireViewer() {
  bar = viewerBar();
  const stage = $('viewerStage');
  slots = [makeSlot(), makeSlot(), makeSlot()];
  stage.replaceChildren(...slots.map(x => x.el));
  backdrop = make('div', 'vw-backdrop'); root().prepend(backdrop);
  $('viewerBar').replaceChildren(bar.el);
  s.on(bar.el, 'click', e => {
    const b = e.target.closest?.('[data-act]'); if (!b || !bar.d) return;
    if (b.tagName === 'A') e.preventDefault();
    hooks.act(b.dataset.act, b.dataset, e, bar.d);
  });
  if (typeof ResizeObserver === 'function') s.observe(new ResizeObserver(() => s.read(() => { stageBox = stage.getBoundingClientRect(); }, 'viewer:box'))).observe(stage);
  for (const x of slots) s.on(x.high, 'load', () => { if (x === current) noteSize(); });
  s.on(stage, 'wheel', e => {
    e.preventDefault();
    if (e.ctrlKey || e.metaKey) zoomView(Math.exp(-e.deltaY / 200), e.clientX, e.clientY);
    else if (view.scale > 1) { view.x -= e.deltaX; view.y -= e.deltaY; applyView(); }
  }, {passive: false});
  let gesture = 1, pan = null;
  s.on(stage, 'gesturestart', e => { e.preventDefault(); gesture = 1; });
  s.on(stage, 'gesturechange', e => { e.preventDefault(); zoomView(e.scale / gesture, e.clientX, e.clientY); gesture = e.scale; });
  s.on(stage, 'dblclick', e => {
    if (view.scale > 1.01) { resetView(); return; }
    const pic = current?.high.naturalWidth ? current.high : current?.low;
    zoomView(Math.max(2, pic ? pic.naturalWidth / (current.frame.clientWidth || pic.naturalWidth) : 2), e.clientX, e.clientY);
  });
  s.on(stage, 'pointerdown', e => { if (view.scale > 1) { pan = {x: e.clientX - view.x, y: e.clientY - view.y}; stage.setPointerCapture(e.pointerId); } });
  s.on(stage, 'pointermove', e => { if (pan) { view.x = e.clientX - pan.x; view.y = e.clientY - pan.y; applyView(); } });
  for (const type of ['pointerup', 'pointercancel']) s.on(stage, type, () => { pan = null; });
  s.on(stage, 'click', e => { if (!e.target.closest('.vw-frame') && view.scale <= 1) closeViewer(); });
  $('viewerClose').onclick = () => closeViewer(); $('viewerPrev').onclick = () => hooks.step(-1); $('viewerNext').onclick = () => hooks.step(1);
}
