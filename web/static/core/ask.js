// 「是不是 TA」的问答（/api/ask/answer）：请求体、写入、断连以后的核对。认一认（ui/day_ask.js，题目由 core/days.js 的 readAsk 读）、
// 整理 › 可能认错（ui/doubts.js，请求体由 core/doubts.js 拼）和详情栏、大图里的提问（ui/detail.js）共用；认一认和可能认错的写入
// 收尾在 ui/answer.js，详情栏的走 ui/grid.js 的 mutate。不碰 DOM，Node 里能直接测（tests/days.test.cjs、tests/doubts.test.cjs、
// tests/detail_core.test.cjs）。
//
// 同一个接口的三种请求体：
//   askBody(batch, how, off, other)  一批图（认一认、可能认错）：{entity, kind, yes, no, switch?}，见下面 askBody 的说明
//   questionBody({q, ident}, how)    详情栏、大图里的一张：{ids: [ident], entity, kind, yes 或 no, switch?}
//   askLater({entity, kind, items})  以后再说：{entity, kind, later}，不改图库，不进撤销
import {api, post} from './session.js';

// 回答一批（一步、一个撤销）：返回后端的回复（带 undo）。失败时抛错：带 status 的是后端明确拒绝了（锁库、参数不对），
// 不带的是没收到回复（断连），这时用 askLanded 核对这一步是不是已经写进去了。
export const sendAsk = body => post('/api/ask/answer', body);
// 「以后再说」：记下这几张问过了（认一认的一批、可能认错的一张卡；7 天里不再列出），不改图库，不进撤销。
export const askLater = ({entity, kind, items}) => post('/api/ask/answer', {entity, kind, later: items.map(x => x.uuid)});
// 断连以后核对：这一批要写的每条人工结论（「是」的图对原角色 confirmed、「不是」的图对原角色 rejected、「点掉的是…」的图对
// 选中的角色 confirmed、可能认错的「都不是」对另一个角色 rejected，core/doubts.js）是不是都已经在详情的 assertions 里。
// 都在才算写进去了；读不到也算没有。
export async function askLanded(body, {signal} = {}) {
  const want = [...body.yes.map(u => [u, body.entity, 'confirmed']), ...body.no.map(u => [u, body.entity, 'rejected']),
    ...(body.switch ? body.no.map(u => [u, body.switch, 'confirmed']) : []), ...(body.also_no ? body.no.map(u => [u, body.also_no, 'rejected']) : [])];
  try {
    for (const [uuid, entity, status] of want) {
      const d = await api('/api/detail?id=' + encodeURIComponent(uuid), {signal});
      if (!(d.assertions || []).some(x => x.id === entity && x.status === status)) return false;
    }
    return true;
  } catch { return false; }
}
// 回答的请求体（交给 ui/answer.js 的 sendAnswer，一步、一个撤销）：
//   how 'ok'    点掉的（off）记成不是 TA，其余记成是 TA
//   how 'none'  整批都记成不是 TA（#675）
//   how 'other' 点掉的是另一个角色 other：只提交点掉的那几张，一步写两条结论（不是 TA、是 other），其余几张留在卡上（#675 第二步）。
//               后端的 switch 要求 yes 为空，所以这一步不带其余几张
export function askBody(batch, how, off, other = '') {
  const ids = batch.items.map(x => x.uuid);
  if (how === 'other') return {entity: batch.entity, kind: batch.kind, yes: [], no: ids.filter(u => off.has(u)), switch: other};
  const no = how === 'none' ? ids : ids.filter(u => off.has(u));
  return {entity: batch.entity, kind: batch.kind, yes: ids.filter(u => !no.includes(u)), no};
}
// 详情栏、大图里提问的回答（一张图）：how 是 'yes'、'no' 或 'switch'（是对手那个角色）。
export const questionBody = ({q, ident}, how) => ({ids: [ident], entity: q.entity, kind: q.kind, ...(how === 'yes' ? {yes: [ident]} : {no: [ident]}),
  ...(how === 'switch' ? {switch: q.rival} : {})});
