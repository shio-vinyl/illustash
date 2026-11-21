// 按天视图顶上的两张卡（#707，0.4 第 C1 步）：同步状态，和「认一认」（#675，含第二步「点掉的是…」）。
// 部件写法照 docs/WEB_V2.md「渲染底座」：{el, update(props), dispose()}；这里不发请求：同步卡的数据由页面（ui/days.js）给，回答交给 ui/day_ask.js。
//
//   const sync = syncCard({open: () => …});            // 点卡片打开任务面板
//   sync.update({sync: state.sync, today, lost});      // lost：读不到同步状态（state.syncLost）；today：今天那一天的数据，写「今天认出几位」；最新一天不是今天、或者开着筛选
//                                                      （数字就不是全库的了）时页面给 null，不写这一句
//
//   const ask = askCard({answer, other, later});
//   ask.update({batch});                               // batch 为空时藏起来
//     answer(how, off)  how 'ok'（点掉的不是 TA、其余是）或 'none'（整批都不是）；off 是点掉的 uuid 集合
//     other(off)        「点掉的是…」：页面打开角色选择器，把点掉的这几张记成选中的角色（一步、一个撤销）
//     later()           以后再说
//   ask.busy(on)                                       // 写入的时候锁住这一批
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
import {judgeCard} from './judge.js';
import {thumbs} from './motion.js';
import {scope} from './scope.js';
import {thumbUrl} from './day.js';
import {releaseImages} from './virtual.js';

const SYNC = tpl`<button class="group day-card day-sync" type="button"><span class="avatar-ring day-ring" data-ref="ring" aria-hidden="true"><svg viewBox="0 0 20 20"><path
  data-ref="tick" d="m6 10.4 2.7 2.6L14 7.6"/></svg></span><span class="day-card-text"><b class="day-card-title" data-ref="title"></b><span class="day-card-sub"
  data-ref="sub"></span></span></button>`;
const ASK = tpl`<section class="group day-card day-ask" aria-labelledby="dayAskTitle"><div class="day-ask-peek" data-ref="peek" aria-hidden="true"></div><div class="day-card-text">
  <h3 class="day-card-title" id="dayAskTitle" data-ref="title"></h3><span class="day-card-sub" data-ref="sub"></span></div><div class="day-ask-tiles" data-ref="tiles"></div>
  <div class="day-ask-foot" data-ref="foot"></div></section>`;
const PEEK_IMG = tpl`<img alt="" draggable="false" loading="lazy" decoding="async">`;
// 收起时露出几张小图。
const PEEK = 4;

// ---------- 同步 ----------
const TITLES = {done: () => t('同步完成'), idle: () => t('还没有同步过'), needs_recognition: () => t('等待识别'), interrupted: () => t('同步已中断'),
  error: () => t('同步失败')};
export function syncCard({open}) {
  const s = scope('day-sync'), {el, refs} = SYNC();
  show(el, false);   // 读到同步状态之前不显示
  s.on(el, 'click', () => open());
  function update({sync, today, lost = false}) {
    const status = sync?.status || 'off', active = SYNC_ACTIVE.includes(status);
    show(el, status !== 'off');
    if (status === 'off') return;
    // 圆环：进行中按进度，完成是满的；还没同步过（idle）、等待识别、出了问题都是空的。
    const part = active ? syncBar(sync, (done, total) => total ? Math.min(1, (done || 0) / total) : 0) || 0 : status === 'done' ? 1 : 0;
    css(refs.ring, '--p', String(Math.round(part * 100)));
    cls(refs.ring, 'is-busy', active); cls(refs.ring, 'is-warn', ['error', 'interrupted', 'needs_recognition'].includes(status));
    show(refs.tick, status === 'done');
    text(refs.title, active ? t('正在同步') : TITLES[status]?.() || t('同步'));
    // 读不到同步状态（轮询失败、正在重试）：不把上一次读到的进度当成现在的，标题、圆环和副标题都这么写。
    if (lost) {
      text(refs.title, t('读不到同步状态')); text(refs.sub, t('正在重新连接…'));
      css(refs.ring, '--p', '0'); cls(refs.ring, 'is-busy', false); show(refs.tick, false);
      return;
    }
    const facts = status === 'done' ? [
      ...(sync.finished_at ? [stamp(sync.finished_at)] : []),
      t('新加入 {n} 张', {n: number(sync.imported || 0)}),
      ...(today?.people ? [t('今天认出 {n} 位角色', {n: number(today.people)})] : []),
    ] : status === 'idle' ? [] : [syncLine(sync)];
    text(refs.sub, facts.join(' · '));
  }
  return {el, update, dispose: () => s.dispose()};
}

// ---------- 认一认 ----------
// 图和按钮是共用的判断卡（ui/judge.js）；这里管收起、展开，没读到时的「重试」，和问句。
export function askCard({answer, other, later, retry}) {
  const s = scope('day-ask'), {el, refs} = ASK();
  show(el, false);   // 有一批要问的才显示
  let batch = null, open = false, lost = false, entity = '';
  const shown = () => !lost && !!batch;
  const judge = judgeCard({scope: s, look: 'tiles', max: Infinity, shrink: true,
    tip: {off: t('点一下：不是 TA'), on: t('点一下：改回「是」')},
    status: ({k}) => !shown() || !open ? '' : k ? t('{n} 张不是 TA', {n: number(k)}) : t('点掉不是 TA 的'),
    actions: [
      {key: 'later', label: t('以后再说'), quiet: true, when: shown, run: () => later()},
      {key: 'none', label: t('都不是'), when: ({n, k}) => shown() && open && k < n, title: ({n}) => t('{n} 张都记成不是 TA', {n: number(n)}), run: off => answer('none', off)},
      {key: 'other', label: t('点掉的是…'), when: ({k}) => shown() && open && k > 0, run: off => other(off)},
      {key: 'open', label: t('不全是'), when: () => shown() && !open, run: () => { open = true; paint(); refs.tiles.querySelector('button')?.focus(); }},
      {key: 'ok', label: ({n, k}) => k === n ? t('都不是') : k ? t('其余 {n} 张是', {n: number(n - k)}) : t('都对'), primary: true, when: shown,
        run: off => answer(off.size === batch.items.length ? 'none' : 'ok', off)},
      {key: 'retry', label: t('重试'), when: () => lost, run: () => retry()},
    ]});
  refs.tiles.append(judge.el); refs.foot.append(judge.acts);
  const peek = keyed(refs.peek, {key: x => x.uuid, scope: s, create: x => {
    const {el} = PEEK_IMG(); attr(el, 'src', thumbUrl(x.uuid)); thumbs.mark(el, x.uuid); return {el, update() {}, dispose() { releaseImages(el); }};
  }});

  function paint() {
    cls(el, 'is-open', open); cls(el, 'is-failed', lost);
    show(refs.peek, shown() && !open); show(refs.tiles, shown() && open);
    judge.paint();
  }
  function update({batch: next, failed = false}) {
    show(el, failed || !!next?.items?.length);
    // 藏起来（没有题、开关关了、NSFW 换了）或没读到：点选和展开都复位，再出现时从头来。
    if (!next?.items?.length) {
      batch = null; entity = ''; open = false; lost = failed; judge.update([]);
      // 标题和 titleFor 一样直接换内容（它是拼出来的节点，不经 text() 的缓存）。
      if (failed) { refs.title.textContent = t('没读到要认的图'); text(refs.sub, ''); }
      paint(); return;
    }
    lost = false;
    // 同一个角色、只是少了几张（「点掉的是…」提交了那几张）：留在展开的样子，点掉的只留还在的。其余情况（换了一批）收起、清掉点掉的。
    const shrunk = batch && next.entity === entity && next.items.every(x => batch.items.some(y => y.uuid === x.uuid));
    const off = shrunk ? new Set([...judge.off].filter(u => next.items.some(x => x.uuid === u))) : new Set();
    if (!shrunk) { open = false; entity = next.entity; titleFor(next); }
    batch = next;
    const why = next.kind === 'hint' ? t('和你确认过的 TA 很像') : next.kind === 'seed' ? '' : t('随机抽查');
    text(refs.sub, [workLabel({id: next.entity, name: next.name}), why, t('机器认出 {n} 张', {n: number(next.items.length)})].filter(Boolean).join(' · '));
    judge.update(next.items, {off}); peek.set(next.items.slice(0, PEEK));
    paint();
  }
  // 问句里的名字是去角色页的链接：用节点拼（ui/el.js 的 rich），名字只进 textContent。
  function titleFor(b) {
    const link = make('a', '', b.name);
    attr(link, 'href', charHash(b.entity));
    refs.title.textContent = '';
    refs.title.append(rich(t('这些是 {name} 吗？'), {name: link}));
  }
  // 写入的时候锁住这一批：按钮和图都点不动，写完再放开（ui/day_ask.js）。
  function busy(on) {
    cls(el, 'is-busy', on); attr(el, 'aria-busy', on ? 'true' : null);
    judge.busy(on);
  }
  return {el, update, busy, dispose: () => s.dispose()};
}
