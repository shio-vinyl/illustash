// 身份整理（#713，#/identity）：长得像的角色对，一对一组（ui/tidy_group.js 的 tidyGroup）。组头写 A ↔ B 和为什么排进来，右边是
// 五个判断；方图那一行换成左右两边各自的几张样图，点一下看大图。数据在 core/tidy.js。
// 一次点击决定一对；每个决定都是一条可撤销的关系，原始记录从不移动。判过的那对原地收起，活页条上的数字跟着滚下去；
// 之后按现在显示的对数从头重读，撤销的那对回到原处，点过「再显示一些」的人不被带回前 12 对（#386 的 2-36）。
// 同一路由上的几次重读只认最后发出的那次。
import {edits} from '../core/edits.js';
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {refreshMeta, setIdentityCount} from '../core/meta.js';
import {implement} from '../core/pages.js';
import {charHash} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {decideIdentity, identityEvidence, identitySuggestion, loadIdentity, minus, pairKey} from '../core/tidy.js';
import {$, error, toast, written} from './dom.js';
import {attr, cls, make, refill, show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';
import {count, refresh as refreshRail} from './rail.js';
import {clearFooter, collapse, drawFailure, leaving, setPeekList, thumbStrip, tidyGroup} from './tidy_group.js';
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
const TITLE = tpl`<span><span data-ref="a"></span><span class="text-meta"> ↔ </span><span data-ref="b"></span></span>`;
const SIDES = tpl`<div class="tidy-pair">
  <div class="tidy-side"><p class="tidy-side-name"><b>A</b><a data-ref="aName"></a><span class="text-meta" data-ref="aN"></span></p><div class="tidy-strip" data-ref="a"></div></div>
  <div class="tidy-side"><p class="tidy-side-name"><b>B</b><a data-ref="bName"></a><span class="text-meta" data-ref="bN"></span></p><div class="tidy-strip" data-ref="b"></div></div>
</div>`;

function pairCard(page, x) {
  const s = page.scope.child('pair'), card = tidyGroup(), acts = ACTS(), sides = SIDES(), title = TITLE(), r = card.refs, p = sides.refs;
  r.acts.append(acts.el);
  r.title.append(title.el);
  r.strip.replaceWith(sides.el);
  const a = thumbStrip(p.a, {scope: s, max: 6, gimg: false}), b = thumbStrip(p.b, {scope: s, max: 6, gimg: false});
  const buttons = [...acts.el.querySelectorAll('[data-act]')];
  let current = x;
  setPeekList(p.a, () => current.a.samples); setPeekList(p.b, () => current.b.samples);
  const side = (name, n, c) => { text(name, c.name); attr(name, 'href', charHash(c.id)); attr(name, 'title', c.tag || c.name); text(n, t('{n} 张', {n: number(c.count)})); };
  const update = y => {
    current = y;
    attr(card.el, 'data-pair', pairKey(y));
    text(title.refs.a, y.a.name); text(title.refs.b, y.b.name);
    text(r.q, identityEvidence(y));
    // 外部资料建议的做法高亮成主按钮。
    const hint = identitySuggestion(y);
    for (const btn of buttons) if (btn.dataset.act !== 'skip') cls(btn, 'is-primary', btn.dataset.act === hint);
    side(p.aName, p.aN, y.a); side(p.bName, p.bN, y.b);
    a.update(y.a.samples); b.update(y.b.samples);
  };
  update(x);
  return {el: card.el, update, dispose: () => s.dispose()};
}

function identityView(scope) {
  const {el, refs} = VIEW();
  $('groupsPage').replaceChildren(el);
  clearFooter();
  const page = {scope, items: [], total: 0, busy: new Set(), loaded: false, seq: 0};
  const list = keyed(refs.list, {key: pairKey, create: x => pairCard(page, x), exit: collapse, scope});
  function draw() {
    list.set(page.items);
    show(refs.moreBox, page.items.length < page.total);
    const empty = page.loaded && !page.items.length;
    show(refs.empty, empty);
    if (empty) refill(refs.empty, 'empty', () => {
      drawEmpty(refs.empty, {icon: 'person', fact: t('没有需要判断的角色了。'), next: [{label: t('去资料库'), href: '#/characters'}]});
      return [...refs.empty.childNodes];
    });
  }
  // how：'more' 在后面再接一页；'keep' 判断或撤销之后按现在显示的对数从头重读；不给就是从头显示前 12 对。
  async function load(how = '') {
    const route = state.route, keep = how === 'keep', offset = how === 'more' ? page.items.length : 0, seq = ++page.seq;
    const limit = keep ? Math.max(PAGE, page.items.length) : PAGE;
    const fresh = () => scope.alive && state.route === route && seq === page.seq;
    let got;
    try { got = await loadIdentity({limit, offset, nsfw: state.nsfw}, scope.signal); } catch (e) {
      if (!fresh()) return;
      if (page.loaded) error(readError(e)); else { show(refs.empty, true); refill(refs.empty, 'fail', () => { drawFailure(refs.empty, e); return [...refs.empty.childNodes]; }); }
      return;
    }
    if (!fresh()) return;
    const have = new Set(page.items.map(pairKey));
    page.items = how === 'more' ? [...page.items, ...got.items.filter(x => !have.has(pairKey(x)))] : got.items;
    page.total = got.total; page.loaded = true;
    setIdentityCount(got.total); refreshRail('tidy');
    const footer = $('resultCount'); if (footer) footer.textContent = t('{n} 对待判断', {n: number(got.total)});
    draw();
  }
  async function decide(key, act) {
    const x = page.items.find(y => pairKey(y) === key); if (!x || page.busy.has(key) || edits.busy) return;
    page.busy.add(key);
    try {
      const {result, message} = await decideIdentity(x, act);
      written(result.undo, message);
      if (!scope.alive) return;
      // 按 pairKey 拿掉：写入期间重读换了一份新对象也能收起。
      leaving([...refs.list.children].find(n => n.dataset.pair === key));
      page.items = page.items.filter(y => pairKey(y) !== key); page.total = minus(page.total, 1);
      count('identity', page.total);
      draw();
      await load('keep');
      refreshMeta().catch(e => error(readError(e)));
    } catch (e) { toast(e.message); } finally { page.busy.delete(key); }
  }
  scope.on(refs.list, 'click', e => {
    const look = e.target.closest('[data-peek]');
    if (look) { e.preventDefault(); peekAt(look); return; }
    const btn = e.target.closest('[data-act]'), card = btn?.closest('[data-pair]');
    if (card) decide(card.dataset.pair, btn.dataset.act);
  });
  scope.on(refs.empty, 'click', e => { if (e.target.closest('[data-empty="retry"]')) load(); });
  scope.on(refs.more, 'click', () => load('more'));
  return {scope, load, page};
}
let view = null;

implement({
  identity: {render: ({samePage, scope}) => { if (!samePage || !view?.scope.alive) view = identityView(scope); return view.load(); },
    refresh: why => { if (why === 'undo') return view?.load('keep'); if (why === 'nsfw') return view?.load(); }},
});
