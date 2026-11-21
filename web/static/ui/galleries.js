// 画廊（#445、#710）：三个页面的登记，一个画廊的页头（标题、副标题、展示方式的切换、「更新画廊」「另存为新画廊」、「⋯」菜单）。
// 工具栏上的「保存为画廊」在 ui/gallery_save.js（框架 ui/shell.js 要用它；这个文件导入 shell，反过来会循环导入）。
//
// 一个画廊按它的展示方式（core/gallery_shows.js 的登记表）落在两页之一：网格页 gallery（瀑布、色彩：显示全部图，就是带着它的条件的
// 图库网格）或展示页 showcase（拼贴、杂志、展厅：ui/gallery_stage.js）。全部画廊的卡片在 ui/gallery_cards.js；拖图进画廊在
// ui/drag.js，选择栏的放进、拿出在 ui/gallery_bar.js，共用的动作在 ui/gallery_actions.js；数据在 core/galleries.js。
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {routeFields, sameParams, savedParams} from '../core/filters.js';
import {deleteGallery, galleryHref, galleryOf, isFav, isManual, setGallery, updateGallery} from '../core/galleries.js';
import {chooseShow, loadShows, pageFor, showFor, SHOWS} from '../core/gallery_shows.js';
import {implement, onPage, pageOf} from '../core/pages.js';
import {go, replaceRoute} from '../core/route.js';
import {state} from '../core/state.js';
import {toast} from './dom.js';
import {attr, make, tpl} from './el.js';
import {askName, kindText, reloadList, write} from './gallery_actions.js';
import {saveNew} from './gallery_save.js';
import {renderIndex, refreshIndex} from './gallery_cards.js';
import {renderStage, refreshStage, stageTotal} from './gallery_stage.js';
import {reload} from './grid.js';
import {createLayer} from './overlay.js';
import {currentPage} from './scope.js';
import {renderChrome} from './shell.js';
import {popOpen, showPop} from './pop.js';

// ---------- 一个画廊：进页面之前 ----------
// 先读画廊列表和各画廊的展示方式。地址的页面和这个画廊的展示方式不符（记着的是展示页，地址是网格页，或反过来）时换到对的那页。
// 网格页不带查询打开时按保存的条件，并把条件写进地址；带查询时按地址（条件可能已经改过）。
// await 之后已经换了页（route 不再是现在的路由）就什么也不做，返回 false。
async function prepare(route) {
  await Promise.all([state.galleries ? null : reloadList(), loadShows()]);
  if (state.route !== route) return false;
  const g = galleryOf(route.id);
  if (!g) { toast(t('画廊不存在')); go({page: 'galleries'}); return false; }
  if (pageFor(showFor(g.id)) !== route.page) { location.replace(galleryHref(g.id)); return false; }
  if (pageOf(route).gallery && !location.hash.includes('?')) { Object.assign(route, routeFields(g.params)); replaceRoute({}); }
  return true;
}

// ---------- 一个画廊：页头 ----------
// 标题是名字；副标题写这是哪种画廊和张数：收藏只写张数，手选画廊写「手选」，按条件的写「按条件：条件的文字」（截图给用户看过，#710）。
// 右边：展示方式的分段切换；按条件的画廊在网格页上改了条件后有「更新画廊」「另存为新画廊」；收藏以外有「⋯」菜单。
// 「播放」「存成一张图」这次不做（#710），以后加在 actions 末尾。
const SHOW_ICONS = {
  collage: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="4" width="8" height="7" rx="1" transform="rotate(-8 7 7.5)"/><rect x="9" y="9"
    width="8" height="7" rx="1" transform="rotate(7 13 12.5)"/></svg>`,
  magazine: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="3" width="8" height="14" rx="1"/><path d="M13.5 4.5h3.5M13.5 8h3.5M13.5 11.5h3.5M13.5 15h3.5"/></svg>`,
  masonry: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="3" width="4" height="8" rx="1"/><rect x="8" y="3" width="4" height="5" rx="1"/><rect
    x="13" y="3" width="4" height="9" rx="1"/><rect x="3" y="12" width="4" height="5" rx="1"/><rect x="8" y="9" width="4" height="8" rx="1"/><rect x="13" y="13"
    width="4" height="4" rx="1"/></svg>`,
  hall: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M2 16.5h16"/><rect x="3.5" y="6" width="4" height="6" rx=".6"/><rect x="9" y="5" width="3"
    height="7" rx=".6"/><rect x="13.5" y="6.5" width="3.5" height="5" rx=".6"/></svg>`,
  color: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="7" cy="7.5" r="3.5"/><circle cx="13" cy="7.5" r="3.5"/><circle cx="10" cy="12.5" r="3.5"/></svg>`,
  other: tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="3" width="14" height="14" rx="2"/></svg>`,
};
// 分段切换：每种一个按钮（图标 + 名字），按登记表的先后；按下的是这个画廊现在的展示方式。建一次，按画廊更新。
let seg = null, segFor = '';
function showSwitch(g) {
  if (!seg) {
    seg = make('div', 'seg gallery-shows');
    attr(seg, 'role', 'group'); attr(seg, 'aria-label', t('展示方式'));
    for (const show of SHOWS) {
      const b = make('button');
      b.type = 'button'; b.dataset.show = show.id;
      b.append((SHOW_ICONS[show.id] || SHOW_ICONS.other)().el, make('span', '', show.label()));
      b.onclick = () => switchShow(galleryOf(segFor), show.id);
      seg.append(b);
    }
  }
  segFor = String(g.id);
  for (const b of seg.children) attr(b, 'aria-pressed', String(b.dataset.show === showFor(g.id)));
  return seg;
}
// 换展示方式：记进偏好（按图库存在浏览器本地）。换到另一页（网格页 ↔ 展示页）时换地址；同一页里换（瀑布 ↔ 色彩、拼贴 ↔ 杂志）
// 时就地重画：网格页按新的分行和排序重读，展示页重排。
function switchShow(g, show) {
  if (!g || show === showFor(g.id)) return;
  const before = pageFor(showFor(g.id));
  chooseShow(g.id, show);
  if (pageFor(show) !== before) { location.hash = galleryHref(g.id, show); return; }
  if (pageOf(state.route).gallery) { renderChrome(); reload(true); } else { renderChrome(); renderStage({samePage: true, scope: currentPage()}); }
}
// 按条件的画廊在网格页上改了条件（不算定义它的那个条件）：「更新画廊」「另存为新画廊」。
const edited = (g, r) => !isManual(g) && pageOf(r).gallery && !sameParams(g.params, savedParams(r));
let updateButtons = null;
function editButtons(g, r) {
  if (!edited(g, r)) return [];
  if (!updateButtons) {
    const update = make('button', 'btn is-quiet', t('更新画廊')), saveAs = make('button', 'btn is-quiet', t('另存为新画廊'));
    update.title = t('用现在的条件替换这个画廊保存的条件');
    update.onclick = () => { const x = galleryOf(state.route.id); if (x) change(x, {params: savedParams(state.route)}); };
    saveAs.onclick = () => saveNew(state.route);
    updateButtons = [update, saveAs];
  }
  return updateButtons;
}
function galleryHead(r, {n}) {
  const g = galleryOf(r.id);
  if (!g) return {title: t('画廊')};
  const stage = pageOf(r).stage, total = stage ? stageTotal() : null, count = stage ? (total === null ? '…' : number(total)) : n;
  return {title: g.name, sub: [kindText(g), t('{n} 张', {n: count})].filter(Boolean).join(' · '),
    actions: [...editButtons(g, r), showSwitch(g), ...(isFav(g.id) ? [] : [moreButton(g)])]};
}

// ---------- 「⋯」菜单 ----------
// 改名、设封面（用详情栏里那张）、封面改回自动、往前移、往后移、删除画廊。每一步都进撤销栈。菜单建一次，打开时按画廊排项。
const MORE = tpl`<div class="tag-control more-control"><button class="btn is-quiet" data-ref="button" title="更多操作" aria-label="更多操作"
  data-t-attr="title aria-label">⋯</button><div class="menu menu-drop more-menu" id="galleryMenu" data-ref="menu" hidden></div></div>`;
let more = null;
const menu = createLayer({kind: 'popover', id: 'gallery-menu', el: () => more?.refs.menu, anchor: () => more?.refs.button, menu: true, focusIn: 'first',
  onDismiss: reason => setMenu(false, reason)});
function setMenu(open, reason) {
  if (!more) return;
  showPop(more.refs.menu, open);
  if (open) menu.open(); else menu.close(reason);
}
function moreButton() {
  if (!more) {
    more = MORE();
    more.refs.button.onclick = () => { const open = !popOpen(more.refs.menu); if (open) fillMenu(galleryOf(state.route.id)); setMenu(open); };
  }
  return more.el;
}
function fillMenu(g) {
  if (!g) return;
  const list = state.galleries || [], at = list.findIndex(x => String(x.id) === String(g.id)), movable = list.every(x => Number.isInteger(x.position));
  const item = (label, run, title = '') => {
    const b = make('button', 'menu-item', label);
    if (title) b.title = title;
    b.onclick = () => { setMenu(false); run(); };
    return b;
  };
  more.refs.menu.replaceChildren(...[
    item(t('改名…'), () => rename(g)),
    item(t('把详情栏里这张设为封面'), () => state.focused ? change(g, {cover: state.focused}) : toast(t('先点开一张图。'))),
    g.cover_manual && item(t('封面改回自动'), () => change(g, {cover: null}), t('用这个画廊排序下的第一张')),
    movable && at > 0 && item(t('往前移'), () => move(g, -1)),
    movable && at >= 0 && at < list.length - 1 && item(t('往后移'), () => move(g, 1)),
    item(t('删除画廊'), () => remove(g)),
  ].filter(Boolean));
}
async function change(g, patch) {
  const result = await write(() => updateGallery(g.id, patch));
  if (!result) return;
  setGallery(g.id, result.gallery);
  await reloadList();
  renderChrome();
}
async function rename(g) {
  const name = await askName(t('改名'), g.name);
  if (name && name !== g.name) change(g, {name});
}
// 和相邻的一个交换先后：两步写入，撤销一次只换回一个，所以位置相同时按 id 排，不会丢画廊。
async function move(g, dir) {
  const list = state.galleries, other = list[list.findIndex(x => String(x.id) === String(g.id)) + dir]; if (!other) return;
  const [a, b] = other.position === g.position ? [other.position, other.position - dir] : [other.position, g.position];
  if (b < 0) return;
  if (!await write(() => updateGallery(g.id, {position: a}))) return;
  await write(() => updateGallery(other.id, {position: b}));
  await reloadList(); renderChrome();
}
async function remove(g) {
  if (!await write(() => deleteGallery(g.id))) return;
  setGallery(g.id, null);
  go({page: 'galleries'});
  reloadList();
}

// ---------- 数据变了 ----------
// 写入、撤销或切换 NSFW 之后：列表重读，卡片、页头和展示页重画。网格本身由 reload() 管。画廊被删掉（撤销了新建）时回到全部画廊。
async function refresh(why) {
  if (why === 'view' || why === 'ask') return;
  const r = state.route;
  if (onPage(r, 'galleries')) { refreshIndex(); return; }
  await reloadList();
  if (state.route !== r || (!pageOf(r).gallery && !pageOf(r).stage)) return;   // 这期间已经换了页
  if (!galleryOf(r.id)) { go({page: 'galleries'}); return; }
  if (pageOf(r).stage) refreshStage(); else renderChrome();
}
const showAll = () => state.nsfw === 'only' ? [{label: t('显示全部'), act: 'nsfw'}] : [];
implement({
  galleries: {render: renderIndex, head: () => ({title: t('画廊'), sub: t('{n} 个', {context: 'gallery', n: number((state.galleries?.length || 0) + 1)})}), refresh},
  gallery: {prepare, head: galleryHead, refresh, onMeta: () => {}, emptyNext: showAll,
    // 条件失效：后端对条件引用的角色、作品、标签不存在回 400。在筛选条上改掉失效的条件后可以「更新画廊」，或者删掉它。
    readFailed: e => e.status === 400 ? t('条件失效：{reason}', {reason: e.message}) : '',
    failedNext: () => [{label: t('删除画廊'), act: 'delete-gallery'}],
    emptyAct: act => { const g = act === 'delete-gallery' && galleryOf(state.route.id); if (g) remove(g); }},
  showcase: {prepare, render: renderStage, head: galleryHead, refresh},
});
