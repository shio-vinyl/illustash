// 全部画廊（#/galleries，#710）：收藏在最前，后面按保存的先后，最后一张是「新建」。每张卡片的预览按那个画廊的展示方式排
// 前几张图（拼贴散放、杂志一大两小、瀑布三列……，样式在 css/galleries.css 的 .gcard-prev.show-<id>），名字前面的小图标
// 和副标题分出两种画廊：收藏是心形，手选画廊是图钉，按条件的是漏斗（截图给用户看过，#710）。
// 卡片和「新建」都能接住拖过来的图（data-drop，ui/drag.js）：卡片是放进那个画廊，「新建」是新建手选画廊。
// 页面的登记在 ui/galleries.js；数据在 core/galleries.js。
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {coversFor, droppable, galleryHref, isFav, isManual, loadShows, previewOf, withFav} from '../core/galleries.js';
import {showFor, showOf} from '../core/gallery_shows.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {$, error} from './dom.js';
import {attr, cls, show, text, tpl} from './el.js';
import {newManual, reloadList} from './gallery_actions.js';
import {keyed} from './keyed.js';
import {thumbs} from './motion.js';
import {renderHeading} from './shell.js';
import {setTint} from './tint.js';

const PAGE = tpl`<div class="gallery-index"><div class="gcards" data-ref="list"></div></div>`;
// 预览固定 PREVIEW（5）个图位，样式按展示方式摆；图不够时多的藏起来。
const CARD = tpl`<a class="gcard"><span class="gcard-prev" data-ref="prev"><img alt="" draggable="false" decoding="async"><img alt="" draggable="false" decoding="async"><img
  alt="" draggable="false" decoding="async"><img alt="" draggable="false" decoding="async"><img alt="" draggable="false" decoding="async"></span><span
  class="gcard-meta"><span class="gcard-kind" data-ref="kind" aria-hidden="true"></span><b data-ref="name"></b><small data-ref="sub"></small></span></a>`;
const KIND = {
  fav: tpl`<svg viewBox="0 0 20 20"><path d="M10 16.4S3.4 12.5 3.4 7.9A3.3 3.3 0 0 1 10 6.3a3.3 3.3 0 0 1 6.6 1.6c0 4.6-6.6 8.5-6.6 8.5Z"/></svg>`,
  manual: tpl`<svg viewBox="0 0 20 20"><path d="M7.5 3.5h5l-.8 4.2 2.8 2.6v1.2H5.5v-1.2l2.8-2.6zM10 11.5v5"/></svg>`,
  condition: tpl`<svg viewBox="0 0 20 20"><path d="M3.5 4.5h13l-5 6v5l-3-1.5v-3.5z"/></svg>`,
};
const NEW = tpl`<div class="gcard gcard-new" data-drop="new"><span class="gcard-prev"><span class="gcard-plus" aria-hidden="true"><svg viewBox="0 0 20 20"><path
  d="M10 4v12M4 10h12"/></svg></span><b data-t>新建画廊</b><small data-t>把图拖到这里，新建一个手选画廊</small></span><span class="gcard-acts"><button class="btn"
  type="button" data-ref="manual" data-t>手选画廊…</button><a class="btn is-quiet" href="#/library" data-ref="cond" title="在图库里加上条件，点「保存为画廊」"
  data-t-attr="title" data-t>按条件…</a></span></div>`;
const kindOf = g => isFav(g.id) ? 'fav' : isManual(g) ? 'manual' : 'condition';
const src = (uuid, k) => `/image?id=${encodeURIComponent(uuid)}&size=${k ? 'thumb' : 'medium'}`;

// 一张卡片。预览和张数由 fill() 读到以后写进来；读之前图位空着，底色是第一张图的主色（读到预览以后）。
function card(g) {
  const {el, refs} = CARD(), imgs = [...refs.prev.children];
  let kind = '';
  const update = x => {
    const k = kindOf(x);
    attr(el, 'href', galleryHref(x.id));
    el.dataset.gallery = String(x.id);
    attr(el, 'data-drop', droppable(x) ? String(x.id) : null);
    if (kind !== k) { kind = k; refs.kind.replaceChildren(KIND[k]().el); el.dataset.kind = k; }
    attr(refs.prev, 'class', `gcard-prev show-${showOf(showFor(x.id)).id}`);
    text(refs.name, x.name);
    const p = x.preview, n = p?.error ? t('条件失效') : p ? t('{n} 张', {n: number(p.total)}) : '…';
    text(refs.sub, [k === 'manual' ? t('手选') : k === 'condition' ? t('按条件') : '', n, showOf(showFor(x.id)).label()].filter(Boolean).join(' · '));
    attr(refs.sub, 'title', p?.error || null);
    const items = p?.items || [];
    imgs.forEach((img, i) => {
      const it = items[i];
      attr(img, 'src', it ? src(it.uuid, i) : null);
      show(img, !!it);
      if (it) thumbs.mark(img, it.uuid);
    });
    setTint(el, items[0]?.color || null);
    cls(el, 'is-empty', !!p && !items.length);
  };
  update(g);
  return {el, update, dispose() {}};
}

// 这一页的部件：进页面时在页面作用域上建一次。之后 load() 只重读列表和预览、更新卡片。
let view = null;
function indexView(scope) {
  const {el, refs} = PAGE();
  $('groupsPage').replaceChildren(el);
  const list = keyed(refs.list, {key: g => String(g.id), create: card, scope});
  const fresh = NEW();
  fresh.refs.manual.onclick = async () => { const g = await newManual(); if (g) location.hash = galleryHref(g.id); };
  const previews = new Map();   // 画廊 id → 读到的预览 {items, total} 或 {error}
  const draw = () => list.set(withFav().map(g => ({...g, preview: previews.get(String(g.id))})));
  return {scope, async load(route) {
    const nsfw = state.nsfw, here = () => scope.alive && state.route === route && state.nsfw === nsfw;
    try { await Promise.all([reloadList(), loadShows()]); } catch (e) { if (here()) error(readError(e)); return; }
    if (!here()) return;
    previews.clear(); draw();
    renderHeading();   // 页头的画廊数
    refs.list.append(fresh.el);   // 「新建」总在最后（keyed 只管画廊卡片，新挂的卡片插在它前面也没关系：下面每次都挪回末尾）
    const all = withFav();
    let covers = {};
    try { covers = await coversFor((state.galleries || []).filter(g => g.cover_manual).map(g => g.id)); } catch { covers = {}; }
    await Promise.all(all.map(async g => {
      let p;
      try { p = await previewOf(g, {nsfw, cover: g.cover_manual ? covers[g.id] : null, signal: scope.signal}); } catch (e) { if (e.name !== 'AbortError' && here()) error(readError(e)); return; }
      if (!here()) return;
      previews.set(String(g.id), p); draw(); refs.list.append(fresh.el);
    }));
  }};
}
// render({samePage, scope})：刚进这一页才建；同一页又画一次只重读（docs/WEB_V2.md「页面样板」）。
export function renderIndex({samePage, scope} = {}) {
  if (!samePage || !view?.scope.alive) view = indexView(scope);
  return view.load(state.route);
}
// 写入、撤销、切换 NSFW 之后：重读列表和预览，卡片就地更新。
export function refreshIndex() { return view?.scope.alive ? view.load(state.route) : null; }
