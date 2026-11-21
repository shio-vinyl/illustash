// 选择栏上画廊的两个按钮（#710）：「放进画廊…」把选中的图放进收藏或某个手选画廊（也可以新建一个），手选画廊里「从画廊拿出」。
// 每一步都进撤销栈。ui/grid_bar.js 画默认那一种选择栏（图库、角色页、画廊）时调这里：
//   galleryButtons(r)            按钮的说明 [{act, label, tip}]，加在末尾；整理页（重复、相似组）上没有
//   galleryAction(act, n, payload)  是这里的按钮就做、返回 true；payload() 是选区（同 /api/change，超过 500 张时带 confirm），n 是张数
// 收藏页上不另加「拿出」：选择栏原有的收藏按钮就是取消收藏。
import {t} from '../core/i18n.js';
import {droppable, galleryOf, isFav, isManual, removeBody, withFav} from '../core/galleries.js';
import {number} from '../core/format.js';
import {pageOf} from '../core/pages.js';
import {state} from '../core/state.js';
import {newManual, putInto, setFavorites} from './gallery_actions.js';
import {favorite, mutate} from './grid.js';
import {openPicker} from './palette.js';

// 放进收藏交给网格原有的收藏写入（见 ui/gallery_actions.js 的 setFavorites）。
setFavorites(selection => favorite(selection, true));

// 现在在哪个手选画廊里（收藏不算）；不在时是 null。
const manualHere = r => { const g = pageOf(r).gallery ? galleryOf(r.id) : null; return g && isManual(g) && !isFav(g.id) ? g : null; };

export function galleryButtons(r) {
  if (pageOf(r).entry === 'tidy') return [];
  const out = [{act: 'gallery-add', label: t('放进画廊…'), tip: t('放进收藏或一个手选画廊')}];
  const here = manualHere(r);
  if (here) out.push({act: 'gallery-remove', label: t('从画廊拿出'), tip: t('从「{name}」拿出，图本身不删', {name: here.name})});
  return out;
}
export function galleryAction(act, n, payload) {
  if (act === 'gallery-add') { pick(n, payload); return true; }
  if (act !== 'gallery-remove') return false;
  const here = manualHere(state.route);
  // 拿出以后图离开这一页：照 grid.js 的 mutate 淡出、重读，撤销在提示条上；封面由 ui/gallery_save.js 在写入之后重读。
  if (here) mutate('/api/galleries', removeBody(here.id, payload()), true);
  return true;
}

// 「放进画廊…」：命令面板里挑收藏、手选画廊（不列这一页自己），或新建一个。
function pick(n, payload) {
  const here = pageOf(state.route).gallery ? String(state.route.id) : '';
  const items = withFav().filter(g => droppable(g) && String(g.id) !== here)
    .map(g => ({id: String(g.id), name: g.name, hint: isFav(g.id) ? t('收藏') : t('手选画廊')}));
  items.push({id: 'new', name: t('新建手选画廊…'), hint: t('起个名字，放进这 {n} 张', {n: number(n)})});
  openPicker({title: t('放进画廊'), items, placeholder: t('筛选画廊'),
    sentence: e => e.id === 'new' ? t('新建手选画廊，放进 {n} 张', {n: number(n)}) : t('把 {n} 张放进「{name}」', {n: number(n), name: e.name}),
    pick: e => e.id === 'new' ? newManual(payload()) : putInto(galleryOf(e.id), payload(), n)});
}
