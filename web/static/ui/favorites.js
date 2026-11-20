// 收藏（#447）：卡片、详情栏、大图和首页上的心形，以及不重读列表的那条写入路径。什么时候收藏、取消，由 ui/grid.js 的
// favorite 决定（选区、F 键、要不要让图离开本页）；判断和本地改值在 core/favorites.js。
// 每个心形都带 data-fav="uuid"，写入成功后 paintHearts 按 uuid 就地改掉，不重画卡片和详情栏。
import {exclusive} from '../core/edits.js';
import {esc} from '../core/format.js';
import {t} from '../core/i18n.js';
import {post} from '../core/session.js';
import {toast, written} from './dom.js';
import {tpl} from './el.js';

export const HEART = '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 16.4S3.4 12.5 3.4 7.9A3.3 3.3 0 0 1 10 6.3a3.3 3.3 0 0 1 6.6 1.6c0 4.6-6.6 8.5-6.6 8.5Z"/></svg>';
// 同一个心形的模板，给部件写法的界面克隆用：网格卡片（ui/grid_card.js）的心形用它，收藏过的一直显示，没收藏的悬停时出现。
export const HEART_ICON = tpl`<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 16.4S3.4 12.5 3.4 7.9A3.3 3.3 0 0 1 10 6.3a3.3 3.3 0 0 1 6.6 1.6c0 4.6-6.6 8.5-6.6 8.5Z"/></svg>`;
// 心形的提示：按下去会做什么。
export const heartTip = on => on ? t('取消收藏（F）') : t('收藏（F）');
const tip = heartTip;
const pressed = (on, label) => `aria-pressed="${on}" title="${tip(on)}" aria-label="${label}"`;

// 只显示、不能点的实心心形；取消收藏后就地藏起来（app.css 的 .fav-mark.still）。
const still = x => `<span class="fav-mark on still" data-fav="${esc(x.uuid)}" aria-hidden="true">${HEART}</span>`;
// 首页缩略图：只显示收藏过的，不能点（首页点图是打开详情）。
export const homeHeart = x => x.fav ? still(x) : '';
// 详情栏顶部按钮行里的切换按钮。
export const detailHeart = (uuid, on) => `<button class="icon fav-toggle${on ? ' on' : ''}" data-fav="${esc(uuid)}" ${pressed(on, t('收藏', {context: 'verb'}))}>${HEART}</button>`;
// 大图底栏的动作：「F 收藏」，已收藏时是「F 已收藏」。
const actLabel = on => on ? t('已收藏') : t('收藏', {context: 'verb'});
export const viewerHeart = (uuid, on) => `<button class="alt fav-act${on ? ' on' : ''}" data-vact="fav" data-fav="${esc(uuid)}" aria-pressed="${on}" title="${tip(on)}">`
  + `<kbd>F</kbd><span class="fav-label">${actLabel(on)}</span></button>`;

// 写入之后把页面上这些图的心形都改成 on（卡片、详情栏、大图底栏、首页）。
export function paintHearts(uuids, on, root = document) {
  for (const el of root.querySelectorAll('[data-fav]')) {
    if (!uuids.has(el.dataset.fav)) continue;
    el.classList.toggle('on', on); el.setAttribute('aria-pressed', on); el.title = tip(on);
    const label = el.querySelector('.fav-label'); if (label) label.textContent = actLabel(on);
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
