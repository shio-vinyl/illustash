// 画廊的几个动作（#445、#710），列表页、页头、拖图进画廊（ui/drag.js）和选择栏（ui/gallery_bar.js）共用：一次画廊写入、
// 给画廊起名的对话框、新建手选画廊、把一批图放进某个画廊。请求在 core/galleries.js；这里只管提示、撤销和之后重读什么。
import {exclusive} from '../core/edits.js';
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {defaultName, routeFields} from '../core/filters.js';
import {addTo, createManual, forgetCovers, isFav, isManual, loadGalleries, setGallery} from '../core/galleries.js';
import {readError} from '../core/session.js';
import {openDialog} from './dialog.js';
import {error, toast, written} from './dom.js';
import {text, tpl} from './el.js';
import {paintHearts} from './favorites.js';
import {conditionLabel} from './filters.js';
import {loadGalleryCovers} from './rail.js';

// 画廊列表重读一次，活页条画廊那一组的封面跟着重读。
export async function reloadList() {
  try { await loadGalleries(); } catch (e) { error(readError(e)); }
  loadGalleryCovers();
}
// 一次画廊写入：同一时间只进行一项改动；成功后进撤销栈（提示条带「撤销」，text 不给时用后端给这一步的名字），
// 封面和预览作废。失败时提示原因，返回 null。
export async function write(run, text) {
  let result;
  try { result = await exclusive(run); } catch (e) { toast(e.message); return null; }
  if (!result) return null;
  if (result.undo) written(result.undo, text);
  forgetCovers();
  return result;
}

// 起名（新建手选画廊、改名）：一个输入框，回车或确认按钮交回名字，取消交回 null。名字 1–60 个字。
// 对话框的外壳用 openDialog（ui/dialog.js），里面的输入框和按钮由静态模板建，挂进它的表单。
const NAME = tpl`<div><label class="gallery-name"><input name="name" maxlength="60" autocomplete="off" spellcheck="false" required data-ref="input"
  aria-label="画廊名称" data-t-attr="aria-label"></label><div class="dialog-actions"><button value="cancel" class="quiet" formnovalidate data-t>取消</button><button
  value="ok" class="primary" data-ref="ok"></button></div></div>`;
export function askName(title, value = '', ok = t('确定')) {
  return new Promise(resolve => {
    const {el, refs} = NAME();
    const d = openDialog({title, backdropClose: false, dirty: () => refs.input.value.trim() !== value,
      onClose: () => resolve(d.returnValue === 'ok' ? refs.input.value.trim() || null : null)});
    d.querySelector('form').append(...el.childNodes);
    text(refs.ok, ok);
    refs.input.value = value; refs.input.focus(); refs.input.select();
  });
}

// 新建手选画廊，可以顺带放进一批图（selection 同 /api/change：{ids} 或 {filter, exclude, confirm}）。先问名字；返回新画廊或 null。
export async function newManual(selection = {}) {
  const name = await askName(t('新建手选画廊'), t('新画廊'), t('新建'));
  if (!name) return null;
  const result = await write(() => createManual(name, selection));
  if (!result?.gallery) return null;
  setGallery(result.gallery.id, result.gallery);
  reloadList();
  return result.gallery;
}

// 把一批图放进画廊 g（收藏或手选画廊）。n 是张数（提示用）；超过 500 张要 confirm（选择栏「全选」以后）。返回是否成功。
// 放进收藏的图就地点亮心形（同 ui/favorites.js 的写入，不重读列表）。
export async function putInto(g, selection, n) {
  const text = isFav(g.id) ? t('已收藏 {n} 张', {n: number(n)}) : t('已放进「{name}」：{n} 张', {name: g.name, n: number(n)});
  const result = await write(() => addTo(g.id, {...selection, ...(n > 500 ? {confirm: true} : {})}), text);
  if (!result) return false;
  if (isFav(g.id) && selection.ids) paintHearts(new Set(selection.ids), true);
  if (result.gallery) setGallery(g.id, result.gallery);
  loadGalleryCovers();
  return true;
}

// 副标题里「这是哪种画廊」：收藏不写，手选画廊写「手选」，按条件的写「按条件：条件的文字」（页头和杂志的导语用）。
export function kindText(g) {
  if (isFav(g.id)) return '';
  if (isManual(g)) return t('手选');
  const r = routeFields(g.params), what = defaultName(r, conditionLabel);
  return what && what !== defaultName({}, conditionLabel) ? t('按条件：{what}', {what}) : t('按条件');
}
