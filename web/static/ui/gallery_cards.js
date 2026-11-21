// 全部画廊（#/galleries，#710）：收藏在最前，后面按保存的先后，最后一张是「新建」。每张卡片的预览按那个画廊的展示方式排
// 前几张图（拼贴散放、杂志一大两小、瀑布三列……，样式在 css/galleries.css 的 .gcard-prev.show-<id>），名字前面的小图标
// 和副标题分出两种画廊：收藏是心形，手选画廊是图钉，按条件的是漏斗（截图给用户看过，#710）。
// 页面的登记在 ui/galleries.js；数据在 core/galleries.js。
import {imageUrl, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {coversFor, galleryHref, isFavGallery, isManual, previewOf, withFav} from '../core/galleries.js';
import {loadShows, showFor, showOf} from '../core/gallery_shows.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {$, error} from './dom.js';
import {attr, cls, show, text, tpl} from './el.js';
import {icon} from './icons.js';
import {newManual, reloadList} from './gallery_actions.js';
import {keyed} from './keyed.js';
import {thumbs} from './motion.js';
import {latest, pageView} from './page_view.js';
import {renderHeading} from './shell.js';
import {setTint} from './tint.js';

// 卡片在 list 里（keyed 管，display: contents，排进外面的网格），「新建」是它后面的兄弟，不随卡片的增减挪动。
const PAGE = tpl`<div class="gallery-index"><div class="gcards" data-ref="grid"><div class="gcards-list" data-ref="list"></div></div></div>`;
// 预览固定 PREVIEW（5）个图位，样式按展示方式摆；图不够时多的藏起来。
const CARD = tpl`<a class="gcard"><span class="gcard-prev" data-ref="prev"><img alt="" draggable="false" decoding="async"><img alt="" draggable="false" decoding="async"><img
  alt="" draggable="false" decoding="async"><img alt="" draggable="false" decoding="async"><img alt="" draggable="false" decoding="async"></span><span
  class="gcard-meta"><span class="gcard-kind" data-ref="kind" aria-hidden="true"></span><b data-ref="name"></b><small data-ref="sub"></small></span></a>`;
// 名字前面的小图标（ui/icons.js）：收藏是心形，手选画廊是图钉，按条件的是漏斗。
const KIND = {fav: 'heart', manual: 'pin', condition: 'funnel'};
const NEW = tpl`<div class="gcard gcard-new"><span class="gcard-prev"><span class="gcard-plus" aria-hidden="true"><svg viewBox="0 0 20 20"><path
  d="M10 4v12M4 10h12"/></svg></span><b data-t>新建画廊</b></span><span class="gcard-acts"><button class="btn"
  type="button" data-ref="manual" data-t>手选画廊…</button><a class="btn is-quiet" href="#/library" data-ref="cond" title="在图库里加上条件，点「保存为画廊」"
  data-t-attr="title" data-t>按条件…</a></span></div>`;
const kindOf = g => isFavGallery(g.id) ? 'fav' : isManual(g) ? 'manual' : 'condition';

// 一张卡片。预览和张数由 fill() 读到以后写进来；读之前图位空着，底色是第一张图的主色（读到预览以后）。
function card(g) {
  const {el, refs} = CARD(), imgs = [...refs.prev.children];
  let kind = '';
  const update = x => {
    const k = kindOf(x);
    attr(el, 'href', galleryHref(x.id));
    el.dataset.gallery = String(x.id);
    if (kind !== k) { kind = k; refs.kind.replaceChildren(icon(KIND[k])); el.dataset.kind = k; }
    attr(refs.prev, 'class', `gcard-prev show-${showOf(showFor(x.id)).id}`);
    text(refs.name, x.name);
    const p = x.preview, n = p?.error ? t('条件失效') : p ? t('{n} 张', {n: number(p.total)}) : '…';
    text(refs.sub, [k === 'manual' ? t('手选') : k === 'condition' ? t('按条件') : '', n, showOf(showFor(x.id)).label()].filter(Boolean).join(' · '));
    attr(refs.sub, 'title', p?.error || null);
    const items = p?.items || [];
    imgs.forEach((img, i) => {
      const it = items[i];
      attr(img, 'src', it ? imageUrl(it.uuid, i ? 'thumb' : 'medium') : null);   // 第一张大，其余小
      show(img, !!it);
      if (it) thumbs.mark(img, it.uuid);
    });
    setTint(el, items[0]?.color || null);
    cls(el, 'is-empty', !!p && !items.length);
  };
  update(g);
  return {el, update, dispose() {}};
}

// 这一页的部件：进页面时在页面作用域上建一次。之后 load() 只重读列表和预览、更新卡片。同一路由上的几次重读（写入、撤销）
// 只认最后发出的那次：先发的晚到，不把旧的列表和预览画回去。
function indexView(scope) {
  const {el, refs} = PAGE();
  $('groupsPage').replaceChildren(el);
  const list = keyed(refs.list, {key: g => String(g.id), create: card, scope});
  const adder = NEW();
  refs.grid.append(adder.el);
  adder.refs.manual.onclick = async () => { const g = await newManual(); if (g) location.hash = galleryHref(g.id); };
  // 画廊 id → 读到的预览 {items, total} 或 {error}。刷新时留着旧的，新的读到一张换一张；只有 NSFW 设置变了才先清掉（该藏的图马上藏起来）。
  const previews = new Map();
  let previewsNsfw = null;
  const begin = latest(scope);
  const draw = () => list.set(withFav().map(g => ({...g, preview: previews.get(String(g.id))})));
  return {scope, async load(route) {
    const nsfw = state.nsfw, fresh = begin(route, () => state.nsfw === nsfw);
    if (previewsNsfw !== nsfw) { previews.clear(); previewsNsfw = nsfw; }
    try { await Promise.all([reloadList(), loadShows()]); } catch (e) { if (fresh()) error(readError(e)); return; }
    if (!fresh()) return;
    draw();
    renderHeading();   // 页头的画廊数
    const all = withFav();
    let covers = {};
    try { covers = await coversFor((state.galleries || []).filter(g => g.cover_manual).map(g => g.id)); } catch { covers = {}; }
    await Promise.all(all.map(async g => {
      let p;
      try { p = await previewOf(g, {nsfw, cover: g.cover_manual ? covers[g.id] : null, signal: scope.signal}); } catch (e) { if (e.name !== 'AbortError' && fresh()) error(readError(e)); return; }
      if (!fresh()) return;
      previews.set(String(g.id), p); draw();
    }));
  }};
}
const view = pageView(indexView);
// render({samePage, scope})：刚进这一页才建；同一页又画一次只重读（docs/WEB_V2.md「页面样板」）。
export const renderIndex = (o = {}) => view.enter(o).load(state.route);
// 写入、撤销、切换 NSFW 之后：重读列表和预览，卡片就地更新。
export const refreshIndex = () => view.get()?.load(state.route) ?? null;
