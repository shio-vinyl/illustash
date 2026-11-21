// 收藏（#447）：卡片、详情卡片、大图和按天视图上的心形，以及不重读列表的那条写入路径。什么时候收藏、取消，由 ui/grid.js 的
// favorite 决定（选区、F 键、要不要让图离开本页）；判断和本地改值在 core/favorites.js。
// 每个心形都带 data-fav="uuid"，写入成功后 paintHearts 按 uuid 就地改掉，不重画卡片和详情栏。
import {exclusive} from '../core/edits.js';
import {t} from '../core/i18n.js';
import {post} from '../core/session.js';
import {toast, written} from './dom.js';
import {attr, cls, text} from './el.js';

// 心形的提示：按下去会做什么。
export const heartTip = on => on ? t('取消收藏（F）') : t('收藏（F）');

// 大图底栏「收藏」按钮上的字（按钮在 ui/viewer.js 建）：已收藏时是「已收藏」。
const actLabel = on => on ? t('已收藏') : t('收藏', {context: 'verb'});

// 写入之后把页面上这些图的心形都改成 on（网格卡片、按天视图的 .day-heart，凡是带 data-fav 的）。经 ui/el.js 的写入函数写，
// 它们记着的上一次的值跟着变，部件之后按自己的数据 update 时照常判断变没变。
export function paintHearts(uuids, on, root = document) {
  for (const el of root.querySelectorAll('[data-fav]')) {
    if (!uuids.has(el.dataset.fav)) continue;
    cls(el, 'on', on); attr(el, 'aria-pressed', String(on)); attr(el, 'title', heartTip(on));
    const label = el.querySelector('.fav-label'); if (label) text(label, actLabel(on));
  }
}

// 不让图离开本页的写入：发给 /api/favorite，成功后提示条写明做了什么并带「撤销」，done() 在本地改掉心形。
// 和 ui/grid.js 的 mutate 一样同一时间只进行一项改动；失败时提示原因，返回 false。
export function writeFavorite(body, done) {
  return exclusive(async () => {
    document.body.classList.add('busy');
    try {
      const r = await post('/api/favorite', body);
      if (r.undo) written(r.undo);
      done();
      return true;
    } catch (e) { toast(e.message); return false; } finally { document.body.classList.remove('busy'); }
  });
}
