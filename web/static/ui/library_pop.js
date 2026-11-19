// 活页条顶上的标志（#706）：点开是一个小浮层，写着当前图库，列出这台电脑上的其他图库，底下是「打开其他图库…」和图库设置。
// 打开、升级、抢救一个图库的流程照旧在 ui/settings.js（openLibrary）；这里只是入口。列表来自 GET /api/libraries（core/settings.js）。
// 单独一个模块：ui/rail.js 不导入 ui/settings.js（settings → grid → shell → rail 会成环）。
import {home} from '../core/format.js';
import {t} from '../core/i18n.js';
import {libraryRequest, loadLibraries} from '../core/settings.js';
import {state} from '../core/state.js';
import {attr, show, text, tpl} from './el.js';
import {app} from './scope.js';
import {createLayer} from './overlay.js';
import {openLibrary} from './settings.js';
import {$, toast} from './dom.js';

const POP = tpl`<div class="library-pop-in"><div class="library-now"><small data-t>当前图库</small><b data-ref="name"></b><span data-ref="path"></span></div>
  <div class="library-list" data-ref="list"><small data-t>其他图库</small></div>
  <div class="library-acts"><button class="btn is-small" data-ref="choose" data-t>打开其他图库…</button><a class="btn is-small is-quiet" href="#/settings/library" data-ref="settings"
  data-t>图库设置</a></div></div>`;
const ROW = tpl`<button class="library-row"><b data-ref="name"></b><span data-ref="path"></span></button>`;

const s = app.child('library-pop');
const layer = createLayer({kind: 'popover', id: 'library-pop', el: () => $('libraryPop'), anchor: () => $('brand'), menu: true, focusIn: 'first',
  onDismiss: reason => setOpen(false, reason)});
let body = null;
// 各行按图库路径留着：打开时先按已有的列表画，/api/libraries 读回来再画一次。只增删、更新文字，顺序没变的行不挪，
// 有焦点的那一行（打开时焦点进第一项）不会被删掉重建、把焦点丢到 body 上。
const rows = new Map();

function draw() {
  const data = state.libraries, cur = data?.info, list = (data?.libraries || []).filter(l => l.path !== data?.current);
  body ||= POP();
  const {refs} = body;
  text(refs.name, cur?.name || t('图库')); text(refs.path, home(data?.current || '')); show(refs.path, !!data?.current);
  const keep = new Set(list.map(l => l.path));
  for (const [path, row] of rows) if (!keep.has(path)) { row.scope.dispose(); row.el.remove(); rows.delete(path); }
  let prev = refs.list.firstElementChild;   // 「其他图库」那行小字
  for (const l of list) {
    let row = rows.get(l.path);
    if (!row) {
      const {el, refs: r} = ROW(), scope = s.child('row');
      row = {el, refs: r, scope, library: l};
      scope.on(el, 'click', () => { setOpen(false); openLibrary(row.library.path, row.library.name); });
      rows.set(l.path, row);
    }
    row.library = l;
    text(row.refs.name, l.name); text(row.refs.path, home(l.path)); attr(row.el, 'title', l.path);
    if (prev.nextElementSibling !== row.el) refs.list.insertBefore(row.el, prev.nextElementSibling);   // 只挪位置不对的（挪动有焦点的节点会丢焦点）
    prev = row.el;
  }
  show(refs.list, list.length > 0);
  if (!$('libraryPop').contains(body.el)) $('libraryPop').replaceChildren(body.el);
}
function setOpen(open, reason) {
  $('libraryPop').hidden = !open; attr($('brand'), 'aria-expanded', String(open));
  if (!open) { layer.close(reason); return; }
  draw(); layer.open();
  loadLibraries().then(() => { if (!$('libraryPop').hidden) draw(); }, () => {});   // 读不到就照已有的列表画
}
async function choose() {
  setOpen(false);
  let r;
  try { r = await libraryRequest('choose', {kind: 'library'}); } catch (e) { toast(e.message); return; }
  if (r?.path) openLibrary(r.path, r.path.split('/').pop().replace(/\.pilib$/, ''));
}

// 页面启动时调用一次。
export function wireLibraryPop() {
  s.on($('brand'), 'click', () => setOpen($('libraryPop').hidden));
  body = POP();
  s.on(body.refs.choose, 'click', choose);
  s.on(body.refs.settings, 'click', () => setOpen(false, 'silent'));
}
