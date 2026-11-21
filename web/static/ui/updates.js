// 检查更新（#191，native/terms/src/app_updates.rs）的文字和按钮：「设置 › 关于」、侧栏的更新入口和导览里的更新提示
// （ui/guide.js）共用。请求在 core/settings.js；监听由 ui/settings.js 的 wireSettings 挂。
// 用户允许之前关闭：新图库的向导问一次，「设置 › 网络与隐私」里有开关。允许之后，后端在每次启动时检查，App 一直开着时
// 每天检查一次；页面只询问是否该检查了。有新版本时在底部提示，有任务运行时绝不提示；「设置 › 关于」里总有手动检查。
// 桌面 App 里还能「下载并重启」（#279，desktop/src/updater.rs）：下载、校验签名和安装都由外壳做，这里只显示进度、在重启前
// 确认；有任务在运行时不能重启。做不到时（不在桌面 App 里、这个安装不能自己更新、下载或安装失败）退回「打开下载页」，
// 在浏览器里打开 Release 页面。
import {t, tb} from '../core/i18n.js';
import {bytes, esc, localStamp} from '../core/format.js';
import {updatesPost} from '../core/settings.js';
import {state} from '../core/state.js';
import {confirmAction} from './dialog.js';
import {$, buttonsHTML, toast} from './dom.js';

// 外壳的状态，只在这个安装能自己更新时；否则 null。
export const selfUpdate = (u = state.updates) => u?.install?.available ? u.install : null;
// 有没有任务在运行：任务面板（ui/tasks.js）知道，由 ui/settings.js 启动时登记进来（这里不导入任务面板，免得成环）。
let busyNow = () => false;
export const setTasksBusy = fn => { busyNow = fn; };
export const tasksBusy = () => busyNow();
// 下载、下好、失败、安装中：用户点过「下载并重启」之后的这几步。
export const updateStarted = (u = state.updates) => ['downloading', 'ready', 'failed', 'installing'].includes(selfUpdate(u)?.stage);
// 下载进度：{done, total}，没在下载时 null（ui/rows.js 的 progress）。
export const updateProgress = (u = state.updates) => {
  const i = selfUpdate(u);
  return i?.stage === 'downloading' && i.total ? {done: i.received, total: i.total} : null;
};
// 下载进度条的 HTML（导览里的更新提示用）。
export const updateBar = (u = state.updates) => {
  const p = updateProgress(u); if (!p) return '';
  return `<div class="progress" role="progressbar" aria-valuemin="0" aria-valuemax="${p.total}" aria-valuenow="${p.done}">`
    + `<span style="width:${Math.min(100, p.done / p.total * 100).toFixed(1)}%"></span></div>`;
};
// 一行说明：下载到哪里了，或者为什么只能打开下载页。默认转义好放进 HTML；plain 为真时是纯文字（写进节点）。
export function updateStatus(u = state.updates, plain = false) {
  const i = u?.install, version = plain ? i?.version || '' : esc(i?.version || ''), e = plain ? x => x : esc;
  if (i?.stage === 'downloading') {
    return i.total ? t('正在下载 {version}：{done} / {total}', {version, done: bytes(i.received), total: bytes(i.total)}) : t('正在下载 {version}…', {version});
  }
  if (i?.stage === 'ready') {
    return tasksBusy() ? t('{version} 已下载好，等任务结束后就能重启安装', {version}) : t('{version} 已下载好，重启 Illustash 后就能用上', {version});
  }
  if (i?.stage === 'failed') return t('没能自动更新：{reason}。可以打开下载页手动更新。', {reason: e(tb(i.error || ''))});
  if (i?.stage === 'installing') return t('正在安装 {version}…', {version});
  if (i && !i.available && i.reason === 'translocated') return t('把 Illustash 拖进「应用程序」文件夹，再从那里打开，才能在应用里直接更新。');
  return '';
}
// 有新版本时的按钮（[{act, label, primary, disabled, title}]，形状同 ui/rows.js 的 buttons）；能自己更新时「打开下载页」是
// 次要按钮，旁边是「下载并重启」。
export function updateButtons(u = state.updates) {
  const i = selfUpdate(u), open = primary => ({act: 'open', label: t('打开下载页'), primary});
  if (i?.stage === 'downloading') return [{act: 'cancel', label: t('取消下载')}];
  if (i?.stage === 'installing') return [];
  if (i?.stage === 'ready') return [{act: 'restart', label: t('重启并安装'), primary: true, disabled: tasksBusy(), title: tasksBusy() ? t('有任务在运行，等它结束再重启') : ''}];
  if (i?.stage === 'failed') return [open(true), {act: 'install', label: t('重试')}];
  if (!u?.newer) return [];
  return i ? [open(false), {act: 'install', label: t('下载并重启'), primary: true}] : [open(true)];
}
// 同样的按钮拼成 HTML，提示里的用 `data-nudge`。
export const updateActions = (u = state.updates, attr = 'data-updates') => buttonsHTML(updateButtons(u), attr);
// 「设置 › 关于」那一行的标题和说明。
export function updatesTitle(u = state.updates) {
  if (!u) return t('检查更新');
  return u.newer ? t('有新版本 {version}', {version: u.latest}) : u.checked_at && !u.error ? t('已是最新版本') : t('检查更新');
}
export function updatesHint(u = state.updates) {
  if (!u) return '';
  const when = u.checked_at ? t('上次检查 {time}', {time: localStamp(Date.parse(u.checked_at) / 1000)}) : t('还没有检查过', {context: 'update'});
  return [u.newer && updateStatus(u, true), when, u.error && tb(u.error), !u.auto && t('自动检查已关闭，可在「设置 › 网络与隐私」里打开')].filter(Boolean).join(' · ');
}
// 侧栏的更新入口（用户 2026-11-14 要求）：有新版本就一直在，不受「暂不」和任务的影响；没有就不显示。点一下直接开始
// 下载并重启，下载时显示进度（点了打开「设置 › 关于」），下好后点了重启安装（先确认）；不能自己更新的安装点了打开下载页。
export function drawUpdateButton(u = state.updates) {
  const button = $('updateButton'), i = selfUpdate(u), stage = i?.stage;
  button.hidden = !(u?.newer || ['downloading', 'ready', 'installing'].includes(stage));
  if (button.hidden) return;
  const percent = stage === 'downloading' && i.total ? Math.floor(i.received / i.total * 100) : null;
  const label = stage === 'downloading' ? (percent === null ? t('下载中') : t('下载 {percent}%', {percent}))
    : stage === 'ready' ? t('重启更新') : stage === 'installing' ? t('安装中') : stage === 'failed' ? t('更新失败') : t('更新');
  $('updateLabel').textContent = label;
  button.classList.toggle('busy', stage === 'downloading' || stage === 'installing');
  button.title = updateStatus(u, true) || (i ? t('有新版本 {version}，点一下下载并重启', {version: u.latest})
    : t('有新版本 {version}，点一下打开下载页', {version: u.latest}));
}
export const openRelease = () => { if (state.updates?.url) window.open(state.updates.url, '_blank', 'noopener'); };
// 「下载并重启」的几个按钮，提示和「设置 › 关于」共用。重启会关掉 Illustash，所以先确认，有任务在运行时不重启。
export async function updateAct(act) {
  if (act === 'open') { openRelease(); return true; }
  if (act === 'restart') {
    if (tasksBusy()) { toast(t('有任务在运行，等它结束再重启')); return false; }
    const version = selfUpdate()?.version || state.updates?.latest || '';
    const note = t('Illustash 会关闭，装好 {version} 后自己重新打开。图库、设置和识别模型都不受影响。', {version});
    const ok = await confirmAction(note, t('重启并安装更新'), t('重启并安装'));
    if (!ok) return false;
    // 确认框开着的时候可能有任务开始了：再查一次，有任务就不装，已下载的更新留着。
    if (tasksBusy()) { toast(t('有任务在运行，等它结束再重启')); return false; }
  }
  const body = {install: {action: 'download'}, cancel: {action: 'cancel'}, restart: {action: 'install'}}[act];
  if (!body) return false;
  try { await updatesPost(body); } catch (err) { toast(err.message); }
  return true;
}
