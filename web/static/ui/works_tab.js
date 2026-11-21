// 资料库 · 作品页签（#711，原来的作品树 #198）：作品卡（ui/work_card.js）按作品树的顺序排（父作品下深度优先、大的在前），
// 上面一行工具（搜索、全部 / 已关注 / 待确认、关注 Danbooru 上的作品…、新建作品），需要确认的事收成卡片上面的一处提示：
// 链接待确认的作品（点了只看它们）、以前关注的 Danbooru 作品还没对应到本地作品、上次检查发现的还没收录的角色。
// 原来每行橙色的「链接待确认」去掉了。
//
//   const view = worksView(s, {open});   // 进页面时建一次；open 见 ui/work_card.js
//   view.tools                           // 工具栏上的节点（页面放进 #toolbar）
//   view.update();                       // 数据（state.index、角色目录、作品树）或条件变了
import {castWorks, shelfRows, WORK_NAMES, workCovers} from '../core/catalog.js';
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {onTyped} from '../core/keys.js';
import {state} from '../core/state.js';
import {adoptTag, createWork, openIndexAdd, searchField, watchAddField} from './catalog.js';
import {attr, make, refill, rich, show, text, tpl, value} from './el.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';
import {workCards} from './work_card.js';

// 搜索词和筛选（all、watched、pending），离开再回来还在。
export const worksFilter = {q: '', filter: 'all'};

const VIEW = tpl`<div class="cat-works-tab"><div class="cat-notice" data-ref="notice"><p data-ref="pending"><span data-ref="pendingText"></span><button type="button"
  class="btn is-small" data-ref="pendingShow" data-t>只看这些</button></p><details data-ref="todo"><summary data-ref="todoText"></summary><div data-ref="todoRows"></div></details><p
  data-ref="found"><span data-ref="foundText"></span><button type="button" class="btn is-primary is-small" data-ref="foundAdd" data-t>查看并添加…</button></p></div><p
  class="cat-loading" data-ref="loading" data-t>正在读取作品…</p><div class="cat-works" data-ref="cards"></div><div data-ref="empty" hidden></div></div>`;
const TOOLS = tpl`<div class="cat-tools"><span class="cat-tools-group"><span data-ref="q"></span><span class="cat-filters"
  data-ref="filters"></span></span><span class="spacer"></span><span data-ref="watch"></span><button type="button" class="btn" data-ref="create" data-t>新建作品</button></div>`;

export function worksView(s, {open}) {
  const {el, refs} = VIEW(), tools = TOOLS(), watch = watchAddField(), search = searchField(t('搜索作品'));
  tools.refs.watch.replaceWith(watch.el); tools.refs.q.replaceWith(search.el);
  const cards = keyed(refs.cards, {key: x => x.id, create: workCards(s, {open}), scope: s});
  onTyped(search.input, () => { worksFilter.q = search.input.value; update(); });
  s.on(tools.refs.create, 'click', createWork);
  s.on(tools.refs.filters, 'click', e => { const b = e.target.closest('[data-filter]'); if (b) { worksFilter.filter = b.dataset.filter; update(); } });
  s.on(refs.pendingShow, 'click', () => { worksFilter.filter = 'pending'; update(); });
  s.on(refs.foundAdd, 'click', () => openIndexAdd(''));
  s.on(refs.todoRows, 'click', e => { const b = e.target.closest('[data-tag]'); if (b) adoptTag(b.dataset.tag); });

  function notice(x, pendingN) {
    const old = x.watch.pending_tags || [], missing = x.check?.missing || [];
    show(refs.pending, pendingN > 0 && worksFilter.filter !== 'pending');
    text(refs.pendingText, t('{n} 部作品的 Danbooru 链接待确认：核对过才能关注它们的新角色', {n: number(pendingN)}));
    show(refs.todo, old.length > 0);
    text(refs.todoText, t('{n} 个以前关注的 Danbooru 作品还没对应到本地作品，对应后才会检查它们的新角色', {n: number(old.length)}));
    refill(refs.todoRows, old.join('|'), () => old.map(tag => {
      const row = make('div', 'cat-todo-row');
      const b = make('button', 'btn is-small', t('对应到作品…')); b.type = 'button'; b.dataset.tag = tag;
      row.append(make('span', 'cat-mono', tag), make('span', 'cat-muted', WORK_NAMES[tag] || ''), make('span', 'spacer'), b);
      return row;
    }));
    show(refs.found, missing.length > 0);
    refill(refs.foundText, String(missing.length), () => [rich(t('上次检查发现 {n} 个还没收录的角色', {n: '{n}'}), {n: make('b', '', number(missing.length))})]);
    show(refs.notice, (pendingN > 0 && worksFilter.filter !== 'pending') || old.length > 0 || missing.length > 0);
  }
  function update() {
    const x = state.index;
    value(search.input, worksFilter.q);
    show(refs.loading, !x?.works);
    if (!x?.works) { show(refs.notice, false); cards.set([]); show(refs.empty, false); return; }
    watch.update(x);
    const {rows, watched, pending} = shelfRows(x, worksFilter);
    const filters = [['all', t('全部'), x.works.length], ['watched', t('已关注'), watched.size], ...(pending || worksFilter.filter === 'pending' ? [['pending', t('链接待确认'), pending]] : [])];
    refill(tools.refs.filters, filters.map(f => f.join(':')).join('|') + worksFilter.filter, () => filters.map(([k, label, n]) => {
      const b = make('button', 'chip', label); b.type = 'button'; b.dataset.filter = k; attr(b, 'aria-pressed', String(worksFilter.filter === k));
      if (n) b.append(make('span', 'chip-count', number(n)));
      return b;
    }));
    notice(x, pending);
    const groups = new Map(castWorks().map(w => [String(w.key), w]));
    cards.set(rows.map(([w]) => ({id: w.id, name: w.name, count: w.count, tag: w.tag, watchable: w.watchable, status: w.status,
      chars: groups.get(String(w.id))?.chars || [], covers: workCovers(w.id), watched: watched.has(w.id)})));
    show(refs.empty, !rows.length);
    if (!rows.length) drawEmpty(refs.empty, x.works.length ? {icon: 'search', fact: t('没有符合条件的作品。')} : {icon: 'gallery', fact: t('还没有作品。')});
  }
  return {el, tools: tools.el, update};
}
