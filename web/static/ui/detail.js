// 近看一张图（桌面布局，#199；0.4 第 D 步改成详情卡片，#709）：打开、翻页、关掉详情卡片，卡片和大图底栏上各个按钮的动作，
// 以及单图快捷键（C、A、S、X、F、G、⌫、1–9、Y／N）。卡片的样子在 ui/detail_card.js，大图在 ui/viewer.js，数据经 core/detail.js 读。
//
// - 卡片建一次，挂在 #inspector 里；翻页、写入后重读只 update，不重建。打开时从右边滑进来，关掉时滑出去再藏起 #inspector。
// - 卡片和大图底栏的点击都交给 act(名字, data-*, 事件, 详情)：同一个按钮在两处做同一件事。
// - 大图的函数照旧从这里导出（ui/grid.js、shell.js 等从这里导入）；它们的实现在 ui/viewer.js。
import {t} from '../core/i18n.js';
import {number} from '../core/format.js';
import {leaves} from '../core/filters.js';
import {questionBody} from '../core/ask.js';
import {adoptDetail, askAnswered, primaryOf, readCharStats, readDetail, setDetail, setQuestion} from '../core/detail.js';
import {openFile, revealFile} from '../core/files.js';
import {fetchChar} from '../core/meta.js';
import {shownUuid} from '../core/modes.js';
import {pageOf, poolIs, viewOf} from '../core/pages.js';
import {filterRoute, go} from '../core/route.js';
import {likeRoute} from '../core/like.js';
import {state} from '../core/state.js';
import {$, toast} from './dom.js';
import {currentItem, endPreview, favoriteOne, kept, loadPage, markDetailKeys, mutate, openTagPicker, previewList, previewing, renderGrid, setCursor, syncCards, toggleInfo} from './grid.js';
import {viewOpts} from '../core/view_opts.js';
import {rowHeight, rowOf, rowTop} from './grid_rows.js';
import {PAGE} from '../core/assets.js';
import {newIds, openPicker, targets} from './palette.js';
import {findArtistWorks, findArtists} from './artists.js';
import {wireLift} from './drag.js';
import {key, write} from './frame.js';
import {cancelExit, enter, exit} from './motion.js';
import {detailCard} from './detail_card.js';
import {bindViewer, closeViewer, openViewer, showViewer, viewerFav, wireViewer} from './viewer.js';

export {closeViewer, openViewer, peek, peekAt, showViewer, wirePeek, zoomView} from './viewer.js';

// 卡片滑进、滑出（只动 transform 和 opacity）。
const SLIDE_IN = [{opacity: 0, transform: 'translateX(24px)'}, {opacity: 1, transform: 'none'}];
const SLIDE_OUT = [{opacity: 1, transform: 'none'}, {opacity: 0, transform: 'translateX(16px)'}];

// 哪一次读取是当前的：更慢、更早的那次绝不会画到卡片上。
let detailGen = 0, card = null, statsFor = null;
function mount() {
  if (card) return card;
  card = detailCard({act, artist: {
    verdict: (ident, artist, name, v) => mutate('/api/artist/verdict', {ids: [ident], artist, verdict: v, name}, !!v && leaves(state.route, `artist:${artist}`, v === 'yes')),
    find: ident => findArtists([ident], () => inspect(ident, true)),
    findMore: (ident, artist, name) => findArtistWorks(artist, name, () => inspect(ident, true)),
  }});
  $('inspector').replaceChildren(card.el);
  // 按住预览图拖动：拿起这张图，放进活页条上的画廊，拖出窗口边缘就是拖到别的应用（ui/drag.js，#710、#450）。
  card.scope.add(wireLift(card.preview, () => [card.uuid].filter(Boolean)));
  return card;
}
const GRID = key('detail:grid');   // 帧任务的令牌（#751）：详情栏全应用只有一份，在模块顶层取
const relayout = () => write(() => renderGrid(), GRID);

// ---------- 打开、关掉 ----------
export function closeInspector() {
  detailGen++; if (state.viewer < 0) kept.hold = null;
  state.focused = null; setDetail(null); if (state.viewer < 0) setQuestion(null);
  pageOf(state.route).focus?.(state.focused); syncCards();
  const box = $('inspector');
  if (box.hidden || !card) { box.hidden = true; document.body.classList.remove('inspecting'); return; }
  // 滑出去以后再藏：中途又打开了（state.focused 有值）就不藏。
  exit(card.el, {to: SLIDE_OUT, remove: false}).then(() => {
    if (state.focused) return;
    box.hidden = true; card.el.hidden = false; document.body.classList.remove('inspecting'); relayout();
  });
}
export async function inspect(ident, quiet = false) {
  const gen = ++detailGen, c = mount(), box = $('inspector');
  state.focused = ident;
  if (!quiet || c.uuid !== ident) c.loading(ident);
  // body.inspecting：详情卡片开着（app.css 按它藏上手清单，不用 body:has(#inspector:not([hidden]))）。
  if (box.hidden || c.el.hidden) { box.hidden = false; c.el.hidden = false; document.body.classList.add('inspecting'); enter(c.el, {from: SLIDE_IN}); }
  else cancelExit(c.el);   // 正在滑出时接回来
  pageOf(state.route).focus?.(state.focused); syncCards(); relayout();
  let d;
  try { d = await readDetail(ident); } catch (e) { if (gen === detailGen) c.failed(e.message); return; }
  if (gen !== detailGen) return;
  const asking = adoptDetail(d, ident, viewOpts.ask);
  const opts = {ask: asking, chart: viewOpts.chart, same: viewOpts.same, meta: viewOpts.meta, skip: !!pageOf(state.route).skip, resumeKey: viewOf(state.route).bar === 'skipped'};
  c.update(d, opts);
  markDetailKeys();
  // 角色卡：换了主角色、或视图选项里两项都关掉时先藏起来；要显示的话读回再画。
  const primary = primaryOf(d), want = primary && (opts.chart || opts.same) ? primary.id : null;
  if (statsFor !== want) { statsFor = want; c.stats(null, opts); }
  if (want) readCharStats(want, ident).then(st => { if (gen === detailGen) c.stats(st, opts, ident, want); });
}

// ---------- 按钮的动作（卡片和大图底栏共用） ----------
function act(name, data, e, d) {
  const ident = d.uuid;
  switch (name) {
    case 'nav': return step(Number(data.step));
    case 'fav': return favoriteOne(ident);
    case 'expand': return openViewer(indexOf(ident));
    case 'close': return closeInspector();
    case 'info': return toggleInfo();
    case 'like': return likeOne(ident);
    case 'copy': return inspect(data.id);
    case 'ask': return answerQuestion(data.how);
    // 主角色是机器认出的：「没错」把它记成你确认的。在「可能认错」里确认了就离开这一页。
    case 'confirm': return mutate('/api/change', {ids: [ident], action: 'confirmed', entity: data.id}, !!state.route.doubt && state.route.char === data.id);
    case 'swap': return swapOne(d, ident, data.id);
    case 'not': return mutate('/api/change', {ids: [ident], action: 'rejected', entity: data.id}, state.route.char === data.id);
    case 'also': return addOne(d, ident);
    case 'alt': return pickAlternative(d, ident, Number(data.i), e.altKey);
    case 'unvariant': return mutate('/api/change', {ids: [ident], action: 'remove_variant', entity: data.id}, !!state.route.variant);
    case 'addVariant': return addVariant(ident, data.id);
    // 从详情点标签和从工具栏选一样：在图库页上叠加到现有的筛选上，在别的页面去图库只看它（#386 的 2-30）。
    case 'tag': return go(filterRoute({tag: data.tag}));
    case 'utag': return go(filterRoute({utag: data.tag}));
    case 'untag': return mutate('/api/tag', {ids: [ident], tag: data.tag, add: false}, leaves(state.route, `utag:${data.tag}`, false));
    case 'addTag': return singleAction('tag');
    case 'file': return (data.kind === 'open' ? openFile(ident) : revealFile(ident)).catch(err => toast(err.message));
    case 'curate': return mutate('/api/curate', {ids: [ident], action: data.curate}, data.curate === 'exclude' || poolIs(state.route, 'excluded', 'pending'));
    case 'resume': return mutate('/api/change', {ids: [ident], action: 'resume'}, poolIs(state.route, 'skipped'));
    case 'skip': case 'exclude': return singleAction(name);
  }
}
// 给一张图追加角色（还没有角色时是设为）：详情、大图的「＋」和 C、A 是同一个面板，都能多选（#424 的 2-24）。
function addOne(d, ident) {
  openPicker({title: d.memberships.length ? t('这张也是…') : t('这张是…'), sentence: e => d.memberships.length ? t('给这张加上 {name}', {name: e.name}) : t('把这张设为 {name}', {name: e.name}), exclude: d.memberships.map(m => m.id), suggest: {ids: [ident]}, multi: true,
    pick: list => mutate('/api/change', {ids: [ident], action: 'confirmed', ...targets(list)}, pageOf(state.route).lead)});
}
function swapOne(d, ident, from) {
  const old = d.memberships.find(m => m.id === from);
  openPicker({title: t('换成…'), sentence: e => t('把这张从 {from} 换成 {name}', {from: old.name, name: e.name}), exclude: d.memberships.map(m => m.id), suggest: {ids: [ident]}, multi: true,
    pick: list => mutate('/api/reassign', {ids: [ident], old: from, new: newIds(list)}, state.route.char === from)});
}
async function addVariant(ident, charId) {
  const epoch = state.readEpoch, mode = state.nsfw;
  const c = await fetchChar(charId);
  if (!c || epoch !== state.readEpoch || mode !== state.nsfw || (state.focused !== ident && state.viewerItem?.uuid !== ident)) return;
  openPicker({title: t('加造型'), subject: [ident], items: c.variants.map(v => ({id: v.id, name: v.name, hint: t('{n} 张', {n: number(v.count)}), counted: true})), placeholder: t('筛选造型'),
    sentence: e => t('给这张加上 {name}', {name: e.name}), pick: e => mutate('/api/change', {ids: [ident], action: 'add_variant', entity: e.id}, false)});
}
export function pickAlternative(d, ident, i, also = false) {
  const alt = d.alternatives?.[i]; if (!alt) return;
  const from = primaryOf(d);
  if (from && !also) mutate('/api/reassign', {ids: [ident], old: from.id, new: alt.id}, state.route.char === from.id);
  else mutate('/api/change', {ids: [ident], action: 'confirmed', entity: alt.id}, pageOf(state.route).lead);
}
// 「是 TA 吗？」的回答（Y、N，或点按钮），发到 /api/ask/answer 供学习。
export function answerQuestion(how) {
  const cur = state.question; if (!cur) return;
  askAnswered();
  // 随机抽查：回答过的图离开列表，所以详情移到下一张。
  if (poolIs(state.route, 'audit') && state.viewer < 0 && state.focused === cur.ident) { const i = indexOf(cur.ident); state.focused = state.items[i + 1]?.uuid || state.items[i - 1]?.uuid || null; }
  mutate('/api/ask/answer', questionBody(cur, how), how !== 'yes' && state.route.char === cur.q.entity, {advance: poolIs(state.route, 'audit')});
}
// 「更多像这张的」（#449）：去相似结果（core/like.js；结果页的标题在 ui/like.js）。在相似结果里再点，换成这一张作例图，比较方式不变。
const likeOne = ident => go(likeRoute([ident], state.route.by));

// 没有选中任何图（或大图盖住了选择栏）时，单图快捷键作用于当前图：大图 ＞ 详情 ＞ 光标（core/modes.js）。
export async function singleAction(kind) {
  const item = currentItem(); if (!item) return;
  const r = state.route;
  if (kind === 'remove') {
    if (r.char) mutate('/api/change', {ids: [item.uuid], action: 'rejected', entity: r.char}, true);
    else if (item.memberships.length === 1) mutate('/api/change', {ids: [item.uuid], action: 'rejected', entity: item.memberships[0].id}, false);
    else if (item.memberships.length) openPicker({title: t('这张不是…'), items: item.memberships, subject: [item.uuid], sentence: e => t('把 {name} 从这张移出', {name: e.name}), pick: e => mutate('/api/change', {ids: [item.uuid], action: 'rejected', entity: e.id}, false)});
  } else if (kind === 'tag') {
    const d = state.detail?.uuid === item.uuid ? state.detail : null;
    openTagPicker(() => ({ids: [item.uuid]}), 1, [{...item, user_tags: d?.user_tags || item.user_tags}], () => false);
  // 大图里跳过、排除之后到下一张，和随机抽查回答之后一样（用户 2026-11-14 定，#424 的 2-1）；改角色仍停在原图。
  } else if (kind === 'skip' && pageOf(r).skip) mutate('/api/change', {ids: [item.uuid], action: 'unknown'}, true, {advance: state.viewer >= 0});
  else if (kind === 'resume') mutate('/api/change', {ids: [item.uuid], action: 'resume'}, poolIs(r, 'skipped'));
  else if (kind === 'fav') favoriteOne(item.uuid);
  else if (kind === 'exclude' || kind === 'include') mutate('/api/curate', {ids: [item.uuid], action: kind}, true, {advance: kind === 'exclude' && state.viewer >= 0});
  else if (kind === 'swap' || kind === 'also') {
    const d = state.detail?.uuid === item.uuid ? state.detail : await readDetail(item.uuid);
    const from = primaryOf(d);
    if (kind === 'swap' && from) swapOne(d, item.uuid, from.id);
    else addOne(d, item.uuid);
  }
}
// 收藏写入成功、没有重读时（ui/grid.js 的 favorite）：详情和大图里那张也改掉。changed 是列表里改到的图，ids 是这次写入
// 点名的图（全选时为空）；这里改到的也加进 changed，好让网格卡片的心形跟着改（ui/favorites.js 的 paintHearts）。
// 详情卡片和大图底栏的收藏按钮不归 paintHearts 管（它绕开 ui/el.js 的缓存），这里经部件自己改。
export function noteFavorite(changed, ids, on) {
  for (const x of [state.detail, state.viewerItem]) if (x && (changed.has(x.uuid) || ids?.has(x.uuid))) { x.fav = on; changed.add(x.uuid); }
  if (card?.uuid && (changed.has(card.uuid) || ids?.has(card.uuid))) card.fav(on);
  for (const uuid of changed) viewerFav(uuid, on);
}

// ---------- 翻页 ----------
export const indexOf = uuid => state.items.findIndex(x => x?.uuid === uuid);
export async function step(delta) {
  const cur = shownUuid();
  if (kept.hold && kept.hold.uuid === cur && Math.abs(delta) === 1) {
    const target = indexOf(delta > 0 ? kept.hold.next : kept.hold.prev); kept.hold = null;
    if (target >= 0) { setCursor(target); ensureVisible(target); if (state.viewer >= 0) showViewer(target); else inspect(state.items[target].uuid); return; }
  }
  kept.hold = null;
  const at = cur ? indexOf(cur) : -1;
  const from = at >= 0 ? at : state.viewer >= 0 ? state.viewer : state.cursor;
  const n = Math.max(0, Math.min(state.total - 1, from + delta)); if (n === from || n < 0) return;
  if (!state.items[n]) await loadPage(Math.floor(n / PAGE) * PAGE);
  if (!state.items[n]) return;
  setCursor(n); ensureVisible(n);
  if (state.viewer >= 0) showViewer(n); else inspect(state.items[n].uuid);
}
// 把网格滚到第 i 张可见。主区不是网格的页面（按天视图从详情栏、大图翻页，按组整理看大图）不按网格的行去滚，交给页面登记的
// reveal 把那张图滚进视野（#424 的 2-11）。
export function ensureVisible(i) {
  if (viewOf(state.route).area !== 'grid') { const uuid = state.items[i]?.uuid; if (uuid) pageOf(state.route).reveal?.(uuid); return; }
  const v = $('viewport'), row = rowOf(i), top = rowTop(row), height = rowHeight(row);
  if (top < v.scrollTop) v.scrollTop = top; else if (top + height > v.scrollTop + v.clientHeight) v.scrollTop = top + height - v.clientHeight;
  syncCards();
}

// 看图器要用的网格和详情的函数（ui/viewer.js 不导入 ui/grid.js、本文件）。全部包一层：ui/grid.js 和本文件互相导入，这时
// grid.js 的导出可能还没初始化。在载入时登记，是 WEB_V2.md「模块载入时不做事」的例外：tests/restore_place.test.cjs 不调
// wireDetail 也要能 showViewer；这里只登记函数，不碰 DOM、不挂监听。
bindViewer({
  loadPage: offset => loadPage(offset), get PAGE() { return PAGE; }, get kept() { return kept; }, setCursor: i => setCursor(i),
  previewList: (ids, i) => previewList(ids, i), endPreview: () => endPreview(), previewing: () => previewing(),
  ensureVisible: i => ensureVisible(i), inspect: (id, quiet) => inspect(id, quiet), step: d => step(d), markDetailKeys: () => markDetailKeys(),
  askEnabled: () => viewOpts.ask, act: (...args) => act(...args),
});
// 页面这一部分注册的处理函数。页面启动时调用一次。
export function wireDetail() { wireViewer(); }
