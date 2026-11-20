// 未识别 · 按组整理（#713，#/groups）和一组图的页面（#/g/<id>，网格页）进入前的读取、空了时的「下一组」。
// 数据在 core/tidy.js，分组的样子在 ui/tidy_group.js。
//
// CCIP 即使叫不出名字（45%），也能可靠地把同一个人的图放在一起（99%）。大多数代表图意见一致的组整组问「是 TA 吗？」；
// 其余的组给出「设为…」/「原创角色」/「跳过」。点一张图，把它排除在这组的回答之外；长按或 Space 看大图。
// 回答过的组原地收起（ui/tidy_group.js 的 collapse），活页条上「未识别」的数字跟着滚下去；不重新加载，页面不跳。
// 组按 cid 复用（ui/keyed.js）：重读、撤销以后没变的组留在原处，图不重新加载。
import {number, stamp} from '../core/format.js';
import {t} from '../core/i18n.js';
import {implement, onPage} from '../core/pages.js';
import {charHash} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {groupOrder, loadClusters, nearText, nextGroupId, regroup} from '../core/tidy.js';
import {$, error, toast} from './dom.js';
import {attr, cls, make, rich, show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';
import {mutate} from './grid.js';
import {markOc, openPicker, targets} from './palette.js';
import {count} from './rail.js';
import {collapse, minus, setPeekList, thumbStrip, tidyGroup} from './tidy_group.js';

const PAGE = 40;
const VIEW = tpl`<div class="tidy-page">
  <div class="tidy-bar"><span class="tidy-status" data-ref="status"></span>
    <a class="btn is-quiet is-small" data-ref="rest" href="#/unmatched"></a>
    <button type="button" class="btn is-quiet is-small" data-ref="regroup" title="用当前的未识别图重新分组" data-t data-t-attr="title">重新分组</button></div>
  <div class="tidy-list" data-ref="list"></div>
  <div class="tidy-more" data-ref="moreBox" hidden><button type="button" class="btn" data-ref="more"></button></div>
  <div data-ref="empty" hidden></div>
</div>`;
const ACTS = tpl`<span class="tidy-acts-in">
  <button type="button" class="btn is-primary is-small" data-gact="yes" data-ref="yes"></button>
  <button type="button" class="btn is-small" data-gact="no" data-ref="no" data-t>都不是</button>
  <button type="button" class="btn is-small" data-gact="assign" data-ref="assign"></button>
  <button type="button" class="btn is-quiet is-small" data-gact="oc" title="把这组图都设为「原创角色（未细分）」" data-t data-t-attr="title">原创角色</button>
  <button type="button" class="btn is-quiet is-small" data-gact="skip" data-t>跳过</button>
  <a class="btn is-quiet is-small" data-ref="open" data-t>逐张看</a>
</span>`;

// 一组：组头「第 N 组 · M 张 · 特征」和问题，右边是整组的操作；下面一排方图。
function groupCard(view, g) {
  const s = view.scope.child('group'), card = tidyGroup(), acts = ACTS(), r = card.refs, a = acts.refs;
  r.acts.append(acts.el);
  const strip = thumbStrip(r.strip, {scope: s});
  let current = g;
  setPeekList(card.el, () => current.members);
  const update = x => {
    current = x;
    const off = view.off.get(x.cid) || new Set(), n = x.members.length - off.size;
    attr(card.el, 'data-cid', String(x.cid));
    text(r.title, t('第 {id} 组', {id: x.cid}));
    text(r.meta, [t('{n} 张', {n: number(x.size)}), ...(x.traits || []).slice(0, 2)].join(' · '));
    if (x.lead) {
      const link = make('a', '', x.lead.name); link.href = charHash(x.lead.id);
      r.q.replaceChildren(rich(t('是 {name} 吗？'), {name: link}), make('small', '', t('{n} 张的线索都指向 TA', {n: number(x.lead.count)})));
    } else r.q.replaceChildren(make('span', 'tidy-unknown', t('还不知道是谁')));
    show(a.yes, !!x.lead); show(a.no, !!x.lead);
    text(a.yes, (off.size ? t('其余 {n} 张是', {n: number(n)}) : t('都是')) + ' ✓');
    attr(a.yes, 'disabled', !n);
    text(a.assign, x.lead ? t('是别人…') : t('设为…'));
    cls(a.assign, 'is-primary', !x.lead); cls(a.assign, 'is-quiet', !!x.lead);
    attr(a.assign, 'disabled', !x.lead && !n);
    attr(a.open, 'href', '#/g/' + x.cid);
    strip.update(x.members, {off, rest: '#/g/' + x.cid});
    show(r.note, !!x.near); text(r.note, x.near ? nearText(x) : '');
  };
  update(g);
  return {el: card.el, update, dispose: () => s.dispose()};
}

function groupsView(scope) {
  const {el, refs} = VIEW();
  $('groupsPage').replaceChildren(el);
  const view = {scope, list: [], learner: null, shown: PAGE, off: new Map(), loaded: false};
  const list = keyed(refs.list, {key: g => g.cid, create: g => groupCard(view, g), exit: collapse, scope});
  function draw() {
    const all = view.list, images = all.reduce((n, g) => n + g.size, 0), named = all.filter(g => g.lead).length;
    const busy = ['learning', 'grouping', 'waiting'].includes(view.learner?.status), rest = Math.max(0, (state.pools.unmatched || 0) - images);
    text(refs.status, [t('{n} 组 · 共 {images} 张', {n: number(all.length), images: number(images)}),
      named ? t('{n} 组能直接确认是谁', {n: number(named)}) : '', all[0] ? t('分组于 {time}', {time: stamp(all[0].built_at)}) : '',
      busy ? view.learner.status === 'waiting' ? view.learner.message || '' : t('正在学习…') : ''].filter(Boolean).join(' · '));
    show(refs.rest, !!rest); text(refs.rest, t('没成组的 {n} 张', {n: number(rest)}) + ' →');
    list.set(all.slice(0, view.shown));
    show(refs.moreBox, all.length > view.shown);
    text(refs.more, t('再显示 {n} 组', {n: number(Math.min(PAGE, all.length - view.shown))}));
    show(refs.empty, view.loaded && !all.length);
    if (view.loaded && !all.length) drawEmpty(refs.empty, {icon: 'check', fact: state.pools.unmatched ? t('没有成组的未识别图。') : t('没有未识别的图。'),
      next: state.pools.unmatched ? [{label: t('全部未识别'), href: '#/unmatched'}] : [{label: t('去图库'), href: '#/library'}]});
  }
  // keep：重读以后留在已显示的组数和排除的图（撤销、学习落定、大图里改了图）；不给是刚进来或换了 NSFW，从前 40 组开始。
  async function load(keep = false) {
    const route = state.route, nsfw = state.nsfw;
    let result;
    try { result = await loadClusters(nsfw, scope.signal); } catch (e) { if (scope.alive && state.route === route && nsfw === state.nsfw) error(readError(e)); return; }
    if (!scope.alive || state.route !== route || nsfw !== state.nsfw) return;
    view.list = groupOrder(result.clusters); view.learner = result.learner; view.loaded = true;
    if (!keep) { view.shown = PAGE; view.off = new Map(); }
    draw();
  }
  // 这组答完了：从列表里拿掉（收起），活页条的数字按这组的张数先减。
  function done(g, ids) {
    view.list = view.list.filter(x => x !== g); view.off.delete(g.cid);
    count('unmatched', minus(state.pools.unmatched, ids.length));
    draw();
  }
  async function act(g, what) {
    const off = view.off.get(g.cid) || new Set(), ids = g.members.filter(u => !off.has(u)), n = ids.length;
    const finish = ok => { if (ok && scope.alive) done(g, ids); };
    if (what === 'yes') finish(await mutate('/api/change', {ids, action: 'confirmed', entity: g.lead.id}, false));
    else if (what === 'no') { if (await mutate('/api/change', {ids, action: 'rejected', entity: g.lead.id}, false)) load(true); }
    else if (what === 'skip') finish(await mutate('/api/change', {ids, action: 'unknown'}, false));
    else if (what === 'oc') finish(await markOc({ids}, false));
    // 面板顶上写「第 N 组 · M 张」，和缩略图一起（#424 的 2-3）；能多选，和选择栏的「设为…」一样（2-24）。
    else if (what === 'assign') openPicker({title: t('这 {n} 张是…', {n: number(n)}), subjectText: t('第 {id} 组', {id: g.cid}) + ' · ' + t('{n} 张', {n: number(n)}),
      sentence: x => t('把这 {n} 张设为 {name}', {n: number(n), name: x.name}), suggest: {ids}, exclude: g.lead ? [g.lead.id] : [], multi: true,
      pick: async picked => finish(await mutate('/api/change', {ids, action: 'confirmed', ...targets(picked)}, false))});
  }
  scope.on(refs.list, 'click', e => {
    const sec = e.target.closest('[data-cid]'), g = sec && view.list.find(x => String(x.cid) === sec.dataset.cid); if (!g) return;
    const img = e.target.closest('[data-gimg]');
    if (img) {
      const off = view.off.get(g.cid) || new Set(), u = img.dataset.gimg;
      if (off.has(u)) off.delete(u); else off.add(u);
      view.off.set(g.cid, off); draw(); return;
    }
    const what = e.target.closest('[data-gact]')?.dataset.gact;
    if (what) act(g, what);
  });
  scope.on(refs.more, 'click', () => { view.shown += PAGE; draw(); });
  scope.on(refs.regroup, 'click', async () => {
    try { await regroup(); toast(t('正在重新分组…')); scope.timeout(() => load(), 3000); } catch (e) { toast(e.message); }
  });
  return {scope, load};
}

let view = null;
// 大图关掉时把最后看的那张缩略图滚进视野。
function revealThumb(uuid) { [...$('groupsPage').querySelectorAll('[data-peek]')].find(x => x.dataset.peek === uuid)?.scrollIntoView({block: 'nearest'}); }

// 「下一组」：重读分组，取原来排在这组后面、还在的第一组；后面都没了就取最前面那组；一组都没了回到按组整理。
async function nextGroup() {
  let result; try { result = await loadClusters(state.nsfw); } catch (e) { error(readError(e)); return; }
  const pick = nextGroupId(state.groupOrder, state.route.id, result.clusters);
  location.hash = pick ? '#/g/' + encodeURIComponent(pick) : '#/groups';
}

implement({
  // 在这页开的大图里改了图（'edit' 时大图还开着）：重读分组，免得组里还留着已经设了角色、跳过或排除的图。组头按钮的写入
  // 不走这里，它们自己把答过的组收起。
  groups: {render: ({samePage, scope}) => { if (!samePage || !view?.scope.alive) view = groupsView(scope); return view.load(); }, reveal: revealThumb,
    refresh: why => { if (why === 'undo' || why === 'learned' || why === 'edit' && state.viewer >= 0) return view?.load(true); if (why === 'nsfw') return view?.load(); }},
  // 一组图的页面（#/g/<id>，网格页）要先读分组：标题里相近的一组和这组的特征，以及空了时「下一组」的先后。
  group: {prepare: async route => {
    let result; try { result = await loadClusters(state.nsfw); } catch (e) { if (state.route === route) error(readError(e)); return false; }
    if (state.route !== route) return false;  // 分组加载期间已经导航走了：这个回应不属于任何页面
    const g = result.clusters.find(x => String(x.cid) === route.id);
    state.groupNear = g?.near ? nearText(g) : null; state.groupNearId = g?.near || null; state.groupTraits = g?.traits || [];
    state.groupOrder = groupOrder(result.clusters).map(x => String(x.cid));
  },
  // 一组整理完了（#386 的 2-12）：去下一组，或回到按组整理。
  emptyNext: r => [...(state.groupOrder.some(id => id !== r.id) ? [{label: t('下一组') + ' →', act: 'next-group'}] : []), {label: t('回到按组整理'), href: '#/groups'}],
  emptyAct: act => { if (act === 'next-group') nextGroup(); }},
});
