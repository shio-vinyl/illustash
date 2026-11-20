// 身份整理（#713，#/identity）：长得像的角色对，一对一组（ui/tidy_group.js）。组头写 A ↔ B 和为什么排进来，右边是五个判断；
// 下面左右两边各是这个角色的几张样图，点一下看大图。数据在 core/tidy.js。
// 一次点击决定一对；每个决定都是一条可撤销的关系，原始记录从不移动。判过的那对原地收起，活页条上的数字跟着滚下去；
// 之后按现在显示的对数从头重读，撤销的那对回到原处，点过「再显示一些」的人不被带回前 12 对（#386 的 2-36）。
import {edits} from '../core/edits.js';
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {refreshMeta, setIdentityCount} from '../core/meta.js';
import {implement} from '../core/pages.js';
import {charHash} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {decideIdentity, identityEvidence, identitySuggestion, loadIdentity, pairKey} from '../core/tidy.js';
import {$, error, toast, written} from './dom.js';
import {attr, cls, make, show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';
import {count, refresh as refreshRail} from './rail.js';
import {collapse, minus, setPeekList, thumbStrip, tidyGroup} from './tidy_group.js';
import {peekAt} from './viewer.js';

const PAGE = 12;
const VIEW = tpl`<div class="tidy-page">
  <div class="tidy-list" data-ref="list"></div>
  <div class="tidy-more" data-ref="moreBox" hidden><button type="button" class="btn" data-ref="more" data-t>再显示一些</button></div>
  <div data-ref="empty" hidden></div>
</div>`;
const ACTS = tpl`<span class="tidy-acts-in">
  <button type="button" class="btn is-small" data-act="same" title="把两个角色合在一起显示，图和人工结论留在原处" data-t-attr="title" data-t>记为同一角色（不搬图）</button>
  <button type="button" class="btn is-small" data-act="b_of_a" title="B 挂到 A 下面作为造型（服装、异格、变身等）" data-t-attr="title" data-t>B 是 A 的一个造型</button>
  <button type="button" class="btn is-small" data-act="a_of_b" title="A 挂到 B 下面作为造型" data-t-attr="title" data-t>A 是 B 的一个造型</button>
  <button type="button" class="btn is-small" data-act="different" title="记为不同角色，以后不再问这一对" data-t-attr="title" data-t>不同角色</button>
  <button type="button" class="btn is-quiet is-small" data-act="skip" title="先放一边，队列里不再出现" data-t-attr="title" data-t>暂不判断</button>
</span>`;
const SIDES = tpl`<div class="tidy-pair">
  <div class="tidy-side"><p class="tidy-side-name"><b>A</b><a data-ref="aName"></a><span class="tidy-meta" data-ref="aN"></span></p><div class="tidy-strip" data-ref="a"></div></div>
  <div class="tidy-side"><p class="tidy-side-name"><b>B</b><a data-ref="bName"></a><span class="tidy-meta" data-ref="bN"></span></p><div class="tidy-strip" data-ref="b"></div></div>
</div>`;

function pairCard(view, x) {
  const s = view.scope.child('pair'), card = tidyGroup(), acts = ACTS(), sides = SIDES(), r = card.refs, p = sides.refs;
  r.acts.append(acts.el);
  r.strip.replaceWith(sides.el);
  const a = thumbStrip(p.a, {scope: s, max: 6, gimg: false}), b = thumbStrip(p.b, {scope: s, max: 6, gimg: false});
  let current = x;
  setPeekList(p.a, () => current.a.samples); setPeekList(p.b, () => current.b.samples);
  const side = (name, n, c) => { text(name, c.name); attr(name, 'href', charHash(c.id)); attr(name, 'title', c.tag || c.name); text(n, t('{n} 张', {n: number(c.count)})); };
  const update = y => {
    current = y;
    attr(card.el, 'data-pair', pairKey(y));
    r.title.replaceChildren(make('span', '', y.a.name), make('span', 'tidy-meta', ' ↔ '), make('span', '', y.b.name));
    text(r.meta, '');
    text(r.q, identityEvidence(y));
    const hint = identitySuggestion(y);
    for (const btn of acts.el.querySelectorAll('[data-act]')) { cls(btn, 'is-primary', btn.dataset.act === hint); cls(btn, 'is-quiet', btn.dataset.act === 'skip' && hint !== 'skip'); }
    side(p.aName, p.aN, y.a); side(p.bName, p.bN, y.b);
    a.update(y.a.samples); b.update(y.b.samples);
  };
  update(x);
  return {el: card.el, update, dispose: () => s.dispose()};
}

function identityView(scope) {
  const {el, refs} = VIEW();
  $('groupsPage').replaceChildren(el);
  const view = {scope, items: [], total: 0, busy: new Set(), loaded: false};
  const list = keyed(refs.list, {key: pairKey, create: x => pairCard(view, x), exit: collapse, scope});
  function draw() {
    list.set(view.items);
    show(refs.moreBox, view.items.length < view.total);
    const empty = view.loaded && !view.items.length;
    show(refs.empty, empty);
    if (empty) drawEmpty(refs.empty, {icon: 'person', fact: t('没有需要判断的角色了。'), next: [{label: t('去资料库'), href: '#/characters'}]});
  }
  // how：'more' 在后面再接一页；'keep' 判断或撤销之后按现在显示的对数从头重读；不给就是从头显示前 12 对。
  async function load(how = '') {
    const route = state.route, keep = how === 'keep', offset = how === 'more' ? view.items.length : 0;
    const limit = keep ? Math.max(PAGE, view.items.length) : PAGE;
    let page;
    try { page = await loadIdentity({limit, offset, nsfw: state.nsfw}, scope.signal); } catch (e) { if (scope.alive && state.route === route) error(readError(e)); return; }
    if (!scope.alive || state.route !== route) return;
    view.items = how === 'more' ? [...view.items, ...page.items.filter(x => !view.items.some(y => pairKey(y) === pairKey(x)))] : page.items;
    view.total = page.total; view.loaded = true;
    setIdentityCount(page.total); refreshRail('tidy');
    text($('resultCount'), t('{n} 对待判断', {n: number(page.total)}));
    draw();
  }
  async function decide(key, act) {
    const x = view.items.find(y => pairKey(y) === key); if (!x || view.busy.has(key) || edits.busy) return;
    view.busy.add(key);
    try {
      const {result, message} = await decideIdentity(x, act);
      written(result.undo, message);
      if (!scope.alive) return;
      view.items = view.items.filter(y => y !== x); view.total = minus(view.total, 1);
      count('identity', view.total);
      draw();
      await load('keep');
      refreshMeta().catch(e => error(readError(e)));
    } catch (e) { toast(e.message); } finally { view.busy.delete(key); }
  }
  scope.on(refs.list, 'click', e => {
    const look = e.target.closest('[data-peek]');
    if (look) { e.preventDefault(); peekAt(look); return; }
    const btn = e.target.closest('[data-act]'), card = btn?.closest('[data-pair]');
    if (card) decide(card.dataset.pair, btn.dataset.act);
  });
  scope.on(refs.more, 'click', () => load('more'));
  return {scope, load};
}
let view = null;

implement({
  identity: {render: ({samePage, scope}) => { if (!samePage || !view?.scope.alive) view = identityView(scope); return view.load(); },
    refresh: why => { if (why === 'undo') return view?.load('keep'); if (why === 'nsfw') return view?.load(); }},
});
