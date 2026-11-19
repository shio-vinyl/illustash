// 画廊的展示页（#/showcase/<id>，#710）：拼贴、杂志、展厅这类只放前几张、为了好看的展示方式。显示全部图的那几种（瀑布、色彩）
// 是图库网格，不在这里。哪个画廊用哪一种、放几张在 core/gallery_shows.js 的登记表；框在 core/layout.js 算；样式在
// css/galleries.css 的 .stage.show-<id>。
//
// 加一种展示页：在 STAGES 里加一项 {boxes(ratios, size, g), …}，布局函数写进 core/layout.js。每张图是同一个部件（piece），
// 按框摆（transform、宽高），框上的 rot、z、tape、no、role 写成角度、叠放次序、胶带、编号和样式类；lead 框放标题和导语。
//
// 点图开详情卡片，双击看大图（列表借给详情和大图翻页：grid.js 的 showList）；按住拖动把图拖进画廊（ui/drag.js）。
// 末尾「全部 N 张」去显示全部图的那一种（ALL_SHOW，瀑布）。不能选：选择和批量操作在瀑布、色彩里做。
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {chooseShow, galleryHref, galleryOf, isFav, isManual, showcaseOf} from '../core/galleries.js';
import {ALL_SHOW, showFor, showOf} from '../core/gallery_shows.js';
import {aspect, collage, hall, hashSeed, magazine} from '../core/layout.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {indexOf, inspect, openViewer} from './detail.js';
import {$, error} from './dom.js';
import {wireLift} from './drag.js';
import {attr, cls, css, show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {kindText} from './gallery_actions.js';
import {setNsfw, showList} from './grid.js';
import {keyed} from './keyed.js';
import {thumbs} from './motion.js';
import {renderHeading} from './shell.js';
import {setTint} from './tint.js';

// 展示页的画法。boxes 返回 {boxes, height, width?, lead?}：width 给了就是横向延伸的墙（展厅），容器横向滚动。
//   size：{width, height}，舞台的可用宽度和高度（高度是视口里扣掉页头以后的，展厅用它定墙高）
const STAGES = {
  collage: {boxes: (ratios, {width}, g) => collage(ratios, {width, seed: hashSeed(`gallery:${g.id}`)})},
  magazine: {boxes: (ratios, {width}) => magazine(ratios, {width})},
  hall: {boxes: (ratios, {height}) => ({...hall(ratios, {height}), height})},
};

const PAGE = tpl`<div class="stage-page"><div class="stage" data-ref="stage"><div class="stage-lead" data-ref="lead" hidden><small data-ref="kicker"></small><b
  data-ref="title"></b><span data-ref="line"></span></div></div><div class="stage-foot" data-ref="foot" hidden><button class="btn" type="button"
  data-ref="all"></button></div><div data-ref="empty" hidden></div></div>`;
const PIECE = tpl`<figure class="piece"><span class="piece-frame" data-ref="frame"><img data-ref="img" alt="" draggable="false" decoding="async"><i class="piece-tape"
  data-ref="tape" hidden></i></span><figcaption data-ref="cap"><b class="piece-no" data-ref="no"></b><span data-ref="name"></span></figcaption></figure>`;
const nameOf = x => x.memberships?.[0]?.name || '';

// 一张图。框（x、y、w、h）写成定位，其余写成样式：rot 角度、z 叠放、tape 胶带、no 编号、role 样式类。
function piece(box) {
  const {el, refs} = PIECE();
  let shown = '';
  const update = b => {
    const x = b.item;
    css(el, 'transform', `translate(${Math.round(b.x)}px,${Math.round(b.y)}px)${b.rot ? ` rotate(${b.rot}deg)` : ''}`);
    css(el, 'width', `${Math.round(b.w)}px`); css(el, 'height', `${Math.round(b.h)}px`);
    css(el, 'z-index', b.z ? String(b.z) : null);
    attr(el, 'class', `piece${b.role ? ' is-' + b.role : ''}`);
    el.dataset.id = x.uuid;
    if (shown !== x.uuid) { shown = x.uuid; attr(refs.img, 'src', `/image?id=${encodeURIComponent(x.uuid)}&size=${b.w > 360 ? 'preview' : 'medium'}`); thumbs.mark(refs.img, x.uuid); }
    setTint(el, x.color || null);
    show(refs.tape, !!b.tape);
    if (b.tape) { css(refs.tape, 'left', `${Math.round(b.tape.at * 100)}%`); css(refs.tape, 'transform', `translateX(-50%) rotate(${b.tape.rot}deg)`); }
    text(refs.no, b.no ? String(b.no).padStart(2, '0') : ''); show(refs.no, !!b.no);
    text(refs.name, nameOf(x));
    show(refs.cap, !!(b.no || nameOf(x)));
  };
  update(box);
  return {el, update, dispose() {}};
}

let view = null;
function stageView(scope) {
  const {el, refs} = PAGE();
  $('groupsPage').replaceChildren(el);
  const pieces = keyed(refs.stage, {key: b => b.item.uuid, create: piece, scope});
  let data = null, kind = '', g = null;
  // 按现在的宽高排一次。宽度变了（窗口、活页条抽屉）重排；展厅的墙高跟着视口高。
  function layout() {
    if (!data?.items?.length || !g) return;
    const stage = STAGES[kind], width = refs.stage.clientWidth, height = Math.max(380, Math.min(560, $('viewport').clientHeight - 220));
    const got = stage.boxes(data.items.map(aspect), {width, height}, g);
    css(refs.stage, 'height', `${Math.ceil(got.height)}px`);
    css(refs.stage, 'width', got.width ? `${Math.ceil(got.width)}px` : null);
    pieces.set(got.boxes.map(b => ({...b, item: data.items[b.i]})));
    show(refs.lead, !!got.lead);
    if (got.lead) {
      css(refs.lead, 'transform', `translate(${Math.round(got.lead.x)}px,${Math.round(got.lead.y)}px)`);
      css(refs.lead, 'width', `${Math.round(got.lead.w)}px`); css(refs.lead, 'height', `${Math.round(got.lead.h)}px`);
    }
  }
  if (typeof ResizeObserver === 'function') scope.observe(new ResizeObserver(() => scope.read(layout, 'stage:layout'))).observe(refs.stage);
  // 点图开详情，双击看大图；按住拖动是拖进画廊（拖过以后吞掉那次点击）。
  scope.on(refs.stage, 'click', e => { const p = e.target.closest('.piece'); if (p) inspect(p.dataset.id); });
  scope.on(refs.stage, 'dblclick', e => { const p = e.target.closest('.piece'); if (p) openViewer(indexOf(p.dataset.id)); });
  wireLift(refs.stage, target => { const p = target.closest?.('.piece'); return p ? [p.dataset.id] : null; }, scope);
  scope.on(refs.empty, 'click', e => { if (e.target.closest('[data-empty="nsfw"]')) setNsfw(''); });
  refs.all.onclick = () => { if (!g) return; chooseShow(g.id, ALL_SHOW); location.hash = galleryHref(g.id, ALL_SHOW); };
  return {scope, total: () => data && !data.error ? data.total : null, async load(route) {
    g = galleryOf(route.id); kind = showOf(showFor(route.id)).id;
    if (!g || !STAGES[kind]) return;
    const nsfw = state.nsfw, take = showOf(kind).take;
    attr(refs.stage, 'class', `stage show-${kind}`); el.dataset.show = kind;
    let got;
    try { got = await showcaseOf(g, take, {nsfw, signal: scope.signal}); } catch (e) {
      if (e.name !== 'AbortError' && scope.alive && state.route === route) error(readError(e));
      return;
    }
    if (!scope.alive || state.route !== route) return;
    data = got;
    const items = got.items || [];
    showList(items);
    text(refs.kicker, kindText(g) || t('收藏'));
    text(refs.title, g.name);
    text(refs.line, t('{n} 张', {n: number(got.total || 0)}));
    text(refs.all, t('全部 {n} 张', {n: number(got.total || 0)}));
    show(refs.foot, !got.error && got.total > items.length);
    show(refs.stage, !!items.length);
    show(refs.empty, !items.length);
    cls(el, 'is-empty', !items.length);
    if (!items.length) drawEmpty(refs.empty, emptyOf(g, got, nsfw));
    else layout();
    renderHeading();   // 页头的张数
  }};
}
// 空着时：条件失效写原因；隐藏或只看 NSFW 时照图库的说法；收藏、手选画廊说怎么放进来。
function emptyOf(g, got, nsfw) {
  if (got.error) return {icon: 'gallery', fact: t('条件失效：{reason}', {reason: got.error})};
  if (nsfw === 'only') return {icon: 'search', fact: t('「只看 NSFW」开着，没有符合条件的 NSFW 图片。'), next: [{label: t('显示全部'), act: 'nsfw'}]};
  if (isFav(g.id)) return {icon: 'heart', fact: t('还没有收藏。在详情面板或大图里点心形，或者按 F 收藏。')};
  if (isManual(g)) return {icon: 'gallery', fact: t('这个画廊还没有图。把图拖到活页条上的画廊封面，或者在选择栏里点「放进画廊…」。')};
  return {icon: 'gallery', fact: t('没有符合条件的图片。')};
}
// render({samePage, scope})：刚进这一页才建；同一页又画一次（换了展示方式、筛选）只重读、重排（docs/WEB_V2.md「页面样板」）。
export function renderStage({samePage, scope} = {}) {
  if (!samePage || !view?.scope.alive) view = stageView(scope);
  return view.load(state.route);
}
// 写入、撤销、切换 NSFW 之后重读。
export function refreshStage() { return view?.scope.alive ? view.load(state.route) : null; }
// 页头写的张数：读到之前是 null。
export const stageTotal = () => view?.scope.alive ? view.total() : null;
