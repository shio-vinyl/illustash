// 重复 · 相似的图（#713，#/similar）和相似组 N（#/s/<id>，网格页）页头上的「判断关系」「看过了」（#427）。
// 数据在 core/tidy.js。
//
// - 列表：每组一张卡片，封面按张数拼 1、2 或 4 张填满（core/layout.js 的 cellTiles），下面是组号、张数和关系类型。卡片按组号
//   复用（ui/keyed.js），重读、撤销、学习落定之后内容没变的留在原处，封面不重新加载（#601）。先显示 40 组，「再显示」接着往下；
//   换关系类型、切换看过的组、切换 NSFW 时回到前 40 组。
// - 标过「看过了」的组不在列表里，「看过的 N 组」（#/similar?seen=1）只列它们；组里进了新图，后端就不再算它看过。
// - 检查进行中每 3 秒重读一次（页面作用域的计时器，离开这一页就停）。同一路由上的几次重读只认最后发出的那次，轮询也只由它续，
//   所以撤销、学习落定、「重新检查」再多次也只有一条轮询。
import {exclusive} from '../core/edits.js';
import {imageUrl, number, thumbSize} from '../core/format.js';
import {t} from '../core/i18n.js';
import {SIMILAR_ACTIVE, similarLine} from '../core/jobs.js';
import {RELATION_KINDS, RELATION_NAMES} from '../core/labels.js';
import {cellTiles} from '../core/layout.js';
import {implement} from '../core/pages.js';
import {go, hashFor} from '../core/route.js';
import {state} from '../core/state.js';
import {loadSimilar, readSimilarSeen, scanSimilar, setSimilarSeen} from '../core/tidy.js';
import {gridFilters as filters, listIds} from '../core/assets.js';
import {$, toast, written} from './dom.js';
import {attr, cls, css, make, refill, show, text, tpl} from './el.js';
import {drawEmpty, showFailure} from './empty.js';
import {keyed} from './keyed.js';
import {openRelate} from './palette.js';
import {renderHeading} from './shell.js';
import {latest, pageView} from './page_view.js';
import {releaseImages} from './virtual.js';

const PAGE = 40;
// 封面四张图之间的缝，按封面边长的百分比。
const GAP = 2;
// 封面大约多宽（css/tidy.css 的 .tidy-cards 是 minmax(240px, 1fr)，常见窗口里一格 240–320px），按它选缩略图档位。
const COVER_PX = 280;
const VIEW = tpl`<div class="tidy-page">
  <div class="tidy-bar">
    <nav class="seg" data-ref="kinds" aria-label="关系类型" data-t-attr="aria-label"></nav>
    <span class="tidy-status"><span class="local-dot" data-ref="dot"></span><span data-ref="status"></span></span>
    <a class="btn is-quiet is-small" data-ref="other" hidden></a>
    <button type="button" class="btn is-quiet is-small" data-ref="scan" title="比较新加入的图和还没比较过的候选" data-t data-t-attr="title">重新检查</button>
  </div>
  <div class="progress" data-ref="progress" hidden><span data-ref="bar"></span></div>
  <div class="tidy-cards" data-ref="list"></div>
  <div class="tidy-more" data-ref="moreBox" hidden><button type="button" class="btn" data-ref="more"></button></div>
  <div data-ref="empty" hidden></div>
</div>`;
const CARD = tpl`<a class="tidy-card">
  <span class="tidy-covers" data-ref="covers"><img alt="" decoding="async" loading="lazy" draggable="false"><img alt="" decoding="async" loading="lazy"
    draggable="false"><img alt="" decoding="async" loading="lazy" draggable="false"><img alt="" decoding="async" loading="lazy" draggable="false"></span>
  <span class="tidy-card-meta"><b data-ref="title"></b><span class="text-meta" data-ref="n"></span></span>
  <span class="tidy-kinds" data-ref="kinds"></span>
</a>`;

function similarCard(g) {
  const {el, refs} = CARD(), imgs = [...refs.covers.children];
  const update = x => {
    attr(el, 'href', '#/s/' + x.gid);
    text(refs.title, t('相似组 {id}', {id: x.gid}));
    text(refs.n, t('{n} 张', {n: number(x.size)}));
    const tiles = cellTiles(Math.min(4, x.covers.length), {width: 100, height: 100, gap: GAP});
    imgs.forEach((img, k) => {
      const box = tiles[k], uuid = box && x.covers[box.i];
      if (!uuid) { attr(img, 'src', null); show(img, false); return; }
      attr(img, 'src', imageUrl(uuid, thumbSize(COVER_PX * box.w / 100)));
      show(img, true);
      css(img, 'left', `${box.x}%`); css(img, 'top', `${box.y}%`); css(img, 'width', `${box.w}%`); css(img, 'height', `${box.h}%`);
    });
    const kinds = Object.entries(x.kinds).sort((a, b) => b[1] - a[1]);
    refill(refs.kinds, JSON.stringify(kinds), () => kinds.map(([k, n]) => make('span', 'chip is-small', (RELATION_NAMES[k] || k) + (n > 1 ? ' ×' + n : ''))));
  };
  update(g);
  return {el, update, dispose: () => releaseImages(el)};
}

function similarView(scope) {
  const {el, refs} = VIEW();
  $('groupsPage').replaceChildren(el);
  const page = {scope, groups: [], others: 0, status: {}, shown: PAGE, loaded: false}, begin = latest(scope);
  const list = keyed(refs.list, {key: g => g.gid, create: similarCard, scope});
  function draw(route) {
    const {groups, others, status: s} = page, kind = route.kind || '', seen = route.seen === '1';
    // 关系类型的链接带着「看过的组」：同一页从没看过切到看过的组时也要重建。
    refill(refs.kinds, `${kind}|${seen ? '1' : ''}`, () => RELATION_KINDS.map(([k, label]) => {
      const a = make('a', '', label); a.href = hashFor({page: 'similar', kind: k, seen: seen ? '1' : ''});
      if (k === kind) a.setAttribute('aria-current', 'page');
      return a;
    }));
    const busy = SIMILAR_ACTIVE.includes(s.status), line = similarLine(s);
    const n = {n: number(groups.length), images: number(groups.reduce((m, g) => m + g.size, 0))};
    cls(refs.dot, 'busy', busy);
    text(refs.status, (seen ? t('看过的 {n} 组 · 共 {images} 张', n) : t('{n} 组 · 共 {images} 张', n)) + (line ? ' · ' + line : ''));
    show(refs.other, !!others);
    attr(refs.other, 'href', hashFor({page: 'similar', kind, seen: seen ? '' : '1'}));
    text(refs.other, (seen ? t('没看过的 {n} 组', {n: number(others)}) : t('看过的 {n} 组', {n: number(others)})) + ' →');
    attr(refs.scan, 'disabled', busy || s.status === 'off');
    show(refs.progress, busy && !!s.total);
    if (busy && s.total) css(refs.bar, 'width', Math.round((s.done || 0) / s.total * 100) + '%');
    list.set(groups.slice(0, page.shown));
    show(refs.moreBox, groups.length > page.shown);
    text(refs.more, t('再显示 {n} 组', {n: number(Math.min(PAGE, groups.length - page.shown))}));
    const empty = page.loaded && !groups.length;
    show(refs.empty, empty);
    if (!empty) return;
    // 空状态按内容的签名重画：检查进行中每 3 秒重读，内容没变就不动（焦点在链接上时不丢）。
    const fact = seen ? t('没有看过的组。') : others ? t('相似的组都看过了。') : busy ? t('还在检查，找到的组会陆续出现。') : t('没有找到相似的图。');
    const next = seen ? [{label: t('回到相似的图'), href: '#/similar'}] : others ? [{label: t('看过的 {n} 组', {n: number(others)}), href: hashFor({page: 'similar', kind, seen: '1'})}] : [];
    drawEmpty(refs.empty, {icon: 'check', fact, next});
  }
  // keep：轮询、撤销、学习落定以后重读，留在已显示的组数；不给是换了关系类型、看过的组或 NSFW，从前 40 组开始。
  async function load(keep = false) {
    const route = state.route, kind = route.kind || '', seen = route.seen === '1', fresh = begin(route);
    let result;
    try { result = await loadSimilar({kind, nsfw: state.nsfw}, scope.signal); } catch (e) { if (fresh()) showFailure(refs.empty, e, {loaded: page.loaded}); return; }
    if (!fresh()) return;
    const groups = result.groups.filter(g => !!g.seen === seen);
    Object.assign(page, {groups, others: result.groups.length - groups.length, status: result.state, loaded: true});
    if (!keep) page.shown = PAGE;
    draw(route);
    // 只有最后一次读取续轮询：期间又发了别的读取，就由那一次接着排。
    if (SIMILAR_ACTIVE.includes(result.state.status)) scope.timeout(() => { if (fresh()) load(true); }, 3000);
  }
  scope.on(refs.more, 'click', () => { page.shown += PAGE; draw(state.route); });
  scope.on(refs.empty, 'click', e => { if (e.target.closest('[data-empty="retry"]')) load(); });
  scope.on(refs.scan, 'click', async () => { try { await scanSimilar(); scope.timeout(() => load(true), 800); } catch (e) { toast(e.message); } });
  return {scope, load, page};
}
const view = pageView(similarView);

// ---------- 相似组 N（#/s/<id>，网格页）的「看过了」（#427） ----------
// 进入前读一次这组算不算看过、看得到几张。读不到就不显示按钮，不挡住这一页。
async function readSeen(route) {
  let g; try { g = await readSimilarSeen(route.id, state.nsfw); } catch { g = null; }
  if (state.route !== route) return false;
  state.sgroupSeen = g;
}
function sgroupActions() {
  const out = [], g = state.sgroupSeen;
  if (state.total >= 2 && state.total <= 12) {
    const b = make('button', 'btn is-quiet is-small', t('判断关系')); b.type = 'button'; b.id = 'sgroupRelate';
    b.onclick = async () => openRelate(await listIds(filters(), 12));
    out.push(b);
  }
  if (g) {
    const b = make('button', 'btn is-quiet is-small', g.seen ? t('标为没看过') : t('看过了')); b.type = 'button'; b.id = 'sgroupSeen';
    b.title = g.seen ? t('回到「相似的图」列表') : t('从「相似的图」列表里隐去；组里进了新图会再出现');
    b.onclick = toggleSeen;
    out.push(b);
  }
  return out;
}
// 标为看过以后回到列表，这组不在了；撤销在提示条上。张数和读的时候对不上（组刚变过）后端拒绝，这里重读。
async function toggleSeen() {
  const route = state.route;
  if (state.sgroupSeen && state.sgroupSeen.nsfw !== state.nsfw) await readSeen(route);
  const g = state.sgroupSeen; if (!g || state.route !== route) return;
  await exclusive(async () => {
    let r; try { r = await setSimilarSeen({gid: Number(route.id), size: g.size, nsfw: g.nsfw, seen: !g.seen}); }
    catch (e) { toast(e.message); if (e.status === 409 && await readSeen(route) !== false) renderHeading(); return; }
    written(r.undo);
    if (!g.seen) go({page: 'similar'});
    else if (state.route === route) { state.sgroupSeen = {...g, seen: false}; renderHeading(); }
  });
}

implement({
  // 撤销之后重画（#386 的 2-41）：撤销的可能就是「看过了」。撤销、学习落定后按组比对、留在已显示的组数（#601）。
  similar: {render: o => view.enter(o).load(),
    refresh: why => { if (why === 'undo' || why === 'learned') return view.get()?.load(true); if (why === 'nsfw') return view.get()?.load(); }},
  sgroup: {prepare: readSeen, head: (r, {n}) => ({title: t('相似组 {id}', {id: r.id}), sub: t('{n} 张', {n}), actions: sgroupActions()}),
    // 改动、撤销、同步之后组里看得到的张数可能变了，重读一次，免得点「看过了」时张数对不上。
    refresh: async why => { if (why !== 'view' && why !== 'ask') { const route = state.route; if (await readSeen(route) !== false) renderHeading(); } }},
});
