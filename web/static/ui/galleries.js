// 画廊（#445）：活页条画廊的二级层随列表变、全部画廊的封面卡片（#/galleries）、一个画廊的页头（#/gallery/<id>），以及工具栏上的
// 「保存为画廊」。画廊里的图就是带着它的条件的图库，网格和筛选条与图库共用；数据和接口在 core/galleries.js。
import {exclusive} from '../core/edits.js';
import {esc, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {defaultName, routeFields, savable, sameParams, savedParams} from '../core/filters.js';
import {countOf, coversFor, createGallery, deleteGallery, forgetCovers, galleryOf, loadGalleries, setGallery, updateGallery} from '../core/galleries.js';
import {composing} from '../core/keys.js';
import {onMeta} from '../core/meta.js';
import {implement, onPage, pageOf} from '../core/pages.js';
import {go, replaceRoute} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {$, error, readFailure, toast, written} from './dom.js';
import {conditionLabel} from './filters.js';
import {createLayer} from './overlay.js';
import {loadGalleryCovers} from './rail.js';

const galleryRoute = g => ({page: 'gallery', id: String(g.id), ...routeFields(g.params)});
// 画廊列表变了：活页条画廊的二级层跟着换（收藏在最前，封面按 NSFW 筛选批量读）。
function showNav() { loadGalleryCovers(); }
async function reloadList() {
  try { await loadGalleries(); } catch (e) { error(readError(e)); }
  showNav();
}
// 一次画廊写入：同一时间只进行一项改动；成功后进撤销栈，回复里的 gallery 换进列表，列表再重读一次（顺序可能变了）。
async function write(run) {
  let result;
  try { result = await exclusive(run); } catch (e) { toast(e.message); return null; }
  if (!result) return null;
  written(result.undo);
  forgetCovers();
  return result;
}

// ---------- 工具栏：保存为画廊 ----------
// 在图库和角色页上有条件（含搜索词）时出现；画廊页上换成页头的「更新画廊」「另存为新画廊」。
export const saveButton = r => savable(r) && !pageOf(r).gallery ? `<button class="quiet" id="saveGallery" title="${t('把现在的条件存成画廊')}">${t('保存为画廊')}</button>` : '';
export function wireSaveButton(r) { $('saveGallery')?.addEventListener('click', () => saveNew(r)); }
// 存成新画廊：名字按条件的文字生成，存好后进入这个画廊，标题处于改名状态。
async function saveNew(r) {
  const result = await write(() => createGallery(savedParams(r), defaultName(r, conditionLabel)));
  if (!result?.gallery) return;
  setGallery(result.gallery.id, result.gallery);
  renaming = String(result.gallery.id);
  showNav();
  go(galleryRoute(result.gallery));
  reloadList();
}

// ---------- 全部画廊 ----------
async function renderIndex() {
  const box = $('groupsPage');
  let list; try { list = await loadGalleries(); } catch (e) { if (onPage(state.route, 'galleries')) readFailure(box, e); return; }
  if (!onPage(state.route, 'galleries')) return;
  showNav(); drawHeading();
  if (!list.length) { box.innerHTML = `<p class="empty">${t('还没有画廊。在图库里加上条件，点「保存为画廊」。')}</p>`; return; }
  const poster = g => `<a class="cast-poster" href="#/gallery/${encodeURIComponent(g.id)}" data-gallery="${esc(g.id)}">` +
    `<span class="img"></span><span class="cap"><b title="${esc(g.name)}">${esc(g.name)}</b><small>…</small></span></a>`;
  box.innerHTML = `<div class="cast-posters">${list.map(poster).join('')}</div>`;
  const nsfw = state.nsfw, route = state.route, here = () => state.route === route && state.nsfw === nsfw;
  const card = id => box.querySelector(`[data-gallery="${CSS.escape(String(id))}"]`);
  coversFor(list.map(g => g.id)).then(covers => {
    if (!here()) return;
    for (const [id, uuid] of Object.entries(covers)) {
      const img = uuid && card(id)?.querySelector('.img');
      if (img) img.innerHTML = `<img src="/image?id=${encodeURIComponent(uuid)}&size=medium" alt="" loading="lazy">`;
    }
  }).catch(e => { if (here()) error(readError(e)); });
  for (const g of list) {
    countOf(g.params).then(({n, error: problem}) => {
      const small = here() && card(g.id)?.querySelector('small'); if (!small) return;
      small.textContent = problem ? t('条件失效') : t('{n} 张', {n: number(n)});
      small.title = problem || '';
    }).catch(() => {});
  }
}

// ---------- 一个画廊 ----------
// 打开 #/gallery/<id>：不带查询时按保存的条件打开，并把条件写进地址；带查询时按地址（条件可能已经改过）。
// 条件失效时（引用的作品、标签没了）网格读取回 400，原因只写在空状态里（readFailed），不弹错误条（#647）。
let renaming = '';
async function prepare(route) {
  if (!state.galleries) await reloadList();
  const g = galleryOf(route.id);
  if (!g) { toast(t('画廊不存在')); go({page: 'galleries'}); return false; }
  if (!location.hash.includes('?')) { Object.assign(route, routeFields(g.params)); replaceRoute({}); }
  showNav();
  return true;
}
const changed = (g, r) => !sameParams(g.params, savedParams(r));
function drawGalleryHeading(h, r) {
  const g = galleryOf(r.id);
  if (!g) { h.innerHTML = `<div class="heading-main"><h1 class="title-page">${t('画廊')}</h1><p class="page-sub"></p></div>`; return; }
  const sub = t('{n} 张', {n: state.totalReady ? number(state.total) : '…'});
  // 正在改名：输入框还在就只换副标题，不打断输入。
  if (renaming === String(g.id) && $('galleryName')?.tagName === 'INPUT') { h.querySelector('.heading-main p').innerHTML = sub; return; }
  const list = state.galleries, at = list.indexOf(g), movable = list.every(x => Number.isInteger(x.position));
  const edited = changed(g, r);
  const item = (id, label, title = '') => `<button class="menu-item" id="${id}"${title ? ` title="${title}"` : ''}>${label}</button>`;
  const title = renaming === String(g.id)
    ? `<input id="galleryName" maxlength="60" size="${Math.min(40, Math.max(12, [...g.name].length + 2))}" value="${esc(g.name)}" aria-label="${t('画廊名称')}">`
    : `<button class="link-button" id="galleryName" title="${t('点一下改名')}">${esc(g.name)}</button>`;
  h.innerHTML = `<div class="heading-main"><h1 class="title-page">${title}</h1><p class="page-sub">${sub}</p></div><div class="heading-actions">` +
    (edited ? `<button class="quiet" id="galleryUpdate" title="${t('用现在的条件替换这个画廊保存的条件')}">${t('更新画廊')}</button><button class="quiet" id="gallerySaveAs">${t('另存为新画廊')}</button>` : '') +
    `<div class="tag-control more-control"><button class="quiet" id="galleryMore" title="${t('更多操作')}" aria-label="${t('更多操作')}">⋯</button>` +
    `<div id="galleryMenu" class="menu menu-drop more-menu" hidden>` +
    item('galleryCover', t('把详情栏里这张设为封面')) + (g.cover_manual ? item('galleryAutoCover', t('封面改回自动'), t('用这个画廊排序下的第一张')) : '') +
    (movable && at > 0 ? item('galleryUp', t('往前移')) : '') + (movable && at < list.length - 1 ? item('galleryDown', t('往后移')) : '') +
    item('galleryDelete', t('删除画廊')) + `</div></div></div>`;
  wireHeading(g, r);
}
const menu = createLayer({kind: 'popover', id: 'gallery-menu', el: () => $('galleryMenu'), anchor: () => $('galleryMore'), menu: true, focusIn: 'first', onDismiss: reason => setMenu(false, reason)});
function setMenu(open, reason) {
  const el = $('galleryMenu'); if (el) el.hidden = !open;
  if (open) menu.open(); else menu.close(reason);
}
function wireHeading(g, r) {
  const id = String(g.id), name = $('galleryName');
  if (name.tagName === 'INPUT') {
    name.focus(); name.select();
    let done = false;
    const finish = save => {
      if (done) return; done = true; renaming = '';
      const value = name.value.trim();
      if (save && value && value !== g.name) rename(g, value); else drawHeading();
    };
    name.addEventListener('keydown', e => {
      if (composing(e)) return;
      if (e.key === 'Enter') { e.preventDefault(); finish(true); } else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); finish(false); }
    });
    name.addEventListener('blur', () => finish(true));
  } else name.onclick = () => { renaming = id; drawHeading(); };
  $('galleryUpdate')?.addEventListener('click', () => update(g, {params: savedParams(r)}));
  $('gallerySaveAs')?.addEventListener('click', () => saveNew(r));
  $('galleryMore').onclick = () => setMenu($('galleryMenu').hidden);
  const act = (key, run) => $(key)?.addEventListener('click', () => { setMenu(false); run(); });
  act('galleryCover', () => state.focused ? update(g, {cover: state.focused}) : toast(t('先点开一张图。')));
  act('galleryAutoCover', () => update(g, {cover: null}));
  act('galleryUp', () => move(g, -1));
  act('galleryDown', () => move(g, 1));
  act('galleryDelete', () => remove(g));
}
async function update(g, patch) {
  const result = await write(() => updateGallery(g.id, patch));
  if (!result) return;
  setGallery(g.id, result.gallery);
  await reloadList();
  drawHeading();
}
const rename = (g, name) => update(g, {name});
// 和相邻的一个交换先后：两步写入，撤销一次只换回一个，所以位置相同时按 id 排，不会丢画廊。
async function move(g, dir) {
  const list = state.galleries, other = list[list.indexOf(g) + dir]; if (!other) return;
  const [a, b] = other.position === g.position ? [other.position, other.position - dir] : [other.position, g.position];
  if (b < 0) return;
  if (!await write(() => updateGallery(g.id, {position: a}))) return;
  await write(() => updateGallery(other.id, {position: b}));
  await reloadList(); drawHeading();
}
async function remove(g) {
  if (!await write(() => deleteGallery(g.id))) return;
  setGallery(g.id, null); showNav();
  go({page: 'galleries'});
  reloadList();
}
const indexHeading = () => [t('画廊'), t('{n} 个', {context: 'gallery', n: number(state.galleries?.length || 0)})];
// 页头：画廊页由 drawGalleryHeading 画；全部画廊用通用标题。
function drawHeading() {
  if (onPage(state.route, 'gallery')) drawGalleryHeading($('heading'), state.route);
  else if (onPage(state.route, 'galleries')) {
    const [title, sub] = indexHeading();
    $('heading').innerHTML = `<div class="heading-main"><h1 class="title-page">${title}</h1><p class="page-sub">${sub}</p></div>`;
  }
}

// 写入、撤销或切换 NSFW 之后：列表重读，卡片和页头重画。网格本身由 reload() 管。
async function refresh(why) {
  if (why === 'view' || why === 'ask') return;
  forgetCovers();
  if (onPage(state.route, 'galleries')) { renderIndex(); return; }
  await reloadList();
  if (!onPage(state.route, 'gallery')) return;
  if (!galleryOf(state.route.id)) { go({page: 'galleries'}); return; }
  drawHeading();
}
implement({
  galleries: {render: () => renderIndex(), headingHTML: indexHeading, refresh},
  gallery: {prepare, drawHeading: drawGalleryHeading, refresh, onMeta: () => {},
    emptyNext: () => state.nsfw === 'only' ? [{label: t('显示全部'), act: 'nsfw'}] : [],
    // 条件失效：后端对条件引用的角色、作品、标签不存在回 400。在筛选条上改掉失效的条件后可以「更新画廊」，或者删掉它。
    readFailed: e => e.status === 400 ? t('条件失效：{reason}', {reason: e.message}) : '',
    failedNext: () => [{label: t('删除画廊'), act: 'delete-gallery'}],
    emptyAct: act => { const g = act === 'delete-gallery' && galleryOf(state.route.id); if (g) remove(g); }},
});
// 页面启动时调用一次：每次读元数据（启动、写入、撤销、同步之后）顺带重读画廊列表，侧栏入口跟着显示或隐藏。
export function wireGalleries() {
  onMeta(() => { reloadList(); });
}
