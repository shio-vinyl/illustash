// 画师抽查（#713，#/artist-audit）：「找回画师」的验收（docs/PROVENANCE.md 第 8 节）。一份固定的随机样本（服务端按 id 的哈希排序），
// 每张图一组（ui/tidy_group.js 的浅底组）：左边是图，点一下看大图（按住 ⌘／Ctrl、Shift 点在新窗口开原图）；右边是找到的画师，
// 用和详情面板一样的「对」/「不对」回答，所以每个回答都进撤销历史，同时也修正图库。上面一行是准确率和各方法的统计。
// 数据在 core/tidy.js。图按 uuid 复用（ui/keyed.js），判断以后只换判断过的那一组的画师行（#452）。
import {exclusive} from '../core/edits.js';
import {imageUrl, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {implement} from '../core/pages.js';
import {prefs} from '../core/prefs.js';
import {hashFor} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {artistVerdict, auditMethod, auditTally, loadArtistAudit} from '../core/tidy.js';
import {$, error, toast, written} from './dom.js';
import {attr, cls, make, refill, show, text, tpl} from './el.js';
import {keyed} from './keyed.js';
import {renderHeading} from './shell.js';
import {setPeekList} from './tidy_group.js';
import {peekAt} from './viewer.js';

const METHODS = {pixel_hash: [t('像素哈希'), t('和 Danbooru 上的帖子是同一个文件，画师取自帖子的画师标签')],
  gallery_match: [t('本地比对'), t('和 TA 公开作品的小图逐张比对一致')], ocr: [t('签名识别'), t('图上的签名或水印')]};
const VIEW = tpl`<div class="tidy-page">
  <div class="tidy-stats" data-ref="stats"></div>
  <p class="tidy-meta" data-ref="about"></p>
  <div class="tidy-list" data-ref="list"></div>
  <div class="tidy-more" data-ref="moreBox" hidden><button type="button" class="btn" data-ref="more" data-t>再加 10 张</button></div>
</div>`;
const ITEM = tpl`<section class="group tidy-group tidy-audit">
  <a class="tidy-audit-img" target="_blank" title="看大图" data-t-attr="title" data-ref="img"><img alt="" decoding="async" loading="lazy" draggable="false"><b
    class="tidy-audit-no" data-ref="no"></b></a>
  <div class="tidy-audit-body" data-ref="body"></div>
</section>`;
const STAT = tpl`<div class="tidy-stat"><b data-ref="big"></b><span data-ref="label"></span><small class="tidy-meta" data-ref="small"></small></div>`;

const links = (prefix, list) => {
  const p = make('p', 'tidy-meta', prefix + ' ');
  list.forEach((x, k) => {
    if (k) p.append(' · ');
    const a = make('a', 'link', x.label); a.href = x.url; a.target = '_blank'; a.rel = 'noreferrer noopener'; p.append(a);
  });
  return p;
};
// 一位画师：名字（点了在图库里只看 TA）、库里几张、主页、怎么找到的，和判断按钮。按钮上的 data-* 给点击用。
function artistRow(it, a) {
  const row = make('div', 'tidy-artist'), top = make('p', 'tidy-artist-top');
  const name = make('a', 'artist-name', a.name); name.href = hashFor({page: 'library', artist: a.id}); name.title = t('在图库里只看 TA 的图');
  top.append(name, make('span', 'tidy-meta', t('库里 {n} 张', {n: number(a.count)})));
  row.append(top);
  if (a.homes.length) row.append(links(t('主页'), a.homes));
  const m = auditMethod(a), [mName, mHint] = METHODS[m] || [m || t('其他'), ''];
  const how = make('p', 'tidy-meta', mName); how.title = mHint; row.append(how);
  const btns = make('div', 'tidy-acts-in');
  const btn = (label, verdict, cls_ = 'btn is-small') => {
    const b = make('button', cls_, label); b.type = 'button';
    Object.assign(b.dataset, {aa: verdict, uuid: it.uuid, artist: a.id, name: a.name});
    return b;
  };
  if (a.confirmed || a.rejected) {
    const said = make('span', 'tidy-verdict' + (a.confirmed ? ' is-yes' : ' is-no'), a.confirmed ? '✓ ' + t('对') : '✗ ' + t('不对'));
    btns.append(said, btn(t('改'), '', 'btn is-quiet is-small'));
  }
  else btns.append(btn(t('对'), 'yes', 'btn is-primary is-small'), btn(t('不对'), 'no'));
  row.append(btns);
  return row;
}
function auditItem(it, i) {
  const {el, refs} = ITEM(), img = refs.img.firstElementChild;
  const update = (x, k) => {
    attr(refs.img, 'href', imageUrl(x.uuid, 'full')); attr(refs.img, 'data-peek', x.uuid);
    attr(img, 'src', imageUrl(x.uuid, 'medium'));
    text(refs.no, String(k + 1));
    cls(el, 'is-no', x.artists.some(a => a.rejected));
    cls(el, 'is-yes', !!x.artists.length && x.artists.every(a => a.confirmed));
    const posts = x.posts?.length ? [links(t('原帖'), x.posts.map(p => ({url: p.url, label: p.site})))] : [];
    refill(refs.body, JSON.stringify([x.artists, x.posts]), () => [...x.artists.map(a => artistRow(x, a)), ...posts]);
  };
  update(it, i);
  return {el, update};
}

function auditView(scope) {
  const {el, refs} = VIEW();
  $('groupsPage').replaceChildren(el);
  const view = {scope, items: []};
  setPeekList(refs.list, () => view.items.map(x => x.uuid));
  const list = keyed(refs.list, {key: x => x.uuid, create: auditItem, scope});
  const stat = (big, label, small, title = '') => { const s = STAT(); text(s.refs.big, big); text(s.refs.label, label); text(s.refs.small, small); if (title) s.el.title = title; return s.el; };
  async function load() {
    const route = state.route, n = Number(prefs.get('artistAuditN', 50)) || 50;
    let d;
    try { d = await loadArtistAudit(n, scope.signal); } catch (e) { if (scope.alive && state.route === route) error(readError(e)); return; }
    if (!scope.alive || state.route !== route) return;
    view.items = d.items;
    const tl = auditTally(d.items), pct = (x, places) => x.done ? (x.yes / x.done * 100).toFixed(places) + '%' : '—';
    refill(refs.stats, JSON.stringify(tl), () => [
      stat(tl.accuracy === null ? '—' : (tl.accuracy * 100).toFixed(1) + '%', t('准确率'), t('目标 ≥ 95%')),
      stat(`${number(tl.all.done)} / ${number(tl.all.all)}`, t('已判断'), t('对 {yes} · 不对 {no}', {yes: number(tl.all.yes), no: number(tl.all.no)})),
      ...tl.methods.map(([m, x]) => stat(pct(x, 0), METHODS[m][0], t('{done} / {all} 已判', {done: number(x.done), all: number(x.all)}), METHODS[m][1]))]);
    text(refs.about, t('从 {pool} 张有画师的图里按固定的随机顺序取的前 {n} 张。判断会写进图库，判为「不对」的画师不再显示在这张图上。', {pool: number(d.pool), n: number(d.items.length)}));
    list.set(d.items);
    show(refs.moreBox, d.items.length < d.pool);
  }
  scope.on(refs.list, 'click', async e => {
    const look = e.target.closest('[data-peek]');
    if (look && !e.metaKey && !e.ctrlKey && !e.shiftKey) { e.preventDefault(); peekAt(look); return; }
    const b = e.target.closest('[data-aa]'); if (!b) return;
    const ran = await exclusive(async () => {
      b.disabled = true;
      try { written((await artistVerdict({uuid: b.dataset.uuid, artist: b.dataset.artist, verdict: b.dataset.aa, name: b.dataset.name})).undo); }
      catch (err) { toast(err.message); b.disabled = false; }
      return true;
    });
    if (ran) load();
  });
  scope.on(refs.more, 'click', () => { prefs.set('artistAuditN', (Number(prefs.get('artistAuditN', 50)) || 50) + 10); load(); });
  return {scope, load};
}
let view = null;

implement({
  // 画师抽查不按 NSFW 筛选（固定的一批随机图）：切换 NSFW 只重画标题里的提醒（#386 的 2-35）。
  'artist-audit': {render: ({samePage, scope}) => { if (!samePage || !view?.scope.alive) view = auditView(scope); return view.load(); },
    refresh: why => { if (why === 'undo') return view?.load(); if (why === 'nsfw') renderHeading(); }},
});
