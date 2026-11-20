// 资料库 · 画师（#448，0.4 #711 改成圆头像卡）：按画师浏览图库。读 GET /api/artist-directory（docs/WEB_V2.md「画师目录」），
// 一次一页，滚到底接着读；点一位画师进入按画师筛选的图库（#/library?artist=<key>，和详情里点画师名同一个选区）。
// 地址是 #/characters/artists，资料库页的一个页签（ui/catalog_page.js 建这里的部件、要页头的副标题 dirHeading）。
// 读到的列表留在模块里：从某位画师的图库后退回来时（restore），排序、搜索和读过的页都在，直接画，滚动位置由框架恢复。
// 没有画师时是统一的空状态：「还没有画师」、一句说明和「找回画师…」（去设置 › 画师，那里的「找回全库画师…」开任务）。
import {artistQuery, readArtists} from '../core/catalog.js';
import {dayLabel, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {onTyped} from '../core/keys.js';
import {onPage} from '../core/pages.js';
import {prefs} from '../core/prefs.js';
import {hashFor} from '../core/route.js';
import {readError, session} from '../core/session.js';
import {state} from '../core/state.js';
import {make, show, text, tpl, value} from './el.js';
import {searchField} from './catalog.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';
import {personCard} from './poster.js';

export const ORDERS = [['count', t('张数最多')], ['recent', t('最近收图')], ['name', t('名字')]];
export const PAGE = 120;
const saved = prefs.get('artistOrder', 'count');
// q、order 是工具栏上的条件；items、total、next 是按这组条件读到的部分（total 为 null 表示第一页还没回来）；
// failed 是最近一次读取的错误；key 是读这一份时的条件，条件变了就整份作废。
export const dir = {q: '', order: ORDERS.some(([k]) => k === saved) ? saved : 'count', items: [], total: null, next: 0, failed: null, key: ''};
let gen = 0, busy = null, drawn = null;

const keyOf = () => [dir.q, dir.order, state.nsfw, session.token].join('|');
export const onDir = route => onPage(route, 'characters') && route.section === 'artists';
// 第 offset 位起的一页。
export const dirQuery = offset => artistQuery({q: dir.q, order: dir.order, offset, limit: PAGE});

// 按现在的条件从头读：已经在读的那一页回来时丢掉。
function reset() { gen++; busy = null; Object.assign(dir, {items: [], total: null, next: 0, failed: null, key: keyOf()}); }
const done = () => dir.total !== null && dir.next >= dir.total;
const draw = () => { if (onDir(state.route)) drawn?.(); };
// 读下一页；读完、正在读或上次失败时不读。回来后画一次。
export function more() {
  if (busy || dir.failed || done()) return busy || Promise.resolve();
  const g = gen;
  const p = readArtists(dirQuery(dir.next)).then(r => {
    if (g !== gen || !r) return;
    const items = r.items || [], seen = new Set(dir.items.map(a => a.id));
    dir.items.push(...items.filter(a => !seen.has(a.id)));
    dir.total = r.total;
    // 空页说明读到头了（读的过程中画师变少了），不再接着读。
    dir.next = items.length ? (r.offset ?? dir.next) + items.length : Math.max(dir.next, r.total);
  }, e => { if (g === gen) dir.failed = e; }).finally(() => {
    if (g !== gen) return;
    busy = null;
    draw();
  });
  busy = p;
  return p;
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
// 标题下的副标题：读到之前写「…」占着这一行，标题高度不随页签变（#685 第 4 条）。
export const dirHeading = () => dir.total === null ? '…' : t('{n} 位画师', {n: number(dir.total)});

// ---------- 卡片和空状态 ----------
const tip = a => [a.name, t('{n} 张', {n: number(a.count)}), a.last ? t('最近收图：{day}', {day: dayLabel(a.last.slice(0, 10))}) : ''].filter(Boolean).join('\n');
// 一位画师的圆头像卡写什么：第二行按张数，「最近收图」排序时写日期。
export const cardData = a => ({id: a.id, href: hashFor({page: 'library', artist: a.id}), cover: a.cover, color: a.color, name: a.name, title: tip(a),
  sub: dir.order === 'recent' && a.last ? dayLabel(a.last.slice(0, 10)) : t('{n} 张', {n: number(a.count)})});
// 没有画师时说什么：搜索没结果、「只看 NSFW」开着，或者还没有找回过画师（给去处：设置里的「画师」）。
export function emptyState() {
  if (dir.q) return {icon: 'search', fact: t('没有符合的画师。')};
  if (state.nsfw === 'only') return {icon: 'person', fact: t('「只看 NSFW」开着，没有画过 NSFW 图的画师。')};
  return {icon: 'person', fact: t('还没有画师。'), note: t('找回画师按图片查出处，找到的画师列在这里。'), next: [{label: t('找回画师…'), href: '#/settings/artists'}]};
}

const VIEW = tpl`<div class="cat-artists"><p class="cat-loading" data-ref="loading" data-t>正在读取画师…</p><p class="cat-error" role="alert" data-ref="error"></p><div
  class="cat-people" data-ref="cards"></div><div data-ref="empty" hidden></div><button type="button" class="btn cat-more" data-ref="more" data-t>再显示一些</button></div>`;
const TOOLS = tpl`<div class="cat-tools"><span data-ref="q"></span><span class="spacer"></span><select data-ref="order" aria-label="排序" data-t-attr="aria-label"></select></div>`;

// 画师页签：进资料库时建一次（ui/catalog_page.js）；onDraw 在每次画完后调（页头的副标题跟着画师数变）。
export function artistsView(s, {onDraw} = {}) {
  const {el, refs} = VIEW(), tools = TOOLS(), search = searchField(t('搜索画师'));
  tools.refs.q.replaceWith(search.el);
  const cards = keyed(refs.cards, {key: x => x.id, create: personCard, scope: s});
  tools.refs.order.append(...ORDERS.map(([v, label]) => { const o = make('option', '', label); o.value = v; return o; }));
  onTyped(search.input, () => setQuery(search.input.value), 200);
  s.on(tools.refs.order, 'change', () => setOrder(tools.refs.order.value));
  s.on(refs.more, 'click', () => { dir.failed = null; more(); });
  // 「再显示一些」滚进视野（提前 600 px）就读下一页。
  // 每画完一次重新观察：读完一页后按钮如果仍在范围里，交叉状态没变就不会再通知。
  const io = typeof IntersectionObserver === 'function' ? s.observe(new IntersectionObserver(es => { if (es.some(e => e.isIntersecting)) more(); }, {rootMargin: '600px'})) : null;
  let lastEmpty = '';
  function update() {
    value(search.input, dir.q); value(tools.refs.order, dir.order);
    const failedFirst = dir.failed && !dir.items.length;
    show(refs.loading, dir.total === null && !failedFirst);
    show(refs.error, !!dir.failed); text(refs.error, dir.failed ? readError(dir.failed) : '');
    cards.set(dir.items.map(cardData));
    const empty = dir.total !== null && !dir.items.length && !dir.failed;
    show(refs.empty, empty);
    const e = empty ? emptyState() : null, sig = e ? JSON.stringify(e) : '';
    if (e && sig !== lastEmpty) drawEmpty(refs.empty, e);
    lastEmpty = sig;
    show(refs.more, dir.items.length > 0 && !done());
    if (io) { io.unobserve(refs.more); if (!refs.more.hidden) io.observe(refs.more); }
    onDraw?.();
  }
  s.add(() => { if (drawn === update) drawn = null; });
  return {el, tools: tools.el, update,
    // 进入本页签：后退、前进回来且条件没变时沿用读过的列表，否则从头读第一页。
    start: async ({restore = false} = {}) => {
      drawn = update;
      if (!restore || dir.key !== keyOf() || dir.failed || dir.total === null) reset();
      update();
      await more();
    },
    // 数据变了（同步、撤销、改动、NSFW 切换）之后从头重读。
    reload: why => {
      if (!['sync', 'undo', 'edit', 'nsfw'].includes(why) || !onDir(state.route)) return;
      reset(); update(); more();
    }};
}
