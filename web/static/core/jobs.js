// 后台任务（#199）：照片或文件夹同步、外部索引的新角色（native/terms/src/app_index.rs）、显示名、相似图片、找回画师和
// 导入。每个任务怎样轮询、状态怎样用文字说、启动或取消时发什么；不碰 DOM。它们的状态仍在 core/state.js（sync、
// syncHistory、syncDamaged、index、names、provenance、importTask），由这里写入；别的模块要换掉 index、names 时调用
// 这里的 setIndex、setNames（#344）。相似图片的状态随 settings 一起来，settings 由 core/settings.js 写。任务运行期间轮询会自己重新排上；重叠的轮询绝不会让状态倒退（#34）。
import {diagnosticLine} from './diagnostics.js';
import {t, tb, sentences} from './i18n.js';
import {home, isMac, megabytes, number, stamp} from './format.js';
import {session, api, post, shared} from './session.js';
import {state} from './state.js';
import {listeners} from './events.js';
import {poller} from './poll.js';
import {baseDownloadStatus} from './models.js';
const changed = listeners();
// /api/index 的最新回应（关注列表、作品、检查与添加的状态）。资料库、设置页、引导读到新的回应也经这里换上。
export function setIndex(value) { state.index = value; }
// /api/names 的最新回应（覆盖率、设置、运行状态）；mergeNames 只换掉回应里带的那几项。
export function setNames(value) { state.names = value; }
export function mergeNames(patch) { state.names = {...state.names, ...patch}; }
// 每个被采用的回应之后调用 fn(job, change)；job 是 sync、index、names、provenance 或 import。change.was 是之前的状态，
// change.ended 表示一次运行刚结束；对 sync，change.arrived 表示进了图片或一次运行已落定，图库显示的内容可能变了。
export const onJob = changed.on;

// ---------- 同步 ----------
// waiting：角色索引在写入或 WD 扩展检查在跑，同步排在它们后面，结束后宿主自己开始（#607）。
export const SYNC_ACTIVE = ['waiting', 'scanning', 'recognising', 'importing'];
// 系统拒绝让本 App 访问照片图库（#202）：后端用一个代码表示，从不放在错误文字里。出路是「系统设置 ›
// 隐私与安全性 › 照片」，然后退出并重新打开；桌面外壳会在窗口外打开这一个地址（desktop/src/server.rs PHOTOS_PRIVACY），
// 浏览器会先询问。
export const photosRefused = s => s.status === 'error' && s.problem === 'photos_permission';
export const PHOTOS_PRIVACY = 'x-apple.systempreferences:com.apple.preference.security?Privacy_Photos';
export const photosHelp = () => t('没有访问照片图库的权限。到「系统设置 › 隐私与安全性 › 照片」里打开 Illustash，退出并重新打开后再同步。');
// 读不到一张图时说去哪儿查（#401）：只有图库可能来自 iCloud 照片时才提照片。来源在设置页读过就按来源；没读过时 Mac 上
// 算可能有，Windows 没有照片来源。
export function unreadableText() {
  const sources = state.settings?.package?.sources || state.libraries?.info?.sources;
  return (sources ? sources.some(x => x.kind === 'photos') : isMac) ? t('图片暂不可读，请检查来源文件夹，或 iCloud 照片的下载状态与访问权限')
    : t('图片暂不可读，请检查来源文件夹是否还在、外接硬盘是否接上');
}
export const syncBar = (s, bar) => s.status === 'recognising' && s.stage === 'hashing' ? bar(s.input_done, s.input_total)
  : s.status === 'recognising' && s.stage !== 'preparing' ? bar(s.done, s.total)
  : s.status === 'importing' && s.import_total ? bar(s.import_done, s.import_total) : '';
// 预计剩余时间（#360）：新图库第一次识别要十几个小时，满两小时起按小时说；不到两小时按整小时说太粗，仍按分钟。
function remaining(ms) {
  const minutes = Math.max(1, Math.round(ms / 60000));
  return minutes < 120 ? t('约 {n} 分钟', {n: minutes}) : t('约 {n} 小时', {n: Math.round(minutes / 60)});
}
// 识别和导入分开计数：done/total 是已识别或已放弃的图片，import_done/import_total 是已识别并写进图库的。
export function syncLine(s) {
  if (s.status === 'waiting') return state.characterBase?.running
    ? t('基础角色索引更新结束后开始同步') : t('等后台任务结束后开始同步');
  if (s.status === 'scanning') return s.scan_total == null ? t('正在检查来源…') : t('正在检查文件 {done}/{total}', {done: number(s.scanned || 0), total: number(s.scan_total)});
  if (s.status === 'importing') {
    if (!s.import_total) return t('正在导入…');
    return s.stage === 'verifying' ? t('正在校验识别结果（{n} 张）…', {n: number(s.import_total)})
      : t('正在导入识别结果 {done}/{total}', {done: number(s.import_done || 0), total: number(s.import_total)});
  }
  if (s.status === 'recognising') {
    if (s.stage === 'hashing') return t('正在准备识别图片 {done}/{total}', {done: number(s.input_done || 0), total: number(s.input_total || 0)});
    if (s.stage === 'preparing') return t('正在加载识别模型（共 {n} 张）…', {n: number(s.total || 0)});
    const failed = s.failed ? ' · ' + t('{n} 张无法识别', {n: number(s.failed)}) : '';
    const left = s.ms_per_image && s.total > s.done ? ' · ' + remaining((s.total - s.done) * s.ms_per_image) : '';
    const imported = s.import_done ? ' · ' + t('已导入 {n}', {n: number(s.import_done)}) : '';
    return t('正在识别新图片 {done}/{total}', {done: number(s.done || 0), total: number(s.total || 0)}) + failed + imported + left;
  }
  // 扫描读不到的路径保留它们的图片；下次同步再读。
  const kept = s.unreadable ? ' · ' + t('{n} 处暂时读不到，已有图片保留不动', {n: number(s.unreadable)}) : '';
  // 放弃的图片分开说（#112）：读不到的会再试，损坏的等它们的文件改变。
  const gave = (s.failed ? ' · ' + t('{n} 张暂时无法识别，下次同步重试', {n: number(s.failed)}) : '')
    + (s.damaged ? ' · ' + t('{n} 张已损坏或格式不支持，未识别', {n: number(s.damaged)}) : '');
  if (s.status === 'done') return t('上次同步：{time}', {time: stamp(s.finished_at)}) + kept + gave;
  // 自己下载模型的安装（#168）在等模型；否则就是识别服务没有启动。
  if (s.status === 'needs_recognition') {
    const vars = {n: number(s.total || 0)};
    if (baseDownloadStatus() === 'waiting') return t('有 {n} 张图等待识别（正在下载基础角色索引）', vars) + kept;
    return (state.models?.on_demand && state.models.status !== 'ready'
      ? t('有 {n} 张图等待识别（识别模型还没下载）', vars) : t('有 {n} 张图等待识别（识别服务未启动）', vars)) + kept;
  }
  if (s.status === 'interrupted') return s.message ? t('同步已中断（{reason}），可重新扫描', {reason: tb(s.message)}) : t('同步已中断，可重新扫描');
  if (s.status === 'error') return photosRefused(s) ? t('同步失败：没有访问照片图库的权限') : t('同步失败');
  return t('本地图库');
}
// 同步进行中每 2 s 读一次；读失败 10 s 后再试。重叠的轮询由 core/poll.js 的序号守卫。
export const pollSync = poller(async fresh => {
  let reply;
  try { reply = await api('/api/sync'); } catch (e) {
    // 读不到同步状态：记下来、通知一次（按天视图的同步卡写「正在重新连接」），照旧按间隔重试。
    if (fresh() && !state.syncLost) { state.syncLost = true; await changed.emit('sync', {was: state.sync.status, lost: true}); }
    throw e;
  }
  const {state: s, history, damaged} = reply;
  if (!fresh()) return 0;
  state.syncLost = false;
  const previous = state.sync, was = previous.status, ended = SYNC_ACTIVE.includes(was) && !SYNC_ACTIVE.includes(s.status);
  state.sync = s; state.syncHistory = history; state.syncDamaged = damaged;
  // 文件夹里的图片可在扫描时计入 imported，后续用 import_done 判断识别结果何时交付。
  const arrived = (s.imported || 0) > (previous.imported || 0) || (s.import_done || 0) > (previous.import_done || 0) || ended;
  await changed.emit('sync', {was, ended, arrived});
  return SYNC_ACTIVE.includes(s.status) ? 2000 : 0;
});
export const syncDoneText = s => t('同步完成：新加入 {n} 张', {n: number(s.imported)});
export async function startSync() { const r = await post('/api/sync', {}); state.sync = {status: r?.queued ? 'waiting' : 'scanning'}; }

// ---------- 外部索引的新角色 ----------
export const INDEX_ACTIVE = ['checking', 'fetching', 'cleaning', 'building', 'scanning', 'importing'];
export function indexLine(s, check) {
  const n = s.total ? ` ${number(s.done || 0)}/${number(s.total)}` : '';
  return {checking: t('正在检查 Danbooru…'), fetching: t('正在下载参考图') + n, cleaning: t('正在处理参考图'), building: t('正在更新角色特征'), scanning: t('正在重新扫描图库'), importing: t('正在导入图库'),
    paused: t('添加角色已暂停，等你确认后继续') + n,
    error: s.kind === 'add' ? t('添加角色失败：{reason}', {reason: tb(s.message || '')}) : s.kind === 'drop' ? t('移除角色失败：{reason}', {reason: tb(s.message || '')}) : s.kind === 'base' ? t('基础角色索引更新失败：{reason}', {reason: tb(s.message || '')}) : t('检查失败：{reason}', {reason: tb(s.message || '')})}[s.status] || (check ? t('上次检查：{time} · 待添加 {n} 个角色', {time: stamp(check.checked_at), n: number(check.missing.length)}) : t('尚未检查'));
}
export const pollIndex = poller(async fresh => {
  const was = state.index?.state.status;
  const progress = await api('/api/index?progress=1');
  if (!fresh()) return 0;
  // 只在可能有变化时才完整读取（作品、检查结果）；空闲时的轮询保持轻量。
  const full = !state.index?.works || (!INDEX_ACTIVE.includes(progress.state?.status) && progress.state?.status !== was) ? await shared('/api/index') : null;
  if (!fresh()) return 0;
  setIndex(full || {...state.index, ...progress});
  const s = state.index.state;
  await changed.emit('index', {was, ended: INDEX_ACTIVE.includes(was) && !INDEX_ACTIVE.includes(s.status)});
  return INDEX_ACTIVE.includes(s.status) ? 2000 : 0;
});
// 一次运行结束时要说什么，按它怎样结束。
export function indexEndText(s) {
  // 首次准备时图库里还没有识别结果，没有图片要更新，就不说张数。
  if (s.status === 'done' && s.kind === 'base' && !s.changed) return s.enabled ? t('基础角色索引已启用') : t('基础角色索引已撤回');
  if (s.status === 'done' && s.kind === 'base') return s.enabled ? t('基础角色索引已启用，{n} 张图片的识别结果已更新', {n: number(s.changed)}) : t('基础角色索引已撤回，{n} 张图片的识别结果已更新', {n: number(s.changed)});
  if (s.status === 'done') return s.kind === 'drop' ? t('已移除，{n} 张图片已重新识别', {n: number(s.changed)}) : t('已添加 {added} 个角色，{n} 张图片的识别结果已更新', {added: number(s.added), n: number(s.changed)});
  if (s.status === 'checked') return s.found ? t('发现 {n} 个可添加的角色，在任务面板里添加', {n: number(s.found)}) : t('未发现新角色');
  if (s.status === 'paused') return t('添加角色已暂停，在任务面板里继续');
  if (s.status === 'error') return t('角色索引更新失败：{reason}', {reason: tb(s.message || '')});
  return '';
}

// 关注列表、最低帖子数、移除角色：回应是索引的新状态。
export async function indexAction(body) { setIndex(await post('/api/index', body)); }
// 一次检查会访问 Danbooru，所以每次都要询问（confirm(message, title) 解析为回答）；期间切换了图库就不检查。
export async function checkIndex(confirm) {
  const token = session.token;
  if (!await confirm(t('本次检查会联网查询 Danbooru，只发送关注作品的 tag，不发送图库图片。允许本次检查联网吗？'), t('检查新角色')) || token !== session.token) return false;
  const result = await post('/api/index', {action: 'check', allow_network: true});
  if (token !== session.token) return false;
  setIndex(result); return true;
}

// 暂停的添加（重启之后，还缺的图片没有人允许联网）在用户再次允许联网后用一次新的运行继续；已有的图片保留（#287）。
export async function resumeIndex(confirm) {
  const token = session.token;
  if (!await confirm(t('继续下载参考图会联网访问 Danbooru，只下载还缺的公开参考图，已下载的不会重下。允许本次联网吗？'), t('继续添加角色')) || token !== session.token) return false;
  const result = await post('/api/index', {action: 'resume', allow_network: true});
  if (token !== session.token) return false;
  setIndex(result); return true;
}
// 放弃一次暂停的添加会删掉它下载的图片；它没有往图库里写任何东西。
export async function discardIndex(confirm) {
  const token = session.token;
  if (!await confirm(t('放弃这次添加？已下载的参考图会删掉，图库不受影响。'), t('放弃添加'), t('放弃')) || token !== session.token) return false;
  const result = await post('/api/index', {action: 'discard'});
  if (token !== session.token) return false;
  setIndex(result); return true;
}

// ---------- 显示名：Bangumi 优先，用户选择开启的模型 API 最后 ----------
export const NAMES_ACTIVE = ['fetching', 'matching', 'translating', 'running'];
export const namesLine = s => s.status === 'off' ? t('当前未启用') : NAMES_ACTIVE.includes(s.status) ? t('正在获取 {progress}', {progress: s.total ? `${number(s.done || 0)} / ${number(s.total)}` : '…'}) : s.status === 'error' ? t('获取失败，将在下次启动时重试') : s.status === 'unauthorized' ? t('未允许联网查询 · 可在设置 › 网络与隐私里允许') : t('已是最新');
export const pollNames = poller(async fresh => {
  const was = state.names?.state?.status, running = NAMES_ACTIVE.includes(was);
  const progress = await api('/api/names?progress=1');
  if (!fresh()) return 0;
  // 运行期间保留现有的覆盖率和设置；完成时刷新一次。
  const full = !state.names?.counts || (running && !NAMES_ACTIVE.includes(progress.state?.status)) ? await api('/api/names') : null;
  if (!fresh()) return 0;
  setNames(full || {...state.names, ...progress});
  const now = NAMES_ACTIVE.includes(state.names.state?.status);
  await changed.emit('names', {was, ended: running && !now});
  return now ? 2000 : 0;
});
// 设置卡片刚显示的一次运行，2 s 后再轮询。
export const pollNamesSoon = () => pollNames.later(2000);
export async function refreshNames() { mergeNames(await post('/api/names', {action: 'refresh'})); }

// ---------- 相似图片（它的状态随 /api/settings 一起来） ----------
export const SIMILAR_ACTIVE = ['reading', 'pairing', 'comparing', 'waiting'];
export function similarLine(s) {
  if (s.status === 'reading') return t('正在读图算签名 {done}/{total}', {done: number(s.done || 0), total: number(s.total || 0)});
  if (s.status === 'pairing') return t('正在找候选…');
  if (s.status === 'comparing') return t('正在逐对比较 {done}/{total} · 已找到 {found} 对', {done: number(s.done || 0), total: number(s.total || 0), found: number(s.found || 0)});
  if (s.status === 'waiting') return tb(s.message) || t('输入有变化，稍后自动重试');
  if (s.status === 'error') return t('出错：{reason}', {reason: tb(s.message || '')});
  if (s.status === 'off') return t('本次启动关闭了相似检测');
  return s.at ? t('上次检查 {time}', {time: stamp(s.at)}) : '';
}
export const taskSimilar = s => s.status === 'off' ? t('这次启动没有开相似检测') : similarLine(s) || t('已是最新');

// ---------- 找回画师（docs/PROVENANCE.md 第 8 节） ----------
export const PROV_ACTIVE = ['running', 'stopping'];
export const RUN_STATUS = {done: t('完成'), stopped: t('已停止'), failed: t('出错'), running: t('进行中')};
export const loadProvenance = poller(async fresh => {
  const was = state.provenance?.state?.status;
  let answer; try { answer = await api('/api/provenance'); } catch {
    if (!fresh()) return 0;
    // 一次读取失败不能结束对进行中运行的轮询：保留已显示的内容，再试一次。
    if (PROV_ACTIVE.includes(was)) return 4000;
    state.provenance = null; await changed.emit('provenance', {was, ended: false});
    return 0;
  }
  if (!fresh()) return 0;
  state.provenance = answer;
  const s = state.provenance?.state?.status;
  await changed.emit('provenance', {was, ended: PROV_ACTIVE.includes(was) && !PROV_ACTIVE.includes(s)});
  return PROV_ACTIVE.includes(s) ? 1500 : 0;
});
// 启动（{action: 'start', allow_network}）或停止一次运行；两种情况都会再轮询。
export async function provenanceAction(body) {
  try { state.provenance = await post('/api/provenance', body); } finally { loadProvenance(); }
}

// ---------- 导入（#88）：归档或识别结果，一次一个 ----------
// 和其余 *_ACTIVE 一样保留 export：native/terms 的 app_activity 测试按 `export const X = [...]` 这一行核对后端的进行中状态。
export const IMPORT_ACTIVE = ['running', 'stopping'];
export const importActive = () => IMPORT_ACTIVE.includes(state.importTask?.status);
export const IMPORT_STAGES = {
  archive: {reading: t('正在读取归档'), unpacking: t('正在解压'), cleaning: t('正在清除模型连接设置和联网授权'), checking: t('正在检查新图库能否打开'), publishing: t('正在登记新图库')},
  human_conclusions: {reading: t('正在读取文件'), matching: t('正在核对图片'), validating: t('正在校验'), committing: t('正在提交')},
  recognition: {models: t('正在核对模型指纹'), reading: t('正在读取结果包'), verifying: t('正在校验识别结果'), writing: t('正在写入识别结果'), committing: t('正在提交'), projecting: t('正在更新图库索引')},
};
export function importCount(job) {
  if (job.done == null || !job.total) return '';
  if (job.stage === 'unpacking') return ` ${megabytes(job.done)}/${megabytes(job.total)} MB`;
  if (job.stage === 'reading') return t('（第 {n}/{total} 个文件）', {n: Math.min(job.done + 1, job.total), total: job.total});
  return ' ' + t('{done}/{total} 张', {done: number(job.done), total: number(job.total)});
}
export function importLine(job) {
  return sentences(importSummary(job), diagnosticLine(job?.diagnostic));
}
export function importSummary(job) {
  if (!job || job.status === 'idle') return '';
  const recognition = job.kind === 'recognition';
  if (job.kind === 'human_conclusions') return humanSummary(job);
  const name = job.file ? t('「{file}」', {file: job.file}) : '';
  if (job.status === 'running') return (recognition ? t('导入识别结果{name}：{step}', {name, step: (IMPORT_STAGES[job.kind] || {})[job.stage] || t('正在准备')}) : t('导入归档{name}：{step}', {name, step: (IMPORT_STAGES[job.kind] || {})[job.stage] || t('正在准备')})) + importCount(job) + '…';
  if (job.status === 'stopping') return recognition ? t('正在取消导入识别结果{name}…', {name}) : t('正在取消导入归档{name}…', {name});
  if (job.status === 'cancelled') return t('{reason}；图库没有改动。', {reason: tb(job.reason) || t('导入已取消')});
  if (job.status === 'error') return (recognition ? t('导入识别结果{name}失败：{reason}', {name, reason: tb(job.error)}) : t('导入归档{name}失败：{reason}', {name, reason: tb(job.error)}));
  if (job.status !== 'done') return '';
  const r = job.result || {};
  // 已发布但没有登记（#111）：打开它就会登记；再导入会另建一个带编号的图库。
  if (job.kind === 'archive' && r.listed === false) return t('已导入「{name}」，保存在 {path}，但没能加入最近图库列表：{reason}。打开它会再试一次加入列表；再导入会另建一个带编号的图库。模型接口与名称联网授权需重新设置。', {name: r.name, path: home(r.path), reason: tb(r.error)});
  if (job.kind === 'archive') return t('已导入「{name}」，模型接口与名称联网授权需重新设置。', {name: r.name});
  if (r.status === 'pending') return t('识别结果已导入，后续处理尚未完成（{reason}）。', {reason: tb(r.error) || t('可稍后恢复')});
  return r.duplicate ? t('这份识别结果之前已经导入过。') : t('识别结果已导入。');
}
// 人工结论的导入和从自动快照恢复（#441）：同一个导入槽。完成后的报告和撤销由 core/human.js、ui/human.js 处理。
function humanSummary(job) {
  if (job.status === 'running') return t('导入人工结论：{step}', {step: IMPORT_STAGES.human_conclusions[job.stage] || t('正在准备')}) + (job.stage === 'matching' ? importCount(job) : '') + '…';
  if (job.status === 'stopping') return t('正在取消导入人工结论…');
  if (job.status === 'cancelled') return t('{reason}；图库没有改动。', {reason: tb(job.reason) || t('导入已取消')});
  if (job.status === 'error') return t('导入人工结论失败：{reason}', {reason: tb(job.error)});
  if (job.status !== 'done') return '';
  return job.result?.undo != null ? t('人工结论已导入。') : t('人工结论导入完成，没有新内容。');
}
// 几次轮询会重叠（启动的回应、定时器、取消）：更早的回应最后才到时不能让状态倒退，页面之后换了会话的回应丢掉（#34）。
export const pollImports = poller(async fresh => {
  const token = session.token, current = () => fresh() && token === session.token;
  let job; try { job = await api('/api/imports'); } catch { return current() ? 5000 : 0; }
  if (!current()) return 0;
  const was = state.importTask;
  state.importTask = job;
  await changed.emit('import', {was: was?.status, ended: IMPORT_ACTIVE.includes(was?.status) && was.id === job.id && !IMPORT_ACTIVE.includes(job.status)});
  return IMPORT_ACTIVE.includes(job.status) ? 700 : 0;
});
// 已启动的导入（POST /api/libraries/import 的回应）立即显示，并一直轮询到它结束。
export function importStarted(task) { state.importTask = task; changed.emit('import', {was: undefined, ended: false}); pollImports(); }
export async function cancelImport() {
  try { await api('/api/imports/cancel', {method: 'POST', body: '{}'}); } finally { pollImports(); }
}
