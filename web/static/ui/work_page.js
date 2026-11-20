// 作品页（#711，地址 #/w/<作品键>）：顶部是这部作品最近加入的图切成竖条拼成的横幅，背后是作品配色的环境光；横幅左上是返回（写
// 回到哪里，按来路一级一级退，core/catalog.js 的 backTo），右上是「钉到活页条／已钉住」和「关注新角色」，左下是作品名、数量，
// 旁边是「在图库里看全部」和「编辑作品…」。下面是角色海报（点进角色页）和最近加入（点了在图库里看这部作品）。
// 原创角色、未归作品（oc、other）也有这一页：没有关注、钉住、编辑和图库链接，图是各角色的封面。
//
// 进来的过渡：从作品卡（或活页条二级层钉住的那一项）展开：横幅先裁成卡片的大小、放在卡片的位置，再展开到原位，文字和下面的内容
// 随后依次淡入（ui/motion.js；开了「减少动态效果」时直接到终态）。卡片在点击时用 launch() 记下自己的位置。
import {backTo, castWorks, catalogChanged, PSEUDO_NOTES, recentOf, workColor, workCovers, workImages} from '../core/catalog.js';
import {imageUrl, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {implement, onPage} from '../core/pages.js';
import {cameFrom, hashFor, stepHistory} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {setWatch, workDialog} from './catalog.js';
import {$, error} from './dom.js';
import {attr, make, prop, refill, show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';
import {animate, enter, reduced} from './motion.js';
import {pinButton} from './pin_button.js';
import {posters} from './poster.js';
import {onPick} from './rail.js';
import {setTint} from './tint.js';

const PAGE = tpl`<div class="cat-workpage"><div class="cat-glow" data-ref="glow"></div><section class="cat-hero" data-ref="hero"><div class="cat-strip" data-ref="strip"></div><div
  class="cat-hero-top" data-ref="top"><a class="btn glass cat-back" data-ref="back"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M12 4.5 6.5 10l5.5 5.5"/></svg><span
  data-ref="backLabel"></span></a><span class="spacer"></span><span data-ref="pin"></span><label class="btn glass cat-hero-watch" data-ref="watch"><input type="checkbox"
  class="switch" data-ref="box"><span data-t>关注新角色</span></label></div><div class="cat-hero-in" data-ref="info"><small data-ref="kind"></small><h1 class="cat-hero-title"
  data-ref="name"></h1><p data-ref="meta"></p><p class="cat-hero-note" data-ref="note"></p><div class="cat-hero-acts" data-ref="acts"><a class="btn glass" data-ref="lib"
  data-t>在图库里看全部</a><button type="button" class="btn glass" data-ref="edit" data-t>编辑作品…</button></div></div></section><section class="cat-sec" data-ref="castSec"><h3
  class="cat-h"><span data-t>角色</span><small data-ref="castN"></small></h3><div class="cat-posters" data-ref="posters"></div></section><section class="cat-sec"
  data-ref="recentSec"><h3 class="cat-h"><span data-t>最近加入</span><small data-ref="recentN"></small></h3><div class="cat-recent" data-ref="recent"></div></section><div
  data-ref="empty" hidden></div></div>`;
const PSEUDO = new Set(['oc', 'other']);
const STRIP = 14;

// ---------- 从哪里展开 ----------
// 作品卡、活页条的项被点时记下位置；作品页画好以后在一秒内取用（换页之间的空当），过期作废。
let from = null;
export function launch(key, el) { from = {key: String(key), box: el.getBoundingClientRect(), at: performance.now()}; }
onPick('catalog', (item, e) => {
  const el = e?.target?.closest?.('.rail-item') || document.querySelector(`.rail-item[data-key="${CSS.escape(item.key)}"]`);
  if (item.kind === 'work' && el) launch(item.key.slice(2), el);
  return false;
});
const takeLaunch = key => { const x = from; from = null; return x && x.key === key && performance.now() - x.at < 1000 ? x.box : null; };
// 横幅从 box 展开：先裁成那一块、放在那里，再展开到原位；文字、按钮和下面的内容依次淡入。
function expand(refs, box) {
  const to = refs.hero.getBoundingClientRect();
  if (!box || reduced() || !to.width) return;
  const dx = box.left - to.left, dy = box.top - to.top;
  animate(refs.hero, [
    {transform: `translate(${dx}px, ${dy}px)`, clipPath: `inset(0px ${Math.max(0, to.width - box.width)}px ${Math.max(0, to.height - box.height)}px 0px round 12px)`},
    {transform: 'none', clipPath: 'inset(0px 0px 0px 0px round 18px)'}], {dur: 3});
  [refs.top, refs.info].forEach((el, i) => enter(el, {dur: 2, delay: 200 + i * 40}));
  [refs.glow, refs.castSec, refs.recentSec].forEach((el, i) => enter(el, {from: i ? 'rise' : 'fade', dur: 2, delay: 240 + i * 60}));
}

// ---------- 页面 ----------
function workView(s) {
  const {el, refs} = PAGE(), pinned = pinButton(s, {wide: true});
  pinned.el.classList.add('glass');
  refs.pin.replaceWith(pinned.el);
  $('charIndex').replaceChildren(el);
  const people = keyed(refs.posters, {key: x => x.c.id, create: posters(s), scope: s});
  const recent = keyed(refs.recent, {key: x => x.uuid, create: thumb, scope: s});
  let info = null, images = [], loaded = '';
  s.on(refs.back, 'click', e => { const b = backTo(cameFrom()); if (b.step) { e.preventDefault(); stepHistory(-1); } });
  s.on(refs.edit, 'click', () => { if (info) workDialog(info); });
  s.on(refs.box, 'change', () => { const on = refs.box.checked; prop(refs.box, 'checked', !on); if (info) setWatch(info, on); });

  function draw(key) {
    const work = castWorks().find(w => String(w.key) === key) || null;
    info = state.index?.works?.find(w => w.id === key) || null;
    const pseudo = PSEUDO.has(key), name = work?.name || info?.name || t('未命名作品');
    const chars = work?.chars || [], count = info?.count ?? work?.images ?? 0, recentN = chars.reduce((n, c) => n + recentOf(c), 0);
    const covers = images.length ? images : workCovers(key);
    setTint(el, workColor(covers));
    const back = backTo(cameFrom());
    text(refs.backLabel, back.label); attr(refs.back, 'href', back.href);
    text(refs.kind, pseudo ? '' : t('作品'));
    text(refs.name, name);
    text(refs.meta, [t('{n} 位角色', {n: number(chars.length)}), t('{n} 张', {n: number(count)}), recentN ? t('最近 30 天 +{n}', {n: number(recentN)}) : ''].filter(Boolean).join(' · '));
    text(refs.note, pseudo ? PSEUDO_NOTES[key] : ''); show(refs.note, pseudo);
    show(refs.watch, !pseudo && !!info); prop(refs.box, 'checked', !!info && !!state.index?.watch.works.includes(info.id));
    attr(refs.watch, 'title', info?.watchable ? t('定期检查 Danbooru 上这部作品还没收录的角色（只发送作品名）') : t('先在「编辑作品」里核对 Danbooru 作品 tag，才能关注'));
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
    show(refs.recentSec, images.length > 0);
    recent.set(images.slice(0, 12).map(x => ({...x, href: pseudo ? '' : hashFor({page: 'library', work: key})})));
    const none = !work && !info;
    show(refs.empty, none); show(refs.hero, !none);
    if (none) drawEmpty(refs.empty, {icon: 'person', fact: t('没有这部作品。'), next: [{label: t('回到作品'), href: '#/characters/works'}]});
  }
  async function load(route) {
    const key = route.id;
    if (loaded !== key) { images = []; loaded = key; }
    draw(key);
    const box = takeLaunch(key);
    if (box) expand(refs, box);
    try { images = await workImages(key, s.signal); } catch (e) { if (s.alive && e?.name !== 'AbortError') error(readError(e)); return; }
    if (!s.alive || state.route !== route) return;
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

let view = null;
implement({work: {
  render: ({samePage, scope}) => { if (!samePage || !view?.scope.alive) view = workView(scope); return view.load(state.route); },
  refresh: () => view?.redraw(),
  onMeta: () => view?.redraw(),
  // 作品名写在横幅上，页头只留右侧的全局控件。
  head: () => ({title: '', sub: ''}),
}});
