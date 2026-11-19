// 资料库 · 画师（#448）：按画师浏览图库。读 GET /api/artist-directory（docs/WEB_V2.md「画师目录」），一次一页，
// 滚到底接着读；点一位画师进入按画师筛选的图库（#/library?artist=<key>，和详情里点画师名同一个选区）。
// 地址是 #/characters/artists，资料库页的一个子页（core/pages.js 的 characters）；工具栏和标题副标题由 ui/shell.js
// 向这里要（dirToolbar、dirHeading），进入和刷新由 ui/catalog.js 转过来（renderArtistDir、refreshArtistDir）。
// 本模块不导入 shell.js、catalog.js（不新增 ui/ 循环导入）：画完一次就发 dirDrawn，shell 借它重画标题。
// 读到的列表留在模块里：从某位画师的图库后退回来时（restore），排序、搜索和读过的页都在，直接画，滚动位置由 shell 恢复。
import {listeners} from '../core/events.js';
import {dayLabel, esc, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {onTyped} from '../core/keys.js';
import {onPage} from '../core/pages.js';
import {prefs} from '../core/prefs.js';
import {hashFor} from '../core/route.js';
import {api, readError, session} from '../core/session.js';
import {state} from '../core/state.js';
import {$, emptyLink, searchIcon} from './dom.js';

export const ORDERS = [['count', t('张数最多')], ['recent', t('最近收图')], ['name', t('名字')]];
export const PAGE = 120;
const saved = prefs.get('artistOrder', 'count');
// q、order 是工具栏上的条件；items、total、next 是按这组条件读到的部分（total 为 null 表示第一页还没回来）；
// failed 是最近一次读取的错误；key 是读这一份时的条件，条件变了就整份作废。
export const dir = {q: '', order: ORDERS.some(([k]) => k === saved) ? saved : 'count', items: [], total: null, next: 0, failed: null, key: ''};
// 每画完一次目录发一次（shell 重画标题的副标题）。
export const dirDrawn = listeners();
let gen = 0, busy = null, nearing = null;

const keyOf = () => [dir.q, dir.order, state.nsfw, session.token].join('|');
export const onDir = route => onPage(route, 'characters') && route.section === 'artists';
// 第 offset 位起的一页。
export const dirQuery = offset => '/api/artist-directory?' + new URLSearchParams({...(dir.q ? {q: dir.q} : {}), order: dir.order, ...(state.nsfw ? {nsfw: state.nsfw} : {}), offset, limit: PAGE});

// 按现在的条件从头读：已经在读的那一页回来时丢掉。
function reset() { gen++; busy = null; Object.assign(dir, {items: [], total: null, next: 0, failed: null, key: keyOf()}); }
const done = () => dir.total !== null && dir.next >= dir.total;
// 读下一页；读完、正在读或上次失败时不读。回来后画一次。
export function more() {
  if (busy || dir.failed || done()) return busy || Promise.resolve();
  const g = gen;
  const p = api(dirQuery(dir.next)).then(r => {
    if (g !== gen || !r) return;
    const items = r.items || [], seen = new Set(dir.items.map(a => a.id));
    dir.items.push(...items.filter(a => !seen.has(a.id)));
    dir.total = r.total;
    // 空页说明读到头了（读的过程中画师变少了），不再接着读。
    dir.next = items.length ? (r.offset ?? dir.next) + items.length : Math.max(dir.next, r.total);
  }, e => { if (g === gen) dir.failed = e; }).finally(() => {
    if (g !== gen) return;
    busy = null;
    if (onDir(state.route)) draw();
  });
  busy = p;
  return p;
}

// ---------- 进入、刷新 ----------
// 进入本页：后退、前进回来且条件没变时沿用读过的列表，否则从头读第一页。
export async function renderArtistDir({restore = false} = {}) {
  $('hint').textContent = t('点画师，在图库里只看 TA');
  if (!restore || dir.key !== keyOf() || dir.failed || dir.total === null) reset();
  draw();
  await more();
}
// 数据变了（同步、撤销、改动、R18 切换）之后从头重读。
export function refreshArtistDir(why) {
  if (!['sync', 'undo', 'edit', 'nsfw'].includes(why) || !onDir(state.route)) return;
  reset(); draw(); more();
}

// ---------- 工具栏和标题 ----------
export const dirToolbar = () => `<div class="search-wrap wide">${searchIcon()}<input type="search" id="artistSearch" spellcheck="false" autocorrect="off" autocapitalize="off"` +
  ` value="${esc(dir.q)}" placeholder="${t('搜索画师')}" aria-label="${t('搜索画师')}"></div><div class="spacer"></div>` +
  `<select id="artistOrder" aria-label="${t('排序')}">${ORDERS.map(([v, label]) => `<option value="${v}"${dir.order === v ? ' selected' : ''}>${label}</option>`).join('')}</select>`;
export function wireDirToolbar() {
  const search = $('artistSearch');
  onTyped(search, () => setQuery(search.value), 200);
  $('artistOrder').onchange = e => setOrder(e.target.value);
}
export function setQuery(q) {
  q = q.trim();
  if (q === dir.q) return;
  dir.q = q; reset(); draw(); more();
}
export function setOrder(order) {
  if (!ORDERS.some(([k]) => k === order) || order === dir.order) return;
  dir.order = order; prefs.set('artistOrder', order);
  reset(); draw(); more();
}
// 标题下的副标题：读到之前和一位也没有时是空的（没有时由空状态说）。
export const dirHeading = () => dir.total ? t('{n} 位画师', {n: number(dir.total)}) : '';

// ---------- 目录 ----------
const tip = a => [a.name, t('{n} 张', {n: number(a.count)}), a.last ? t('最近收图：{day}', {day: dayLabel(a.last.slice(0, 10))}) : ''].filter(Boolean).join('\n');
export const card = a => `<a class="cast-rs" href="${hashFor({page: 'library', artist: a.id})}" data-artist-dir="${esc(a.id)}" title="${esc(tip(a))}">` +
  `<i class="cast-av" style="width:72px;height:72px">${a.cover ? `<img src="/image?id=${encodeURIComponent(a.cover)}&size=thumb" alt="" loading="lazy" decoding="async">` : ''}</i>` +
  `<b>${esc(a.name)}</b><small>${dir.order === 'recent' && a.last ? esc(dayLabel(a.last.slice(0, 10))) : t('{n} 张', {n: number(a.count)})}</small></a>`;
// 没有画师时说什么：搜索没结果、「只看 R18」开着，或者还没有找回过画师（给去处：设置里的「画师」）。
export function emptyText() {
  if (dir.q) return `<p class="empty">${t('没有符合的画师。')}</p>`;
  if (state.nsfw === 'only') return `<p class="empty">${t('「只看 NSFW」开着，没有画过 NSFW 图的画师。')}</p>`;
  return `<div class="empty"><div>${t('还没有画师。')}<div>${emptyLink('#/settings/artists', t('设置 › 画师'))}</div></div></div>`;
}
export function dirBody() {
  if (dir.failed && !dir.items.length) return `<p class="empty" role="alert">${esc(readError(dir.failed))}</p>`;
  if (dir.total === null) return `<p class="muted-note">${t('正在读取画师…')}</p>`;
  if (!dir.items.length) return emptyText();
  // 后面的某一页读失败：写出原因，按钮留着，点它再试。
  const tail = (dir.failed ? `<p class="muted-note" role="alert">${esc(readError(dir.failed))}</p>` : '') +
    (done() ? '' : `<button class="quiet cast-more" id="artistDirMore">${t('再显示一些')}</button>`);
  return `<section class="cast-sec"><div class="cast-roster">${dir.items.map(card).join('')}</div></section>${tail}`;
}
export function draw() {
  if (!onDir(state.route)) return;
  const root = $('charIndex');
  root.className = 'char-index cast';
  root.innerHTML = dirBody();
  // 「再显示一些」：点它，或者它滚进视野（提前 600 px）就读下一页。
  const button = $('artistDirMore');
  if (button) {
    button.onclick = () => { dir.failed = null; more(); };
    if (typeof IntersectionObserver === 'function') {
      nearing?.disconnect();
      nearing = new IntersectionObserver(es => { if (es.some(e => e.isIntersecting)) more(); }, {rootMargin: '600px'});
      nearing.observe(button);
    }
  }
  dirDrawn.emit();
}
