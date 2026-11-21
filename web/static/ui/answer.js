// 「是不是 TA」的回答在界面上的两件事，认一认（ui/day_ask.js）、可能认错（ui/doubts.js）共用，pickFor 按组整理（ui/groups.js）也用：
//
// sendAnswer(body, scope, {refused})：发一次回答（/api/ask/answer，一步、一个撤销）。
// - 后端明确拒绝了（回复带错误码，例如锁库）：提示原因，返回 false，卡片和点选由调用方留着，可以再点一次。
// - 没收到回复（断连）：逐张读详情，核对这一步要写的人工结论是不是都已经在了（core/ask.js 的 askLanded）；都在才当写进去了。
//   不看「最近一步撤销」变没变：别的写入（收藏）也会改它（#757 交叉审查）。
// - 另一项修改正在进行（core/edits.js 的 exclusive 返回 undefined）：什么也不写，返回 false。
// 写成了：给撤销提示，重读元数据（活页条的数字），开着的详情卡片重读（和 ui/grid.js 的 mutate 一样）。页面离开了也返回 false。
// refused(e)：后端明确拒绝时（e.status 是 HTTP 状态码）调用，例如可能认错的卡片过期（400）时页面重读队列；它返回 true 表示
// 提示也由它给了，这里就不再显示后端的原文。
//
// pickFor(scope, {ids, exclude, sentence, still, pick}, choose)：「点掉的是…」打开角色选择器。ids 是点掉的 uuid（选择器顶上列出这几张），
// exclude 是不该出现的角色；选了以后先问 still()（卡还在、这一页还在、没在写入），为假时不写，为真时调 pick(角色)。同一页再打开一次，
// 前一次的选择作废。离开这一页（作用域释放）时关掉本页打开的选择器。choose 默认是 ui/palette.js 的 openPicker（测试换成自己的）。
import {askLanded, sendAsk} from '../core/ask.js';
import {exclusive} from '../core/edits.js';
import {refreshMeta} from '../core/meta.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {t} from '../core/i18n.js';
import {inspect} from './detail.js';
import {$, toast, written} from './dom.js';
import {openPicker} from './palette.js';

export async function sendAnswer(body, scope, {refused = null} = {}) {
  let ok = false;
  try {
    const r = await exclusive(() => sendAsk(body));
    if (r === undefined) return false;
    if (r.undo) written(r.undo);
    ok = true;
  } catch (e) {
    if (e.status) { if (!(scope.alive && refused?.(e))) toast(readError(e)); }
    else if (scope.alive) {
      ok = await askLanded(body, {signal: scope.signal});
      if (!ok) toast(readError(e));
    }
  }
  if (!ok || !scope.alive) return false;
  refreshMeta().catch(() => {});
  if (state.focused) inspect(state.focused, true);
  return true;
}

const tickets = new WeakMap();   // 作用域 -> 这一页现在打开的那一次选择（没有时是 null）
export function pickFor(scope, {ids, exclude = [], sentence, still, pick}, choose = openPicker) {
  if (!tickets.has(scope)) scope.add(() => { if (tickets.get(scope) && $('picker')?.open) $('picker').close(); tickets.delete(scope); });
  const ticket = {};
  tickets.set(scope, ticket);
  $('picker')?.addEventListener('close', () => { if (tickets.get(scope) === ticket) tickets.set(scope, null); }, {once: true});
  return choose({title: t('点掉的是…'), subject: ids, exclude, suggest: {ids}, sentence,
    pick: async e => {
      if (tickets.get(scope) !== ticket || !scope.alive || !still()) return;
      tickets.set(scope, null);
      await pick(e);
    }});
}
