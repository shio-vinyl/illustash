// 按天视图顶上的两张卡（#707，0.4 第 C1 步）：同步状态，和「认一认」（#675，含第二步「点掉的是…」）。
// 部件写法照 docs/WEB_V2.md「渲染底座」：{el, update(props), dispose()}；这里不发请求，回答交给页面（ui/days.js）。
//
//   const sync = syncCard({open: () => …});            // 点卡片打开任务面板
//   sync.update({sync: state.sync, today});            // today：今天那一天的数据（不是今天就是 null），写「今天认出几位」
//
//   const ask = askCard({answer, other, later});
//   ask.update({batch});                               // batch 为空时藏起来
//     answer(how, off)  how 'ok'（点掉的不是 TA、其余是）或 'none'（整批都不是）；off 是点掉的 uuid 集合
//     other(off)        「点掉的是…」：页面打开角色选择器，把点掉的这几张记成选中的角色（一步、一个撤销）
//     later()           以后再说
//
// 认一认收起时是一行：几张小图、问句、「不全是」「都对」。点「不全是」展开：图放大，点一张是「不是 TA」（再点改回），
// Space 或长按看大图（data-peek，页面用 ui/detail.js 的 wirePeek 接）；底下是「以后再说」「都不是」「点掉的是…」和主按钮。
// 主按钮随点掉的张数变：「都对」→「其余 N 张是」→ 全部点掉后是「都不是」，这时常驻的「都不是」不再画（#675）。
import {SYNC_ACTIVE, syncBar, syncLine} from '../core/jobs.js';
import {number, stamp} from '../core/format.js';
import {t} from '../core/i18n.js';
import {charHash} from '../core/route.js';
import {workLabel} from '../core/labels.js';
import {attr, cls, css, make, rich, show, text, tpl} from './el.js';
import {keyed} from './keyed.js';
import {exit, thumbs} from './motion.js';
import {scope} from './scope.js';
import {releaseImages} from './virtual.js';

const SYNC = tpl`<button class="group day-card day-sync" type="button"><span class="day-ring" data-ref="ring" aria-hidden="true"><svg viewBox="0 0 20 20"><path
  data-ref="tick" d="m6 10.4 2.7 2.6L14 7.6"/></svg></span><span class="day-card-text"><b class="day-card-title" data-ref="title"></b><span class="day-card-sub"
  data-ref="sub"></span></span></button>`;
const ASK = tpl`<section class="group day-card day-ask" aria-labelledby="dayAskTitle"><div class="day-ask-peek" data-ref="peek" aria-hidden="true"></div><div class="day-card-text">
  <h3 class="day-card-title" id="dayAskTitle" data-ref="title"></h3><span class="day-card-sub" data-ref="sub"></span></div><div class="day-ask-tiles" data-ref="tiles"></div>
  <div class="day-ask-foot"><span class="day-ask-state" data-ref="state" role="status"></span><button class="btn is-small is-quiet" type="button" data-ref="later" data-t>以后再说</button>
  <button class="btn is-small" type="button" data-ref="none" data-t>都不是</button><button class="btn is-small" type="button" data-ref="other" data-t>点掉的是…</button>
  <button class="btn is-small" type="button" data-ref="open" data-t>不全是</button><button class="btn is-small is-primary" type="button" data-ref="ok"></button></div></section>`;
const ASK_TILE = tpl`<button class="day-ask-tile" type="button"><img data-ref="img" alt="" draggable="false" loading="lazy" decoding="async"><i aria-hidden="true">✕</i></button>`;
const PEEK_IMG = tpl`<img alt="" draggable="false" loading="lazy" decoding="async">`;
const thumbUrl = uuid => `/image?id=${encodeURIComponent(uuid)}&size=thumb`;
const ratio = x => x.width > 0 && x.height > 0 ? Math.min(2.4, Math.max(.4, x.width / x.height)) : .75;
// 收起时露出几张小图。
const PEEK = 4;

// ---------- 同步 ----------
const TITLES = {done: () => t('同步完成'), needs_recognition: () => t('等待识别'), interrupted: () => t('同步已中断'), error: () => t('同步失败')};
export function syncCard({open}) {
  const s = scope('day-sync'), {el, refs} = SYNC();
  s.on(el, 'click', () => open());
  function update({sync, today}) {
    const status = sync?.status || 'off', active = SYNC_ACTIVE.includes(status);
    show(el, status !== 'off');
    if (status === 'off') return;
    const part = active ? syncBar(sync, (done, total) => total ? Math.min(1, (done || 0) / total) : 0) || 0 : 1;
    css(refs.ring, '--p', String(Math.round(part * 100)));
    cls(refs.ring, 'is-busy', active); cls(refs.ring, 'is-warn', ['error', 'interrupted', 'needs_recognition'].includes(status));
    show(refs.tick, status === 'done');
    text(refs.title, active ? t('正在同步') : TITLES[status]?.() || t('同步'));
    const facts = status === 'done' ? [
      ...(sync.finished_at ? [stamp(sync.finished_at)] : []),
      t('新加入 {n} 张', {n: number(sync.imported || 0)}),
      ...(today?.people ? [t('今天认出 {n} 位角色', {n: number(today.people)})] : []),
    ] : [syncLine(sync)];
    text(refs.sub, facts.join(' · '));
  }
  return {el, update, dispose: () => s.dispose()};
}

// ---------- 认一认 ----------
function askTile(x, toggle) {
  const {el, refs} = ASK_TILE();
  el.dataset.peek = x.uuid;
  attr(refs.img, 'src', thumbUrl(x.uuid)); thumbs.mark(refs.img, x.uuid);   // 看图器从这张 <img> 展开、缩回（#709）
  css(el, 'aspect-ratio', String(ratio(x)));
  el.addEventListener('click', () => toggle(x.uuid));
  return {el, update() {}, dispose() { releaseImages(el); }};
}
export function askCard({answer, other, later}) {
  const s = scope('day-ask'), {el, refs} = ASK();
  let batch = null, off = new Set(), open = false, entity = '';
  const toggle = uuid => { if (off.has(uuid)) off.delete(uuid); else off.add(uuid); paint(); };
  const tiles = keyed(refs.tiles, {key: x => x.uuid, create: x => askTile(x, toggle), exit: el => exit(el, {to: 'scale'}), scope: s});
  const peek = keyed(refs.peek, {key: x => x.uuid, scope: s, create: x => {
    const {el} = PEEK_IMG(); attr(el, 'src', thumbUrl(x.uuid)); thumbs.mark(el, x.uuid); return {el, update() {}, dispose() { releaseImages(el); }};
  }});
  s.on(refs.open, 'click', () => { open = true; paint(); refs.tiles.querySelector('button')?.focus(); });
  s.on(refs.ok, 'click', () => answer(off.size === batch.items.length ? 'none' : 'ok', new Set(off)));
  s.on(refs.none, 'click', () => answer('none', new Set(off)));
  s.on(refs.other, 'click', () => other(new Set(off)));
  s.on(refs.later, 'click', () => later());

  // 每次写全部状态；值没变的不动 DOM（ui/el.js）。
  function paint() {
    if (!batch) return;
    const n = batch.items.length, k = off.size, all = k === n;
    cls(el, 'is-open', open);
    show(refs.peek, !open); show(refs.tiles, open); show(refs.open, !open); show(refs.state, open);
    show(refs.none, open && !all); show(refs.other, open && k > 0);
    text(refs.state, k ? t('{n} 张不是 TA', {n: number(k)}) : t('点掉不是 TA 的'));
    attr(refs.none, 'title', t('{n} 张都记成不是 TA', {n: number(n)}));
    text(refs.ok, all ? t('都不是') : k ? t('其余 {n} 张是', {n: number(n - k)}) : t('都对'));
    for (const tile of refs.tiles.children) {
      if (tile.inert) continue;
      const no = off.has(tile.dataset.peek);
      attr(tile, 'aria-pressed', String(no));
      attr(tile, 'title', (no ? t('点一下：改回「是」') : t('点一下：不是 TA')) + ' · ' + t('长按或 Space 看大图'));
    }
  }
  function update({batch: next}) {
    show(el, !!next?.items?.length);
    if (!next?.items?.length) { batch = null; return; }
    // 换了一个角色：收起、清掉点掉的；同一个角色少了几张（「点掉的是…」提交了那几张）：留在展开的样子，点掉的只留还在的。
    if (next.entity !== entity) { open = false; off = new Set(); entity = next.entity; titleFor(next); }
    else off = new Set([...off].filter(u => next.items.some(x => x.uuid === u)));
    batch = next;
    const why = next.kind === 'hint' ? t('和你确认过的 TA 很像') : next.kind === 'seed' ? '' : t('随机抽查');
    text(refs.sub, [workLabel({id: next.entity, name: next.name}), why, t('机器认出 {n} 张', {n: number(next.items.length)})].filter(Boolean).join(' · '));
    tiles.set(next.items); peek.set(next.items.slice(0, PEEK));
    paint();
  }
  // 问句里的名字是去角色页的链接：用节点拼（ui/el.js 的 rich），名字只进 textContent。
  function titleFor(b) {
    const link = make('a', '', b.name);
    attr(link, 'href', charHash(b.entity));
    refs.title.textContent = '';
    refs.title.append(rich(t('这些是 {name} 吗？'), {name: link}));
  }
  return {el, update, dispose: () => s.dispose()};
}
