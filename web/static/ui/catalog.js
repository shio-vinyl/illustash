// 资料库（桌面布局，#199）：角色的书架和列表、带关注开关的作品树，以及编辑作品和添加新角色的对话框。它们读取和更改
// 的内容在 core/catalog.js。
import {t} from '../core/i18n.js';
import {esc, number} from '../core/format.js';
import {composing, onTyped} from '../core/keys.js';
import {implement, onPage} from '../core/pages.js';
import {prefs} from '../core/prefs.js';
import {api, session, readError} from '../core/session.js';
import {state} from '../core/state.js';
import {nameTip} from '../core/labels.js';
import {pollIndex, indexAction} from '../core/jobs.js';
import {remoteNames, remoteNote} from '../core/settings.js';
import {catalog, forgetActivity, recentOf, loadActivity as readActivity, searchCharacters, loadTagged, loadWorkTree, loadLocalLinks, castWorks, PSEUDO_NOTES, BUCKETS, castRows,
  WORK_NAMES, workKey, workName, loadIndex, searchWorks, shelfRows, saveWatch, watchTarget, saveWork} from '../core/catalog.js';
import {$, toast, toastMore, error, readFailure, wireFolds, namesAllowButton, reconcileSections, searchIcon, showArea} from './dom.js';
import {chooseWork, openDialog} from './dialog.js';
import {make} from './el.js';
import {createLayer, openModal} from './overlay.js';
import {openLocalLinks, openPicker, toggleMascot} from './palette.js';
import {combo} from './combo.js';
import {charHash, charRoute, go} from '../core/route.js';
import {fetchChar, refreshMeta} from '../core/meta.js';
import {closeInspector} from './detail.js';
import {renderHeading} from './shell.js';
import {dirHeading, refreshArtistDir, renderArtistDir} from './artist_dir.js';

// ---------- 角色：书架（作品标签页、海报、分档）和列表 ----------
// 「角色」页显示什么：书架还是列表、哪个作品标签页、搜索结果、列表的排序和档位。
export const cast = {mode: prefs.get('castMode', 'shelf'), tab: 'all', list: null, sort: 'count', dir: -1, bucket: 'all', pop: false, popQ: ''};
async function renderIndex(opts) {
  showArea(); closeInspector(); $('selectionBar').hidden = true;
  $('resultCount').textContent = '';
  if (state.route.section === 'artists') { await renderArtistDir(opts); return; }
  if (state.route.section === 'works') { renderShelf(); return; }
  drawn.clear();
  $('hint').textContent = t('点角色，在图库里只看 TA');
  cast.list = null; setPop(false, 'silent', false);
  if (state.chars.some(c => c.summary)) {
    try { await refreshMeta(); } catch (e) { error(e.message); return; }
    if (!onPage(state.route, 'characters')) return;
  }
  drawCast();
  loadTagged().then(() => drawCast()).catch(e => error(readError(e)));
  await loadWorkTree().catch(e => error(readError(e)));
  loadActivity(); refreshLocalLinks();
}
// 名字是 Danbooru tag 的自有角色（只拿这些名字去问 Danbooru；没有就不发请求）。
export async function refreshLocalLinks() {
  if (await loadLocalLinks() && onPage(state.route, 'characters')) renderHeading();
}
async function loadActivity() {
  let fresh; try { fresh = await readActivity(); } catch (e) { error(readError(e)); return; }
  if (fresh && onPage(state.route, 'characters')) drawCast();
}
export async function searchIndex() {
  const route = state.route, token = session.token;
  const q = $('charSearch').value.trim();
  if (!q) { cast.list = null; drawCast(); return; }
  let list; try { list = await searchCharacters(q); } catch (e) { if (route === state.route && token === session.token && q === $('charSearch').value.trim()) readFailure($('charIndex'), e); return; }
  if (route === state.route && token === session.token && q === $('charSearch').value.trim()) { cast.list = list; drawCast(); }
}
// 留给元数据变化后刷新页面的调用方。
export function renderIndexBody() {
  if (!onPage(state.route, 'characters') || state.route.section === 'artists') return;
  if (state.route.section === 'works') drawShelf(); else { drawCast(); loadActivity(); }
}
const cover = (c, size = 'thumb') => c?.cover ? `<img src="/image?id=${encodeURIComponent(c.cover)}&size=${size}" alt="" loading="lazy" decoding="async">` : '';
const avatar = (c, px) => `<i class="cast-av" style="width:${px}px;height:${px}px">${cover(c)}</i>`;
// ---------- 海报的图片：静止时随机一张，指针下看其他的（#293） ----------
// 每次打开资料库，海报都显示随机抽到的一张这个角色的图，而不总是它的封面（先抽只有它一个人的图，免得海报上是别人）。
// 指针放在海报上时，海报就是一个小查看器：横向移动在同一次抽取的图里挑（最多 12 张，抽中的那张在最前），纵向移动
// 上下看这张图。图片跟着指针滑动，两端各有一条带子，进入即到顶或到底；右边的小地图显示图有多高、正在显示哪一部分；
// 比海报宽的图整张显示在它自己的模糊副本上；看下半部分时名字淡出。海报有焦点时，← → 翻图，↑ ↓ 上下看。每张海报在
// 接近屏幕时读一次（很小）；页面重画时保留；在那之前显示封面。
const drawn = new Map();
const drawnKey = c => `${c.id}|${state.nsfw}|${state.readEpoch}`;
function drawPictures(c) {
  const offset = Math.floor(Math.random() * Math.max(1, c.count - 11));
  return api('/api/assets?' + new URLSearchParams({entity: c.id, order: 'newest', offset, limit: 12, count: 0, ...(state.nsfw ? {nsfw: state.nsfw} : {})}))
    .then(r => {
      const items = r.items || [], alone = items.filter(x => x.memberships?.length === 1), pool = alone.length ? alone : items;
      if (!pool.length) return [c.cover];
      const pick = pool[Math.floor(Math.random() * pool.length)].uuid;
      return [pick, ...items.map(x => x.uuid).filter(u => u !== pick)];
    }, () => [c.cover]);
}
const LOOK_EDGE = .14;   // 顶部和底部那条进入即到头的带子
const bigPoster = a => !!(a.closest('.cast-board') && !a.previousElementSibling);
const lookSrc = (a, u) => `/image?id=${encodeURIComponent(u)}&size=${bigPoster(a) ? 'medium' : 'thumb'}`;
// 海报的第一张图加载好之后才变成查看器，这样封面绝不会闪一下变成空白。
function lookFill(a, list) {
  const first = new Image();
  first.onload = first.onerror = () => {
    if (!a.isConnected || a.look) return;
    a.classList.add('look');
    a.look = {list, i: 0, target: .22, at: .22, cap: 1, capTarget: 1, over: false, raf: 0};
    const changed = list[0] !== state.charMap.get(a.dataset.char)?.cover;
    a.querySelector('.img').outerHTML = `<span class="img look-img"><img class="bg" alt="" decoding="async" src="${first.src}"><img class="pic on${changed ? ' drawn' : ''}" alt="" decoding="async" src="${first.src}"><img class="pic" alt="" decoding="async"></span>`;
    a.insertAdjacentHTML('beforeend', `${list.length > 1 ? `<span class="ticks">${list.map((_, i) => `<i class="${i ? '' : 'on'}"></i>`).join('')}</span>` : ''}<span class="mini" hidden><i></i></span>`);
    lookKick(a);
  };
  first.src = lookSrc(a, list[0]);
}
// 显示的图在海报里怎么放：高的（往下看）、宽的（整张显示）或正好合适。
function lookShape(a) {
  const pic = a.querySelector('.look-img .pic.on'), box = a.querySelector('.look-img');
  if (!pic?.naturalWidth) return {kind: 'fit', seen: 1};
  const r = pic.naturalWidth / pic.naturalHeight, rb = box.clientWidth / box.clientHeight;
  return r < rb * .94 ? {kind: 'tall', seen: r / rb} : r > rb * 1.06 ? {kind: 'wide', seen: 1} : {kind: 'fit', seen: 1};
}
function lookDraw(a) {
  const L = a.look, shape = lookShape(a), reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
  L.at += reduce ? L.target - L.at : (L.target - L.at) * .2;
  L.cap += reduce ? L.capTarget - L.cap : (L.capTarget - L.cap) * .2;
  const box = a.querySelector('.look-img'), mini = a.querySelector('.mini');
  if (!box) { L.raf = 0; return; }
  box.classList.toggle('whole', L.over && shape.kind === 'wide');
  for (const p of box.querySelectorAll('.pic')) p.style.objectPosition = shape.kind === 'tall' ? `50% ${(L.at * 100).toFixed(2)}%` : '';
  mini.hidden = !(L.over && shape.kind === 'tall' && shape.seen < .9);
  if (!mini.hidden) { const thumb = mini.firstChild; thumb.style.height = `${shape.seen * 100}%`; thumb.style.top = `${L.at * (1 - shape.seen) * 100}%`; }
  a.querySelector('.cap').style.opacity = L.cap.toFixed(3);
  const moving = Math.abs(L.target - L.at) > .001 || Math.abs(L.capTarget - L.cap) > .002;
  L.raf = moving ? requestAnimationFrame(() => lookDraw(a)) : 0;
}
const lookKick = a => { if (!a.look.raf) a.look.raf = requestAnimationFrame(() => lookDraw(a)); };
// 要显示的图：隐藏的那一层先加载它，然后两层以一个短的淡入淡出互换。
function lookShow(a, i) {
  const L = a.look; if (i === L.i) return;
  L.i = i;
  const box = a.querySelector('.look-img'), [p0, p1] = box.querySelectorAll('.pic'), back = p0.classList.contains('on') ? p1 : p0, front = back === p0 ? p1 : p0;
  back.classList.remove('drawn'); back.src = lookSrc(a, L.list[i]);
  const swap = () => { if (L.i !== i) return; back.classList.add('on'); front.classList.remove('on'); box.querySelector('.bg').src = back.src; lookKick(a); };
  back.complete && back.naturalWidth ? swap() : back.addEventListener('load', swap, {once: true});
  a.querySelectorAll('.ticks i').forEach((x, k) => x.classList.toggle('on', k === i));
  for (const n of [i + 1, i + 2]) if (L.list[n]) new Image().src = lookSrc(a, L.list[n]);
}
function lookMove(e) {
  const a = e.target.closest?.('.cast-poster.look'); if (!a) return;
  const L = a.look;
  if (e.type === 'leave') { L.over = false; L.target = .22; L.capTarget = 1; lookShow(a, 0); lookKick(a); return; }
  const r = a.getBoundingClientRect(), fx = (e.clientX - r.left) / r.width, fy = (e.clientY - r.top) / r.height;
  L.over = true;
  L.target = Math.min(1, Math.max(0, (fy - LOOK_EDGE) / (1 - 2 * LOOK_EDGE)));
  L.capTarget = 1 - .88 * Math.min(1, Math.max(0, (fy - .5) / .3));
  lookShow(a, Math.min(L.list.length - 1, Math.max(0, Math.floor(fx * L.list.length))));
  lookKick(a);
}
function lookKey(a, key) {
  const L = a.look; L.over = true;
  if (key === 'ArrowLeft' || key === 'ArrowRight') lookShow(a, Math.min(L.list.length - 1, Math.max(0, L.i + (key === 'ArrowLeft' ? -1 : 1))));
  else { L.target = Math.min(1, Math.max(0, L.target + (key === 'ArrowUp' ? -.25 : .25))); L.capTarget = L.target > .5 ? .15 : 1; }
  lookKick(a);
}
// 触摸屏没有悬停（#300）：横向拖动海报翻它的图，每 LOOK_STEP px 一张（向左是下一张，和划照片一样）；手指一直按着时，
// 上下移动就是上下看这张图。上下方向开始的拖动滚动页面（touch-action: pan-y）；轻点仍然打开角色，拖动结束时的那次
// 点击则不会。手指抬起时停在翻到的那张图上。
const LOOK_STEP = 36;
let lookDrag = null;
function lookDown(e) {
  if (e.pointerType === 'mouse') return;
  const a = e.target.closest?.('.cast-poster.look');
  lookDrag = a ? {a, id: e.pointerId, x: e.clientX, y: e.clientY, i: a.look.i, sideways: false} : null;
}
function lookPull(e) {
  const d = lookDrag; if (!d || e.pointerId !== d.id) return;
  const {a} = d, L = a.look, dx = e.clientX - d.x, dy = e.clientY - d.y;
  if (!d.sideways) {
    if (Math.abs(dx) < 10 || Math.abs(dx) < Math.abs(dy)) { if (Math.abs(dy) > 10) lookDrag = null; return; }
    d.sideways = true; a.classList.add('dragging'); a.setPointerCapture?.(e.pointerId);
  }
  const r = a.getBoundingClientRect(), fy = (e.clientY - r.top) / r.height;
  L.over = true;
  L.target = Math.min(1, Math.max(0, (fy - LOOK_EDGE) / (1 - 2 * LOOK_EDGE)));
  L.capTarget = 1 - .88 * Math.min(1, Math.max(0, (fy - .5) / .3));
  lookShow(a, Math.min(L.list.length - 1, Math.max(0, d.i + Math.round(-dx / LOOK_STEP))));
  lookKick(a);
}
function lookLift(e) {
  const d = lookDrag; if (!d || e.pointerId !== d.id) return;
  lookDrag = null;
  if (!d.sideways) return;
  const L = d.a.look; L.over = false; L.capTarget = 1;
  d.a.classList.remove('dragging'); d.a.dragged = performance.now(); lookKick(d.a);
}
let nearing = null;
function watchPosters(root) {
  nearing ||= new IntersectionObserver(es => {
    for (const e of es) {
      if (!e.isIntersecting) continue;
      nearing.unobserve(e.target);
      const a = e.target, c = state.charMap.get(a.dataset.char); if (!c?.cover) continue;
      const key = drawnKey(c);
      if (!drawn.has(key)) drawn.set(key, c.count > 1 ? drawPictures(c) : Promise.resolve([c.cover]));
      drawn.get(key).then(list => lookFill(a, list));
    }
  }, {rootMargin: '300px'});
  for (const a of root.querySelectorAll('.cast-poster[data-char]:not(.look)')) nearing.observe(a);
}
function poster(c, sub, rank = 0, big = false) {
  const r = recentOf(c), skins = (c.variants || []).filter(v => v.count).length;
  return `<a class="cast-poster" href="${charHash(c.id)}" data-char="${esc(c.id)}"><span class="img">${cover(c, big ? 'medium' : 'thumb')}</span>${rank ? `<span class="rank">${rank}</span>` : ''}${r ? `<span class="pill ${rank ? 'r' : 'l'}"><i></i>+${number(r)}</span>` : ''}${!rank && skins ? `<span class="pill r">${t('{n} 个造型', {n: skins})}</span>` : ''}<span class="cap"><b title="${esc(nameTip(c))}">${c.kind === 'oc' ? `<em>${t('原创')}</em>` : ''}${esc(c.name)}</b><small>${sub}</small></span></a>`;
}
const fan = chars => `<span class="cast-fan">${[1, 2, 0].map(i => `<i>${cover(chars[i])}</i>`).join('')}</span>`;
function tiers(list, showWork) {
  const big = list.filter(c => c.count >= 20), mid = list.filter(c => c.count >= 5 && c.count < 20), small = list.filter(c => c.count < 5);
  const sub = c => (showWork && c.work_name ? esc(c.work_name) + ' · ' : '') + t('{n} 张', {n: number(c.count)});
  return (big.length ? `<section class="cast-sec"><h3>${t('主力')}<small>${t('20 张以上 · {n} 位', {n: number(big.length)})}</small></h3><div class="cast-posters">${big.map(c => poster(c, sub(c))).join('')}</div></section>` : '') +
    (mid.length ? `<section class="cast-sec"><h3>${t('常见')}<small>${t('5–19 张 · {n} 位', {n: number(mid.length)})}</small></h3><div class="cast-roster">${mid.map(c => `<a class="cast-rs" href="${charHash(c.id)}" title="${esc(nameTip(c))}">${avatar(c, 72)}<b>${esc(c.name)}</b><small>${showWork && c.work_name ? esc(c.work_name) + ' · ' : ''}${t('{n} 张', {n: c.count})}</small></a>`).join('')}</div></section>` : '') +
    (small.length ? `<section class="cast-sec"><h3>${t('偶尔')}<small>${t('不到 5 张 · {n} 位', {n: number(small.length)})}</small></h3><div class="cast-chips">${small.map(c => `<a class="cast-chip" href="${charHash(c.id)}" title="${esc(nameTip(c))}">${avatar(c, 22)}${esc(c.name)}<small>${c.count}</small></a>`).join('')}</div></section>` : '');
}
// 总览、最大的几个作品，然后是排在最后的原创角色和未归作品（#293：它们不是作品；「最近 30 天」在总览里）。
function castTabs(works) {
  const real = works.filter(w => !w.pseudo), top = real.slice(0, 14);
  const current = !['all', 'recent'].includes(cast.tab) && !works.some(w => w.pseudo && w.key === cast.tab) && !top.some(w => w.key === cast.tab) ? works.find(w => w.key === cast.tab) : null;
  const tab = (key, label, sub, lead = '', title = '') => `<button class="cast-tab${lead ? '' : ' plain'}${title ? ' pseudo' : ''}${cast.tab === key ? ' on' : ''}" data-tab="${esc(key)}"${title ? ` title="${esc(title)}"` : ''}>${lead}${esc(label)}${sub ? `<small>${sub}</small>` : ''}</button>`;
  return `<div class="cast-tabs"><div class="cast-tabrow">${tab('all', t('总览'))}${[...(current ? [current] : []), ...top].map(w => tab(w.key, w.name, number(w.images), avatar(w.chars[0], 24))).join('')}${works.filter(w => w.pseudo).map(w => tab(w.key, w.name, number(w.images), '', PSEUDO_NOTES[w.key])).join('')}</div>
    <button class="cast-allw${cast.pop ? ' on' : ''}" id="castAll" aria-expanded="${cast.pop}">${t('全部作品 {n}', {n: number(works.filter(w => !w.pseudo).length)})}<svg viewBox="0 0 20 20" aria-hidden="true"><path d="m6 8 4 4 4-4"/></svg></button>${cast.pop ? castPop(works) : ''}</div>`;
}
// "全部作品"是非模态浮层：Esc、点外关闭由浮层管理器（ui/overlay.js）统一处理。它的元素每次 drawCast 都重画，所以层按 id 找。
const popLayer = createLayer({kind: 'popover', id: 'cast-works', el: () => $('castPop'), anchor: () => $('castAll'), onDismiss: reason => setPop(false, reason)});
function setPop(on, reason, redraw = true) {
  cast.pop = on;
  if (on) popLayer.open();
  if (redraw) drawCast();
  if (!on) popLayer.close(reason);
}
// 导出给样式清单页调用（tools/styleguide/live.js，#352）：改名或改参数时，tools/check_styleguide.py 会报出来。
export function castPop(works) {
  const q = cast.popQ.trim().toLowerCase(), list = works.filter(w => !q || w.name.toLowerCase().includes(q));
  return `<div class="menu cast-pop" id="castPop"><div class="search-wrap">${searchIcon()}<input type="search" id="castPopQ" placeholder="${t('在 {n} 部作品里找', {n: number(works.filter(w => !w.pseudo).length)})}" value="${esc(cast.popQ)}" autocomplete="off"></div>
    <div class="cast-wcols">${list.map(w => `<button class="cast-wrow${w.pseudo ? ' pseudo' : ''}" data-tab="${esc(w.key)}"${w.pseudo ? ` title="${esc(PSEUDO_NOTES[w.key])}"` : ''}>${avatar(w.chars[0], 20)}<span>${esc(w.name)}</span><small>${number(w.images)}</small></button>`).join('') || `<p class="muted-note">${t('没有匹配的作品')}</p>`}</div></div>`;
}
function castPane(works) {
  if (cast.list) return `<div class="cast-head"><div><h2>${t('搜索结果')}</h2><div class="cast-tags"><span>${t('{n} 位角色', {n: number(cast.list.length)})}</span></div></div></div>${cast.list.length ? tiers(cast.list, true) : `<p class="empty">${t('没有匹配的角色')}</p>`}`;
  if (cast.tab === 'all') {
    const top = [...state.chars].sort((a, b) => b.count - a.count).slice(0, 9);
    const rec = state.chars.filter(c => recentOf(c)).sort((a, b) => recentOf(b) - recentOf(a)), max = rec[0] ? recentOf(rec[0]) : 1;
    return `<section class="cast-sec"><h3>${t('你最常看的')}<small>${t('按张数')}</small></h3><div class="cast-board">${top.map((c, i) => poster(c, (c.work_name ? esc(c.work_name) + ' · ' : '') + t('{n} 张', {n: number(c.count)}), i + 1, i === 0)).join('')}</div></section>
      ${rec.length ? `<section class="cast-sec"><h3>${t('最近 30 天在收')}<small>${t('{n} 位角色有新图', {n: number(rec.length)})}</small><button class="link-button strong" data-tab="recent">${t('全部')} ›</button></h3><div class="cast-recent">${rec.slice(0, 24).map(c => `<a class="cast-rc" href="${charHash(c.id)}" title="${esc(nameTip(c))}"><span class="ring" style="--p:${Math.max(8, Math.round(recentOf(c) / max * 100))}">${avatar(c, 66)}<em>+${number(recentOf(c))}</em></span><b>${esc(c.name)}</b><small>${esc(c.work_name || '')}</small></a>`).join('')}</div></section>` : ''}
      <section class="cast-sec"><h3>${t('作品')}<small>${t('{n} 部 · 按张数', {n: number(works.filter(w => !w.pseudo).length)})}</small></h3><div class="cast-works">${works.slice(0, 24).map(w => `<button class="cast-wt" data-tab="${esc(w.key)}">${fan(w.chars)}<b>${esc(w.name)}</b><small>${t('{n} 位 · {images} 张', {n: number(w.chars.length), images: number(w.images)})}${w.recent ? ` · <em>+${number(w.recent)}</em>` : ''}</small></button>`).join('')}</div></section>`;
  }
  if (cast.tab === 'recent') {
    const rec = state.chars.filter(c => recentOf(c)).sort((a, b) => recentOf(b) - recentOf(a));
    return `<div class="cast-head"><div><h2>${t('最近 30 天在收')}</h2><div class="cast-tags"><span class="hot">${t('{n} 位角色', {n: number(rec.length)})}</span><span>${t('按新增张数')}</span></div></div></div>${rec.length ? `<section class="cast-sec"><div class="cast-posters">${rec.slice(0, 120).map(c => poster(c, (c.work_name ? esc(c.work_name) + ' · ' : '') + t('近 30 天 +{n}', {n: number(recentOf(c))}))).join('')}</div></section>` : `<p class="empty">${t('最近 30 天没有新图。')}</p>`}`;
  }
  const w = works.find(x => x.key === cast.tab);
  if (!w) { cast.tab = 'all'; return castPane(works); }
  const real = !['oc', 'other'].includes(w.key), info = state.index?.works?.find(x => x.id === w.key);
  const manage = real && info ? `<div class="cast-manage">${watchSwitch(info)}<button class="quiet" data-shelf-edit="${esc(w.key)}">${t('编辑作品…')}</button></div>` : '';
  return `<div class="cast-head">${fan(w.chars)}<div><h2>${esc(w.name)}</h2><div class="cast-tags"><span>${t('{n} 位角色', {n: number(w.chars.length)})}</span><span>${t('{n} 张', {n: number(w.images)})}</span>${w.recent ? `<span class="hot">${t('近 30 天 +{n}', {n: number(w.recent)})}</span>` : ''}</div>${w.pseudo ? `<p class="cast-note">${esc(PSEUDO_NOTES[w.key])}</p>` : ''}${manage}</div>${real ? `<button class="primary" data-work-lib="${esc(w.key)}">${t('在图库里看全部')}</button>` : ''}</div>${tiers(w.chars, false)}`;
}
function castList() {
  const base = cast.list || state.chars, rows = castRows(base, cast);
  const max = Math.max(1, ...base.map(c => c.count)), shown = rows.slice(0, cast.limit || 300);
  const th = (k, label) => `<th data-sort="${k}" class="${cast.sort === k ? 'on' : ''}" aria-sort="${cast.sort === k ? (cast.dir < 0 ? 'descending' : 'ascending') : 'none'}">${label}${cast.sort === k ? (cast.dir < 0 ? ' ↓' : ' ↑') : ''}</th>`;
  return `<div class="cast-buckets">${BUCKETS.map(([k, label, f]) => `<button data-bucket="${k}" class="${cast.bucket === k ? 'on' : ''}">${label}<small>${number(base.filter(f).length)}</small></button>`).join('')}</div>
    <table class="cast-table"><thead><tr>${th('name', t('角色'))}${th('work', t('作品'))}${th('count', t('张数'))}${th('recent', t('近 30 天'))}${th('last', t('最后加入'))}${th('skins', t('造型'))}</tr></thead><tbody>
    ${shown.map(c => { const a = catalog.activity.get(c.id), skins = (c.variants || []).filter(v => v.count).length, orig = c.tag && c.tag !== c.name ? c.tag.replaceAll('_', ' ') : '';
      return `<tr data-char="${esc(c.id)}"><td><div class="cast-who">${avatar(c, 36)}<span><b>${esc(c.name)}</b>${orig ? `<small>${esc(orig)}</small>` : ''}</span></div></td><td>${esc(c.kind === 'oc' ? t('原创角色') : c.work_name || '—')}</td><td><div class="cast-bar"><i style="width:${Math.max(4, Math.log(c.count + 1) / Math.log(max + 1) * 110)}px"></i>${number(c.count)}</div></td><td>${a?.recent ? `<span class="cast-plus">+${number(a.recent)}</span>` : '<span class="dim">—</span>'}</td><td class="mono">${esc(a?.last || '')}</td><td>${skins || '<span class="dim">—</span>'}</td></tr>`; }).join('')}
    </tbody></table>${rows.length > shown.length ? `<button class="quiet cast-more" id="castMore">${t('再显示 300 位（共 {n} 位）', {n: number(rows.length)})}</button>` : ''}`;
}
export function drawCast() {
  if (!onPage(state.route, 'characters') || state.route.section) return;
  const works = castWorks(), root = $('charIndex');
  // 带前缀：单独一个 'shelf' 会套上「作品」页的 .shelf（窄的 flex 列，#274）。
  root.className = 'char-index cast cast-' + cast.mode;
  reconcileSections(root, cast.mode === 'list' ? [['list', `<div class="cast-listview">${castList()}</div>`]]
    : [...(cast.list ? [] : [['tabs', castTabs(works)]]), ['pane', `<div class="cast-pane">${castPane(works)}</div>`]]);
  const foot = (state.tagged || []).length ? `<p class="cast-tagged">${t('不算角色、只当标签：')}${state.tagged.map(x => `<a href="${charHash(x.id)}">${esc(x.name)}</a><small>${number(x.count)}</small>`).join('')}<button class="link-button" data-tagged-manage>${t('管理…')}</button></p>` : '';
  let el = root.querySelector('.cast-tagged'); if (el) el.remove();
  if (foot) root.insertAdjacentHTML('beforeend', foot);
  const tagged = $('taggedButton'); if (tagged) tagged.innerHTML = t('只当标签') + (state.tagged?.length ? ` <small>${number(state.tagged.length)}</small>` : '');
  if (cast.pop) $('castPopQ')?.focus();
  watchPosters(root);
}

// 只当标签（#293）：吉祥物和玩家替身（包括默认的那些）是标签，不是角色。在一处看到全部，把一个角色变成标签，或把它
// 变回来。
export async function openTagged() {
  const d = openDialog({title: t('只当标签的角色'), cls: 'tagged-dialog', backdropClose: true,
    body: `<p class="muted-note">${t('吉祥物、玩家化身（老师、博士、训练员这类）可以只当标签：不算角色，不出现在角色列表里，同框也不算多人图；图上仍挂着这个标签，可以按它筛选。')}</p>
    <div class="tagged-list" id="taggedList"></div>`,
    actions: `<button type="button" class="quiet" id="taggedAdd">${t('把一个角色设为只当标签…')}</button><div class="spacer"></div><button value="ok" class="primary">${t('完成')}</button>`});
  const list = () => {
    const rows = state.tagged || [];
    d.querySelector('#taggedList').innerHTML = rows.length ? rows.map(x => `<div class="tagged-row"><a href="${charHash(x.id)}" data-close>${esc(x.name)}</a><small>${t('{n} 张', {n: number(x.count)})}</small><div class="spacer"></div><button type="button" class="quiet small" data-untag="${esc(x.id)}">${t('改回角色')}</button></div>`).join('')
      : `<p class="muted-note">${t('现在没有只当标签的角色。')}</p>`;
  };
  const refresh = async () => { await loadTagged().catch(e => error(readError(e))); if (d.open) list(); drawCast(); };
  d.addEventListener('click', async e => {
    const b = e.target.closest('[data-untag]'); if (!b) return;
    b.disabled = true;
    const x = state.tagged.find(r => r.id === b.dataset.untag);
    await toggleMascot({id: x.id, name: x.name, mascot: true}); await refresh();
  });
  d.querySelector('#taggedAdd').onclick = () => openPicker({title: t('把一个角色设为只当标签'), jump: true, exclude: (state.tagged || []).map(x => x.id),
    sentence: e => t('{name} 不再算角色，只当标签', {name: e.name}),
    pick: async e => { await toggleMascot({id: e.id, name: e.name, mascot: false}); await refresh(); if (!d.open) openModal(d, {backdropClose: true, id: 'tagged'}); }});
  list();
  refresh();
}

// ---------- 资料库 · 作品：作品树、它的 Danbooru 关联和关注列表（#198） ----------
// 关注一个作品，意味着索引检查会去问 Danbooru（只发作品名）有哪些本地索引还没有的角色。
export const shelf = {q: '', filter: 'all'};
export async function renderShelf() {
  const route = state.route, token = session.token;
  $('hint').textContent = t('点作品名看它的角色');
  drawShelf();
  try { await loadIndex(); } catch (e) { if (route === state.route && token === session.token) readFailure($('charIndex'), e); return; }
  if (route !== state.route || token !== session.token) return;
  if (onPage(state.route, 'characters') && state.route.section === 'works') { renderHeading(); drawShelf(); }
}
// 关注开关需要一个用户核对过的 Danbooru 作品 tag；索引检查按这个 tag 搜索。
function watchSwitch(w) {
  const on = state.index?.watch.works.includes(w.id);
  return `<label class="shelf-watch${w.watchable ? '' : ' off'}" title="${w.watchable ? t('定期检查 Danbooru 上这部作品还没收录的角色（只发送作品名）') : t('先在「编辑作品」里核对 Danbooru 作品 tag，才能关注')}"><input type="checkbox" class="switch" data-watch="${esc(w.id)}" ${on ? 'checked' : ''} ${w.watchable || on ? '' : 'disabled'}><span>${t('关注新角色')}</span></label>`;
}
function workLink(w) {
  if (w.tag) return `<a class="shelf-link ok" href="https://danbooru.donmai.us/posts?tags=${encodeURIComponent(w.tag)}" target="_blank" rel="noreferrer noopener" title="${t('在 Danbooru 查看')}">${esc(w.tag)}</a>`;
  if (w.status === 'pending') return `<span class="shelf-link warn">${t('链接待确认')}</span>`;
  return `<span class="shelf-link">${t('未关联 Danbooru')}</span>`;
}
export function drawShelf() {
  if (!onPage(state.route, 'characters') || state.route.section !== 'works') return;
  const root = $('charIndex'), x = state.index;
  root.className = 'char-index shelf';
  if (!x?.works) { root.innerHTML = `<p class="muted-note shelf-empty">${t('正在读取作品…')}</p>`; return; }
  const missing = x.check?.missing || [], oldTags = x.watch.pending_tags || [];
  const fresh = w => w.tag ? missing.filter(r => r.copyright === w.tag).length : 0;
  const people = new Map();
  for (const c of state.chars) if (c.work_id) people.set(c.work_id, (people.get(c.work_id) || 0) + 1);
  const {rows: shown, watched, pending: pendingN} = shelfRows(x, shelf);
  const chip = (k, label, n) => `<button class="chip-filter${shelf.filter === k ? ' on' : ''}" data-shelf-filter="${k}">${label}${n ? ` <span>${number(n)}</span>` : ''}</button>`;
  const todo = oldTags.length ? `<details class="shelf-todo" data-fold-key="shelfTodo"${prefs.get('settingsOpen', {}).shelfTodo ? ' open' : ''}><summary data-more="${t('展开')}" data-less="${t('收起')}"><b>${t('待处理')}</b><span>${t('{n} 个以前关注的 Danbooru 作品还没对应到本地作品，对应后才会检查它们的新角色', {n: number(oldTags.length)})}</span></summary>
      ${oldTags.map(tag => `<div class="shelf-todo-row"><span class="mono">${esc(tag)}</span><span class="muted-note">${esc(WORK_NAMES[tag] || '')}</span><div class="spacer"></div><button class="quiet small" data-shelf-adopt="${esc(tag)}">${t('对应到作品…')}</button></div>`).join('')}</details>` : '';
  const found = missing.length ? `<div class="shelf-found"><span class="local-dot busy"></span><span>${t('上次检查发现 {n} 个还没收录的角色', {n: `<b>${number(missing.length)}</b>`})}</span><div class="spacer"></div><button class="primary" data-index-add>${t('查看并添加…')}</button></div>` : '';
  const row = ([w, depth]) => {
    const n = fresh(w), chars = people.get(w.id) || 0;
    return `<div class="shelf-row" style="--depth:${depth}">
      <button class="shelf-name" data-shelf-open="${esc(w.id)}" title="${t('看这部作品的角色')}"><b>${esc(w.name)}</b>${w.qualifier ? `<small>${esc(w.qualifier)}</small>` : ''}</button>
      ${workLink(w)}
      <span class="shelf-num">${chars ? t('{n} 位角色', {n: number(chars)}) + ' · ' : ''}${t('{n} 张', {n: number(w.count || 0)})}</span>
      ${n ? `<button class="shelf-new" data-index-add="${esc(w.tag)}" title="${t('Danbooru 上有、本地索引还没有的角色')}">${t('+{n} 个新角色', {n: number(n)})}</button>` : '<span></span>'}
      ${watchSwitch(w)}
      <button class="quiet small" data-shelf-edit="${esc(w.id)}">${t('编辑…')}</button></div>`;
  };
  root.innerHTML = `${todo}${found}
    <div class="shelf-bar"><div class="chips-row">${chip('all', t('全部'), x.works.length)}${chip('watched', t('已关注'), watched.size)}${pendingN ? chip('pending', t('链接待确认'), pendingN) : ''}</div><div class="spacer"></div>
      <span class="watch-add"><input id="watchAdd" value="${esc(watchDraft)}" placeholder="${t('关注 Danbooru 上的作品…')}" spellcheck="false" autocomplete="off" role="combobox" aria-expanded="false" aria-controls="watchMenu"><div id="watchMenu" class="menu menu-drop watch-menu" role="listbox" hidden></div></span></div>
    <div class="shelf-list">${shown.map(row).join('') || `<p class="muted-note shelf-empty">${x.works.length ? t('没有符合条件的作品') : t('还没有作品。')}</p>`}</div>`;
  wireWatchAdd(x, adoptWatch); wireFolds(root);
}
async function saveWatchList(works) {
  try { await saveWatch(works); }
  catch (e) { toast(e.message); }
  renderIndexBody(); renderHeading();
}
// 来自 Danbooru 搜索：带这个 tag 的已有本地作品，或用户为它挑选或新建的一个。
async function adoptWatch(r) {
  try {
    const id = await watchTarget(r, (title, works) => chooseWork(title, works, '', t('新建作品')));
    if (id === null) return;
    watchDraft = '';
    if (!state.index.watch.works.includes(id)) await saveWatchList([...state.index.watch.works, id]); else renderIndexBody();
  } catch (e) { toast(e.message); }
}
// 图片设为角色之后（#293）：它们属于同一个作品时，toast 提议关注这个作品的新角色，或说明它已在关注。没有核对过的
// Danbooru tag 的作品会打开它的对话框去找一个。
export async function offerWatch(ids) {
  const chars = await Promise.all(ids.map(id => state.charMap.get(id) || fetchChar(id).catch(() => null)));
  const works = [...new Set(chars.map(c => c?.kind === 'oc' ? null : c?.work_id).filter(Boolean))];
  if (works.length !== 1) return;
  let x = state.index; if (!x?.works) { try { x = await loadIndex(); } catch { return; } }
  const w = x?.works?.find(y => y.id === works[0]); if (!w) return;
  if (x.watch.works.includes(w.id)) { toastMore(' · ' + t('「{name}」已关注', {name: w.name})); return; }
  toastMore('', w.watchable ? t('关注「{name}」的新角色', {name: w.name}) : t('关注「{name}」…', {name: w.name}), async () => {
    if (!w.watchable) { const r = await workDialog(w, {watch: true}); if (r?.watch) toast(t('已关注「{name}」', {name: r.name})); return; }
    try { await saveWatch([...state.index.watch.works, w.id]); toast(t('已关注「{name}」', {name: w.name})); renderIndexBody(); renderHeading(); }
    catch (e) { toast(e.message); }
  });
}
export async function createWork() {
  const w = await workDialog(null);
  if (w) toast(w.watch ? t('已新建作品「{name}」并关注它的新角色', {name: w.name}) : t('已新建作品「{name}」', {name: w.name}));
}
// 新建作品和编辑作品共用一个对话框：一个填 tag（名字为空时也填名字）的 Danbooru 搜索、名字、用户核对过的 tag、可选的
// 上级作品，以及（有 tag 时）关注它的新角色（#293）。
// 导出给样式清单页调用（tools/styleguide/live.js，#352）：改名或改参数时，tools/check_styleguide.py 会报出来。
export function workDialog(w, {watch = false} = {}) {
  const works = state.index?.works || [], watched = new Set(state.index?.watch?.works || []);
  return new Promise(resolve => {
    const parents = works.filter(x => !w || x.id !== w.id);
    const body = `${watch && !w?.tag ? `<p class="dialog-lead">${t('关注要用 Danbooru 上的作品 tag。在下面找到这部作品，选中后保存。')}</p>` : ''}
      <label class="field"><span>${t('在 Danbooru 上找')}</span><div class="combo"><input id="wdFind" autocomplete="off" spellcheck="false" placeholder="${t('输入作品名，中文、日文、英文都行')}"><div class="combo-menu" id="wdFindMenu" hidden></div></div>
        <small>${t('选中后自动填好下面的 tag，名称空着时也一起填上。只发送你输入的文字。')}</small></label>
      <label class="field"><span>${t('名称')}</span><input id="wdName" name="title" value="${esc(w?.name || '')}" maxlength="120" required autocomplete="off">
        <small>${t('界面上显示的名字，和 Danbooru tag 无关。')}</small></label>
      <label class="field"><span>${t('Danbooru 作品 tag')}<em class="opt">${t('可不填')}</em></span><input id="wdTag" name="tag" value="${esc(w?.tag || '')}" placeholder="${t('例如 blue_archive')}" spellcheck="false" autocomplete="off">
        <small id="wdTagNote"></small></label>
      <label class="field"><span>${t('上级作品')}<em class="opt">${t('可不填')}</em></span><select id="wdParent" name="parent"><option value="">${t('无')}</option>${parents.map(x => `<option value="${esc(x.id)}" ${w?.parent_id === x.id ? 'selected' : ''}>${esc(x.name)}</option>`).join('')}</select>
        <small>${t('只在这部作品属于某个系列时填，比如「Fate/Grand Order」的上级是「Fate 系列」。在图库里按上级作品筛选时，下级作品的图也算在内。')}</small></label>
      <label class="check-row" id="wdWatchRow"><input type="checkbox" id="wdWatch"><span><b>${t('关注这部作品的新角色')}</b><small>${t('定期检查 Danbooru 上这部作品还没收录的角色（只发送作品名），检查前会确认联网')}</small></span></label>`;
    const d = openDialog({title: w ? t('编辑作品') : t('新建作品'), cls: 'work-dialog', body, backdropClose: false, focus: w && !watch ? '#wdName' : '#wdFind',
      actions: `<button type="button" class="quiet" data-close>${t('取消')}</button><button value="ok" class="primary">${w ? t('保存') : t('新建')}</button>`,
      onClose: async d => {
        const data = new FormData(d.querySelector('form')), ok = d.returnValue === 'ok', watch = d.querySelector('#wdWatch').checked;
        if (!ok) { resolve(null); return; }
        const name = String(data.get('title')).trim(), parent = data.get('parent'), tag = String(data.get('tag')).trim();
        try {
          const id = await saveWork(w?.id, {name, parent, tag});
          if (watch && tag && !state.index.watch.works.includes(id)) await saveWatch([...state.index.watch.works, id]);
          await loadWorkTree().catch(e => error(readError(e)));
          await refreshMeta().catch(e => error(readError(e)));
          renderIndexBody(); renderHeading();
          resolve({id, name, watch});
        } catch (err) { toast(err.message); resolve(null); }
      }});
    const q = sel => d.querySelector(sel), already = w && watched.has(w.id);
    let filled = '';  // 搜索填进来的名字，下一次挑选会替换它
    const tagNote = () => {
      const tag = q('#wdTag').value.trim(), owner = tag && works.find(x => x.tag === tag && x.id !== w?.id);
      q('#wdTagNote').innerHTML = owner ? `<span class="warn">${t('图库里的「{name}」已经用了这个 tag', {name: esc(owner.name)})}</span>`
        : w?.status === 'pending' && !tag ? t('链接还没确认：核对过就填上 tag；留空保存表示不关联。')
        : t('用上面的搜索填，或手动填你核对过的 tag，不会按名字自动关联。填了才能关注这部作品的新角色。');
      q('#wdWatchRow').hidden = !tag || already; if (!tag) q('#wdWatch').checked = false;
    };
    q('#wdTag').oninput = tagNote; tagNote();
    q('#wdFind').addEventListener('keydown', e => { if (!composing(e) && e.key === 'Enter') e.preventDefault(); });
    combo(q('#wdFind'), q('#wdFindMenu'), (text, show) => {
      if (!text) { show([]); return; }
      show([{label: 'Danbooru', items: [], note: t('正在查 Danbooru…')}]);
      remoteNames('/api/works?' + new URLSearchParams({q: text, remote: 1})).then(r => show([r.found
        ? {label: 'Danbooru', items: r.found.map(x => { const mine = works.find(y => y.tag === x.tag && y.id !== w?.id);
          return {tag: x.tag, label: workName(x.tag) || x.tag, detail: x.tag, extra: mine ? t('图库里已有：{name}', {name: mine.name}) : t('D 站 {n}', {n: number(x.posts)})}; }),
          note: r.found.length ? '' : t('没有找到这个作品。可以换个写法，比如英文或日文名')}
        : {label: 'Danbooru', items: [], note: remoteNote(r), allow: r.unauthorized}]));
    }, r => {
      q('#wdTag').value = r.tag;
      const name = q('#wdName');
      if (!name.value.trim() || name.value === filled) name.value = filled = r.label;
      tagNote(); q('#wdFind').value = '';
      if (!already && (!w || watch)) q('#wdWatch').checked = true;
    });
  });
}
// 上次索引检查在 Danbooru 上为关注的作品找到的角色；添加一个会下载约 20 张公开参考图。
export function openIndexAdd(tag = '') {
  const token = session.token, x = state.index, list = (x?.check?.missing || []).filter(r => !tag || r.copyright === tag);
  if (!list.length) return;
  const body = `<p class="muted-note">${t('这些角色在 Danbooru 上有、本地索引还没有。添加时每个角色下载约 20 张公开参考图（约 {mb} MB），算好特征后重新扫描图库。', {mb: x.mb_per_character})}</p>
    <div class="index-add-list"><table class="settings-table index-table"><tr><th><input type="checkbox" id="iaAll" aria-label="${t('全选')}" checked></th><th>${t('角色 tag')}</th><th>${t('作品')}</th><th>${t('D 站图数')}</th></tr>
      ${list.map(r => `<tr><td><input type="checkbox" data-pick="${esc(r.short_tag)}" ${indexSkip.has(r.short_tag) ? '' : 'checked'}></td><td><a class="link" href="https://danbooru.donmai.us/posts?tags=${encodeURIComponent(r.tag)}" target="_blank" rel="noreferrer noopener">${esc(r.tag)}</a></td><td>${esc(x.works.find(w => w.tag === r.copyright)?.name || workName(r.copyright))}</td><td>${number(r.post_count)}</td></tr>`).join('')}</table></div>`;
  const d = openDialog({title: t('添加新角色'), cls: 'index-add', body, backdropClose: false,
    actions: `<span class="muted-note" id="iaNote"></span><div class="spacer"></div><button value="cancel" class="quiet">${t('取消')}</button><button value="ok" class="primary" id="iaApply"></button>`,
    onClose: async d => {
      const tags = picks().filter(b => b.checked).map(b => b.dataset.pick), ok = d.returnValue === 'ok';
      if (!ok || !tags.length || token !== session.token) return;
      try { await indexAction({action: 'apply', tags, allow_network: true}); toast(t('开始添加 {n} 个角色，进度在任务面板里', {n: number(tags.length)})); pollIndex(); renderIndexBody(); }
      catch (e) { toast(e.message); }
    }});
  const q = s => d.querySelector(s), picks = () => [...d.querySelectorAll('[data-pick]')];
  const note = () => {
    const n = picks().filter(b => b.checked).length, mb = n * x.mb_per_character;
    q('#iaApply').textContent = t('添加 {n} 个（约 {mb} MB）', {n: number(n), mb: mb >= 10 ? Math.round(mb) : mb.toFixed(1)}); q('#iaApply').disabled = !n;
    q('#iaNote').textContent = mb > 500 ? t('下载量超过 500 MB，网络不方便时可以分几次添加') : '';
    indexSkip = new Set([...indexSkip, ...picks().filter(b => !b.checked).map(b => b.dataset.pick)].filter(t => !picks().some(b => b.checked && b.dataset.pick === t)));
  };
  for (const b of picks()) b.onchange = note;
  q('#iaAll').onchange = e => { for (const b of picks()) b.checked = e.target.checked; note(); };
  note();
}

// ---------- 关注作品：作品树下面的选择器 ----------
let indexSkip = new Set(), watchDraft = '';
// 作品选择器：先是你的图库已有的作品（即时，本地名字和别名），然后是按输入文字查到的 Danbooru copyright tag（只发出
// 文字）。挑一个就得到一个确切的 tag。
export function wireWatchAdd(x, add) {
  const token = session.token, input = $('watchAdd'), menu = $('watchMenu'), watched = new Set(x.watch.works);
  const here = () => token === session.token && input === $('watchAdd');
  const row = (r, i, on) => `<button class="menu-item watch-option${on ? ' on' : ''}" data-i="${i}" role="option" ${watched.has(r.id) || (r.id && !r.watchable) ? 'disabled' : ''}>
    <span class="watch-name">${esc(r.name || workName(r.tag))}</span><small>${esc(r.tag || t('D 站上还没有这个作品'))}</small>
    <em>${watched.has(r.id) ? t('已关注') : [r.count ? t('你有 {n} 张', {n: number(r.count)}) : '', r.posts ? t('D 站 {n}', {n: number(r.posts)}) : ''].filter(Boolean).join(' · ')}</em></button>`;
  // 每个部分都有标题；说明（可能带「允许查询…」按钮，所以是 HTML）放在最下面。
  const draw = (sections, index) => {
    let i = 0;
    const note = sections.map(sec => sec.note).find(Boolean) || '';
    return sections.filter(sec => sec.items.length).map(sec => `<div class="palette-label">${sec.label}</div>` + sec.items.map(r => { const at = i++; return row(r, at, at === index); }).join('')).join('')
      + (note ? `<p class="muted-note watch-note">${note}</p>` : '');
  };
  let remoteTimer = 0;
  const search = async (q, show) => {
    let local; try { local = await searchWorks(q); } catch (e) { if (here()) show([{label: '', items: [], note: esc(readError(e))}]); return; }
    if (!here()) return;
    if (!q) { show([{label: t('你的图库里还没关注的作品'), items: local.filter(r => !watched.has(r.id)).slice(0, 10)}]); return; }
    const mine = local.slice(0, 8);
    show([{label: t('你的图库里有'), items: mine, note: t('正在查 Danbooru…')}]);
    clearTimeout(remoteTimer);
    remoteTimer = setTimeout(async () => {
      const result = await remoteNames('/api/works?' + new URLSearchParams({q, remote: 1})), remote = result.found;
      if (!here()) return;
      const posts = new Map((remote || []).map(r => [r.tag, r.posts]));
      for (const r of mine) if (posts.has(r.tag)) r.posts = posts.get(r.tag);
      const known = new Set(mine.map(r => r.tag));
      const others = (remote || []).filter(r => !known.has(r.tag));
      show([{label: t('你的图库里有'), items: mine}, {label: 'Danbooru', items: others,
        note: remote === null ? (result.unauthorized ? esc(remoteNote(result)) + ' ' + namesAllowButton : esc(t('{reason}，可以稍后再试', {reason: remoteNote(result)}))) : !mine.length && !others.length ? t('没有找到这个作品。可以换个写法，比如英文或日文名') : ''}]);
    }, 450);
  };
  combo(input, menu, search, add, {id: 'watch-menu', delay: 150, draw, first: list => list.findIndex(r => !watched.has(r.id)),
    allowed: r => !watched.has(r.id) && !(r.id && !r.watchable), typed: value => { watchDraft = value; }});
  if (document.activeElement === input || watchDraft) { input.focus(); input.setSelectionRange(input.value.length, input.value.length); }
}

// 资料库各页的处理函数。页面启动时调用一次。
export function wireCatalog() {
  // 海报的查看器（#293）：指针在海报上横向和纵向移动、离开海报，以及有焦点的海报上的方向键。
  $('charIndex').addEventListener('mousemove', lookMove);
  $('charIndex').addEventListener('mouseout', e => { const a = e.target.closest?.('.cast-poster.look'); if (a && !a.contains(e.relatedTarget)) lookMove({type: 'leave', target: a}); });
  $('charIndex').addEventListener('keydown', e => { const a = e.target.closest?.('.cast-poster.look'); if (a && !composing(e) && ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) { e.preventDefault(); lookKey(a, e.key); } });
  $('charIndex').addEventListener('pointerdown', lookDown);
  $('charIndex').addEventListener('pointermove', lookPull);
  for (const kind of ['pointerup', 'pointercancel']) $('charIndex').addEventListener(kind, lookLift);
  $('charIndex').addEventListener('click', e => { const a = e.target.closest?.('.cast-poster.look'); if (a?.dragged && performance.now() - a.dragged < 500) { e.preventDefault(); e.stopPropagation(); } }, true);
  $('charIndex').addEventListener('click', e => {
    if (!onPage(state.route, 'characters')) return;
    const hit = e.target;
    if (hit.closest('[data-tagged-manage]')) { openTagged(); return; }
    const tab = hit.closest('[data-tab]');
    if (tab) { cast.tab = tab.dataset.tab; cast.popQ = ''; setPop(false, 'silent', false); drawCast(); $('viewport').scrollTop = 0; $('charIndex').querySelector('.cast-tab.on')?.scrollIntoView({inline: 'center', block: 'nearest'}); return; }
    if (hit.closest('#castAll')) { setPop(!cast.pop); return; }
    const lib = hit.closest('[data-work-lib]'); if (lib) { go({page: 'library', work: lib.dataset.workLib}); return; }
    const s = hit.closest('[data-sort]');
    if (s) { const k = s.dataset.sort; cast.dir = cast.sort === k ? -cast.dir : (k === 'name' || k === 'work' ? 1 : -1); cast.sort = k; drawCast(); return; }
    const b = hit.closest('[data-bucket]'); if (b) { cast.bucket = b.dataset.bucket; cast.limit = 300; drawCast(); return; }
    if (hit.closest('#castMore')) { cast.limit = (cast.limit || 300) + 300; drawCast(); return; }
    const row = hit.closest('tr[data-char]'); if (row) go(charRoute(row.dataset.char));
  });
  onTyped($('charIndex'), e => { if (e.target.id === 'castPopQ') { cast.popQ = e.target.value; const pos = e.target.selectionStart; drawCast(); const i = $('castPopQ'); i?.setSelectionRange(pos, pos); } });
  $('charIndex').addEventListener('click', async e => {
    if (!onPage(state.route, 'characters')) return;
    const hit = e.target, works = state.index?.works || [];
    const f = hit.closest('[data-shelf-filter]'); if (f) { shelf.filter = f.dataset.shelfFilter; drawShelf(); return; }
    const open = hit.closest('[data-shelf-open]'); if (open) { cast.tab = open.dataset.shelfOpen; go({page: 'characters'}); return; }
    const edit = hit.closest('[data-shelf-edit]'); if (edit) { const w = works.find(x => x.id === edit.dataset.shelfEdit); if (w) workDialog(w); return; }
    const adopt = hit.closest('[data-shelf-adopt]'); if (adopt) { adoptWatch({tag: adopt.dataset.shelfAdopt, name: WORK_NAMES[adopt.dataset.shelfAdopt]}); return; }
    const add = hit.closest('[data-index-add]'); if (add) openIndexAdd(add.dataset.indexAdd || '');
  });
  $('charIndex').addEventListener('change', e => {
    const b = e.target.closest('[data-watch]'); if (!b || !state.index) return;
    const id = b.dataset.watch, list = state.index.watch.works.filter(w => w !== id);
    saveWatchList(b.checked ? [...list, id] : list);
  });
}

// 资料库的页头：三个页签各写自己的数量；角色页签另有「N 个自建角色能对上 Danbooru」的按钮。
function indexHead(r) {
  const sub = r.section === 'artists' ? dirHeading() : r.section === 'works'
    ? t('{n} 部作品', {n: number(state.index?.works?.length || 0)}) + (state.index?.watch?.works.length ? ' · ' + t('关注 {n} 部', {n: number(state.index.watch.works.length)}) : '')
    : t('{n} 位角色 · {works} 部作品', {n: number(state.chars.length), works: number(new Set(state.chars.map(workKey).filter(k => k !== 'oc' && k !== 'other')).size)});
  const actions = [];
  if (!r.section && state.localLinks?.length) {
    const b = make('button', 'quiet', t('{n} 个自建角色能对上 Danbooru', {n: number(state.localLinks.length)}));
    b.title = t('补上中文名、别名和作品，和已有角色重复的可以合并'); b.onclick = openLocalLinks;
    actions.push(b);
  }
  return {title: t('资料库'), sub, actions};
}
// 元数据刷新角色目录，撤销后还要重读作品计数或「只当标签」清单；画师子页由 ui/artist_dir.js 自己重读。
implement({characters: {head: indexHead,
  // 计数到达后只重画目录。
  onMeta: ({directory}) => { if (directory && (state.route.section || !$('charSearch')?.value)) renderIndexBody(state.chars); },
  render: opts => renderIndex(opts), refresh: async why => {
  if (state.route.section === 'artists') { refreshArtistDir(why); return; }
  if (why === 'nsfw') { forgetActivity(); readActivity(); }
  if (why !== 'undo') return;
  const route = state.route;
  if (route.section === 'works') { await renderShelf(); return; }
  await loadTagged().catch(e => error(readError(e)));
  if (state.route !== route) return;
  renderHeading(); await searchIndex();
}}});
