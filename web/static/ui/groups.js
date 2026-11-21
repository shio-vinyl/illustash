// 未识别 · 按组整理（#713，#/groups）和一组图的页面（#/g/<id>，网格页）进入前的读取、空了时的「下一组」。
// 数据在 core/tidy.js，分组的样子在 ui/tidy_group.js。
//
// CCIP 即使叫不出名字（45%），也能可靠地把同一个人的图放在一起（99%）。大多数代表图意见一致的组整组问「是 TA 吗？」；
// 其余的组给出「设为…」/「原创角色」/「跳过」。点一张图，把它排除在这组的回答之外；长按或 Space 看大图。点掉的图是别的已有角色时
// 用「点掉的是…」直接设成那个角色（#787），这组留着、少了那几张。组卡是共用的判断卡（ui/judge.js）。
// 回答过的组原地收起（ui/tidy_group.js 的 collapse），活页条上「未识别」的数字跟着滚下去；不重新加载，页面不跳。
// 组按 cid 复用（ui/keyed.js）：重读、撤销以后没变的组留在原处，图不重新加载。同一路由上的几次重读只认最后发出的那次。
import {number, stamp} from '../core/format.js';
import {t} from '../core/i18n.js';
import {implement, onPage} from '../core/pages.js';
import {charHash} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {groupOrder, loadClusters, minus, nearText, nextGroupId, regroup} from '../core/tidy.js';
import {$, error, toast} from './dom.js';
import {attr, make, refill, rich, show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';
import {mutate} from './grid.js';
import {markOc, openPicker, targets} from './palette.js';
import {count} from './rail.js';
import {pickFor} from './answer.js';
import {judgeCard} from './judge.js';
import {clearFooter, collapse, drawFailure, leaving, revealThumb, setPeekList, tidyGroup} from './tidy_group.js';

const PAGE = 40;
const VIEW = tpl`<div class="tidy-page">
  <div class="tidy-bar"><span class="tidy-status" data-ref="status"></span>
    <a class="btn is-quiet is-small" data-ref="rest" href="#/unmatched" hidden></a>
    <button type="button" class="btn is-quiet is-small" data-ref="regroup" title="用当前的未识别图重新分组" data-t data-t-attr="title">重新分组</button></div>
  <div class="tidy-list" data-ref="list"></div>
  <div class="tidy-more" data-ref="moreBox" hidden><button type="button" class="btn" data-ref="more"></button></div>
  <div data-ref="empty" hidden></div>
</div>`;
// 这组现在算在组里的图（重读后组里已经没有的图不算，off 里留着也不影响）。
const kept = (g, off) => g.members.filter(u => !off?.has(u));

// 一组：组头「第 N 组 · M 张 · 特征」和问题，右边是整组的操作；下面一排方图（判断卡）。
// 全部点掉以后没有图可写：整组的按钮都不能点（「都不是」也是对这些图说的），只剩「点掉的是…」。
function groupCard(page, g) {
  const s = page.scope.child('group'), card = tidyGroup(), r = card.refs;
  let current = g;
  const none = ({kept}) => !kept.length, act = what => () => page.act(current, what);
  const judge = judgeCard({scope: s, look: 'strip', tip: {off: t('点一下：不算在这组里'), on: t('点一下：算回这组')},
    onOff: off => page.off.set(current.cid, off),
    actions: [
      {key: 'yes', label: ({k, kept}) => (k ? t('其余 {n} 张是', {n: number(kept.length)}) : t('都是')) + ' ✓', primary: true, when: () => !!current.lead, disabled: none, run: act('yes')},
      {key: 'no', label: t('都不是'), when: () => !!current.lead, disabled: none, run: act('no')},
      {key: 'other', label: t('点掉的是…'), when: ({k}) => k > 0, title: t('把点掉的图设为另一个角色'), run: act('other')},
      {key: 'assign', label: () => current.lead ? t('是别人…') : t('设为…'), primary: () => !current.lead, quiet: true, disabled: none, run: act('assign')},
      {key: 'oc', label: t('原创角色'), quiet: true, title: t('把这组图都设为「原创角色（未细分）」'), disabled: none, run: act('oc')},
      {key: 'skip', label: t('跳过'), quiet: true, disabled: none, run: act('skip')},
      {key: 'open', label: t('逐张看'), quiet: true, href: () => '#/g/' + current.cid},
    ]});
  r.strip.replaceWith(judge.el); r.acts.append(judge.acts);   // 方图换成判断卡的图（.tidy-strip 只给身份整理）
  setPeekList(card.el, () => current.members);
  const update = x => {
    current = x;
    attr(card.el, 'data-cid', String(x.cid));
    text(r.title, t('第 {id} 组', {id: x.cid}));
    text(r.meta, [t('{n} 张', {n: number(x.size)}), ...(x.traits || []).slice(0, 2)].join(' · '));
    // 问题那一句只在角色、张数变了时重画，焦点在角色链接上时不丢。
    refill(r.q, x.lead ? `${x.lead.id}|${x.lead.name}|${x.lead.count}` : '', () => {
      if (!x.lead) return [make('span', 'tidy-unknown', t('还不知道是谁'))];
      const link = make('a', '', x.lead.name); link.href = charHash(x.lead.id);
      return [rich(t('是 {name} 吗？'), {name: link}), make('small', '', t('{n} 张的线索都指向 TA', {n: number(x.lead.count)}))];
    });
    judge.update(x.members.map(uuid => ({uuid})), {off: page.off.get(x.cid) || new Set(), rest: '#/g/' + x.cid});
    show(r.note, !!x.near); text(r.note, x.near ? nearText(x) : '');
  };
  update(g);
  return {el: card.el, update, dispose: () => s.dispose()};
}

function groupsView(scope) {
  const {el, refs} = VIEW();
  $('groupsPage').replaceChildren(el);
  clearFooter();
  // pick：打开角色选择器（测试换成自己的）。
  const page = {scope, list: [], learner: null, shown: PAGE, off: new Map(), loaded: false, seq: 0, pick: openPicker};
  const list = keyed(refs.list, {key: g => g.cid, create: g => groupCard(page, g), exit: collapse, scope});
  function draw() {
    const all = page.list, images = all.reduce((n, g) => n + g.size, 0), named = all.filter(g => g.lead).length;
    const busy = ['learning', 'grouping', 'waiting'].includes(page.learner?.status), rest = Math.max(0, (state.pools.unmatched || 0) - images);
    text(refs.status, [t('{n} 组 · 共 {images} 张', {n: number(all.length), images: number(images)}),
      named ? t('{n} 组能直接确认是谁', {n: number(named)}) : '', all[0] ? t('分组于 {time}', {time: stamp(all[0].built_at)}) : '',
      busy ? page.learner.status === 'waiting' ? page.learner.message || '' : t('正在学习…') : ''].filter(Boolean).join(' · '));
    show(refs.rest, !!rest); text(refs.rest, t('没成组的 {n} 张', {n: number(rest)}) + ' →');
    list.set(all.slice(0, page.shown));
    show(refs.moreBox, all.length > page.shown);
    text(refs.more, t('再显示 {n} 组', {n: number(Math.min(PAGE, all.length - page.shown))}));
    const empty = page.loaded && !all.length, any = !!state.pools.unmatched;
    show(refs.empty, empty);
    if (empty) refill(refs.empty, any ? 'some' : 'none', () => {
      drawEmpty(refs.empty, {icon: 'check', fact: any ? t('没有成组的未识别图。') : t('没有未识别的图。'),
        next: any ? [{label: t('全部未识别'), href: '#/unmatched'}] : [{label: t('去图库'), href: '#/library'}]});
      return [...refs.empty.childNodes];
    });
  }
  // keep：重读以后留在已显示的组数和排除的图（撤销、学习落定、大图里改了图）；不给是刚进来或换了 NSFW，从前 40 组开始。
  // 同一路由上的几次重读只认最后发出的那次（seq）：先发的晚到，不把点掉的图和已显示的组数复位。
  async function load(keep = false) {
    const route = state.route, nsfw = state.nsfw, seq = ++page.seq;
    const fresh = () => scope.alive && state.route === route && nsfw === state.nsfw && seq === page.seq;
    let result;
    try { result = await loadClusters(nsfw, scope.signal); } catch (e) {
      if (!fresh()) return;
      if (page.loaded) error(readError(e)); else { show(refs.empty, true); refill(refs.empty, 'fail', () => { drawFailure(refs.empty, e); return [...refs.empty.childNodes]; }); }
      return;
    }
    if (!fresh()) return;
    page.list = groupOrder(result.clusters); page.learner = result.learner; page.loaded = true;
    if (!keep) { page.shown = PAGE; page.off = new Map(); }
    draw();
  }
  // 这组答完了：从列表里拿掉（收起），活页条的数字按这组的张数先减。按 cid 找：写入期间重读换了一份新对象也能拿掉。
  function done(cid, ids) {
    leaving(refs.list.querySelector(`[data-cid="${cid}"]`));
    page.list = page.list.filter(x => x.cid !== cid); page.off.delete(cid);
    count('unmatched', minus(state.pools.unmatched, ids.length));
    draw();
  }
  // 组里的几张设成了别的角色：从这组拿掉（组还在），活页条的数字先减；拿光了就和答完一样收起。
  function shed(cid, ids) {
    const g = page.list.find(x => x.cid === cid); if (!g) return;
    const members = g.members.filter(u => !ids.includes(u));
    if (!members.length) { done(cid, ids); return; }
    page.list = page.list.map(x => x === g ? {...g, members, size: Math.max(members.length, g.size - ids.length)} : x);
    const off = page.off.get(cid); if (off) for (const u of ids) off.delete(u);
    count('unmatched', minus(state.pools.unmatched, ids.length));
    draw();
  }
  async function act(g, what) {
    const ids = kept(g, page.off.get(g.cid)), n = ids.length;
    if (!n && what !== 'other') return;
    const finish = ok => { if (ok && scope.alive) done(g.cid, ids); };
    if (what === 'yes') finish(await mutate('/api/change', {ids, action: 'confirmed', entity: g.lead.id}, false));
    else if (what === 'no') { if (await mutate('/api/change', {ids, action: 'rejected', entity: g.lead.id}, false)) load(true); }
    else if (what === 'skip') finish(await mutate('/api/change', {ids, action: 'unknown'}, false));
    else if (what === 'oc') finish(await markOc({ids}, false));
    // 「点掉的是…」（#787）：点掉的几张设为选中的角色，一个撤销；这组留着，少了那几张。选的时候这一页或这一组已经不在了就不写。
    else if (what === 'other') {
      const off = [...(page.off.get(g.cid) || [])].filter(u => g.members.includes(u));
      if (off.length) pickFor(scope, {ids: off, exclude: g.lead ? [g.lead.id] : [], sentence: x => t('把 {n} 张设为 {name}', {n: number(off.length), name: x.name}),
        still: () => onPage(state.route, 'groups') && page.list.some(x => x.cid === g.cid),
        pick: async x => { if (await mutate('/api/change', {ids: off, action: 'confirmed', entity: x.id}, false) && scope.alive) shed(g.cid, off); }}, page.pick);
    }
    // 面板顶上写「第 N 组 · M 张」，和缩略图一起（#424 的 2-3）；能多选，和选择栏的「设为…」一样（2-24）。
    else if (what === 'assign') openPicker({title: t('这 {n} 张是…', {n: number(n)}), subjectText: t('第 {id} 组', {id: g.cid}) + ' · ' + t('{n} 张', {n: number(n)}),
      sentence: x => t('把这 {n} 张设为 {name}', {n: number(n), name: x.name}), suggest: {ids}, exclude: g.lead ? [g.lead.id] : [], multi: true,
      pick: async picked => finish(await mutate('/api/change', {ids, action: 'confirmed', ...targets(picked)}, false))});
  }
  page.act = act;
  scope.on(refs.empty, 'click', e => { if (e.target.closest('[data-empty="retry"]')) load(); });
  scope.on(refs.more, 'click', () => { page.shown += PAGE; draw(); });
  scope.on(refs.regroup, 'click', async () => {
    try { await regroup(); toast(t('正在重新分组…')); scope.timeout(() => load(), 3000); } catch (e) { toast(e.message); }
  });
  return {scope, load, done, shed, page};
}

let view = null;

// 「下一组」：重读分组，取原来排在这组后面、还在的第一组；后面都没了就取最前面那组；一组都没了回到按组整理。
// 读的时候离开了这一组就不带人走。
async function nextGroup() {
  const route = state.route;
  let result; try { result = await loadClusters(state.nsfw); } catch (e) { if (state.route === route) error(readError(e)); return; }
  if (state.route !== route) return;
  const pick = nextGroupId(state.groupOrder, route.id, result.clusters);
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
// 测试用：当前这一页（tests/tidy.test.cjs 用它模拟「这组答完了」）。
export const current = () => view;
