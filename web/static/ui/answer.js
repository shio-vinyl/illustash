// 发一次「是不是 TA」的回答（/api/ask/answer，一步、一个撤销）：认一认（ui/day_ask.js）和可能认错（ui/doubts.js）共用。
// - 后端明确拒绝了（回复带错误码，例如锁库）：提示原因，返回 false，卡片和点选由调用方留着，可以再点一次。
// - 没收到回复（断连）：逐张读详情，核对这一步要写的人工结论是不是都已经在了（core/days.js 的 askLanded）；都在才当写进去了。
//   不看「最近一步撤销」变没变：别的写入（收藏）也会改它（#757 交叉审查）。
// - 另一项修改正在进行（core/edits.js 的 exclusive 返回 undefined）：什么也不写，返回 false。
// 写成了：给撤销提示，重读元数据（活页条的数字），开着的详情卡片重读（和 ui/grid.js 的 mutate 一样）。页面离开了也返回 false。
// refused(e)：后端明确拒绝时（e.status 是 HTTP 状态码）调用，例如可能认错的卡片过期（400）时页面重读队列。
import {askLanded, answerAsk} from '../core/days.js';
import {exclusive} from '../core/edits.js';
import {refreshMeta} from '../core/meta.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {inspect} from './detail.js';
import {toast, written} from './dom.js';

export async function sendAnswer(body, scope, {refused = null} = {}) {
  let ok = false;
  try {
    const r = await exclusive(() => answerAsk(body));
    if (r === undefined) return false;
    if (r.undo) written(r.undo);
    ok = true;
  } catch (e) {
    if (e.status) { toast(readError(e)); if (scope.alive) refused?.(e); }
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
