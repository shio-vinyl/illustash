// 选择栏上画廊的两个按钮（#710）：「放进画廊…」把选中的图放进收藏或某个手选画廊（也可以新建一个），手选画廊里「从画廊拿出」。
// 每一步都进撤销栈。ui/grid.js 画选择栏时调这里：
//   galleryButtons(r)                          默认那一种选择栏（图库、角色页、画廊）末尾的按钮，HTML 字符串
//   galleryAct(act, payload, n, {mutate, openPicker})  点了按钮：处理了返回 true。payload() 是选区（同 /api/change），n 是张数；
//                                              mutate、openPicker 由 grid.js 交进来（这个文件不导入 grid.js、palette.js，免得循环导入）
// 收藏页上不另加「拿出」：选择栏原有的收藏按钮就是取消收藏。
import {t} from '../core/i18n.js';
import {droppable, forgetCovers, galleryOf, isFav, isManual, withFav} from '../core/galleries.js';
import {esc, number} from '../core/format.js';
import {pageOf} from '../core/pages.js';
import {state} from '../core/state.js';
import {newManual, putInto} from './gallery_actions.js';
import {loadGalleryCovers} from './rail.js';

// 现在在哪个手选画廊里（收藏不算）；不在时是 null。
const manualHere = r => { const g = pageOf(r).gallery ? galleryOf(r.id) : null; return g && isManual(g) && !isFav(g.id) ? g : null; };

export function galleryButtons(r) {
  const here = manualHere(r);
  return `<button class="quiet" data-act="gallery-add" title="${t('放进收藏或一个手选画廊')}">${t('放进画廊…')}</button>`
    + (here ? `<button class="quiet" data-act="gallery-remove" title="${t('从「{name}」拿出，图本身不删', {name: esc(here.name)})}">${t('从画廊拿出')}</button>` : '');
}

export function galleryAct(act, payload, n, {mutate, openPicker}) {
  if (act === 'gallery-add') {
    const here = String(state.route.id || '');
    const items = withFav().filter(g => droppable(g) && !(pageOf(state.route).gallery && String(g.id) === here))
      .map(g => ({id: String(g.id), name: g.name, hint: isFav(g.id) ? t('收藏') : t('手选画廊')}));
    items.push({id: 'new', name: t('新建手选画廊…'), hint: t('起个名字，放进这 {n} 张', {n: number(n)})});
    openPicker({title: t('放进画廊'), items, placeholder: t('筛选画廊'),
      sentence: e => e.id === 'new' ? t('新建手选画廊，放进 {n} 张', {n: number(n)}) : t('把 {n} 张放进「{name}」', {n: number(n), name: e.name}),
      pick: e => e.id === 'new' ? newManual(payload()) : putInto(galleryOf(e.id), payload(), n)});
    return true;
  }
  if (act === 'gallery-remove') {
    const g = manualHere(state.route);
    // 拿出以后图离开这一页：照 grid.js 的 mutate 淡出、重读，撤销在提示条上。
    if (g) Promise.resolve(mutate('/api/galleries', {action: 'remove', id: Number(g.id), ...payload()}, true)).then(() => { forgetCovers(); loadGalleryCovers(); });
    return true;
  }
  return false;
}
