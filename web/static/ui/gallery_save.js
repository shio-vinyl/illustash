// 工具栏上的「保存为画廊」（#445）。框架（ui/shell.js）画工具栏时用它，所以单独一个文件：画廊页本身（ui/galleries.js）要导入
// shell，放在一起会循环导入。
import {t} from '../core/i18n.js';
import {defaultName, savable, savedParams} from '../core/filters.js';
import {createGallery, galleryHref, setGallery} from '../core/galleries.js';
import {pageOf} from '../core/pages.js';
import {$} from './dom.js';
import {conditionLabel} from './filters.js';
import {askName, reloadList, write} from './gallery_actions.js';

// 在图库和角色页上有条件（含搜索词）时出现；画廊页上换成页头的「更新画廊」「另存为新画廊」。「更多像这张的」页上不出现：
// 画廊存不下例图，存出来的和屏幕上看到的不一样（#646，用户 2026-11-18 选 A）。
export const saveButton = r => savable(r) && !pageOf(r).gallery && !r.like
  ? `<button class="quiet" id="saveGallery" title="${t('把现在的条件存成画廊')}">${t('保存为画廊')}</button>` : '';
export function wireSaveButton(r) { $('saveGallery')?.addEventListener('click', () => saveNew(r)); }
// 存成新画廊：先问名字（按条件的文字生成好，回车就用它），存好后进入这个画廊。
export async function saveNew(r) {
  const name = await askName(t('保存为画廊'), defaultName(r, conditionLabel), t('保存'));
  if (!name) return;
  const result = await write(() => createGallery(savedParams(r), name));
  if (!result?.gallery) return;
  setGallery(result.gallery.id, result.gallery);
  location.hash = galleryHref(result.gallery.id);
  reloadList();
}
