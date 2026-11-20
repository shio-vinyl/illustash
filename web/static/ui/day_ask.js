// 按天视图的「认一认」（#675、#707）：读下一批，三种回答（都对 / 其余是、都不是、以后再说）和「点掉的是…」。卡片部件在
// ui/day_cards.js，这里管什么时候读、写入、换下一批。页面（ui/days.js）在进这一页时建一次：dayAsk(scope)。
//
// - 写入（#757 交叉审查）：
//   · 后端明确拒绝了（回复带错误码，例如锁库）：提示原因，卡片和点选都留着，可以再点一次。
//   · 没收到回复（断连）：逐张读详情，核对这一步要写的人工结论是不是都已经在了（core/days.js 的 askLanded）；都在才当写进去了。
//     不看「最近一步撤销」变没变：别的写入（收藏）也会改它。
//   · 写成了：这一批一直锁着（按钮、图都点不动），直到下一批到了（或者剩下的几张换上来）才放开，同一批不会提交两次。
//     「以后再说」、整批换角色都走这一条。写成以后，开着的详情卡片重读（和 ui/grid.js 的 mutate 一样）。
//   · 写的途中卡片换了一批（切 NSFW、关掉「偶尔问一句」）：锁不在途中放开；写成以后只把旧批记进 skip，不把它放回卡上（#757 核对）。
// - 读到的一批只认最后一次读取，而且 NSFW 设置和「偶尔问一句」开关都和发出时一样。读失败时卡片写「没读到」并给「重试」，
//   不和「没有可问的」一样整张藏起来。
// - 「点掉的是…」打开的角色选择器属于这一页：离开这一页时关掉；选的时候卡片已经换了一批或离开了这一页，就不写。
import {askAnswer, askLanded, askLater, answerAsk, readAsk} from '../core/days.js';
import {edits, exclusive} from '../core/edits.js';
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {refreshMeta} from '../core/meta.js';
import {onPage, pageOf} from '../core/pages.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {inspect} from './detail.js';
import {$, toast, written} from './dom.js';
import {askCard} from './day_cards.js';
import {viewOpts} from './grid.js';
import {openPicker} from './palette.js';

// pick：打开角色选择器（默认 ui/palette.js 的 openPicker；测试换成自己的）。
export function dayAsk(scope, {pick: choose = openPicker} = {}) {
  const skip = [];
  let batch = null, seq = 0, busy = false, writing = false, picking = null;
  const card = scope.add(askCard({answer, other, later, retry: () => next()}));
  const lock = on => { busy = on; if (scope.alive) card.busy(on); };
  // 换上一批（或藏起来、或写「没读到」）：这时才放开锁；写入途中换的不放（写完再放）。
  const put = (b, failed = false) => { batch = b; card.update({batch: b, failed}); if (!writing) lock(false); };
  // 写完以后卡片还是这一批吗：不是（写的途中换了），就放开锁、不再动卡片，返回 false。
  const still = b => { if (batch === b) return true; lock(false); return false; };
  // 离开这一页：关掉本页打开的角色选择器。
  scope.add(() => { if (picking && $('picker').open) $('picker').close(); picking = null; });

  async function next() {
    const mine = ++seq, nsfw = state.nsfw;
    if (!viewOpts.ask) { put(null); return; }
    let b;
    try { b = await readAsk(nsfw, skip, {signal: scope.signal}); } catch {
      if (scope.alive && mine === seq) put(null, true);
      return;
    }
    if (scope.alive && mine === seq && nsfw === state.nsfw && viewOpts.ask) put(b);
  }
  // NSFW 换了、开关变了：已经不该显示的那一批马上撤下，再读。
  function reset() { seq++; put(null); next(); }

  // 写一步：写成了（或者断连但核对过已经写进去了）返回 true，这时锁不放开，等调用方换下一批。
  async function write(body) {
    writing = true; lock(true);
    let ok = false;
    try {
      const r = await exclusive(() => answerAsk(body));
      if (r === undefined) { writing = false; lock(false); return false; }   // 另一项修改正在进行
      if (r.undo) written(r.undo);
      ok = true;
    } catch (e) {
      if (e.status) toast(readError(e));                   // 后端明确拒绝了
      else if (scope.alive) {
        ok = await askLanded(body, {signal: scope.signal});
        if (!ok) toast(readError(e));
      }
    }
    writing = false;
    if (!ok || !scope.alive) { lock(false); return false; }
    pageOf(state.route).refresh?.('edit');
    refreshMeta().catch(() => {});
    if (state.focused) inspect(state.focused, true);
    return true;
  }
  async function answer(how, off) {
    const b = batch; if (!b || busy || edits.busy) return;
    if (!await write(askAnswer(b, how, off))) return;
    skip.push(b.entity);
    if (still(b)) next();
  }
  // 「点掉的是…」（#675 第二步）：选一个角色，点掉的那几张一步记成「不是 TA、是选中的角色」，一个撤销；其余几张留在卡上。
  function other(off) {
    const b = batch; if (!b || !off.size || busy || edits.busy) return;
    const ids = b.items.filter(x => off.has(x.uuid)).map(x => x.uuid), ticket = {};
    picking = ticket;
    $('picker').addEventListener('close', () => { if (picking === ticket) picking = null; }, {once: true});
    choose({title: t('点掉的是…'), subject: ids, exclude: [b.entity], suggest: {ids},
      sentence: e => t('把 {n} 张从 {from} 换成 {name}', {n: number(ids.length), from: b.name, name: e.name}),
      pick: async e => {
        if (picking !== ticket || !scope.alive || batch !== b || busy || !onPage(state.route, 'days')) return;
        picking = null;
        if (!await write(askAnswer(b, 'other', off, e.id))) return;
        if (!still(b)) { skip.push(b.entity); return; }   // 写的途中换了一批：剩下的几张不放回去
        const rest = b.items.filter(x => !off.has(x.uuid));
        if (rest.length) put({...b, items: rest}); else { skip.push(b.entity); next(); }
      }});
  }
  async function later() {
    const b = batch; if (!b || busy) return;
    writing = true; lock(true);
    try { await askLater(b); } catch (e) { writing = false; lock(false); toast(readError(e)); return; }
    writing = false;
    if (!scope.alive) return;
    skip.push(b.entity);
    if (still(b)) next();
  }
  return {el: card.el, next, reset, get batch() { return batch; }};
}
