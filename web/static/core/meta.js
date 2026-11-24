// 图库的元数据（#199）：角色、复核池和计数、用户的标签以及最近的撤销步骤，一起读取；期间的写入不会被更早的读取覆盖。
// 不碰 DOM：onMeta(detail) 在每次被采用的读取之后通知（{total, directory, queue}），或通知一次读取失败（{error}）。
import {api, adoptToken} from './session.js';
import {setLatest} from './edits.js';
import {pageOf} from './pages.js';
import {state} from './state.js';
import {listeners} from './events.js';
const changed = listeners();
export const onMeta = changed.on;
const failed = error => changed.emit({error});
export const fetchChar = async id => (await api('/api/characters?' + new URLSearchParams({id, nsfw: state.nsfw})))[0];
// 当前角色页的角色（不在角色页时为 null）：换页时由 ui/shell.js 经这里换上，元数据重读后这里换成新读到的那份。
export function setChar(c) { state.char = c; }
// 身份整理还剩几对：元数据读取带回一次，身份整理页翻页时也会带回最新的总数。
export function setIdentityCount(n) { state.identityCount = n; }
let metaPending = null, metaRevision = 0, metaFull = false, charsNsfw = null;
// 资料库能先画当前筛选的角色摘要；切换 NSFW 以后必须等新筛选的读取。
export const canShowChars = () => charsNsfw === state.nsfw;
// 后端接受了一次写入：仍在进行中的更早的读取不能覆盖它（标签只需要重读标签）。
export function metaWritten(tagsOnly) { if (metaPending) { metaRevision++; metaFull ||= !tagsOnly; } }
// 编辑之后，侧栏计数和角色列表（服务端约 0.6 s 的工作）可以等编辑停下再刷新：连续回答 Y / N 只刷新一次，而不是每个回答
// 刷新一次。
let metaTimer = null, metaTagsOnly = true;
export function metaSoon(tagsOnly) {
  metaTagsOnly &&= tagsOnly; clearTimeout(metaTimer);
  metaTimer = setTimeout(() => { const t = metaTagsOnly; metaTagsOnly = true; refreshMeta({tagsOnly: t}).catch(failed); }, 700);
}
// 后台学习落定（#400、#404）：确认或撤销角色之后，服务端还要在后台学习，学到的结果发布以后，未识别计数和分组才是最终的
// 样子；写入请求返回时还没到这一步。GET /api/learning 回答编号不超过 upto 的这类写入是否都已处理完（契约见 #404）：
// done（已发布，或者没有学习 worker、写入时就处理完了）、pending（会自己继续）、sync（同步结束后才开始）、
// error（失败，稍后自动重试）、stopped（本次打开图库不会再自己继续）。服务端从不等待，等由这里轮询。
// 写入回复之后先读一次（learningNow），不带 upto：那时读到的最大编号一定不小于这次写入的编号，以后都带上它。
export const learningNow = () => api('/api/learning').catch(() => null);
const learningAt = upto => api('/api/learning?upto=' + upto);
// 最多等这么久（按累计的等待时间算，不读时钟）；前 2 秒每 250 ms 读一次，之后每秒一次。
export const LEARN_LIMIT = 60000;
const LEARN_FAST = 2000;
let learnRun = 0, syncWait = null;
// 等待期间计数旁显示「正在更新」（state.learning，ui/shell.js 的标题读它）。这里只改字段、不重画，设上时由调用方重画。
// 等待结束时不去掉：调用方重读完列表、计数和分组以后再调 learningShown，免得分组还没重画，标记先没了。
const mark = on => { state.learning = on; };
export const learningShown = () => mark(false);
// 从 learningNow 读到的 first 接着等，解析为：
//   'done'        学到的结果已经发布，现在重读就是最终值；
//   'sync'        同步进行中，同步结束后才学，afterSyncLearning 接着等同一个 upto；
//   'error'       学习失败或停下了，或者读不到状态：重读一次当前值，不再等；
//   'timeout'     等满 LEARN_LIMIT 还没落定：重读一次当前值，不再等；
//   'superseded'  更新的一次调用接手了等待（每次调用都会让之前的作废），什么也不用做。
// marking：还要等时，等待期间显示「正在更新」。
export async function learningSettled(first, {marking = false} = {}) {
  const run = ++learnRun;
  syncWait = null; mark(false);
  let s = first;
  for (let waited = 0; ; ) {
    if (!s) return 'error';
    if (s.state === 'done') return 'done';
    if (s.state === 'sync') { syncWait = {upto: s.upto, marking}; return 'sync'; }
    if (s.state !== 'pending') return 'error';
    if (waited >= LEARN_LIMIT) return 'timeout';
    if (marking) mark(true);
    const step = waited < LEARN_FAST ? 250 : 1000;
    await new Promise(done => setTimeout(done, step)); waited += step;
    if (run !== learnRun) return 'superseded';
    try { s = await learningAt(first.upto); } catch { s = null; }
    if (run !== learnRun) return 'superseded';
  }
}
// 同步结束了：之前因为同步而没等完的那次（learningSettled 解析为 'sync'），用同一个 upto 接着等，返回它的 promise；没有就返回 null。
export function afterSyncLearning() {
  const wait = syncWait; syncWait = null;
  if (!wait) return null;
  return learningSettled({upto: wait.upto, state: 'pending'}, {marking: wait.marking});
}
export function refreshMeta({tagsOnly = false, boot = null} = {}) {
  // 这次读取包含了还没到时间的 metaSoon 要读的（#452）：写入之后紧接着撤销、学习落定后的重读，不再隔 700 ms 又读一遍。
  if (!tagsOnly || metaTagsOnly) { clearTimeout(metaTimer); metaTimer = null; metaTagsOnly = true; }
  metaRevision++; metaFull ||= !tagsOnly;
  const bootEpoch = state.readEpoch, bootMode = state.nsfw;
  if (!metaPending) metaPending = Promise.resolve().then(async () => {
    try {
      for (;;) {
        const revision = metaRevision, full = metaFull; metaFull = false;
        try { await refreshMetaOnce(full, revision, bootEpoch === state.readEpoch && bootMode === state.nsfw ? boot : null); }
        catch (e) { if (revision === metaRevision) throw e; }
        boot = null;
        if (revision === metaRevision) return;
        // 读取进行中发生了写入，需要随后再读一次新的，不能用它过时的结果。
        metaFull ||= full;
      }
    } finally { metaPending = null; }
  });
  return metaPending;
}
async function refreshMetaOnce(full, revision, bootSeed) {
  if (!full) {
    const tags = await api('/api/user-tags');
    if (revision === metaRevision) state.userTags = tags;
    return;
  }
  const nsfw = new URLSearchParams({nsfw: state.nsfw});
  const charId = state.char?.id;
  const directory = !!pageOf(state.route).directory;
  const [boot, pools, chars, tags, queue, detail] = await Promise.all([bootSeed || api('/api/bootstrap?' + nsfw), api('/api/review-pools?' + nsfw), api('/api/characters?min=1&' + nsfw + (directory ? '' : '&summary=1')), api('/api/user-tags'), api('/api/identity/count?' + nsfw).catch(e => { failed(e); return null; }), charId && !directory ? fetchChar(charId) : null]);
  const currentChar = charId && (detail || (directory && (chars.find(c => c.id === charId) || await fetchChar(charId))));
  if (revision !== metaRevision || nsfw.get('nsfw') !== state.nsfw || !adoptToken(boot.token)) return;
  state.userTags = tags;
  setLatest(boot.undo);
  state.pools = Object.fromEntries(pools.map(p => [p.kind, p.count]));
  charsNsfw = state.nsfw;
  state.chars = chars; state.charMap = new Map(chars.map(c => [c.id, c]));
  state.workNames = new Map(chars.filter(c => c.work && c.work_name).map(c => [c.work, c.work_name]));
  state.dupInfo = pools.find(p => p.kind === 'duplicates') || state.dupInfo;
  state.exclInfo = pools.find(p => p.kind === 'excluded') || state.exclInfo;
  state.pendingCount = state.pools.pending || 0;
  state.recentCount = state.pools.recent || 0;
  if (queue) state.identityCount = queue.total;
  if (state.char) state.char = (state.char.id === charId && currentChar) || state.char;
  await changed.emit({total: boot.total, directory, queue: !!queue});
  // 读摘要行期间进入了目录：它需要完整的行。
  if (pageOf(state.route).directory && !directory) refreshMeta().catch(failed);
}

