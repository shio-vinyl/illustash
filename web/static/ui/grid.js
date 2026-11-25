// 图片网格（桌面布局，#199）：分页加载图片、按行挂卡片和它的缩放、选择、选择栏的操作、键盘快捷键、视图选项和 NSFW
// 筛选。行坐标在 ui/grid_rows.js，卡片部件在 ui/grid_card.js（#708）。
import {edits, exclusive, undoLatest, undoneText} from '../core/edits.js';
import {esc, isMac, number, platformText, screenScale, shortcut} from '../core/format.js';
import {t} from '../core/i18n.js';
import {composing} from '../core/keys.js';
import {leaves} from '../core/filters.js';
import {undoRefusal} from '../core/human.js';
import {afterSyncLearning, learningNow, learningSettled, learningShown, metaSoon, refreshMeta} from '../core/meta.js';
import {prefs} from '../core/prefs.js';
import {viewOpts} from '../core/view_opts.js';
import {SHORTCUTS, currentSource, keyAction, keySelection, modes, selectionSize, shownUuid, viewerTarget} from '../core/modes.js';
import {keyScope, pageOf, viewOf} from '../core/pages.js';
import {browsing, cleanup, hashFor, reRoute, stepHistory} from '../core/route.js';
import {post, readError} from '../core/session.js';
import {PAGE, countAssets, gridFilters as filters, listIds, readPage} from '../core/assets.js';
import {state} from '../core/state.js';
import {offerWatch} from './catalog.js';
import {$, error, showArea, toast, written} from './dom.js';
import {drawEmpty} from './empty.js';
import {railKey, railNsfw} from './rail.js';
import {renderChrome, renderHeading} from './shell.js';
import {lift, onPress} from './drag.js';
import {answerQuestion, closeInspector, closeViewer, ensureVisible, indexOf, inspect, noteFavorite, openViewer, peek, pickAlternative, showViewer, singleAction, step, zoomView} from './detail.js';
import {jumpToCharacter, openPicker} from './palette.js';
import {inPopover, popoverOpen} from './overlay.js';
import {FAV, favLeaves, isFav, markLocal, wantsAdd, withCondition} from '../core/favorites.js';
import {paintHearts, writeFavorite} from './favorites.js';
import {key as frameKey, write as frameWrite} from './frame.js';
import {dupHue, gridCard, gridContext, skeletonCard} from './grid_card.js';
import {cardRect, fixedLayout, flowing, indexAt, layout, layoutKey, rowAt, rowCount, rowHeight, rowOf, rowReach, rowStart, rowTop, headResized, verticalStep} from './grid_rows.js';
import {gridWindow, pagesBetween, visibleEnd, visibleOffsets} from './grid_window.js';
import {enter} from './motion.js';
import {app} from './scope.js';

// ---------- 加载图片 ----------
// 缩放是一个连续的目标列宽；网格对齐到整列，并在两者之间做动画。
const SIZE_MIN = 72, SIZE_MAX = 480;
const sliderToSize = v => Math.round(SIZE_MIN * Math.pow(SIZE_MAX / SIZE_MIN, v / 100));
const sizeToSlider = s => Math.round(100 * Math.log(s / SIZE_MIN) / Math.log(SIZE_MAX / SIZE_MIN));
// 视图选项浮层里的缩放滑块（ui/grid_view.js）：滑块的位置（0–100）和目标格宽互换。
export const zoomSlider = () => sizeToSlider(state.size);
export function zoomFromSlider(v) { requestZoom(() => setSize(sliderToSize(v), undefined, undefined, true)); }
// 进行中的分页读取，以及读取视口附近各页的防抖。查询参数和读取在 core/assets.js。
let abortList, pageTimer;
// ---------- 回到原位置（#402） ----------
// 离开网格页时按地址记下位置：第一张可见的图的序号、在它那一行里滚过的比例、这一屏到哪一张，以及详情栏里的图和键盘光标。
// 回到同一地址（侧栏、后退、从角色页返回）时，reload 先读这几张所在的页，再一步定位到那里，不先读顶部再跳。
// 记的是序号而不是像素：窗口宽度、缩放或方格／原比例变了，回来时第一行仍是同一张。只存在这个页面里，重新载入就没了；
// NSFW 筛选不同算不同的列表。
const places = new Map(), PLACES_MAX = 30;
const placeKey = route => `${hashFor(route)}|${state.nsfw}`;
// 换页之前调用（ui/shell.js applyRoute），此时 state.route 和滚动位置还是要离开的那一页的。停在网格头部（角色页横幅）里时记像素位置 inHead。
export function rememberPlace() {
  if (pageOf(state.route).panel || $('grid').hidden || !state.totalReady || !state.total) return;
  const v = $('viewport'), row = rowAt(v.scrollTop), anchor = rowStart(row), key = placeKey(state.route), inHead = v.scrollTop < rowTop(0) ? v.scrollTop : null;
  places.delete(key);
  places.set(key, {anchor, within: Math.max(0, (v.scrollTop - rowTop(row)) / rowHeight(row)), inHead, end: visibleEnd(v), focused: state.focused, cursor: state.cursor});
  if (places.size > PLACES_MAX) places.delete(places.keys().next().value);
}
export const placeFor = route => places.get(placeKey(route)) || null;
// 数据到了、画网格之前：先开详情栏（它占掉的宽度决定列数），再按新的布局滚到记下的那一张。详情栏里那张图不一定
// 在这次读到的几页里（离开前可能已经滚开了），所以不按列表核对，照样打开；已经开着别的（同一页换参数）就不动。
function restorePlace(place) {
  if (place.focused && !state.focused) inspect(place.focused);
  if (place.cursor >= 0 && place.cursor < state.total) setCursor(place.cursor);
  if (!state.total) return;
  layout();
  const row = rowOf(Math.min(place.anchor, state.total - 1));
  $('grid').style.height = `${rowTop(rowCount())}px`;
  $('viewport').scrollTop = place.inHead ?? rowTop(row) + Math.min(place.within, 1) * rowHeight(row);   // 行顶含头部高度，头部里的不按行算
}
// 先取可见的几页，再一步换入数据：编辑之后不会闪出骨架。place 是 placeFor() 记下的位置：读那附近的几页并定位过去。
export async function reload(reset = false, place = null) {
  if (pageOf(state.route).panel) return;
  const epoch = state.readEpoch, route = state.route;
  const gen = ++state.generation; abortList?.abort(); abortList = new AbortController(); state.pending.clear();
  showArea('grid'); error('');
  if (reset) {
    windowRows.reset();
    $('viewport').scrollTop = 0; $('resultCount').textContent = t('正在读取…'); state.totalReady = false;
  }
  layout();
  try {
    const offsets = place ? pagesBetween(place.anchor, place.end) : reset ? [0] : visibleOffsets($('viewport'));
    const pages = await Promise.all(offsets.map((o, i) => readPage(o, {count: i === 0, signal: abortList.signal})));
    if (gen !== state.generation || route !== state.route) return;
    if (epoch !== state.readEpoch) return reload(reset, place);
    const same = reuseSame(state.items);
    state.total = pages[0].total; state.totalReady = true; state.countDirty = false; state.items = new Array(state.total); state.pages = new Set();
    pages.forEach((p, i) => { p.items.forEach((item, j) => { if (offsets[i] + j < state.total) state.items[offsets[i] + j] = same(item); }); state.pages.add(offsets[i]); });
    state.dataVersion++; state.gridKey = '';
    if (place) restorePlace(place);
    renderGrid(); updateCount(); renderSelection();
    $('empty').hidden = !!state.total;
    const view = viewOf(state.route), fact = view.empty?.(state.route, {nsfw: state.nsfw}) || '';
    if (!state.total) drawEmpty($('empty'), {icon: emptyIcon(view), fact, next: fact ? view.emptyNext?.(state.route) : []});
    renderHeading();
    if (state.route.char) refreshMaybe(gen);
  } catch (e) {
    if (e.name === 'AbortError' || gen !== state.generation || route !== state.route) return;
    const view = viewOf(route), text = view.readFailed?.(e);   // 这一页认得的失败当作没有结果（core/pages.js 的 readFailed）
    if (text) { showNone(text, view.failedNext?.(route)); return; }
    error(e.message); $('resultCount').textContent = t('读取失败');
  }
}
// 重读回来的图和已显示的那份内容相同，就沿用原来的对象（#452）：renderGrid 按对象判断卡片要不要重建，写入之后只有
// 变了的那几张重画。
function reuseSame(items) {
  const shown = new Map();
  for (const x of items) if (x) shown.set(x.uuid, x);
  return item => { const was = shown.get(item.uuid); return was && JSON.stringify(was) === JSON.stringify(item) ? was : item; };
}
// 空状态的图标：「只看 NSFW」时是搜索（多半是这个筛选造成的），否则是这一页登记的。
const emptyIcon = view => state.nsfw === 'only' ? 'search' : (typeof view.emptyIcon === 'function' ? view.emptyIcon(state.route) : view.emptyIcon) || 'image';
function showNone(text, next = []) {
  state.total = 0; state.totalReady = true; state.countDirty = false; state.items = []; state.pages = new Set(); state.dataVersion++; state.gridKey = '';
  renderGrid(); updateCount(); renderSelection();
  $('empty').hidden = false;
  drawEmpty($('empty'), {icon: 'search', fact: text, next});
  renderHeading();
}
// 按天视图借用网格的列表放它自己的图（详情栏翻页、大图都按这个列表走）。
// 面板页看一组图的大图（previewList：按组整理、按天视图「认一认」、身份整理、画师抽查，#424 的 2-2、2-34）时列表先换成那一组，
// 关大图时还回去（endPreview，ui/detail.js 的 closeViewer 调用）；这期间页面自己再 showList（按天视图写入后重画），换的是要还回去的那份。
let lent = null;
export function showList(items) { if (lent) { lent = items; return; } state.items = items; state.total = items.length; }
export function previewList(ids, i) {
  if (!lent) lent = state.items;
  state.items = ids.map(uuid => ({uuid, memberships: []})); state.total = ids.length; openViewer(i);
}
export function endPreview() { if (!lent) return false; const items = lent; lent = null; showList(items); return true; }
export const previewing = () => !!lent;
// 后端接受了一次写入、或同步进了新图：已读到的计数和分页作废，下一次读取从头来（ui/effects.js 调用）。
export function listChanged() { state.countDirty = true; state.readEpoch++; }
export async function loadPage(offset) {
  if (state.pages.has(offset) || state.pending.has(offset) || offset >= state.total) return;
  if (state.countDirty) {
    if (!state.countReload) state.countReload = reload().finally(() => { state.countReload = null; });
    await state.countReload; return;
  }
  const gen = state.generation, epoch = state.readEpoch, route = state.route; state.pending.add(offset);
  try {
    const data = await readPage(offset, {count: false, signal: abortList?.signal}); if (gen !== state.generation || epoch !== state.readEpoch || route !== state.route) return;
    data.items.forEach((item, j) => { if (offset + j < state.total) state.items[offset + j] = item; });
    state.pages.add(offset); state.dataVersion++; renderGrid();
  } catch (e) { if (e.name !== 'AbortError' && gen === state.generation && route === state.route) error(e.message); }
  finally { if (gen === state.generation) state.pending.delete(offset); }
}
function updateCount() {
  const loaded = state.items.filter(Boolean).length;
  $('resultCount').textContent = t('{n} 张', {n: number(state.total)}) + (loaded < state.total ? ' · ' + t('已加载 {n}', {n: number(loaded)}) : '');
  $('hint').textContent = viewOf(state.route).hint?.() || '';
}

// ---------- 网格 ----------
// 方形裁切，或像「照片」那样的两端对齐行；分行和每张卡片的框在 ui/grid_rows.js。
// 方格／原比例：设置页的选项和 R 键都经这里改，并记进偏好。
export function setFit(on) { if (state.fit === on) return; state.fit = on; prefs.set('fit', on); }
export function toggleFit() {
  if (fixedLayout()) return;   // 页面定了排法（画廊的瀑布、色彩）：开关不起作用
  const v = $('viewport'), anchor = rowStart(rowAt(v.scrollTop));
  setFit(!state.fit); layout();
  v.scrollTop = rowTop(rowOf(anchor)); renderGrid(true);
  for (const b of $('fitToggle')?.querySelectorAll('[data-fit]') || []) b.classList.toggle('on', Boolean(Number(b.dataset.fit)) === state.fit);
}
// 机器认为可能也是这个角色的图有多少张（在角色页上显示成一个开关）。
async function refreshMaybe(gen) {
  const r = state.route, total = status => countAssets({entity: r.char, status, ...(state.nsfw ? {nsfw: state.nsfw} : {})}).catch(e => { error(readError(e)); return null; });
  const [count, doubt] = await Promise.all([r.maybe ? state.total : total('maybe'), r.doubt ? state.total : total('ambiguous')]);
  if (count === null || doubt === null || gen !== state.generation || (count === state.maybeCount && doubt === state.doubtCount)) return;
  state.maybeCount = count; state.doubtCount = doubt; renderChrome();
}
// 重复页的角标颜色（算法在 ui/grid_card.js 的 dupHue，按读到的列表和数据版本缓存）。
const hueAt = i => dupHue(state.items, state.dataVersion, i);
// 网格顶部跟着滚动的头部（角色页的横幅，#712）：#grid 的第一个子节点，开始是藏着的。往里放东西、显示出来就行，
// 它的高度变了网格自己重新分行（ui/grid_rows.js 的 setHead），卡片都往下挪。
export const gridHead = () => $('gridHead');
// 挂着的卡片（ui/grid_card.js 的部件）：键是图的 uuid，还没读到的是「#序号」的骨架。按图找（#452）：写入后图合拢、
// 重新排序时，内容没变的卡片只换位置，不重建。值是 {comp, item, i, box, ctx, hue, preview}，记着上一次画的是什么，没变就不调 update。
let mounted = new Map(), cards = null, ctx = null, ctxKey = '';
// 滚出范围的卡片部件留着复用（park），不一直新建、丢弃：Chromium 里丢掉的卡片和它的 <img> 迟迟不释放（#708）。
const spare = [], SPARE_MAX = 160;
const windowRows = gridWindow(() => scheduleGrid());
export function renderGrid(relayout = false) {
  if ($('grid').hidden) return;
  const v = $('viewport'), grid = $('grid');
  windowRows.track(v.scrollTop);
  // 钉住第一张可见的图：它上方的行重新排布时（分页到达、新的估计值），视图保持不动。
  const anchorRow = rowAt(v.scrollTop), anchor = rowStart(anchorRow), within = v.scrollTop - rowTop(anchorRow);
  const oldLayout = layoutKey();
  const before = [state.cols, state.rowH]; layout();
  if (flowing() && oldLayout !== layoutKey() && anchor < state.total) {
    const row = rowOf(anchor), top = rowTop(row) + Math.min(within, rowHeight(row) - 1);
    if (Math.abs(top - v.scrollTop) > .5) v.scrollTop = top;
  }
  if (relayout || before[0] !== state.cols || before[1] !== state.rowH) state.gridKey = '';
  const rows = rowCount(); grid.style.height = `${rowTop(rows)}px`;
  const {first, last, ahead, behind, key: previewKey} = windowRows.measure(v, rows);
  // 屏幕倍率也算进键：窗口拖到倍率不同的屏幕后，下一次重画（滚动、改窗口大小）就按新倍率换缩略图档位（#452）。
  const scale = screenScale(), key = `${first}:${last}:${state.dataVersion}:${layoutKey()}:${scale}:${previewKey}`;
  if (key !== state.gridKey) { state.gridKey = key; placeCards(rowStart(first), rowStart(last), scale); }
  syncCards();
  clearTimeout(pageTimer); const gen = state.generation;
  pageTimer = setTimeout(() => {
    if (gen !== state.generation) return;
    for (let o = Math.floor(rowStart(first) / PAGE) * PAGE; o < rowStart(last); o += PAGE) loadPage(o);
    for (let o = Math.floor(behind / PAGE) * PAGE; o < ahead; o += PAGE) loadPage(o);
  }, 60);
}
// 挂上第 from 到 to - 1 张，摘掉范围外的（摘掉时放掉解码后的位图，#683）。
function placeCards(from, to, scale) {
  const grid = $('grid'), view = viewOf(state.route), dup = !!view.card?.dup;
  // 这一轮共用的上下文（页面、视图选项、格宽）：内容没变就沿用上一份，卡片据此判断要不要重画。
  const next = gridContext({route: state.route, view, fit: flowing(), cellW: state.cellW, hints: viewOpts.hints, hue: hueAt});
  const k = JSON.stringify({...next, hue: 0, scale});
  if (k !== ctxKey) { ctxKey = k; ctx = next; }
  cards ||= app.child('grid-cards');
  const keep = new Map(), order = [];
  for (let i = from; i < to; i++) {
    const item = state.items[i];
    let key = item ? item.uuid : '#' + i;
    if (keep.has(key)) key = '#' + i;   // 读的过程中列表变了，同一张图出现两次：后一个先当骨架
    const m = mounted.get(key) || {comp: item ? spare.pop() || gridCard(cards.child('card')) : skeletonCard()};
    mounted.delete(key);
    // 重复页的角标颜色看前一组：前面一页后读到时颜色可能变，变了的卡片要重画。
    const box = cardRect(i), hue = dup && item?.dup ? hueAt(i) : null;
    const preview = windowRows.preview(box);
    if (m.item !== item || m.i !== i || m.ctx !== ctx || m.hue !== hue || m.preview !== preview || !sameBox(m.box, box)) {
      m.comp.update({item, index: i, box, ctx, preview});
      Object.assign(m, {item, i, box, ctx, hue, preview});
    }
    keep.set(key, m); order.push(m.comp.el);
  }
  for (const m of mounted.values()) { m.comp.el.remove(); if (m.item && spare.length < SPARE_MAX) spare.push((m.comp.park(), m.comp)); else m.comp.dispose(); }
  mounted = keep;
  // 卡片按序号排（Tab 和读屏按 DOM 的先后走），只挪不在位置上的那几张。头部留在最前。
  let at = $('gridHead').nextElementSibling;
  for (const el of order) { if (el === at) at = at.nextElementSibling; else grid.insertBefore(el, at); }
}
const sameBox = (a, b) => !!a && a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h;
// 选中、详情栏里那张、键盘光标：只改这三样（选择、开关详情栏、方向键时调）。
export function syncCards() {
  $('grid').classList.toggle('selecting', state.selecting || selCount() > 0);
  for (const m of mounted.values()) if (m.item) m.comp.mark({selected: isSelected(m.item.uuid), focused: m.item.uuid === state.focused, cursor: m.i === state.cursor});
}
const RENDER = frameKey('grid:render'), AUTOSCROLL = frameKey('grid:autoscroll');   // 帧任务的令牌：网格只有一份，在模块顶层取（#751）
export function scheduleGrid() { frameWrite(() => renderGrid(), RENDER); }   // 滚动、尺寸变化：排到下一帧画一次（ui/frame.js，同一帧只画一次）
// ---------- 缩放 ----------
// 让指针下的图（或第一个可见的行）保持在原处，然后让卡片从旧的框滑到新的框（FLIP，ui/motion.js），这样每次换列数读起来都是
// 一个动作。起止的框都是算出来的，不量 DOM。
const ZOOM = frameKey('grid:zoom'), requestZoom = task => frameWrite(task, ZOOM);
function setSize(size, clientX, clientY, fromSlider = false) {
  size = Math.max(SIZE_MIN, Math.min(SIZE_MAX, size));
  const v = $('viewport'), vr = v.getBoundingClientRect(), gr = $('grid').getBoundingClientRect();
  const x = clientX ?? gr.left + 2, y = clientY ?? vr.top + 2;
  const anchor = state.total ? indexAt(x, y) : 0;
  const anchorRow = rowOf(anchor), within = (y - gr.top - rowTop(anchorRow)) / rowHeight(anchorRow), offset = y - vr.top, oldScroll = v.scrollTop;
  const onScreen = (box, scroll) => ({x: box.x, y: box.y - scroll, w: box.w, h: box.h});
  const before = new Map();
  for (const m of mounted.values()) if (m.item) before.set(m.item.uuid, onScreen(m.box, oldScroll));
  const old = [state.cols, state.rowH];
  state.size = size; prefs.set('size', Math.round(size));
  if (!fromSlider && $('zoom')) $('zoom').value = sizeToSlider(size);
  layout();
  if (old[0] === state.cols && old[1] === state.rowH) return;
  v.scrollTop = oldScroll === 0 && clientY === undefined ? 0 : Math.max(0, rowTop(rowOf(anchor)) + within * rowHeight(rowOf(anchor)) - offset);
  renderGrid(true);
  const scroll = v.scrollTop;
  for (const m of mounted.values()) {
    if (!m.item) continue;
    const from = before.get(m.item.uuid);
    if (from) m.comp.flip(from, onScreen(m.box, scroll)); else enter(m.comp.el, {from: 'fade'});
  }
}
// 键盘调整列数：一次一级。
function stepColumns(delta) {
  const width = $('viewport').clientWidth - 2, cols = Math.max(1, state.cols + delta);
  setSize((width + state.gap) / cols - state.gap);
}
let pinchFactor = 1, pinchPoint = null, gestureStart = 0;
// 主区是不是网格：缩放网格（⌘＋／⌘－、⌘滚轮、捏合）只在这样的页面生效。
const onGrid = () => viewOf(state.route).area === 'grid';
// ---------- 选择 ----------
export const selCount = () => selectionSize();
export const isSelected = id => state.sel.all ? !state.sel.exclude.has(id) : state.sel.ids.has(id);
function setSelected(id, on) {
  if (state.sel.all) on ? state.sel.exclude.delete(id) : state.sel.exclude.add(id);
  else on ? state.sel.ids.add(id) : state.sel.ids.delete(id);
}
function clearSelection() { state.sel = {all: false, ids: new Set(), exclude: new Set()}; state.anchor = -1; state.armed = null; renderSelection(); }
export function selectAll() { if (!state.total) return; state.sel = {all: true, ids: new Set(), exclude: new Set()}; state.selecting = true; renderSelection(); }
export function toggleSelecting() { state.selecting = !state.selecting; if (!state.selecting) clearSelection(); renderSelection(); }
// 取消选择并退出选择模式（Esc、选择栏的 ×、写入之后、切换 NSFW）。
export function quitSelecting() { state.selecting = false; clearSelection(); }
// 换页时复位网格这一侧的状态：退出选择模式、清空选择、光标归位、张数等新读取（ui/shell.js applyRoute 调用）。
export function resetForRoute() { windowRows.reset(); quitSelecting(); state.cursor = -1; state.totalReady = false; }
// 键盘光标移到第 i 张（详情栏和大图翻页时跟着走）。
export function setCursor(i) { state.cursor = i; }
// I 和大图底栏的「信息」：开关详情栏（#424 的 2-7）。大图里是关掉大图、在详情栏里看这张；网格里看光标那张（没有光标取
// 第一张），详情栏已经开着同一张就关掉，开着别的（整理页点图只选中、不换详情栏）就换成光标那张。按天视图没有光标，只能关。
// 不显示详情栏的面板页，以及面板页借列表看的大图（previewList）不响应。
export function toggleInfo() {
  const scope = keyScope(viewOf(state.route)); if (!scope || previewing()) return;
  if (state.viewer >= 0) { const uuid = state.viewerItem?.uuid; closeViewer(); if (uuid && state.focused !== uuid) inspect(uuid); return; }
  const i = scope === 'grid' ? Math.max(0, state.cursor) : -1, item = state.items[i];
  if (state.focused && (!item || item.uuid === state.focused)) { closeInspector(); return; }
  if (!item) return;
  setCursor(i); ensureVisible(i); inspect(item.uuid);
}
// 写入接口的选区参数：全选时是筛选条件减去排除的，否则是选中的 ID。
export function selectionPayload() { return state.sel.all ? {filter: filters(), exclude: [...state.sel.exclude]} : {ids: [...state.sel.ids]}; }
// 命令面板顶上画哪几张（ui/palette.js 的 subject）：全选时没有展开的 ID，只写张数。
export const selectionIds = () => state.sel.all ? [] : [...state.sel.ids];
async function selectRange(a, b, on = true) {
  const lo = Math.min(a, b), hi = Math.max(a, b);
  if (hi - lo + 1 > 5000) { toast(t('一次最多连选 5,000 张；需要更多请用 {key}', {key: shortcut('A')})); return; }
  let ids = state.items.slice(lo, hi + 1);
  if (ids.length !== hi - lo + 1 || ids.some(x => !x)) {
    ids = await listIds({...filters(), offset: lo}, hi - lo + 1);
  } else ids = ids.map(x => x.uuid);
  for (const id of ids) setSelected(id, on);
  renderSelection();
}
function handleCardClick(index, e, onCheck = false) {
  const item = state.items[index]; if (!item) return;
  state.cursor = index;
  if (e.shiftKey && state.anchor >= 0) { selectRange(state.anchor, index, true); return; }
  // 角上的圆圈总是选择，和「照片」一样；在图库里普通点击打开详情。
  if (browsing() && !onCheck && !state.selecting && !selCount() && !e.metaKey && !e.ctrlKey) { inspect(item.uuid); syncCards(); return; }
  if (browsing() && (onCheck || e.metaKey || e.ctrlKey)) state.selecting = true;
  setSelected(item.uuid, !isSelected(item.uuid)); state.anchor = index; renderSelection();
}

// 指针：点击、在卡片上拖动选择（像「照片」那样）、从空白处拉框选择、到边缘时自动滚动。
let drag = null;
function applyDrag() {
  if (!drag?.moved) return;
  state.sel = {all: drag.base.all, ids: new Set(drag.base.ids), exclude: new Set(drag.base.exclude)};
  let indices = [];
  if (drag.start !== null) {
    const now = indexAt(drag.lastX, drag.lastY), lo = Math.min(drag.start, now), hi = Math.max(drag.start, now);
    for (let i = lo; i <= hi; i++) indices.push(i);
  } else {
    const gr = $('grid').getBoundingClientRect(), x2 = drag.lastX - gr.left, y2 = drag.lastY - gr.top;
    const [l, r, t, b] = [Math.min(drag.gx, x2), Math.max(drag.gx, x2), Math.min(drag.gy, y2), Math.max(drag.gy, y2)];
    Object.assign($('marquee').style, {left: `${l}px`, top: `${t}px`, width: `${r - l}px`, height: `${b - t}px`}); $('marquee').hidden = false;
    const r0 = rowAt(t - rowReach()), r1 = rowAt(b);
    for (let row = r0; row <= r1; row++) for (let i = rowStart(row); i < rowStart(row + 1); i++) {
      const box = cardRect(i);
      if (box.x <= r && box.x + box.w >= l && box.y <= b && box.y + box.h >= t) indices.push(i);
    }
  }
  for (const i of indices) if (state.items[i]) setSelected(state.items[i].uuid, drag.mode);
  renderSelection();
}
function autoScroll() {
  if (!drag?.moved) return;
  const vr = $('viewport').getBoundingClientRect(), edge = 48;
  const speed = drag.lastY < vr.top + edge ? -(vr.top + edge - drag.lastY) : drag.lastY > vr.bottom - edge ? drag.lastY - (vr.bottom - edge) : 0;
  if (speed) { $('viewport').scrollTop += speed * .6; applyDrag(); }
  frameWrite(autoScroll, AUTOSCROLL);
}
// ---------- 选择栏与操作 ----------
// 选择栏（按钮、分派）在 ui/grid_bar.js：它导入这里的写入函数，载入时用 setBar() 登记进来，这里不反过来导入它（不产生循环导入）。
const bar = {render() {}, run() {}, answerAudit() {}};
/** ui/grid_bar.js 登记选择栏：render() 画栏，run(act) 按 data-act 分派，answerAudit(yes) 回答随机抽查；没登记时不画。 */
export function setBar(impl) { Object.assign(bar, impl); }
// 选择栏上等「再点一次」确认的按钮（ui/grid_bar.js 的 needsArm）：记下是哪一个，null 是没有。
export function setArmed(key) { state.armed = key; }
// 选择变了：卡片上的选中样子、「选择」按钮、选择栏一起更新。样式清单页（tools/styleguide/live.js，#352）也调它。
export function renderSelection() { syncCards(); markSelectButton(); bar.render(); }
// 浏览页的「选择」按钮：进了选择模式就显示按下的样子，再点一次退出（#386 的 2-27）。整理页上它是「全选」，没有这个状态。
export function markSelectButton() {
  const b = $('selectButton'); if (!b || cleanup()) return;
  const on = state.selecting || selCount() > 0;
  b.classList.toggle('on', on); b.setAttribute('aria-pressed', on); b.title = on ? t('退出选择模式（Esc）') : t('进入选择模式');
}
// 详情栏和选择栏同时在时，单图快捷键作用于选中的图（core/modes.js 的 keySelection）：详情栏按钮上的键标藏起来，
// 免得「排除 X」看起来是排除详情栏里那张（#422 的 2-38，docs/WEB_UI_MAP.md 第 6 节）。问题条的 Y／N 照旧回答详情栏
// 那张，只有随机抽查页（选择栏上有 Y／N）也藏。详情栏重画后（ui/detail.js 的 inspect）再调一次。
export function markDetailKeys() {
  const off = keySelection() > 0, audit = viewOf(state.route).bar === 'audit';
  for (const k of $('inspector').querySelectorAll('kbd')) k.hidden = off && (audit || !k.closest('.ask-btns'));
}
// 人工标签复用命令面板：已有的标签、采用一个 WD tag，或新建一个。所有选中图片上都已有的条目提供移除。
// leaving(tag, add)：加上或去掉这个标签以后图是不是离开本页。默认只有去掉的正是「含」的标签、加上的正是「不含」的标签时
// 才算（#386 的 2-39、#644）。
export function openTagPicker(payload, n, loaded, leaving = (tag, add) => leaves(state.route, `utag:${tag}`, add)) {
  const present = new Map();
  for (const item of loaded || []) for (const tag of item.user_tags || []) present.set(tag, (present.get(tag) || 0) + 1);
  openPicker({title: t('标签'), tagMode: true, present, subject: (loaded || []).map(x => x.uuid), placeholder: t('输入标签，或搜索 WD 通用标签…'),
    sentence: e => e.remove ? t('从 {n} 张去掉标签 {tag}', {n: number(n), tag: e.tag}) : t('给 {n} 张加上标签 {tag}', {n: number(n), tag: e.tag}),
    pick: e => mutate('/api/tag', {...payload(), tag: e.tag, add: !e.remove}, leaving(e.tag, !e.remove))});
}

// 写入：立即淡出要离开的卡片，服务端一同意就合上空隙，然后换入新数据，不丢滚动位置。
// 编辑查看器或详情面板里打开的那张图，绝不会把你带到另一张：列表可能重新排序（可疑优先会把确认过的图排到最后）或
// 去掉它，但它留在屏幕上，好让接下来能继续编辑，之后 ←/→ 从第一次编辑之前它的邻居接着走。只有随机抽查的回答会前进。
// 列表在下面变化时保持在视野里的那张图：{uuid, item, prev, next}。
export const kept = {hold: null};
function holdCurrent() {
  const uuid = shownUuid();
  if (!uuid) { kept.hold = null; return; }
  if (kept.hold?.uuid === uuid) return;
  const i = indexOf(uuid);
  kept.hold = {uuid, item: state.items[i] || state.viewerItem, prev: state.items[i - 1]?.uuid, next: state.items[i + 1]?.uuid};
}
// 当前图：单图快捷键和单图动作作用在它上面。来源的优先级（大图 ＞ 详情栏 ＞ 光标）在 core/modes.js 的 currentSource。
// 那张图已经离开列表时，取保持在视野里的那份（kept.hold）或大图自己记着的那份。
export function currentItem() {
  const src = currentSource(); if (!src) return null;
  if (src.from === 'cursor') return state.items[src.index];
  const uuid = src.uuid, i = indexOf(uuid);
  return i >= 0 ? state.items[i] : uuid === kept.hold?.uuid ? kept.hold.item : state.viewerItem?.uuid === uuid ? state.viewerItem : null;
}
export function mutate(path, body, leaving, {advance = false} = {}) {
  return exclusive(async () => {
    document.body.classList.add('busy');
    try { return await write(path, body, leaving, advance); } finally { document.body.classList.remove('busy'); }
  });
}
async function write(path, body, leaving, advance) {
  const started = performance.now();
  const advanceFrom = advance && state.viewer >= 0 ? state.viewerItem?.uuid : null;
  if (advance) kept.hold = null; else holdCurrent();
  const gone = leaving ? new Set(body.ids || state.items.filter(x => x && isSelected(x.uuid)).map(x => x.uuid)) : null;
  if (gone) for (const m of mounted.values()) if (m.item && gone.has(m.item.uuid)) m.comp.leave(true);
  const confirmed = path === '/api/change' && body.action === 'confirmed';
  try {
    const r = await post(path, body);
    quitSelecting();
    written(r.undo);
    // 确认角色之后后台会学习，学完未识别计数和分组还会变（#400、#404）。先读一次学习状态再重读（#452）：这时已经落定
    // （没有学习 worker，或者这次不用学），接下来这次重读就是最终值，不用再补读；还在学习的，落定后再读一次。
    const learning = confirmed ? learningNow() : null;
    if (confirmed) offerWatch(body.entities || [body.entity]).catch(() => {});
    if (gone) { await new Promise(done => setTimeout(done, Math.max(0, 170 - (performance.now() - started)))); collapse(gone); }
    const first = await learning;
    await reload(); pageOf(state.route).refresh?.('edit'); metaSoon(path === '/api/tag');
    if (state.focused) inspect(state.focused, true);
    if (state.viewer >= 0) {
      if (advance && advanceFrom && indexOf(advanceFrom) >= 0) showViewer(Math.min(indexOf(advanceFrom) + 1, state.total - 1));
      else if (advance || !kept.hold) showViewer(Math.min(state.viewer, state.total - 1));
      else { const i = indexOf(kept.hold.uuid); showViewer(i >= 0 ? i : state.viewer, i >= 0 ? state.items[i] : state.viewerItem); }
    }
    if (first && first.state !== 'done') afterLearning(first);
    return true;
  } catch (e) { toast(e.message); for (const m of mounted.values()) m.comp.leave(false); }
}
function collapse(gone) {
  const left = []; let removed = 0;
  for (let i = 0; i < state.total; i++) { const x = state.items[i]; if (x && gone.has(x.uuid)) { removed++; continue; } left[i - removed] = x; }
  left.length = state.total - removed; state.items = left; state.total = left.length; state.dataVersion++;
  // 光标停在离开的最后一张上时，收回到新的最后一张，接着按键还有对象（#386 的 2-6）。
  if (state.cursor >= state.total) state.cursor = state.total - 1;
  renderGrid(true); updateCount();
}
// 撤销之后重读列表、当前面板页、详情栏和大图；why 是交给面板页的原因（'undo'，或后台学习收尾后的 'learned'）。
// 计数先读完再重画面板页：按组整理页的「没成组的 N 张」是拿未识别总数减去分组里的张数，计数晚到就会算错。
async function reread(why) {
  // 角色的「只当标签」状态也决定图片列表可用的排序。
  await refreshMeta().catch(e => error(readError(e))); await reload();
  await pageOf(state.route).refresh?.(why);
  if (state.focused) inspect(state.focused, true);
  if (state.viewer >= 0) { const i = indexOf(state.viewerItem?.uuid); showViewer(i >= 0 ? i : state.viewer, i >= 0 ? state.items[i] : state.viewerItem); }
}
// 确认角色之后等后台学习落定（#404，等法见 core/meta.js 的 learningSettled）。学到的结果发布了，或者等满时限，就再读一次；
// 学习只会让计数接着往同一个方向变，不会显示错误的结论，所以等待期间不加标记。那时正有别的写入在进行，就交给它自己的重读。
async function afterLearning(first) {
  const outcome = await learningSettled(first);
  if ((outcome === 'done' || outcome === 'timeout') && !edits.busy) await reread('learned');
}
// 同步进行中撤销提示的文字（#404 的 Q2）。
const syncUndone = () => t('已撤销，计数在同步结束后更新');
// 撤销之后（#404 的 Q1、Q2）：先读一次学习状态。已经落定（没有学习 worker，或者学得快）就立即重读，这次就是最终值；
// 还在学习就先不重读，计数旁显示「正在更新」，落定后一次重读成最终结果，不显示中间值；同步进行中则立即重读一次，
// 提示计数在同步结束后更新，同步结束后再重读一次（afterSync）；学习失败、停下或读不到状态时立即重读当前值。
export async function undo() {
  if (!edits.latest) return;
  const waiting = await exclusive(async () => {
    let step;
    try { step = await undoLatest(); } catch (e) {
      // 历史可能已经往前走了（另一个窗口，或服务端提交之后才失败）：显示它最新的一步。
      // 撤销一次人工结论导入被拒（后面有改动依赖它）时，说清楚为什么（core/human.js）。
      toast(undoRefusal(e, edits.latest)); refreshMeta().catch(err => error(readError(err)));
      return null;
    }
    const first = await learningNow();
    if (first?.state === 'pending') {
      const settled = learningSettled(first, {marking: true});
      toast(undoneText(step)); renderHeading();
      return settled;
    }
    // 同步进行中：同步结束后接着等的那一段也显示「正在更新」。
    learningSettled(first, {marking: true});
    toast(first?.state === 'sync' ? syncUndone() : undoneText(step)); await reread('undo');
    return null;
  });
  if (!waiting) return;
  const outcome = await waiting;
  if (outcome === 'superseded') return;
  if (outcome === 'sync') toast(syncUndone());
  // 落定时正有别的写入在进行：它自己的重读会读到落定后的值。
  await exclusive(() => reread('undo'));
  settled();
}
// 等待结束、重读完：去掉「正在更新」。
function settled() { learningShown(); renderHeading(); }
// 同步结束（ui/effects.js 的 afterJob）：之前因为同步没等完的学习，用同一个编号接着等，落定后重读一次。
// 同步结束时没有新图，也要靠这里重读。
export async function afterSync() {
  const waiting = afterSyncLearning();
  if (!waiting) return;
  renderHeading();
  const outcome = await waiting;
  if (outcome === 'superseded') return;
  if (outcome !== 'sync') await exclusive(() => reread('learned'));
  settled();
}
export async function sweep() {
  const c = state.char; if (!c?.unchecked) return;
  await mutate('/api/sweep', {entity: c.id}, false);
}

// ---------- 收藏（#447） ----------
// 心形和不重读的写入在 ui/favorites.js，判断在 core/favorites.js。这一页按收藏筛选、图会离开本页时（「只看收藏」里取消）
// 照其他写入走 mutate：淡出、重读；否则只在本地改掉这些图和同组副本的心形，面板页（按天视图）自己重读。
export function favorite(payload, add) {
  if (favLeaves(add)) return mutate('/api/favorite', {...payload, add}, true);
  const ids = payload.ids && new Set(payload.ids), exclude = state.sel.exclude;
  const pick = ids ? x => ids.has(x.uuid) : x => !exclude.has(x.uuid);
  return writeFavorite({...payload, add}, () => {
    quitSelecting();
    const changed = markLocal(state.items, pick, add);
    if (kept.hold?.item && (changed.has(kept.hold.uuid) || ids?.has(kept.hold.uuid))) { kept.hold.item.fav = add; changed.add(kept.hold.uuid); }
    noteFavorite(changed, ids, add); paintHearts(changed, add);
    if (pageOf(state.route).panel) pageOf(state.route).refresh?.('edit');
  });
}
// 一张图：切换它的收藏（卡片上的心形、详情栏和大图的按钮、没有选中时的 F）。
export function favoriteOne(uuid) {
  const item = state.items[indexOf(uuid)] || (state.viewerItem?.uuid === uuid ? state.viewerItem : kept.hold?.uuid === uuid ? kept.hold.item : {uuid});
  return favorite({ids: [uuid]}, !isFav(item));
}
// 选中的图：只要有一张还没收藏就全部收藏，否则全部取消（选择栏的按钮和 F）。全选时问一次后端选区里还有没有没收藏的。
export async function favoriteSelection(payload) {
  let add;
  if (state.sel.all) {
    try { add = await countAssets(withCondition(filters(), 'not', FAV)) > 0; } catch (e) { toast(readError(e)); return; }
  } else add = wantsAdd(state.items.filter(x => x && isSelected(x.uuid)), state.sel.ids.size);
  return favorite(payload, add);
}

// ---------- NSFW 筛选 ----------
// 一次点击在 显示 → 隐藏 → 仅 NSFW 之间循环；H 切换隐藏。NSFW = WD 评级 questionable/explicit。
// NSFW 圆形按钮：中性色 = 全部显示，强调色描边 = 隐藏，实心警戒色 = 只看 NSFW。
const NSFW_TIP = {'': t('NSFW：显示中 · 点击隐藏（H）'), hide: t('NSFW：已隐藏 · 点击只看 NSFW'), only: t('NSFW：只看 NSFW · 点击恢复全部')};
function renderNsfw() {
  const b = $('nsfwButton');
  b.className = 'icon nsfw-toggle s-' + (state.nsfw || 'all'); b.title = NSFW_TIP[state.nsfw]; b.setAttribute('aria-label', NSFW_TIP[state.nsfw]);
}
export function setNsfw(mode) {
  state.nsfw = mode; prefs.set('nsfw', mode); reRoute({}); renderNsfw(); quitSelecting();   // 新路由对象：切换前发出的读取晚回来不画
  toast({'': t('已显示全部内容'), hide: t('已隐藏 NSFW'), only: t('只看 NSFW')}[mode]);
  if (!pageOf(state.route).panel) reload(true);
  else pageOf(state.route).refresh?.('nsfw');
  refreshMeta().catch(e => error(readError(e)));
  railNsfw();
}

// 帮助对话框的快捷键表从 core/modes.js 的 SHORTCUTS 生成，和键盘处理是同一张表。wireGrid 调用一次；
// 也导出给样式清单页调用（tools/styleguide/live.js，#352）。
export function renderHelpKeys() {
  const text = x => typeof x === 'function' ? x() : x;
  $('helpKeys').innerHTML = SHORTCUTS.map(s => `<dt>${esc(platformText(text(s.keys)))}</dt><dd>${esc(platformText(s.does()))}</dd>`).join('');
}

// 焦点在这些控件上时 Enter、Space 交给控件自己（链接另外只认 Enter）。
const ACTIVATES = 'button,summary,[role=button],[role=switch],[role=menuitem],[role=tab]';

// 页面这一部分注册的处理函数。页面启动时调用一次。
export function wireGrid() {
  state.size = Math.min(SIZE_MAX, Math.max(SIZE_MIN, prefs.get('size', 185)));
  // 网格顶部的头部（gridHead）：高度变了就重新分行，卡片跟着往下挪。
  const head = document.createElement('div');
  head.id = 'gridHead'; head.className = 'grid-head'; head.hidden = true;
  $('grid').insertBefore(head, $('grid').firstElementChild);
  // 头部的高度从回调参数里读（borderBoxSize），不再量一次 offsetHeight；滚动位置怎么跟着挪见 ui/grid_rows.js 的 headResized。
  new ResizeObserver(([entry]) => { headResized(head.hidden ? 0 : entry?.borderBoxSize?.[0]?.blockSize ?? head.offsetHeight); scheduleGrid(); }).observe(head);
  $('viewport').addEventListener('scroll', scheduleGrid, {passive: true});
  new ResizeObserver(scheduleGrid).observe($('viewport'));

  // ⌘滚轮、捏合：只在主区是网格的页面缩放网格；别的页面什么也不做，也不交给浏览器缩放整页（与 ⌘＋／⌘－ 一致）。
  $('viewport').addEventListener('wheel', e => {
    if (!e.ctrlKey && !e.metaKey) return;
    e.preventDefault(); if (!onGrid()) return;
    pinchFactor *= Math.exp(-e.deltaY * .01); pinchPoint = [e.clientX, e.clientY];
    requestZoom(() => { const f = pinchFactor; pinchFactor = 1; setSize(state.size * f, ...pinchPoint); });
  }, {passive: false});
  $('viewport').addEventListener('gesturestart', e => { e.preventDefault(); gestureStart = state.size; });
  $('viewport').addEventListener('gesturechange', e => {
    e.preventDefault(); if (!onGrid()) return;
    const x = e.clientX, y = e.clientY, s = gestureStart * e.scale; requestZoom(() => setSize(s, x, y));
  });

  // 在卡片的「+ 角色」引导上点一次：把图设为那个角色，和在选择器里选它一样。
  $('grid').addEventListener('click', e => {
    const fav = e.target.closest('button[data-fav]'); if (fav) { if (!edits.busy) favoriteOne(fav.dataset.fav); return; }
    const hit = e.target.closest('[data-tagact]');
    if (hit) {
      const item = state.items[Number(hit.closest('.card').dataset.index)], yes = hit.dataset.tagact === 'yes', entity = hit.dataset.entity;
      if (!item) return;
      if (item.audit && entity === item.audit.id) mutate('/api/audit/answer', {ids: [item.uuid], ...(yes ? {yes: [item.uuid]} : {no: [item.uuid]})}, false);
      else mutate('/api/change', {ids: [item.uuid], action: yes ? 'confirmed' : 'rejected', entity}, false);
      return;
    }
    const b = e.target.closest('[data-hint]'); if (!b) return;
    const item = state.items[Number(b.closest('.card').dataset.index)];
    if (item?.hint) mutate('/api/change', {ids: [item.uuid], action: 'confirmed', entity: item.hint.id}, pageOf(state.route).lead);
  });
  onPress($('grid'), e => {   // 拖到别的应用以后的第一次按下只有 mousedown（ui/press.js）；网格头部（横幅）上按下不算，文字照常能选
    if (e.button !== 0 || e.pointerType === 'touch' || e.target.closest('[data-hint],[data-tagact],button[data-fav],#gridHead')) return;
    const card = e.target.closest('.card[data-id]');
    drag = {x: e.clientX, y: e.clientY, start: card ? Number(card.dataset.index) : null, check: !!e.target.closest('.check'), moved: false, id: e.pointerId ?? 1, base: null, lastY: e.clientY, lastX: e.clientX};
  });
  window.addEventListener('pointermove', e => {
    if (!drag || e.pointerId !== drag.id) return;
    drag.lastX = e.clientX; drag.lastY = e.clientY;
    if (!drag.moved && Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 6) return;
    if (!drag.moved) {
      // 按住一张已选中的图拖动：拿起选中的图（ui/drag.js，#710），放进画廊或拖出窗口到别的应用（#450 ①）。全选时没有展开的 ID，不拖。
      const held = drag.start !== null ? state.items[drag.start] : null;
      if (held && !state.sel.all && isSelected(held.uuid)) { drag = null; lift([...state.sel.ids], e); return; }
      drag.moved = true; state.selecting = state.selecting || browsing();
      drag.base = {all: state.sel.all, ids: new Set(state.sel.ids), exclude: new Set(state.sel.exclude)};
      const startItem = drag.start !== null ? state.items[drag.start] : null;
      drag.mode = startItem && isSelected(startItem.uuid) ? false : true;
      const gr = $('grid').getBoundingClientRect(); drag.gx = drag.x - gr.left; drag.gy = drag.y - gr.top;
      document.body.classList.add('dragging'); autoScroll();
    }
    applyDrag();
  });
  window.addEventListener('pointerup', e => {
    if (!drag || e.pointerId !== drag.id) return;
    const d = drag; drag = null; document.body.classList.remove('dragging'); $('marquee').hidden = true;
    if (!d.moved && d.start !== null) handleCardClick(d.start, e, d.check);
    else if (d.moved && d.start !== null) state.anchor = indexAt(e.clientX, e.clientY);
  });
  $('grid').addEventListener('dblclick', e => { const card = e.target.closest('.card[data-id]'); if (card && !e.target.closest('button[data-fav]')) openViewer(Number(card.dataset.index)); });

  // ---------- 键盘 ----------
  // 按键先看最上面的模式（core/modes.js）：大图打开时它盖住选择栏，单图快捷键都作用于大图里那张。哪些键、在哪生效
  // 写在 core/modes.js 的 SHORTCUTS（帮助对话框由它生成），这里加减按键要同步改那张表（tests/help_keys.test.cjs 核对）。
  // 对话框或导览开着时整个让开：导览的按钮自己处理 Enter、Space、Tab，Esc 由浮层管理器处理（#398）。
  document.addEventListener('keydown', e => {
    if (composing(e) || document.querySelector('dialog[open], #tour:not([hidden])')) return;
    const mod = e.metaKey || e.ctrlKey, key = e.key.toLowerCase(), m = modes();
    // 面板页没有单图快捷键，但面板页借列表开的大图（previewList）里，单图键照样作用于大图那张。
    const view = viewOf(state.route), scope = keyScope(view) || (m.viewer ? 'shown' : ''), grid = scope === 'grid';
    if (mod && key === 'k') { e.preventDefault(); jumpToCharacter(); return; }
    // 后退、前进（#526）：Windows 上 Alt+← →，macOS 上 ⌘[ ⌘]（桌面 App 里由菜单接，网页形态在这里接），输入框里也生效。
    // 浏览器自己的后退会退出图库页面，这里改走 stepHistory，只在这个图库的页面之间走。
    const back = isMac ? e.metaKey && !e.altKey && (e.key === '[' || e.key === ']') : e.altKey && !mod && (e.key === 'ArrowLeft' || e.key === 'ArrowRight');
    if (back) { e.preventDefault(); stepHistory(e.key === '[' || e.key === 'ArrowLeft' ? -1 : 1); return; }
    // 活页条（#706）：⌘1–4 切一级，⌥↑↓ 在二级里移动，⌥⇧↑↓ 挪动钉住的项。
    if (railKey(e)) return;
    // ⌘＋／⌘－：大图打开时缩放大图；否则只在有网格的页面改网格列数，别的页面和换页读取中什么也不做（也不交给浏览器缩放）。
    const zoom = mod && (key === '=' || key === '+' || key === '-'), frozen = $('viewport').inert; if (zoom || frozen) {
      if (zoom) e.preventDefault(); if (frozen) return; const bigger = key !== '-';
      if (m.viewer) zoomView(bigger ? 1.4 : 1 / 1.4, innerWidth / 2, innerHeight / 2);
      else if (onGrid()) stepColumns(bigger ? -1 : 1);
      return;
    }
    // 输入框里的 Esc：搜索框有字时清空并取消这次搜索（发一次 input，由框自己的处理函数重新筛选），空的时候才离开输入框（#401）。
    if (e.target.closest('input,textarea,select,[contenteditable]')) {
      if (e.key !== 'Escape') return;
      if (e.target.type === 'search' && e.target.value) { e.preventDefault(); e.target.value = ''; e.target.dispatchEvent(new Event('input', {bubbles: true})); }
      else e.target.blur();
      return;
    }
    // Space 看大图的缩略图（按组整理、按天视图「认一认」，ui/detail.js 的 peek）：焦点在图上或鼠标停在图上时 Space 是看大图，
    // 不是按下这个按钮（点一下是「不算」，#424 的 2-2）。
    if (e.key === ' ' && !mod && !m.viewer && !edits.busy && peek(e.target)) { e.preventDefault(); return; }
    // 焦点在按钮、开关、链接上时，Enter、Space 归它自己（#422 的 2-5）：链接只认 Enter，Space 照常是页面的快捷键。
    // 网格卡片不拿焦点（焦点在 #viewport 上），不受影响。
    if (!mod && (e.key === 'Enter' || e.key === ' ') && e.target.matches?.(e.key === 'Enter' ? ACTIVATES + ',a[href]' : ACTIVATES)) return;
    if (mod && key === 'z') { e.preventDefault(); undo(); return; }
    // 焦点在开着的非模态浮层（任务面板、菜单……）里：按键归浮层自己的控件，单键快捷键不响应（#422 的 2-9、2-10）。
    if (inPopover(e.target)) return;
    // ⌘A 是选择栏的快捷键：大图打开时选择栏被盖住，不响应。
    if (mod && key === 'a' && grid) { e.preventDefault(); if (!m.viewer) selectAll(); return; }
    // 浮层（含大图）的 Esc 已由浮层管理器处理；走到这里说明栈是空的，接着关详情栏、取消选择。
    if (e.key === 'Escape') { if (m.detail) closeInspector(); else quitSelecting(); return; }
    if (key === 'r' && !mod && !pageOf(state.route).panel) { e.preventDefault(); toggleFit(); return; }
    if (key === 'h' && !mod) { e.preventDefault(); setNsfw(state.nsfw === 'hide' ? '' : 'hide'); return; }
    // /：聚焦本页工具栏里的搜索框（图库一类页面、资料库三个页签，#422）。
    const box = e.key === '/' && document.querySelector('#toolbar input[type=search]'); if (box) { e.preventDefault(); box.focus(); return; }
    const n = keySelection(), character = !!state.route.char;
    if ((key === 'y' || key === 'n') && !mod && !edits.busy && view.bar === 'audit' && n) { e.preventDefault(); bar.answerAudit(key === 'y'); return; }
    if ((key === 'y' || key === 'n') && !mod && !edits.busy && state.question && [state.focused, m.viewer && state.viewerItem?.uuid].includes(state.question.ident)) { e.preventDefault(); answerQuestion(key === 'y' ? 'yes' : 'no'); return; }
    if (edits.busy || !scope) return;
    // 按天视图（scope 'shown'）没有网格光标：下面的键只作用于详情栏或大图里那张，两者都没开时不响应；方向键只有 ← → 翻页，
    // ↑ ↓ 照常滚动页面。
    if (!grid && !m.viewer && !m.detail) return;
    if (key === 'i' && !mod) { e.preventDefault(); toggleInfo(); return; }
    if (e.key === ' ') { e.preventDefault(); if (m.viewer) closeViewer(); else openViewer(grid ? viewerTarget() : indexOf(state.focused)); return; }
    if (['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) {
      // 有非模态浮层开着时方向键不动网格（菜单的 ↑ ↓ 由浮层管理器先处理，#422 的 2-9）。
      if (popoverOpen()) return;
      if (!grid) { if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') { e.preventDefault(); step(e.key === 'ArrowLeft' ? -1 : 1); } return; }
      e.preventDefault();
      const from = state.cursor < 0 ? 0 : state.cursor;
      const delta = e.key === 'ArrowLeft' ? -1 : e.key === 'ArrowRight' ? 1 : verticalStep(from, e.key === 'ArrowUp' ? -1 : 1) - from || (e.key === 'ArrowUp' ? -state.cols : state.cols);
      // 大图里只有 ← → 翻页；详情栏里 ↑ ↓ 也翻到上一行、下一行的图。
      if (m.viewer || m.detail) { if (Math.abs(delta) === 1 || !m.viewer) step(delta); return; }
      setCursor(Math.max(0, Math.min(state.total - 1, (state.cursor < 0 ? 0 : state.cursor + delta))));
      // 用方向键在网格里走时焦点回到网格：鼠标点过的按钮（WebView2 里点击会让按钮拿焦点，网格头部里的也算）不再接着收 Enter、Space。
      if (!$('viewport').contains(document.activeElement) || $('gridHead').contains(document.activeElement)) $('viewport').focus({preventScroll: true});
      if (e.shiftKey && state.items[state.cursor]) { setSelected(state.items[state.cursor].uuid, true); renderSelection(); }
      if (!state.items[state.cursor]) loadPage(Math.floor(state.cursor / PAGE) * PAGE);
      ensureVisible(state.cursor); return;
    }
    if (e.key === 'Enter' && grid && state.cursor >= 0 && !m.viewer) { e.preventDefault(); const item = state.items[state.cursor]; if (!item) return; if (cleanup() || state.selecting) { setSelected(item.uuid, !isSelected(item.uuid)); state.anchor = state.cursor; renderSelection(); } else inspect(item.uuid); return; }
    if (mod || e.altKey && !/^[1-9]$/.test(e.key)) return;
    if (['c', 'a', 't', 's', 'g', 'f'].includes(key) || /^[1-9]$/.test(e.key)) e.preventDefault(); // 别让这个字母进到命令面板的输入框里
    const remove = e.key === 'Backspace' || e.key === 'Delete';
    if (remove || ['c', 'a', 't', 'g', 'x', 's', 'f'].includes(key)) {
      if (remove) e.preventDefault();
      const item = n ? null : currentItem(); if (!n && !item) return;   // 没有选中也没有当前图：什么都不做
      const act = keyAction(remove ? 'remove' : key, {view, char: character, n, item, maybe: !!state.route.maybe, variants: !!state.char?.variants?.length});
      if (act?.bar) bar.run(act.bar);
      else if (act) singleAction(act.single);
      // 选择栏上没有这个键的按钮，或整理页里这个键没有对应的动作：说一声，不再静默吞掉（#422 的 2-21）。
      else if (n || view.click === 'cleanup') toast(t('这一页没有 {key} 对应的操作', {key: remove ? '⌫' : key.toUpperCase()}));
      return;
    }
    if (/^[1-9]$/.test(e.key) && !n && state.detail && (m.detail || m.viewer)) { pickAlternative(state.detail, state.detail.uuid, Number(e.key) - 1, e.altKey); return; }
  });

  renderHelpKeys();


  $('nsfwButton').onclick = () => setNsfw({'': 'hide', hide: 'only', only: ''}[state.nsfw]);
  renderNsfw();
}
