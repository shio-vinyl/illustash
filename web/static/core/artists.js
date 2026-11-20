// 按需查找画师的请求和等它结束（docs/PROVENANCE.md 第 9 节）：详情卡片的「查找画师」「找 TA 的其他图」和选择栏的
// 「查找画师」都走这里。征得同意、提示条在 ui/artists.js。不碰 DOM。
import {poller} from './poll.js';
import {api, session} from './session.js';

/** 发起一次查找。token 是征得同意之前记下的会话令牌：中途换了图库就不发（返回 false）。 */
export async function startLookup(body, token) {
  if (token !== session.token) return false;
  await api('/api/provenance', {method: 'POST', body: JSON.stringify(body), headers: {'X-CSRF-Token': token}});
  return true;
}

// 等这次查找结束（1.5 s 一次，读失败 2 s 后再试），把结果交给 done({status, found, reason})。新的查找替换旧的（只等最后一次）。
let done = null;
const poll = poller(async fresh => {
  const p = await api('/api/provenance');
  if (!fresh()) return 0;
  const s = p.state || {};
  if (['running', 'stopping'].includes(s.status)) return 1500;
  done?.({status: s.status, found: s.found || 0, reason: s.reason});
  return 0;
}, {retry: 2000});
/** 1.5 秒后开始等查找结束，结束时调 then。 */
export function awaitLookup(then) { done = then; poll.later(1500); }
