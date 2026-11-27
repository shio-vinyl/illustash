// 资料库 · 角色页签（#711）：书架和列表两种看法。
// - 书架：「你最常看的」海报便当盒（第一张 2×2）、「最近 30 天在收」进度圆环，然后全部角色按张数分三档：主力（20 张以上，海报）、
//   常见（5–19 张，圆头像卡）、偶尔（不到 5 张，芯片）。搜索时只有三档，角色名下带作品名。
// - 列表：张数分档的芯片和一张可以按列排序的表，一次 300 行，「再显示 300 位」接着加。
// - 最下面一行「不算角色、只当标签：…」和「管理…」（#293）。
// 页签的部件在进页面时建一次（castView），之后 update() 只按数据改；数据和排序在 core/catalog.js。各项都是链接，点了去角色页。
import {BUCKETS, castRows, catalog, recentOf} from '../core/catalog.js';
import {coverUrl, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {nameTip, roleName} from '../core/labels.js';
import {prefs} from '../core/prefs.js';
import {charHash} from '../core/route.js';
import {state} from '../core/state.js';
import {openTagged} from './catalog.js';
import {attr, css, make, refill, show, text, tpl} from './el.js';
import {keyed} from './keyed.js';
import {personCard, posters} from './poster.js';
import {setTint} from './tint.js';

// 「角色」页签显示什么：书架还是列表、搜索结果（null 是没在搜）、列表的排序和档位、显示到第几行、圆环一排是否展开全部。
export const cast = {mode: prefs.get('castMode', 'shelf'), list: null, sort: 'count', dir: -1, bucket: 'all', limit: 300, allRecent: false};
const LIMIT = 300, RECENT_ROW = 12;

const VIEW = tpl`<div class="cat-cast"><div data-ref="shelf"><section class="cat-sec" data-ref="topSec"><h3 class="cat-h"><span data-t>你最常看的</span><small
  data-t>按张数</small></h3><div class="cat-bento" data-ref="bento"></div></section><section class="cat-sec" data-ref="recentSec"><h3 class="cat-h"><span
  data-t>最近 30 天在收</span><small data-ref="recentN"></small><button type="button" class="btn is-quiet is-small cat-h-more" data-ref="recentAll"></button></h3><div
  class="cat-rings" data-ref="rings"></div></section><section class="cat-sec" data-ref="bigSec"><h3 class="cat-h"><span data-t>主力</span><small data-ref="bigN"></small></h3><div
  class="cat-posters" data-ref="big"></div></section><section class="cat-sec" data-ref="midSec"><h3 class="cat-h"><span data-t>常见</span><small data-ref="midN"></small></h3><div
  class="cat-people" data-ref="mid"></div></section><section class="cat-sec" data-ref="smallSec"><h3 class="cat-h"><span data-t>偶尔</span><small data-ref="smallN"></small></h3><div
  class="cat-chips" data-ref="small"></div></section><p class="cat-none" data-ref="none" data-t>没有匹配的角色</p></div><div data-ref="listView" hidden><div class="cat-buckets"
  data-ref="buckets"></div><table class="cat-table"><thead><tr data-ref="heads"></tr></thead><tbody data-ref="rows"></tbody></table><button type="button" class="btn cat-more"
  data-ref="more"></button></div><p class="cat-tagged" data-ref="tagged"><span data-t>不算角色、只当标签：</span><span data-ref="taggedList"></span><button type="button"
  class="btn is-quiet is-small" data-ref="taggedManage" data-t>管理…</button></p></div>`;
const RING = tpl`<a class="cat-ring"><span class="avatar-ring" data-ref="ring"><img class="avatar" data-ref="img" alt="" loading="lazy" decoding="async" draggable="false"><em
  data-ref="plus"></em></span><b data-ref="name"></b><small data-ref="qualifier" hidden></small><small data-ref="work"></small></a>`;
const CHIP = tpl`<a class="chip cat-chip"><img class="avatar" data-ref="img" alt="" loading="lazy" decoding="async"><span data-ref="name"></span><span class="chip-count"
  data-ref="n"></span></a>`;
const ROW = tpl`<tr><td><a class="cat-who" data-ref="who"><img class="avatar" data-ref="img" alt="" loading="lazy" decoding="async"><span><b data-ref="name"></b><small
  data-ref="orig"></small></span></a></td><td data-ref="work"></td><td><span class="cat-bar"><i data-ref="bar"></i><span data-ref="count"></span></span></td><td data-ref="recent"></td><td
  class="cat-mono" data-ref="last"></td><td data-ref="skins"></td></tr>`;
const COLUMNS = [['name', () => t('角色')], ['work', () => t('作品')], ['count', () => t('张数')], ['recent', () => t('近 30 天')], ['last', () => t('最后加入')],
  ['skins', () => t('造型')]];

const workOf = c => c.kind === 'oc' ? t('原创角色') : c.work_name || '';
const skinsOf = c => (c.variants || []).filter(v => v.count).length;
const withWork = (c, rest) => [c.work_name, rest].filter(Boolean).join(' · ');

function ringItem(c) {
  const {el, refs} = RING();
  function update(x) {
    attr(el, 'href', charHash(x.c.id)); attr(el, 'title', nameTip(x.c));
    attr(refs.img, 'src', coverUrl(x.c.cover)); setTint(refs.img, x.c.color);
    css(refs.ring, '--p', String(x.p));
    text(refs.plus, '+' + number(recentOf(x.c))); text(refs.name, x.c.name); text(refs.work, x.c.work_name || '');
    const qualifier = x.c.name_disambiguation?.qualifier;
    text(refs.qualifier, qualifier || ''); show(refs.qualifier, !!qualifier);
  }
  update(c);
  return {el, update};
}
function chipItem(c) {
  const {el, refs} = CHIP();
  function update(x) {
    attr(el, 'href', charHash(x.id)); attr(el, 'title', nameTip(x));
    attr(refs.img, 'src', coverUrl(x.cover)); setTint(refs.img, x.color);
    text(refs.name, roleName(x)); text(refs.n, number(x.count));
  }
  update(c);
  return {el, update};
}
function rowItem(x) {
  const {el, refs} = ROW();
  function update({c, max}) {
    const a = catalog.activity.get(c.id), orig = c.tag && c.tag !== c.name ? c.tag.replaceAll('_', ' ') : '', skins = skinsOf(c);
    attr(refs.who, 'href', charHash(c.id)); attr(refs.img, 'src', coverUrl(c.cover)); setTint(refs.img, c.color);
    text(refs.name, roleName(c)); text(refs.orig, orig); show(refs.orig, !!orig);
    text(refs.work, workOf(c) || '—');
    css(refs.bar, 'width', Math.max(4, Math.log(c.count + 1) / Math.log(max + 1) * 110) + 'px'); text(refs.count, number(c.count));
    text(refs.recent, a?.recent ? '+' + number(a.recent) : '—');
    text(refs.last, a?.last || ''); text(refs.skins, c.summary ? '' : skins ? String(skins) : '—');
  }
  update(x);
  return {el, update};
}

// 角色页签。s 是页面的作用域；update() 按 state 和 cast 重画（只改变了的地方）。
export function castView(s) {
  const {el, refs} = VIEW(), poster = posters(s);
  const bento = keyed(refs.bento, {key: x => x.c.id, create: poster, scope: s});
  const big = keyed(refs.big, {key: x => x.c.id, create: poster, scope: s});
  const rings = keyed(refs.rings, {key: x => x.c.id, create: ringItem, scope: s});
  const mid = keyed(refs.mid, {key: x => x.id, create: personCard, scope: s});
  const small = keyed(refs.small, {key: c => c.id, create: chipItem, scope: s});
  const rows = keyed(refs.rows, {key: x => x.c.id, create: rowItem, scope: s});
  let showWork = false;
  const person = c => ({href: charHash(c.id), cover: c.cover, color: c.color, name: c.name, qualifier: c.name_disambiguation?.qualifier, title: nameTip(c),
    sub: showWork ? withWork(c, t('{n} 张', {n: number(c.count)})) : t('{n} 张', {n: number(c.count)})});
  const midItems = list => list.map(c => ({...person(c), id: c.id}));

  s.on(refs.recentAll, 'click', () => { cast.allRecent = !cast.allRecent; update(); });
  s.on(refs.taggedManage, 'click', openTagged);
  s.on(refs.more, 'click', () => { cast.limit += LIMIT; update(); });
  s.on(refs.buckets, 'click', e => { const b = e.target.closest('[data-bucket]'); if (b) { cast.bucket = b.dataset.bucket; cast.limit = LIMIT; update(); } });
  s.on(refs.heads, 'click', e => {
    const h = e.target.closest('[data-sort]'); if (!h) return;
    const k = h.dataset.sort;
    cast.dir = cast.sort === k ? -cast.dir : (k === 'name' || k === 'work' ? 1 : -1); cast.sort = k;
    update();
  });

  function tiers(list) {
    const b = list.filter(c => c.count >= 20), m = list.filter(c => c.count >= 5 && c.count < 20), sm = list.filter(c => c.count < 5);
    show(refs.bigSec, b.length); show(refs.midSec, m.length); show(refs.smallSec, sm.length); show(refs.none, !list.length);
    text(refs.bigN, t('20 张以上 · {n} 位', {n: number(b.length)}));
    text(refs.midN, t('5–19 张 · {n} 位', {n: number(m.length)}));
    text(refs.smallN, t('不到 5 张 · {n} 位', {n: number(sm.length)}));
    big.set(b.map(c => ({c, sub: showWork ? withWork(c, t('{n} 张', {n: number(c.count)})) : t('{n} 张', {n: number(c.count)})})));
    mid.set(midItems(m)); small.set(sm);
  }
  function drawShelf() {
    const searching = !!cast.list;
    showWork = true;
    show(refs.topSec, !searching);
    const top = searching ? [] : [...state.chars].sort((a, b) => b.count - a.count).slice(0, 9);
    bento.set(top.map((c, i) => ({c, rank: i + 1, big: i === 0, sub: withWork(c, t('{n} 张', {n: number(c.count)}))})));
    const rec = searching ? [] : state.chars.filter(c => recentOf(c)).sort((a, b) => recentOf(b) - recentOf(a)), max = rec[0] ? recentOf(rec[0]) : 1;
    show(refs.recentSec, rec.length > 0);
    text(refs.recentN, t('{n} 位角色有新图', {n: number(rec.length)}));
    show(refs.recentAll, rec.length > RECENT_ROW); text(refs.recentAll, cast.allRecent ? t('收起') : t('全部 {n}', {n: number(rec.length)}));
    attr(refs.rings, 'data-open', cast.allRecent ? '' : null);
    rings.set((cast.allRecent ? rec : rec.slice(0, RECENT_ROW)).map(c => ({c, p: Math.max(8, Math.round(recentOf(c) / max * 100))})));
    tiers(cast.list || state.chars);
  }
  function drawList() {
    const base = cast.list || state.chars, list = castRows(base, cast), max = Math.max(1, ...base.map(c => c.count));
    const counts = BUCKETS.map(([, , f]) => base.filter(f).length);
    refill(refs.buckets, counts.join(',') + cast.bucket, () => BUCKETS.map(([k, label], i) => {
      const b = make('button', 'chip', label); b.type = 'button'; b.dataset.bucket = k;
      b.setAttribute('aria-pressed', String(cast.bucket === k));
      b.append(make('span', 'chip-count', number(counts[i])));
      return b;
    }));
    refill(refs.heads, cast.sort + cast.dir, () => COLUMNS.map(([k, label]) => {
      const th = make('th', '', label() + (cast.sort === k ? (cast.dir < 0 ? ' ↓' : ' ↑') : '')); th.dataset.sort = k;
      th.setAttribute('aria-sort', cast.sort === k ? (cast.dir < 0 ? 'descending' : 'ascending') : 'none');
      return th;
    }));
    rows.set(list.slice(0, cast.limit).map(c => ({c, max})));
    show(refs.more, list.length > cast.limit);
    text(refs.more, t('再显示 300 位（共 {n} 位）', {n: number(list.length)}));
  }
  function update() {
    const listMode = cast.mode === 'list';
    show(refs.shelf, !listMode); show(refs.listView, listMode);
    if (listMode) drawList(); else drawShelf();
    const tagged = state.tagged || [];
    show(refs.tagged, tagged.length > 0);
    refill(refs.taggedList, tagged.map(x => x.id + ':' + x.count).join('|'), () => tagged.map(x => {
      const a = make('a', '', x.name); a.href = charHash(x.id); a.append(make('small', '', number(x.count))); return a;
    }));
  }
  return {el, update};
}
