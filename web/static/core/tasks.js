// 任务面板的取数（#715）：面板（ui/tasks.js）轮询时要补读的设置，和面板上「相似图片 › 重新检查」的请求。
// 各任务自己的接口和轮询在 core/jobs.js、core/models.js、core/packs.js、core/files.js；这里只放面板自己发的请求，
// ui/ 不直接发请求（tools/check_frontend.py 规则 22）。
import {api, post, shared} from './session.js';
import {setSettings} from './settings.js';
import {state} from './state.js';
import {SIMILAR_ACTIVE} from './jobs.js';

// 相似图片的状态随 /api/settings 一起来，那个接口还要测量缓存：面板开着时才读，运行中 2.5 s 一次，其余 15 s 一次。
// 轮询的补读和「重新检查」之后的读回共用一个序号：只有最后发出的那一次写进 state.settings，先发后到的旧回复（读到
// done 时其实已经重新开始了）丢掉，进度不会倒退；面板关掉时 dropTaskSettings 让在途的补读作废（#715）。
let settingsAt = 0, seq = 0;
function readSettings(get) {
  const mine = ++seq;
  return get().then(s => { if (mine === seq) setSettings(s); return mine === seq; });
}
/** 面板开着时到点了就补读一次设置（带相似图片的状态）；没到点返回 null。读到的写进 state.settings，被后来的读取盖过时不写。 */
export function refreshTaskSettings(now = Date.now()) {
  const gap = SIMILAR_ACTIVE.includes(state.settings?.tasks?.similar?.status) ? 2500 : 15000;
  if (now - settingsAt <= gap) return null;
  settingsAt = now;
  return readSettings(() => shared('/api/settings'));
}
/** 面板关了：在途的补读不再写入。 */
export function dropTaskSettings() { seq++; }
/** 重新检查相似图片，然后读回设置（新的任务状态在里面）。 */
export async function startSimilar() {
  await post('/api/similar', {});
  await readSettings(() => api('/api/settings'));
}
