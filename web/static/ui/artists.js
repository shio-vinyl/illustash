// 详情卡片里的画师（桌面布局，#199；0.4 第 D 步改成部件，#709）：找到了就列出来，没找到时一个「查找画师」；以及点击即授权
// 的查找。请求和等待在 core/artists.js。
//
//   const block = artistBlock({verdict(uuid, artist, name, v), find(uuid), findMore(uuid, artist, name)}, parentScope);
//   block.update(d);   // d 是 /api/detail 的回复：artists、maybe、posts
import {t, tb} from '../core/i18n.js';
import {number} from '../core/format.js';
import {awaitLookup, startLookup} from '../core/artists.js';
import {hashFor} from '../core/route.js';
import {session} from '../core/session.js';
import {confirmAction} from './dialog.js';
import {toast} from './dom.js';
import {attr, make, refill, rich, show, text, tpl} from './el.js';
import {keyed} from './keyed.js';
import {app} from './scope.js';

// ---------- 按需查找画师（docs/PROVENANCE.md 第 9 节）：点击即授权这一次查找 ----------
const LOOKUP_NOTE = t('查找画师会把所选图片的像素哈希和 IQDB 签名（一串文本，不是图片）发给 Danbooru，按结果显示画师和原帖。\n「找 TA 的其他图」会下载这位画师公开作品的小缩略图，只在本机比对；缩略图比完就删，只留比对用的签名。\n\n以后点这两个按钮不再提示。继续吗？');
async function lookupAllowed() {
  try { if (localStorage.getItem('artistLookupOk')) return true; } catch {}
  if (!await confirmAction(LOOKUP_NOTE, t('查找画师'))) return false;
  try { localStorage.setItem('artistLookupOk', '1'); } catch {}
  return true;
}
async function artistLookup(body, label, after) {
  const token = session.token;
  if (!await lookupAllowed()) return;
  try { if (!await startLookup(body, token)) return; } catch (err) { toast(err.message); return; }
  toast(label);
  awaitLookup(({status, found, reason}) => {
    toast(['done', 'stopped'].includes(status) ? (found ? t('找到 {n} 张图的画师', {n: number(found)}) : t('没有找到新的画师'))
      : t('查找没有完成：{reason}', {reason: tb(reason) || status}));
    after?.(found);
  });
}
export const findArtists = (ids, after) => artistLookup({action: 'lookup', ids}, ids.length > 1 ? t('正在查找 {n} 张图的画师…', {n: number(ids.length)}) : t('正在查找画师…'), after);
export const findArtistWorks = (artist, name, after) => artistLookup({action: 'artist', artist}, t('正在找 {name} 的其他图…', {name: name || t('这位画师')}), after);

// ---------- 画师一节 ----------
const BLOCK = tpl`<section class="dc-section"><h3 class="dc-label" data-t>画师</h3><div class="dc-artists" data-ref="list"></div>
  <div class="dc-artist-none" data-ref="none"><span class="text-meta" data-ref="noneText"></span>
    <button type="button" class="btn is-small" data-ref="find" title="用像素哈希和 IQDB 查这张图的画师，几秒钟（只发哈希文本，不发图片）" data-t-attr="title"><span data-t>查找画师</span></button></div>
  <div class="dc-posts text-meta" data-ref="posts"><span data-t>原帖</span><span data-ref="postLinks"></span></div></section>`;
const ROW = tpl`<div class="dc-artist"><div class="dc-artist-top"><span class="dc-artist-name" data-ref="name"></span>
  <span class="dc-judge" data-ref="judge"><span class="dc-artist-ok" data-ref="ok" title="你确认过" data-t-attr="title">✓ <span data-t>已确认</span></span>
    <button type="button" class="chip" data-ref="undo" title="撤回确认" data-t-attr="title"><span data-t>撤回</span></button>
    <button type="button" class="chip" data-ref="yes" title="确认是 TA 画的" data-t-attr="title"><span data-t>是 TA</span></button>
    <button type="button" class="chip" data-ref="no"><span data-t>不是</span></button></span></div>
  <a class="dc-artist-more" data-ref="more"></a><div class="text-meta" data-ref="reason"></div><div class="dc-homes" data-ref="homes"></div>
  <button type="button" class="btn is-small is-quiet" data-ref="findMore"><span data-t>找 TA 的其他图</span></button></div>`;

// 一串链接（主页、原帖），中间用 · 隔开。只在内容变了时重画（签名是各链接的地址和文字）。
function links(node, list, title = () => '') {
  refill(node, JSON.stringify(list.map(x => [x.url, x.label || x.site])), () => list.flatMap((x, i) => {
    const a = make('a', 'link', x.label || x.site);
    a.href = x.url; a.target = '_blank'; a.rel = 'noreferrer noopener';
    const tip = title(x); if (tip) a.title = tip;
    return i ? [document.createTextNode(' · '), a] : [a];
  }));
}
const homeTitle = h => t('在 {site} 打开 TA 的主页', {site: h.site});

// 一位画师（确定的或「也许是」的）。
function artistRow(on, parent) {
  const {el, refs} = ROW(), s = parent.child('artist');
  let cur = null;
  const link = make('a', 'artist-name');
  s.on(refs.yes, 'click', () => on.verdict(cur.id, cur.name, 'yes'));
  s.on(refs.no, 'click', () => on.verdict(cur.id, cur.name, 'no'));
  s.on(refs.undo, 'click', () => on.verdict(cur.id, cur.name, null));
  s.on(refs.findMore, 'click', () => on.findMore(cur.id, cur.name));
  return {el, dispose: () => s.dispose(), update(a) {
    cur = a;
    const href = hashFor({page: 'library', artist: a.id});
    text(link, a.name); attr(link, 'href', href); attr(link, 'title', t('在图库里只看 TA 的图'));
    if (refs.name.dataset.maybe !== String(!!a.maybe)) {
      refs.name.dataset.maybe = String(!!a.maybe);
      refs.name.replaceChildren(a.maybe ? rich(t('也许是 {name}', {name: '{name}'}), {name: link}) : link);
    }
    show(refs.ok, a.confirmed); show(refs.undo, a.confirmed); show(refs.yes, !a.confirmed); show(refs.no, !a.confirmed);
    attr(refs.no, 'title', a.maybe ? t('不是 TA 画的：不再推荐') : t('不是 TA 画的：这张不再显示这位画师'));
    show(refs.more, !a.maybe && a.count > 1); attr(refs.more, 'href', href); text(refs.more, a.count > 1 ? t('库里还有 {n} 张', {n: number(a.count - 1)}) : '');
    show(refs.reason, !!a.maybe); text(refs.reason, a.maybe ? tb(a.reason || '') : '');
    links(refs.homes, a.homes || [], homeTitle); show(refs.homes, !!a.homes?.length);
    attr(refs.findMore, 'title', a.maybe ? t('下载 TA 公开作品的小缩略图，在本机比对') : t('下载 TA 公开作品的小缩略图，在本机比对出库里 TA 的其他图（一两分钟）'));
  }};
}

/**
 * 画师一节：on 是 {verdict(uuid, id, name, 'yes' | 'no' | null), find(uuid), findMore(uuid, id, name)}，uuid 是这一节正显示
 * 的那张图。换图读取途中（loading() 之后、update() 之前）按钮不响应：显示的还是上一张的画师，不能记到新的那张上。
 * parent 是父作用域（详情卡片的；默认挂在 app 上）。
 */
export function artistBlock(on, parent = app) {
  const {el, refs} = BLOCK(), s = parent.child('artists');
  let shown = null;
  const call = (fn, ...args) => { if (shown) fn(shown, ...args); };
  const rowOn = {verdict: (...args) => call(on.verdict, ...args), findMore: (...args) => call(on.findMore, ...args)};
  const list = keyed(refs.list, {key: a => (a.maybe ? 'maybe:' : '') + a.id, create: a => { const row = artistRow(rowOn, s); row.update(a); return row; }, scope: s});
  s.on(refs.find, 'click', () => call(on.find));
  return {el, dispose: () => s.dispose(), loading() { shown = null; }, update(d) {
    shown = d.uuid;
    const artists = d.artists || [], maybe = d.maybe || [];
    list.set([...artists.map(a => ({...a, maybe: false})), ...maybe.map(a => ({...a, maybe: true}))]);
    show(refs.none, !artists.length);
    text(refs.noneText, maybe.length ? t('还没确定画师') : t('还不知道画师'));
    links(refs.postLinks, d.posts || []); show(refs.posts, !!d.posts?.length);
  }};
}
