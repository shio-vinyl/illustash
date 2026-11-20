// 按天视图的「认一认」（#675、#707）：读下一批，三种回答（都对 / 其余是、都不是、以后再说）和「点掉的是…」。卡片部件在
// ui/day_cards.js，这里管什么时候读、写入、换下一批。页面（ui/days.js）在进这一页时建一次：dayAsk(scope)。
//
// - 写入成功以后才往下走（记进 skip、换下一批或去掉提交了的那几张）；写的时候这一批锁住，按钮点不动。没写成时卡片和点选
//   都留着，可以再点一次。没写成也可能是提交以后连接断了：重读最近一步撤销，变了就当写进去了。
// - 读到的一批只认最后一次读取，而且 NSFW 设置和「偶尔问一句」开关都和发出时一样；读失败时卡片先藏起来，过一会儿再读。
// - 「点掉的是…」打开的角色选择器属于这一页：离开这一页时关掉；选的时候卡片已经换了一批或离开了这一页，就不写。
import {askAnswer, askLater, readAsk} from '../core/days.js';
import {edits} from '../core/edits.js';
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {refreshMeta} from '../core/meta.js';
import {onPage, pageOf} from '../core/pages.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {$, toast} from './dom.js';
import {askCard} from './day_cards.js';
import {mutate, viewOpts} from './grid.js';
import {openPicker} from './palette.js';

// 读失败以后过多久再读一次。
const RETRY_MS = 15000;

export function dayAsk(scope) {
  const skip = [];
  let batch = null, seq = 0, busy = false, picking = null;
  const card = scope.add(askCard({answer, other, later}));
  const put = b => { batch = b; card.update({batch: b}); };
  const lock = on => { busy = on; if (scope.alive) card.busy(on); };
  // 离开这一页：关掉本页打开的角色选择器。
  scope.add(() => { if (picking && $('picker').open) $('picker').close(); picking = null; });

  async function next() {
    const mine = ++seq, nsfw = state.nsfw;
    if (!viewOpts.ask) { put(null); return; }
    let b;
    try { b = await readAsk(nsfw, skip, {signal: scope.signal}); } catch {
      if (scope.alive && mine === seq) { put(null); scope.timeout(() => { if (mine === seq) next(); }, RETRY_MS); }
      return;
    }
    if (scope.alive && mine === seq && nsfw === state.nsfw && viewOpts.ask) put(b);
  }
  // NSFW 换了、开关变了：已经不该显示的那一批马上撤下，再读。
  function reset() { seq++; put(null); next(); }

  // 写一步：成功（或断连但其实已经写进去）返回 true。
  async function write(body) {
    lock(true);
    const before = edits.latest?.id;
    let ok = await mutate('/api/ask/answer', body, false) === true;
    if (!ok && scope.alive) {
      await refreshMeta().catch(() => {});
      ok = edits.latest?.id != null && edits.latest.id !== before;
      if (ok) pageOf(state.route).refresh?.('edit');
    }
    lock(false);
    return ok && scope.alive;
  }
  async function answer(how, off) {
    const b = batch; if (!b || busy || edits.busy) return;
    if (!await write(askAnswer(b, how, off)) || batch !== b) return;
    skip.push(b.entity); next();
  }
  // 「点掉的是…」（#675 第二步）：选一个角色，点掉的那几张一步记成「不是 TA、是选中的角色」，一个撤销；其余几张留在卡上。
  function other(off) {
    const b = batch; if (!b || !off.size || busy || edits.busy) return;
    const ids = b.items.filter(x => off.has(x.uuid)).map(x => x.uuid), ticket = {};
    picking = ticket;
    $('picker').addEventListener('close', () => { if (picking === ticket) picking = null; }, {once: true});
    openPicker({title: t('点掉的是…'), subject: ids, exclude: [b.entity], suggest: {ids},
      sentence: e => t('把 {n} 张从 {from} 换成 {name}', {n: number(ids.length), from: b.name, name: e.name}),
      pick: async e => {
        if (picking !== ticket || !scope.alive || batch !== b || !onPage(state.route, 'days')) return;
        picking = null;
        if (!await write(askAnswer(b, 'other', off, e.id)) || batch !== b) return;
        const rest = b.items.filter(x => !off.has(x.uuid));
        if (rest.length) put({...b, items: rest}); else { skip.push(b.entity); next(); }
      }});
  }
  async function later() {
    const b = batch; if (!b || busy) return;
    lock(true);
    try { await askLater(b); } catch (e) { lock(false); toast(readError(e)); return; }
    lock(false);
    if (!scope.alive || batch !== b) return;
    skip.push(b.entity); next();
  }
  return {el: card.el, next, reset, get batch() { return batch; }};
}
