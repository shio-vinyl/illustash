// 整理 › 可能认错（#787，#/doubts）：机器已经算给角色 A、但和 B 分不太清的图。一张卡问「是 A 还是 B？」，同一对的图排在一起，
// 一张卡最多 16 张（数据和切卡在 core/doubts.js）。卡是共用的判断卡（ui/judge.js）：点一张是点掉（再点算回），长按或 Space 看大图。
// 回答：「是 A」（有点掉的图时「其余 N 张是 A」，点掉的记成不是 A）、「是 B」、「都不是」（A、B 都记不是）、「点掉的是…」（点掉的
// 那几张记成选中的角色，卡上留下其余的）、「以后再说」。前四种各是一个撤销；回答只作用于卡上露出来的图。
//
// - 答完的卡原地收起（ui/tidy_group.js 的 collapse，开了「减少动态效果」时直接消失），活页条上「可能认错」的数字先按本地算的
//   滚下去，元数据回来再写真值。不重新加载，页面不跳。
// - 写入期间这张卡锁着（按钮、图都点不动）；写成了才收起，后端拒绝或断连核对不到就放开，点选留着（ui/answer.js）。
// - 卡按键复用（ui/keyed.js）：撤销、学习落定、大图里改了图以后重读，没变的卡留在原处，图不重新加载。同一路由上的几次重读只认
//   最后发出的那次（page.seq）；重读不把点掉的图复位。
// - 「点掉的是…」打开的角色选择器属于这一页：离开这一页时关掉；选的时候这张卡已经不在了或离开了这一页，就不写。
import {CARD_MAX, doubtAnswer, doubtCards, doubtLater, loadDoubts} from '../core/doubts.js';
import {imageUrl, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {implement, onPage} from '../core/pages.js';
import {DOUBT_POOL} from '../core/rail.js';
import {charHash} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {minus} from '../core/tidy.js';
import {sendAnswer} from './answer.js';
import {$, error, toast} from './dom.js';
import {attr, make, refill, rich, show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {judgeCard} from './judge.js';
import {keyed} from './keyed.js';
import {openPicker} from './palette.js';
import {count} from './rail.js';
import {clearFooter, collapse, drawFailure, leaving, setPeekList} from './tidy_group.js';

// 先显示几张卡，「再显示」每次多几张。
const PAGE = 20;
const VIEW = tpl`<div class="tidy-page">
  <div class="tidy-bar"><span class="tidy-status" data-ref="status"></span></div>
  <div class="tidy-list" data-ref="list"></div>
  <div class="tidy-more" data-ref="moreBox" hidden><button type="button" class="btn" data-ref="more"></button></div>
  <div data-ref="empty" hidden></div>
</div>`;
const CARD = tpl`<section class="group tidy-group doubt-card">
  <header class="tidy-head">
    <span class="doubt-faces" aria-hidden="true"><img class="avatar" data-ref="faceA" alt=""><img class="avatar" data-ref="faceB" alt=""></span>
    <div class="tidy-title"><b class="tidy-q" data-ref="q"></b><span class="text-meta" data-ref="meta"></span></div>
  </header>
  <div data-ref="tiles"></div>
  <div class="tidy-acts" data-ref="acts"></div>
</section>`;

// 角色的圆头像：角色目录里的封面（core/meta.js 读的摘要行）。
const face = (img, c) => { const cover = state.charMap.get(c.id)?.cover; attr(img, 'src', cover ? imageUrl(cover) : null); };
const nameLink = c => { const a = make('a', '', c.name); a.href = charHash(c.id); return a; };

// 一张卡：卡头两个圆头像、「是 A 还是 B？」和张数；下面一排方图；最下面是回答。
function doubtCard(page, c) {
  const s = page.scope.child('doubt'), {el, refs} = CARD();
  let current = c;
  const none = ({kept}) => !kept.length, act = how => off => page.answer(current, how, off);
  const judge = judgeCard({scope: s, look: 'strip', max: CARD_MAX, shrink: true, tip: {off: t('点一下：不是 TA'), on: t('点一下：改回「是」')},
    onOff: off => page.off.set(current.key, off),
    actions: [
      {key: 'a', label: ({k, kept}) => k ? t('其余 {n} 张是 {name}', {n: number(kept.length), name: current.a.name}) : t('是 {name}', {name: current.a.name}),
        primary: true, disabled: none, title: ({k}) => k ? t('点掉的记成不是 {name}', {name: current.a.name}) : '', run: act('a')},
      {key: 'b', label: () => t('是 {name}', {name: current.b.name}), title: ({n}) => t('{n} 张都记成 {name}', {n: number(n), name: current.b.name}), run: act('b')},
      {key: 'none', label: t('都不是', {context: 'pair'}), title: () => t('{a}、{b} 都记成不是', {a: current.a.name, b: current.b.name}), run: act('none')},
      {key: 'other', label: t('点掉的是…'), when: ({k}) => k > 0, run: off => page.other(current, off)},
      {key: 'later', label: t('以后再说'), quiet: true, run: () => page.later(current)},
    ]});
  refs.tiles.replaceWith(judge.el); refs.acts.append(judge.acts);
  setPeekList(el, () => current.items.map(x => x.uuid));
  const update = x => {
    current = x;
    attr(el, 'data-key', x.key);
    face(refs.faceA, x.a); face(refs.faceB, x.b);
    // 问句里的两个名字是去角色页的链接：只在这一对变了时重画，焦点在链接上时不丢。
    refill(refs.q, `${x.a.id}|${x.a.name}|${x.b.id}|${x.b.name}`, () => [rich(t('是 {a} 还是 {b}？'), {a: nameLink(x.a), b: nameLink(x.b)})]);
    text(refs.meta, x.parts > 1 ? t('{n} 张 · 这一对共 {total} 张', {n: number(x.items.length), total: number(x.total)}) : t('{n} 张', {n: number(x.items.length)}));
    judge.update(x.items, {off: page.off.get(x.key) || new Set()});
  };
  update(c);
  return {el, update, busy: on => judge.busy(on), dispose: () => s.dispose()};
}

function doubtsView(scope) {
  const {el, refs} = VIEW();
  $('groupsPage').replaceChildren(el);
  clearFooter();
  // pick：打开角色选择器（测试换成自己的）。
  const page = {scope, cards: [], shown: PAGE, off: new Map(), busy: new Set(), loaded: false, seq: 0, pick: openPicker};
  const list = keyed(refs.list, {key: c => c.key, create: c => doubtCard(page, c), exit: collapse, scope});
  let picking = null;
  // 离开这一页：关掉本页打开的角色选择器。
  scope.add(() => { if (picking && $('picker').open) $('picker').close(); picking = null; });

  function draw() {
    const pairs = new Set(page.cards.map(c => c.a.id + '|' + c.b.id)).size, images = page.cards.reduce((n, c) => n + c.items.length, 0);
    text(refs.status, page.cards.length ? t('{n} 对角色 · 共 {images} 张', {n: number(pairs), images: number(images)}) : '');
    list.set(page.cards.slice(0, page.shown));
    show(refs.moreBox, page.cards.length > page.shown);
    text(refs.more, t('再显示 {n} 张卡', {n: number(Math.min(PAGE, page.cards.length - page.shown))}));
    const empty = page.loaded && !page.cards.length, rest = !!state.pools.unmatched;
    show(refs.empty, empty);
    if (empty) refill(refs.empty, rest ? 'some' : 'none', () => {
      drawEmpty(refs.empty, {icon: 'check', fact: t('没有可能认错的图。'),
        next: rest ? [{label: t('去未识别'), href: '#/groups'}] : [{label: t('去图库'), href: '#/library'}]});
      return [...refs.empty.childNodes];
    });
  }
  // keep：重读以后留在已显示的卡数和点掉的图（撤销、学习落定、大图里改了图）；不给是刚进来或换了 NSFW，从前几张卡开始。
  async function load(keep = false) {
    const route = state.route, nsfw = state.nsfw, seq = ++page.seq;
    const fresh = () => scope.alive && state.route === route && nsfw === state.nsfw && seq === page.seq;
    let groups;
    try { groups = await loadDoubts(nsfw, scope.signal); } catch (e) {
      if (!fresh()) return;
      if (page.loaded) error(readError(e)); else { show(refs.empty, true); refill(refs.empty, 'fail', () => { drawFailure(refs.empty, e); return [...refs.empty.childNodes]; }); }
      return;
    }
    if (!fresh()) return;
    page.cards = doubtCards(groups); page.loaded = true;
    if (!keep) { page.shown = PAGE; page.off = new Map(); }
    draw();
  }
  const cardEl = key => [...refs.list.children].find(n => n.dataset.key === key && !n.inert);
  const lock = (c, on) => { if (on) page.busy.add(c.key); else page.busy.delete(c.key); list.get(c.key)?.busy(on); };
  // 卡上的 ids 写完了：拿掉它们，卡上没图了就收起；活页条的数字按张数先减。
  function shed(c, ids) {
    const rest = c.items.filter(x => !ids.includes(x.uuid));
    if (!rest.length) { leaving(cardEl(c.key)); page.cards = page.cards.filter(x => x.key !== c.key); page.off.delete(c.key); }
    else {
      page.cards = page.cards.map(x => x.key === c.key ? {...x, items: rest} : x);
      const off = page.off.get(c.key); if (off) for (const u of ids) off.delete(u);
    }
    count('doubts', minus(state.pools[DOUBT_POOL], ids.length));
    draw();
  }
  // 写一步：写入期间锁着这张卡；写成了按写掉的图收起或缩小。
  async function write(c, body, ids) {
    if (page.busy.has(c.key)) return;
    lock(c, true);
    const ok = await sendAnswer(body, scope);
    lock(c, false);
    if (ok && scope.alive) shed(c, ids);
  }
  page.answer = (c, how, off) => {
    const all = c.items.map(x => x.uuid);
    write(c, doubtAnswer(c, how, off), all);
  };
  page.other = (c, off) => {
    const ids = c.items.filter(x => off.has(x.uuid)).map(x => x.uuid), ticket = {};
    if (!ids.length || page.busy.has(c.key)) return;
    picking = ticket;
    $('picker').addEventListener('close', () => { if (picking === ticket) picking = null; }, {once: true});
    page.pick({title: t('点掉的是…'), subject: ids, exclude: [c.a.id], suggest: {ids},
      sentence: e => t('把 {n} 张从 {from} 换成 {name}', {n: number(ids.length), from: c.a.name, name: e.name}),
      pick: async e => {
        if (picking !== ticket || !scope.alive || !onPage(state.route, 'doubts') || !page.cards.some(x => x.key === c.key)) return;
        picking = null;
        await write(c, doubtAnswer(c, 'other', new Set(ids), e.id), ids);
      }});
  };
  page.later = async c => {
    if (page.busy.has(c.key)) return;
    lock(c, true);
    try { await doubtLater(c); } catch (e) { lock(c, false); toast(readError(e)); return; }
    lock(c, false);
    if (scope.alive) shed(c, c.items.map(x => x.uuid));
  };
  scope.on(refs.empty, 'click', e => { if (e.target.closest('[data-empty="retry"]')) load(); });
  scope.on(refs.more, 'click', () => { page.shown += PAGE; draw(); });
  return {scope, load, page};
}

let view = null;
// 大图关掉时把最后看的那张缩略图滚进视野。
function revealThumb(uuid) { [...$('groupsPage').querySelectorAll('[data-peek]')].find(x => x.dataset.peek === uuid)?.scrollIntoView({block: 'nearest'}); }

implement({
  // 在这页开的大图里改了图（'edit' 时大图还开着）：重读，免得卡上还留着已经改过的图。卡上按钮的写入不走这里，它们自己收起。
  doubts: {render: ({samePage, scope}) => { if (!samePage || !view?.scope.alive) view = doubtsView(scope); return view.load(); }, reveal: revealThumb,
    refresh: why => { if (why === 'undo' || why === 'learned' || why === 'edit' && state.viewer >= 0) return view?.load(true); if (why === 'nsfw') return view?.load(); }},
});
// 测试用：当前这一页。
export const current = () => view;
