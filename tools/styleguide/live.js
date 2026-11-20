// 样式清单页里“调用生成”的样例（#352）：直接调用 web/static/ui 里生成组件的函数，把它们画出来的 DOM 摆进页面，
// 不再照着模板手抄。index.html 里每个 `data-sg-live="名字"` 的舞台对应下面一个 sample('名字', …)；
// tools/check_styleguide.py 核对两边一一对应、这里 import 的名字在 web/static 里确实导出。
//
// 怎么做到不起宿主也能画：
//   - 页面先以经典脚本加载 web/static/i18n.js（和应用一样），再加载本模块；file:// 下浏览器不加载 ES 模块，
//     这些舞台只显示一行说明（styleguide.js 写的），要经 `python3 -m http.server` 打开。
//   - 应用页面里写好的外壳（#toast、#picker、#taskPanel…）从 web/static/index.html 原样取来，放进一个藏起来的容器，
//     容器放在 <body> 最前面，产品代码按 id 找到的总是它们。
//   - fetch 换成一个桩：只回答 /api/ 开头的读取，回应是下面的合成数据；其余请求照常发出。
//   - <dialog> 的 showModal() 换成只加 open 属性：对话框不进顶层、不出遮罩。
//   - 写进 DOM 的 HTML 里，/image?… 这类图片换成占位图（innerHTML、insertAdjacentHTML 的桩）。
//   - 生成好之后克隆一份放进舞台，原件随即关掉或留在藏起来的容器里。所以舞台里的样例是静态的：
//     点了不会触发应用里的操作。
// 数据全是合成的，不碰真实图库。产品代码的行为没有为这页改过：这里只换了 fetch、showModal、innerHTML 和 state 的初值。
import {chooseWork, confirmAction} from '../../web/static/ui/dialog.js';
import {castPop, openIndexAdd, openTagged, workDialog} from '../../web/static/ui/catalog.js';
import {editProfile, openLocalLinks, openPicker, openRelate, splitOc, stage} from '../../web/static/ui/palette.js';
import {renderHelpKeys, renderSelection} from '../../web/static/ui/grid.js';
import '../../web/static/ui/grid_bar.js';
import {renderViewPop} from '../../web/static/ui/grid_view.js';
import {renderTasks, taskPanel} from '../../web/static/ui/tasks.js';
import {toast, toastMore} from '../../web/static/ui/dom.js';
import {askHTML} from '../../web/static/ui/detail.js';
import {state} from '../../web/static/core/state.js';

// 从应用页面取来的外壳。
const SHELLS = ['toast', 'selectionBar', 'grid', 'picker', 'relateDialog', 'ocSplitDialog', 'help', 'taskButton', 'taskPanel', 'viewPopover'];

// ---- 合成数据 ----
const CHARS = [
  {id: 'c1', name: 'Karen', count: 12, cover: 'u1', work_id: 'ba', work_name: 'Blue Archive'},
  {id: 'c2', name: 'Rio', count: 9, cover: 'u2', work_id: 'ba', work_name: 'Blue Archive'},
  {id: 'c3', name: 'Mash', count: 64, cover: 'u3', work_id: 'fgo', work_name: 'Fate/Grand Order'},
  {id: 'c4', name: '花凛', count: 3, kind: 'oc'},
];
const WORKS = [{id: 'ba', name: 'Blue Archive', tag: 'blue_archive'}, {id: 'fgo', name: 'Fate/Grand Order', tag: 'fate/grand_order'}];
const TAGGED = [{id: 'm1', name: '佩洛洛', count: 12}, {id: 'm2', name: '老师', count: 40}];
const API = {
  '/api/character/profile': {id: 'c1', name: 'Karen', user_name: 'Karen', tag: 'karen_(blue_archive)', aliases: ['カレン', '花凛'], other_aliases: 3,
    work_id: 'ba', work_name: 'Blue Archive', label_only: false, oc: false},
  '/api/tagged': TAGGED,
  '/api/relate': {same: [['u1', 'u3']], missing: [], sizes: {u1: {width: 2000, height: 2500}, u2: {width: 1200, height: 1500}},
    pairs: [{a: 'u1', b: 'u2', kind: 'crop', label: '局部重复', note: '第 2 张是第 1 张的局部',
      boxes_a: [{x: .1, y: .2, w: .4, h: .3}], boxes_b: [{x: .05, y: .05, w: .8, h: .7, match: true}]},
    {a: 'u2', b: 'u3', kind: 'unrelated', label: '', boxes_a: [], boxes_b: []}]},
  '/api/oc/groups': {total: 20, groups: [['u1', 'u2', 'u3', 'u4', 'u5', 'u1', 'u2', 'u3', 'u4', 'u5'], ['u2', 'u4']]},
};

function seedState() {
  Object.assign(state, {
    route: {page: 'library'}, chars: CHARS, charMap: new Map(CHARS.map(c => [c.id, c])), tagged: TAGGED, total: 1200,
    index: {state: {status: 'idle'}, mb_per_character: 3.4, watch: {works: ['fgo']}, works: WORKS,
      check: {checked_at: '2026-11-12T08:00:00', missing: [{short_tag: 'karen', tag: 'karen_(blue_archive)', copyright: 'blue_archive', post_count: 12345},
        {short_tag: 'rio', tag: 'rio_(blue_archive)', copyright: 'blue_archive', post_count: 9876}]}},
    localLinks: [
      {kind: 'link', id: 'c9', name: 'Karen', tag: 'karen_(blue_archive)', work_name: 'Blue Archive', posts: 12345, count: 12},
      {kind: 'link', id: 'c8', name: 'Rio', guess: true, tag: 'rio_(blue_archive)', posts: 9876, count: 3},
      {kind: 'merge', id: 'c7', name: 'カレン', count: 2, tag: 'karen_(blue_archive)', target: {id: 'c1', name: 'Karen'}}],
    sync: {status: 'recognising', done: 1240, total: 5000, ms_per_image: 120},
    syncHistory: [{started_at: '2026-11-12T10:21:00', found: 12, removed: 0, imported: 12, failed: 0}],
    names: {state: {status: 'done'}},
    provenance: {available: true, state: {status: 'failed', reason: '网络不可用'}, runs: []},
  });
}

// ---- 桩 ----
const realFetch = window.fetch.bind(window);
window.fetch = (input, init) => {
  const url = new URL(typeof input === 'string' ? input : input.url, location.href);
  if (url.origin !== location.origin || !url.pathname.startsWith('/api/')) return realFetch(input, init);
  const known = Object.hasOwn(API, url.pathname);
  const body = known ? API[url.pathname] : {error: `清单页没有给 ${url.pathname} 写桩`};
  return Promise.resolve(new Response(JSON.stringify(body), {status: known ? 200 : 404, headers: {'Content-Type': 'application/json'}}));
};
HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
const closeDialog = d => { d.removeAttribute('open'); d.dispatchEvent(new Event('close')); };
// 产品代码里的图片是 /image?id=… 和 /api/entity-preview?…，http.server 上没有，会刷一排 404。写进 DOM 之前换成占位图。
let ph = 0;
const localImages = html => typeof html === 'string' ? html.replace(/src="\/(?:image|api\/entity-preview)\?[^"]*"/g, () => `src="${window.sgPlaceholder(ph++)}"`) : html;
const inner = Object.getOwnPropertyDescriptor(Element.prototype, 'innerHTML');
Object.defineProperty(Element.prototype, 'innerHTML', {...inner, set(html) { inner.set.call(this, localImages(html)); }});
const adjacent = Element.prototype.insertAdjacentHTML;
Element.prototype.insertAdjacentHTML = function (where, html) { return adjacent.call(this, where, localImages(html)); };

async function loadShells() {
  const html = await (await realFetch('../../web/static/index.html', {cache: 'no-store'})).text();
  const doc = new DOMParser().parseFromString(html, 'text/html'), box = document.createElement('div');
  box.hidden = true;
  for (const id of SHELLS) {
    const el = doc.getElementById(id);
    if (!el) throw new Error(`web/static/index.html 里找不到 #${id}：外壳改名或删掉了，改 tools/styleguide/live.js 的 SHELLS`);
    box.append(document.importNode(el, true));
  }
  document.body.prepend(box);
}

// ---- 摆进舞台 ----
// 克隆一份，去掉 hidden（外壳本来藏着，要在应用里点开才显示）。id 保留，app.css 有按 id 写的样式；
// 产品代码按 id 找到的仍是藏起来的原件，因为原件的容器在 <body> 最前面。
function snapshot(el) {
  const copy = el.cloneNode(true);
  copy.hidden = false;
  if (copy instanceof HTMLDialogElement) copy.setAttribute('open', '');  // 页面里写好、还没打开过的对话框（#help）
  return copy;
}
const fromHTML = html => { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content.firstElementChild; };
const $ = id => document.getElementById(id);
// 等产品代码生成的新对话框出现（多数函数要等对话框关闭才返回，所以不能等它们本身），取一份，然后关掉原件。
async function openedDialog(run) {
  const before = new Set(document.querySelectorAll('body > dialog'));
  run();
  for (let i = 0; i < 100; i++) {
    const d = [...document.querySelectorAll('body > dialog')].find(x => !before.has(x));
    if (!d) { await new Promise(r => setTimeout(r, 20)); continue; }
    await new Promise(r => setTimeout(r));
    const copy = snapshot(d);
    // 只靠 open 属性打开的对话框，浏览器的 close() 不派发 close 事件；这里自己派发，让产品代码的 onClose 收尾、
    // 把对话框移除（confirmAction 同一时间只开一个，不收尾下一个样例就开不出来）。returnValue 为空，算取消。
    closeDialog(d); await new Promise(r => setTimeout(r));
    return copy;
  }
  throw new Error('等了 2 秒没有出现新的对话框');
}
// 一组摆在一起的样例；每个带一行小字说明它是怎么调出来的。
const pair = (node, label) => { const p = document.createElement('div'); p.className = 'sg-pair'; const i = document.createElement('i'); i.textContent = label; p.append(node, i); return p; };

const samples = new Map();
const sample = (name, draw) => samples.set(name, draw);

// ---- 对话框 ----
sample('dialog-confirm', async () => [
  pair(await openedDialog(() => confirmAction('把这 3 张图排除出图库？可以在“已排除”里恢复。')), "confirmAction(message)"),
  pair(await openedDialog(() => confirmAction('导出完成。', '导出图库', '知道了', false)), "confirmAction(…, cancel = false)")]);
sample('dialog-choose-work', async () => [await openedDialog(() => chooseWork('选择所属作品', WORKS))]);
sample('dialog-profile', async () => [await openedDialog(() => editProfile(CHARS[0]))]);
sample('dialog-work', async () => [
  pair(await openedDialog(() => workDialog(WORKS[0])), 'workDialog(作品)：编辑，有 tag 时显示关注'),
  pair(await openedDialog(() => workDialog({id: 'w3', name: '某部作品', tag: ''}, {watch: true})), 'workDialog(作品, {watch: true})：关注前先补 tag')]);
sample('dialog-tagged', async () => [await openedDialog(() => openTagged())]);
sample('dialog-index-add', async () => [await openedDialog(() => openIndexAdd())]);
sample('dialog-local-links', async () => [await openedDialog(() => openLocalLinks())]);
sample('dialog-picker', async () => {
  await openPicker({title: '设为角色…', multi: true, items: CHARS, sentence: e => `设为 ${e.name}`});
  stage(CHARS[1]);
  await new Promise(r => setTimeout(r));
  const copy = snapshot($('picker')); closeDialog($('picker')); return [copy];
});
sample('dialog-relate', async () => {
  await openRelate(['u1', 'u2', 'u3']);
  const relate = snapshot($('relateDialog')); closeDialog($('relateDialog'));
  await splitOc(CHARS[3]);
  const split = snapshot($('ocSplitDialog')); closeDialog($('ocSplitDialog'));
  return [pair(relate, 'openRelate(ids) → #relateDialog'), pair(split, 'splitOc(角色) → #ocSplitDialog')];
});
sample('dialog-help', async () => { renderHelpKeys(); return [snapshot($('help'))]; });

// ---- 浮层菜单 ----
sample('pop-view', async () => {
  const out = [];
  for (const [page, label] of [['library', '图库页'], ['home', '首页']]) {
    state.route = {page}; renderViewPop(); out.push(pair(snapshot($('viewPopover')), `renderViewPop()：${label}`));
  }
  state.route = {page: 'library'};
  return out;
});
sample('pop-cast', async () => [fromHTML(castPop([
  {key: 'ba', name: 'Blue Archive', images: 120, chars: [CHARS[0]]}, {key: 'fgo', name: 'Fate/Grand Order', images: 64, chars: [CHARS[2]]},
  {key: 'oc', name: '原创角色', pseudo: true, images: 8, chars: [CHARS[3]]}]))].map(snapshot));

// ---- 任务面板 ----
sample('tasks', async () => {
  taskPanel.open = true; renderTasks(); taskPanel.open = false;
  const side = document.createElement('div'); side.className = 'rail-foot sg-sidebar'; side.append(snapshot($('taskButton')));
  return [pair(side, '#taskButton（.busy）'), snapshot($('taskPanel'))];
});

// ---- toast · 选择栏 ----
sample('toast', async () => {
  toast('已设为 Karen，12 张', true);
  const first = snapshot($('toast'));
  toastMore('。', '关注「Blue Archive」的新角色', () => {});
  return [pair(first, "toast(message, true)"), pair(snapshot($('toast')), 'toastMore(text, label, run)')];
});
sample('selection', async () => {
  const out = [];
  state.route = {page: 'library'}; state.sel = {all: false, ids: new Set(['u1', 'u2', 'u3']), exclude: new Set()}; state.armed = null;
  renderSelection(); out.push(pair(snapshot($('selectionBar')), '图库页，选了 3 张'));
  state.route = {page: 'unmatched'}; state.sel = {all: true, ids: new Set(), exclude: new Set()}; state.armed = 'skip';
  renderSelection(); out.push(pair(snapshot($('selectionBar')), '未识别页，全选 1,200 张，“跳过”等第二次确认（armed）'));
  state.route = {page: 'library'}; state.sel = {all: false, ids: new Set(), exclude: new Set()}; state.armed = null;
  return out;
});

// ---- 提问条 ----
sample('ask', async () => {
  const dark = document.createElement('div'); dark.className = 'sg-dark';
  dark.append(fromHTML(askHTML({name: 'Karen', entity: 'c1', kind: 'audit'}, true)));
  return [pair(fromHTML(askHTML({name: 'Karen', entity: 'c1', kind: 'hint', current_name: 'Rio'})), "askHTML(q)"), pair(dark, "askHTML(q, true)：查看器底部")];
});

// ---- 画 ----
// 表单提交、data-close 在克隆上不该起作用（克隆的 <form method="dialog"> 会关掉舞台里的对话框）。
document.addEventListener('submit', e => { if (e.target.closest('[data-sg-live]')) e.preventDefault(); }, true);

async function draw() {
  seedState();
  await loadShells();
  for (const stageEl of document.querySelectorAll('[data-sg-live]')) {
    const name = stageEl.dataset.sgLive, run = samples.get(name);
    try {
      if (!run) throw new Error(`tools/styleguide/live.js 里没有 sample('${name}')`);
      stageEl.replaceChildren(...await run());
    } catch (e) {
      console.error(`样例 ${name}：`, e);
      stageEl.innerHTML = '<p class="sg-warn"></p>';
      stageEl.firstChild.textContent = `这个样例没画出来：${e.message}。多半是产品代码改了，照报错改 tools/styleguide/live.js。`;
    }
  }
  document.documentElement.dataset.sgDrawn = '1';  // 给走查脚本看：全部样例画完了
}
draw();
