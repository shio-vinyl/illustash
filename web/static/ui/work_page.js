// 作品页（#711，地址 #/w/<作品键>）：顶部是这部作品最近加入的图切成竖条拼成的横幅，背后是作品配色的环境光；横幅左上是返回（写
// 回到哪里，按来路走，ui/back_link.js），右上是「钉到活页条／已钉住」和「关注新角色」，左下是作品名、数量，
// 旁边是「在图库里看全部」和「编辑作品…」。下面是角色海报（点进角色页）和最近加入（点了在图库里看这部作品）。
// 原创角色、未归作品（oc、other）也有这一页：没有关注、钉住、编辑和图库链接，图是各角色的封面。
//
// 进来的过渡：从作品卡（或活页条二级层钉住的那一项）展开：横幅先裁成卡片的大小、放在卡片的位置，再展开到原位，文字和下面的内容
// 随后依次淡入（ui/motion.js；开了「减少动态效果」时直接到终态）。卡片在点击时用 launch() 记下自己的位置。
import {castWorks, catalogChanged, forgetActivity, loadActivity, loadIndex, PSEUDO_NOTES, recentOf, workColor, workCovers, workImages} from '../core/catalog.js';
import {imageUrl, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {implement, onPage} from '../core/pages.js';
import {hashFor} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {watchToggle, workDialog} from './catalog.js';
import {$, error} from './dom.js';
import {attr, make, refill, show, text, tpl} from './el.js';
import {backLink} from './back_link.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';
import {animate, enter, reduced} from './motion.js';
import {latest, pageView} from './page_view.js';
import {pinButton} from './pin_button.js';
import {posters} from './poster.js';
import {onPick} from './rail.js';
import {setTint} from './tint.js';

const PAGE = tpl`<div class="cat-workpage"><div class="cat-glow" data-ref="glow"></div><section class="cat-hero" data-ref="hero"><div class="cat-strip" data-ref="strip"></div><div
  class="cat-hero-top" data-ref="top"><span data-ref="back"></span><span class="spacer"></span><span data-ref="pin"></span><span data-ref="watch"></span></div><div class="cat-hero-in"
  data-ref="info"><small data-ref="kind"></small><h1 class="cat-hero-title"><span data-ref="name"></span><small data-ref="qualifier"></small></h1><p data-ref="meta"></p><a
  class="cat-hero-tag" data-ref="tag" target="_blank"
  rel="noreferrer noopener"></a><p class="cat-hero-note" data-ref="note"></p><div class="cat-hero-acts" data-ref="acts"><a class="btn glass" data-ref="lib"
  data-t>在图库里看全部</a><button type="button" class="btn glass" data-ref="edit" data-t>编辑作品…</button></div></div></section><section class="cat-sec" data-ref="castSec"><h3
  class="cat-h"><span data-t>角色</span><small data-ref="castN"></small></h3><div class="cat-posters" data-ref="posters"></div></section><section class="cat-sec"
  data-ref="recentSec"><h3 class="cat-h"><span data-t>最近加入</span><small data-ref="recentN"></small></h3><div class="cat-recent" data-ref="recent"></div></section><div
  data-ref="empty" hidden></div></div>`;
const PSEUDO = new Set(['oc', 'other']);
// 没有来路（页面打开后的第一条）时，返回去作品页签。
const BACK = {label: t('作品'), href: '#/characters/works'};
const STRIP = 14;

// ---------- 从哪里展开 ----------
// 作品卡、活页条的项被点时记下位置；作品页画好以后在一秒内取用（换页之间的空当），过期作废。
let launched = null;
export function launch(key, el) { launched = {key: String(key), box: el.getBoundingClientRect(), at: performance.now()}; }
const takeLaunch = key => { const x = launched; launched = null; return x && x.key === key && performance.now() - x.at < 1000 ? x.box : null; };
// 页面启动时调用一次（app.js）：点了活页条上钉住的作品（或 ⌥↑↓ 走到它），记下那个小方块的位置。按项的键找元素：
// 键盘走到时事件的目标是原来有焦点的那一项。
export function wireWorkPage() {
  onPick('catalog', item => {
    const el = document.querySelector(`.rail-item[data-key="${CSS.escape(item.key)}"]`);
    if (item.kind === 'work' && el) launch(item.key.slice(2), el);
    return false;
  });
}
// 横幅从 box 展开：先裁成那一块、放在那里，再展开到原位；文字、按钮和下面的内容依次淡入。
function expand(refs, box) {
  const to = refs.hero.getBoundingClientRect();
  if (!box || reduced() || !to.width) return;
  const dx = box.left - to.left, dy = box.top - to.top;
  animate(refs.hero, [
    // 起止的圆角对应 --radius-12（卡片）、--radius-18（横幅）。
    {transform: `translate(${dx}px, ${dy}px)`, clipPath: `inset(0px ${Math.max(0, to.width - box.width)}px ${Math.max(0, to.height - box.height)}px 0px round 12px)`},
    {transform: 'none', clipPath: 'inset(0px 0px 0px 0px round 18px)'}], {dur: 3});
  [refs.top, refs.info].forEach((el, i) => enter(el, {dur: 2, delay: 200 + i * 40}));
  [refs.glow, refs.castSec, refs.recentSec].forEach((el, i) => enter(el, {from: i ? 'rise' : 'fade', dur: 2, delay: 240 + i * 60}));
}

// ---------- 页面 ----------
function workView(s) {
  const {el, refs} = PAGE(), pinned = pinButton(s, {wide: true}), watch = watchToggle(s, 'btn glass cat-hero-watch');
  pinned.el.classList.add('glass');
  const back = backLink(s, BACK);
  refs.back.replaceWith(back.el);
  refs.pin.replaceWith(pinned.el); refs.watch.replaceWith(watch.el);
  $('charIndex').replaceChildren(el);
  const people = keyed(refs.posters, {key: x => x.c.id, create: posters(s), scope: s});
  const recent = keyed(refs.recent, {key: x => x.uuid, create: thumb, scope: s});
  let info = null, images = [];
  const begin = latest(s);
  s.on(refs.edit, 'click', () => { if (info) workDialog(info); });

  function draw(key) {
    const work = castWorks().find(w => String(w.key) === key) || null;
    info = state.index?.works?.find(w => w.id === key) || null;
    const pseudo = PSEUDO.has(key), name = work?.name || info?.name || t('未命名作品');
    const chars = work?.chars || [], count = info?.count ?? work?.images ?? 0, recentN = chars.reduce((n, c) => n + recentOf(c), 0);
    const covers = images.length ? images : workCovers(key);
    setTint(el, workColor(covers));
    back.update();
    text(refs.kind, pseudo ? '' : t('作品'));
    text(refs.name, name); text(refs.qualifier, info?.qualifier || '');
    text(refs.tag, info?.tag ? 'Danbooru · ' + info.tag : ''); show(refs.tag, !!info?.tag);
    attr(refs.tag, 'href', info?.tag ? 'https://danbooru.donmai.us/posts?tags=' + encodeURIComponent(info.tag) : null);
    text(refs.meta, [t('{n} 位角色', {n: number(chars.length)}), t('{n} 张', {n: number(count)}), recentN ? t('最近 30 天 +{n}', {n: number(recentN)}) : ''].filter(Boolean).join(' · '));
    text(refs.note, pseudo ? PSEUDO_NOTES[key] : ''); show(refs.note, pseudo);
    show(watch.el, !pseudo && !!info); watch.update(info, !!info && !!state.index?.watch.works.includes(info.id));
    pinned.update(pseudo ? '' : 'w:' + key);
    show(refs.lib, !pseudo); attr(refs.lib, 'href', hashFor({page: 'library', work: key}));
    show(refs.edit, !pseudo && !!info);
    refill(refs.strip, covers.slice(0, STRIP).map(x => x.uuid).join('|'), () => covers.slice(0, STRIP).map(x => {
      const img = make('img'); img.alt = ''; img.decoding = 'async'; img.src = imageUrl(x.uuid, 'medium'); setTint(img, x.color); return img;
    }));
    text(refs.castN, t('{n} 位 · 按张数', {n: number(chars.length)}));
    show(refs.castSec, chars.length > 0);
    people.set(chars.map(c => ({c, sub: t('{n} 张', {n: number(c.count)})})));
    text(refs.recentN, t('{n} 张', {n: number(count)}));
    // 原创角色、未归作品的图是各角色的封面，海报里已经有了，不另列「最近加入」。
    show(refs.recentSec, !pseudo && images.length > 0);
    recent.set(images.slice(0, 12).map(x => ({...x, href: pseudo ? '' : hashFor({page: 'library', work: key})})));
    const none = !work && !info && !!state.index;
    show(refs.empty, none); show(refs.hero, !none);
    if (none) drawEmpty(refs.empty, {icon: 'person', fact: t('没有这部作品。'), next: [{label: t('回到作品'), href: '#/characters/works'}]});
  }
  // 读这部作品的图；直接打开（刷新、从活页条进来）时索引和最近动态可能还没读到，一起读。索引和最近动态回来只是按现在的数据
  // 重画，不比先后；作品的图在同一路由上的几次重读（同步、写入、撤销）只认最后发出的那次。
  async function load(route) {
    const key = route.id;
    draw(key);
    const box = takeLaunch(key);
    if (box) expand(refs, box);
    const here = () => s.alive && state.route === route, fresh = begin(route);
    if (!state.index) loadIndex().then(() => { if (here()) draw(key); }, () => {});
    loadActivity().then(got => { if (got && here()) draw(key); }, () => {});
    let list;
    try { list = await workImages(key, s.signal); } catch (e) { if (fresh() && e?.name !== 'AbortError') error(readError(e)); return; }
    if (!fresh()) return;
    images = list;
    draw(key);
  }
  const redraw = () => { if (s.alive && onPage(state.route, 'work')) draw(state.route.id); };
  // 关注、编辑作品之后（ui/catalog.js 发 catalogChanged）重画。
  s.add(catalogChanged.on(redraw));
  return {scope: s, load, redraw};
}
// 最近加入的一张：方格缩略图，点了在图库里看这部作品。
function thumb(x) {
  const a = make('a', 'cat-thumb'), img = make('img');
  img.alt = ''; img.loading = 'lazy'; img.decoding = 'async';
  a.append(img);
  const update = y => { attr(a, 'href', y.href || null); attr(img, 'src', imageUrl(y.uuid)); setTint(a, y.color); };
  update(x);
  return {el: a, update};
}

const view = pageView(workView);
implement({work: {
  render: o => view.enter(o).load(state.route),
  // NSFW、同步、撤销、改动之后图和计数都可能变了：重读这部作品的图；别的只重画。
  refresh: why => {
    const v = view.get(); if (!v) return;
    if (['nsfw', 'sync', 'undo', 'edit'].includes(why)) { if (why === 'nsfw') forgetActivity(); return v.load(state.route); }
    v.redraw();
  },
  onMeta: () => view.get()?.redraw(),
  // 作品名写在横幅上，页头只留右侧的全局控件。
  head: () => ({title: '', sub: ''}),
}});
