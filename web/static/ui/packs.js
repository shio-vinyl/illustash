// 角色资料包（#602，合并了 #550 WD 角色扩展和 #559 CCIP 长尾包的两张卡片）：「设置 › 角色资料」里的一张卡片的状态行、
// 按钮、失败列表、新版本和包信息（都是数据，卡片由 ui/settings_characters.js 用行画，#714），确认框，任务面板里的一项，
// 以及结束时的提示。任务面板的项由 ui/tasks.js 列出；状态、轮询和请求在 core/packs.js。对用户是一个包、一个版本、一个开关；
// 两个组成部分（WD 扩展头、CCIP 长尾锚点）只在包信息的折叠区里分开列。
// 用户点「下载」之前什么都不下载（首次准备时点了下载识别模型的，后端一起下了这个包）；导入只读用户选的文件；启用、换包和
// 撤回都不联网。签名目录里有更新的版本时多一行「有新版本」：点了才下载，下好以后点了才换。
import {t, tb, I18N} from '../core/i18n.js';
import {esc, bytes, number, localTime, home} from '../core/format.js';
import {state} from '../core/state.js';
import {packDownloading, packRunning, packFinished, packDownloadFinished, loadPack, onPack, packAction, enablePack, withdrawPack,
  declinePack, cancelPack, packOffer, packChanged} from '../core/packs.js';
import {toast} from './dom.js';
import {confirmAction} from './dialog.js';

export const packName = () => t('角色资料包');
const current = () => state.pack;
const pkg = x => x?.current || {};
const lib = x => x?.library || {};
const size = x => bytes(pkg(x).size || 0);
const left = x => Math.max(0, (pkg(x).size || 0) - (pkg(x).have || 0));
const ready = x => pkg(x).status === 'ready';
const importFailed = d => d?.kind === 'import' && d.status === 'failed';
const importError = d => d.message ? t('角色资料包没能导入：{reason}', {reason: tb(d.message)}) : t('角色资料包没能导入');
// 两个组成部分启用了任何一个都算已启用（换包时一边没换成也是）。
const on = x => !!lib(x).enabled;
// 不能启用或重新检查的原因（library.blocked）。没有识别结果也能启用：新识别的图交付时再检查。
const BLOCKED = {frozen: () => t('图库只读'), syncing: () => t('正在同步，等它结束'), busy: () => t('图库正在导入或导出')};
export const blockedReason = (x = current()) => {
  const b = lib(x).blocked;
  return !b || b === 'no_vectors' ? '' : BLOCKED[b]?.() || b;
};
const partNote = task => task?.result?.wd ? t('；') + t('WD 扩展部分已经更新') : '';
// 版本号：测试版（stage 为 preview，2026-11-18 用户定）后面写「（测试版）」，只是显示不同。纯文本，调用方转义。
export const packVersion = p => p?.stage === 'preview' ? t('{version}（测试版）', {version: p.version}) : p?.version || '';

// 已启用时的一行：扩展候选覆盖几张图，其中几张的归属由它补上。两个数字每次读取时现算，两个组成部分合在一起按图去重。
function enabledLine(x) {
  const {images = 0, assigned = 0} = lib(x);
  if (!images) return t('已启用 · 没有扩展候选');
  return assigned ? t('已启用 · {n} 张图有扩展候选，其中 {assigned} 张由它补上归属', {n: number(images), assigned: number(assigned)})
    : t('已启用 · {n} 张图有扩展候选', {n: number(images)});
}
// 状态行：正在发生什么，或者现在是什么样。纯文本，调用方转义。
export function packLine(x = current()) {
  if (!x) return '…';
  const task = x.task, d = x.download, l = lib(x);
  if (packRunning(x)) {
    if (task.kind === 'withdraw') return t('正在撤回…');
    if (task.phase === 'write') return t('正在写入…');
    // 刚开始、还没数出要检查几张时（total 为 0）不写「0 / 0」。
    if (!task.total) return t('正在检查…');
    return t('正在检查 {done} / {total} 张', {done: number(task.done || 0), total: number(task.total || 0)});
  }
  if (packDownloading(x)) {
    // 首次准备时先读一次包目录（#630），读完再按它下载。
    if (d.kind === 'catalog') return t('正在读取角色资料包目录');
    if (d.kind === 'import') return t('正在核对所选文件 {done} / {total}', {done: bytes(d.done), total: bytes(d.total)});
    return t('正在下载 {done} / {total}', {done: bytes(d.done), total: bytes(d.total)}) + (d.speed ? ' · ' + t('{speed}/秒', {speed: bytes(d.speed)}) : '')
      + (d.source ? ' · ' + t('来自 {source}', {source: d.source}) : '') + (d.proxy ? ' · ' + t('经代理 {proxy}', {proxy: d.proxy}) : '');
  }
  if (importFailed(d)) return importError(d);
  if (task?.status === 'failed') {
    const reason = tb(task.message || '');
    if (task.kind === 'auto') return t('检查新识别的图没有完成，图库没有改动：{reason}', {reason});
    // 两个组成部分分别提交：WD 部分提交以后 CCIP 部分失败的，写明 WD 部分已经更新。
    if (task.result?.wd) return t('CCIP 长尾部分没有完成：{reason}', {reason}) + partNote(task);
    if (task.kind === 'enable' && l.enabled) return t('重新检查没有完成，图库没有改动：{reason}', {reason});
    return task.kind === 'withdraw' ? t('撤回没有完成，图库没有改动：{reason}', {reason}) : t('启用没有完成，图库没有改动：{reason}', {reason});
  }
  if (task?.status === 'cancelled') return task.result?.wd ? t('已取消') + partNote(task) : t('已取消，图库没有改动');
  if (on(x)) return enabledLine(x);
  if (!ready(x) && d?.kind === 'download' && d.status === 'failed' && !packOffer(x)?.downloading) {
    return t('角色资料包没能下载：{reason}', {reason: tb(d.message || '') || t('每个来源都试过了')});
  }
  if (!ready(x) && d?.kind === 'download' && d.status === 'cancelled') return t('已取消');
  if (!ready(x)) return pkg(x).status === 'partial' ? t('角色资料包还没下载完（已有 {have}，共 {size}）', {have: bytes(pkg(x).have), size: size(x)})
    : t('角色资料包还没下载（约 {size}）', {size: bytes(left(x))});
  if (l.prompt === 'ask') return t('角色资料包现在默认启用，本图库还没启用');
  if (l.prompt === 'auto') return t('第一次同步时启用');
  // 新图库第一次同步时包还没到（#630）：包到了以后，等同步等后台任务结束就启用。
  if (l.prompt === 'pending') return t('等后台任务结束后启用');
  return t('本图库未启用');
}
// 状态行左边的圆点：进行中、出错、没有启用。
export function packDot(x = current()) {
  if (!x) return 'off';
  if (packRunning(x) || packDownloading(x)) return 'busy';
  const ccipError = x.ccip?.library?.error;
  if (x.task?.status === 'failed' || importFailed(x.download) || (!ready(x) && x.download?.status === 'failed') || (on(x) && ccipError)) return 'warn';
  return on(x) ? '' : 'off';
}
// 进度 {done, total}（ui/rows.js 的 progress）；写入阶段不报百分比。
export function packProgress(x = current()) {
  const job = packRunning(x) ? x.task : packDownloading(x) ? x.download : null;
  return !job?.total || (x.task === job && job.phase === 'write') ? null : {done: job.done || 0, total: job.total};
}
// 按钮的数据（[{act, label, primary, disabled, title}]，#714）：设置页的卡片用 ui/rows.js 的 buttons 接，任务面板的行
// （ui/task_row.js）建成按钮；点了都走 runPack(act)。
const button = (act, label, primary = false, title = '') => ({act, label, primary, disabled: !!title, title});
// 当前状态下的按钮；`compact` 只留主要的那个（任务面板）。
export function packButtons(x = current(), compact = false) {
  if (!x) return [];
  const l = lib(x), task = x.task, blocked = blockedReason(x);
  if (packRunning(x)) {
    if (task.kind === 'withdraw') return [];
    return [task.phase === 'write' ? button('cancel', t('取消'), false, t('正在写入，不能取消')) : button('cancel', t('取消'))];
  }
  if (packDownloading(x)) return x.download.kind === 'catalog' ? [] : [button('package-cancel', t('取消'))];
  const withdraw = on(x) ? [button('withdraw', t('撤回…'))] : [];
  // 重新检查失败时照样能撤回，不只能重试。重试照失败的那个任务的目标版本再做一遍（换包失败就再换那个版本）。显示的失败
  // 任务不是这个页面记下的那一个（页面重新载入过，或者后来的操作被宿主拒绝、没有起任务），又有下好的新版本时，不给
  // 「重试」：说不准它是查旧版本还是换包，换包走下面「有新版本」那一行。
  if (task?.status === 'failed') {
    if (task.kind === 'withdraw') return [button('withdraw-retry', t('重试'), true)];
    const unsure = retryVersion(task) === undefined && on(x) && packOffer(x)?.status === 'ready';
    return [...(unsure ? [] : [button('enable-retry', t('重试'), true, blocked)]), ...(compact ? [] : withdraw)];
  }
  if (!ready(x)) {
    const failed = x.download?.kind === 'download' && x.download.status === 'failed' && !packOffer(x)?.downloading;
    const partial = pkg(x).status === 'partial';
    const download = button('download', failed ? t('重试') : partial ? t('继续下载（还差 {left}）', {left: bytes(left(x))})
      : t('下载角色资料包（约 {size}）', {size: bytes(left(x))}), !on(x));
    if (compact) return [download];
    return [download, button('import', t('从文件导入…')), ...(partial ? [button('discard', t('删除未完成的部分'))] : []), ...withdraw];
  }
  // CCIP 部分的角色索引更新过：重新检查后才用上，按钮升为主按钮。
  if (on(x)) return compact ? [] : [button('enable', t('重新检查'), !!x.ccip?.library?.needs_scan, blocked), ...withdraw];
  if (l.prompt === 'ask') return [button('enable-confirm', t('启用…'), true, blocked), ...(compact ? [] : [button('decline', t('不用'))])];
  return [button('enable-confirm', t('启用…'), true, blocked)];
}
// 没就位时的手动下载说明（链接）。
export function packManual(x = current()) {
  const manual = x?.wd?.package?.manual;
  return x && !ready(x) && manual && !packRunning(x) && !packDownloading(x) ? {href: manual, label: t('手动下载说明')} : null;
}
// 状态行下面的小字：包不在本机、索引更新后要重新检查、上次自动检查的错误、上次检查时间、启用会补归属，以及没就位时的
// 手动下载说明。
export function packHintText(x = current()) {
  if (!x || packRunning(x) || packDownloading(x)) return '';
  const l = lib(x), parts = [], ccip = x.ccip?.library || {};
  if (on(x) && !ready(x)) parts.push(t('角色资料包不在这台电脑上，新识别的图不检查'));
  if (on(x) && ccip.needs_scan) parts.push(t('角色索引更新过，重新检查后才用上'));
  if (on(x) && ccip.error) parts.push(t('上次自动检查没有完成：{reason}', {reason: tb(ccip.error)}));
  if (on(x) && l.checked_at) parts.push(t('上次检查 {time}', {time: localTime(l.checked_at)}));
  if (!on(x) && l.prompt === 'ask' && ready(x)) parts.push(t('启用会给还没有归属的图补上机器归属'));
  if (!on(x) && l.prompt === 'pending' && !ready(x)) parts.push(t('下载完成后启用'));
  return parts.join(' · ');
}
// 导入对所选的每个文件做了什么（两个组成部分的结果已经在后端按文件名合并）。只在这个页面发起的导入结束后列出：包文件
// 整个 App 一份，导入结果也是，换了图库（页面重新载入）或别的窗口导入的不列。只留在发起导入的那一页（state.route 每次换页
// 都是新的），切走再回来就不列了（#643）。离线包里随附的许可和说明（extra）不是要导入的文件，不列。
let importedHere = null;
// 宿主接受了的启用、重新检查或换包：{task: 起的任务 ID, version: 换包是那个版本号，启用和重新检查是 null（卡片上的版本）}。
// 宿主拒绝的（409 之类，没起任务）不记。失败以后的「重试」只对同一个任务照它再做。
let enableTarget = null;
async function enable(version) {
  const value = await enablePack(version);
  if (value?.task?.id) enableTarget = {task: value.task.id, version: version || null};
  return value;
}
// 这个失败任务重试时用的版本：是这个页面记下的那个任务就照记下的；对不上时 undefined。
const retryVersion = task => enableTarget && task?.id === enableTarget.task ? enableTarget.version : undefined;
const RESULT = {accepted: () => t('已接收'), mismatch: () => t('SHA-256 不符，已拒绝'), already: () => t('已经有了'),
  unknown: () => t('不是角色资料包'),
  // 离线包里的包目录和签名（#602）。
  catalog_file_installed: () => t('角色词表已导入，重启后生效'),
  catalog_file_invalid: () => t('无法导入角色词表'), catalog_file_unsupported: () => t('当前应用不采用此角色词表修订'),
  catalog: () => t('包目录，已读取'), catalog_old: () => t('包目录比本机的旧，没有采用'), catalog_invalid: () => t('包目录签名不对，已拒绝')};
// 包信息（设置卡片的折叠区，#714 改成数据）：存放位置（所有图库共用）、两个组成部分的文件、版本和许可，每项
// {key, text, hint}；没有包信息时为空。
export function packInfoItems(x = current()) {
  const p = pkg(x); if (!p.components) return [];
  const file = (key, label, c) => ({key, text: label, hint: `${c.file.path} · ${bytes(c.file.size)} · ${String(c.file.sha256).slice(0, 12)}… · ${t('{n} 个角色', {n: number(c.heads || 0)})}`,
    title: 'SHA-256 ' + c.file.sha256});
  return [
    ...(x.location ? [{key: 'location', text: t('存放位置'), hint: `${home(x.location)} · ${t('所有图库共用')}`, title: x.location}] : []),
    file('wd', t('WD 扩展头'), p.components.wd), file('ccip', t('CCIP 长尾锚点'), p.components.ccip),
    {key: 'version', text: t('版本'), hint: `${packVersion(p)} · ${t('{n} 个角色', {n: number(p.heads || 0)})}`},
    {key: 'license', text: t('许可'), hint: tb(p.license || '')}];
}
// 已就位时可以删除，正在下载或检查时不行。
export const packDeleteButtons = (x = current()) => !ready(x) || !pkg(x).components ? []
  : [button('delete', t('删除角色资料包'), false, packDownloading(x) || packRunning(x) ? t('正在下载或检查，等它结束再删除') : '')];
// 「有新版本」（#602）：签名目录里比卡片上的版本新的最新一个。没下载时可以下载；正在下载时进度在上面的状态行；下好以后
// 已启用的图库可以换成它（没启用的图库卡片上直接就是它）。返回 {what, hint, buttons}，没有新版本时为 null。
export function packOfferInfo(x = current(), offer = packOffer(x)) {
  if (!x || !offer) return null;
  const n = number(offer.heads || 0), m = offer.candidate_only || 0, version = packVersion(offer);
  const what = m ? t('有新版本 {version}：{n} 个角色，其中 {m} 个只给候选', {version, n, m: number(m)})
    : t('有新版本 {version}：{n} 个角色', {version, n});
  const note = offer.note && (I18N.language === 'zh-CN' ? offer.note['zh-CN'] : offer.note.en);
  const downloading = offer.downloading && packDownloading(x);
  const failed = offer.downloading && x.download?.kind === 'download' && x.download.status === 'failed' && offer.status !== 'ready';
  const busyNow = packDownloading(x) || packRunning(x) ? t('正在下载或检查，等它结束') : '';
  let buttons = [];
  if (offer.status === 'ready') {
    if (on(x)) buttons = [button('offer-switch', t('换成新版本…'), true, blockedReason(x) || busyNow)];
  } else if (!downloading) {
    // 按还要下的算：新版本只换了一个组成部分时，另一个已经在本机。
    const rest = bytes(Math.max(0, (offer.size || 0) - (offer.have || 0)));
    const label = failed ? t('重试') : offer.status === 'partial' ? t('继续下载新版本（还差 {left}）', {left: rest}) : t('下载新版本（约 {size}）', {size: rest});
    buttons = [button('offer-download', label, !on(x), busyNow)];
  }
  const hint = [failed ? t('新版本没能下载：{reason}', {reason: tb(x.download.message || '') || t('每个来源都试过了')}) : '', note || ''].filter(Boolean).join(' · ');
  return {what, hint, buttons};
}
// 下载卡住时每个来源各自失败的原因，导入时对每个文件做了什么：设置卡片里列出的几项 {key, text}（#714 改成数据）。
export function packResultItems(x = current()) {
  const d = x?.download;
  const fails = d?.status === 'failed' ? (d.failures || []).map((f, i) => ({key: 'f' + i, text: t('{source}：{why}', {source: f.source, why: tb(f.why)})})) : [];
  const list = (d?.results || []).filter(r => r.result !== 'extra');
  const imported = importedHere !== state.route || d?.kind !== 'import' || packDownloading(x) ? []
    : list.map((r, i) => ({key: 'r' + i, text: r.name, hint: RESULT[r.result]?.() || r.result}));
  return [...fails, ...imported];
}
// 卡片状态行的「在任务面板查看」：有过下载或任务时。
export const packHasTask = (x = current()) => !!(x?.download || x?.task);
export const packNote = (x = current()) => t('多认 {n} 个 WD 词表和角色索引里没有的角色。', {n: number(pkg(x).heads || 0)});

// 任务面板的一项：下载或导入、启用、撤回共用。运行中和结束后都列出来（同导入、导出）。
export function packTask(x = current()) {
  if (!x || !(x.download || x.task)) return null;
  const running = packRunning(x), downloading = packDownloading(x);
  const busyText = running && x.task.kind === 'withdraw' ? t('撤回角色资料包中') : running ? t('检查角色资料包中')
    : x.download?.kind === 'import' ? t('导入角色资料包中') : t('下载角色资料包中');
  const d = x.download;
  const failed = importFailed(d) ? d : x.task?.status === 'failed' ? x.task : !ready(x) && d?.status === 'failed' ? d : null;
  return {key: 'pack', name: packName(), busy: busyText, active: running || downloading, warn: !!failed, off: packDot(x) === 'off',
    line: packLine(x), progress: packProgress(x), body: '', buttons: packButtons(x, true).map(b => ({...b, attr: 'pack'})), diagnostic: failed?.diagnostic};
}

// 结束时的提示。`before` 是任务运行时最后一次读到的状态：那时已经启用的，这一次是重新检查或换包。同步后对新识别的图自动
// 检查（kind 为 auto）只在有改动或失败时提示。CCIP 部分没有兼容向量的图一并说明。
export function finishedText(task, before, x = current()) {
  const name = packName(), images = lib(x).images || 0;
  const skipped = (task.result?.ccip?.missing || 0) + (task.result?.ccip?.incompatible || 0);
  const more = text => skipped ? text + t('；') + t('{n} 张图缺少兼容的向量，没有检查', {n: number(skipped)}) : text;
  const updated = () => more(t('{name}已更新：{n} 张图有扩展候选', {name, n: number(images)}));
  if (task.kind === 'auto' && task.status !== 'failed') return task.status === 'done' && task.result?.changed ? updated() : '';
  if (task.status === 'failed' || task.status === 'cancelled') return packLine(x);
  if (task.kind === 'withdraw') return t('已撤回角色资料包');
  if (lib(before).enabled) return !task.result?.changed ? more(t('没有变化')) : updated();
  return more(images ? t('{name}已启用：{n} 张图有扩展候选', {name, n: number(images)}) : t('{name}已启用，没有找到扩展候选', {name}));
}
// 下载结束的提示。`before` 是下载期间最后一次读到的状态：下的是不是新版本看那时的 `offer.downloading`。
export function downloadText(d, before, x = current()) {
  if (packOffer(before)?.downloading && d.kind === 'download') {
    return d.status === 'done' ? t('新版本已下载') : d.status === 'failed' ? t('新版本没能下载：{reason}', {reason: tb(d.message || '')}) : '';
  }
  if (importFailed(d)) return importError(d);
  if (d.status === 'failed' && d.kind === 'download') return packLine(x);
  return ready(x) && d.status === 'done' ? t('角色资料包已就位') : '';
}

// 确认框只写做决定前要知道的事实。数字取自状态：启用前要检查的张数（library.vectors），撤回时由它补上的归属数。
const scanText = n => n ? t('用角色资料包检查本图库 {n} 张图。', {n: number(n)}) : t('本图库还没有识别结果，新识别的图会用角色资料包检查。');
const tierText = m => m ? t('其中 {m} 个角色只给候选，不补归属。', {m: number(m)}) : '';
export function enableMessage(x = current()) {
  return scanText(lib(x).vectors) + t('还没有归属、也没有人工确认的图会补上一个机器归属，已有的归属和人工结论不变。')
    + tierText(pkg(x).candidate_only) + t('以后可以撤回。');
}
export function withdrawMessage(x = current()) {
  const n = lib(x).assigned || 0;
  return (n ? t('去掉本图库里角色资料包给出的全部候选，以及它补上的 {n} 个机器归属。', {n: number(n)}) : t('去掉本图库里角色资料包给出的全部候选。'))
    + t('你确认过、否定过的保留。撤回不进撤销历史，要恢复就重新启用。');
}
// 换成新版本：要重新检查的张数，换掉的是旧版本给的候选和归属；新版本里只给候选的角色写明不补归属。
export function switchMessage(offer = packOffer(), x = current()) {
  return scanText(lib(x).vectors) + t('旧版本给出的扩展候选和由它补上的归属换成新版本的结果，已有的其他归属和人工结论不变。')
    + tierText(offer?.candidate_only) + t('以后可以撤回。');
}

export async function runPack(what) {
  // 导入的结果只留到下一个动作：启用、重新检查、撤回、删除、再导入一次以后不再列出（#559 走查）。
  importedHere = null;
  try {
    if (what === 'download') await packAction('download');
    else if (what === 'offer-download') await packAction('download', packOffer()?.version);
    else if (what === 'offer-switch') {
      const offer = packOffer(); if (!offer) return;
      if (!await confirmAction(switchMessage(offer), t('换成新版本'), t('换成新版本'))) return;
      await enable(offer.version);
    } else if (what === 'import') {
      // 宿主受理了这次导入（选了文件）才登记发起的那一页，并马上通知卡片重画：导入很快就到终态时结果立刻列出来。选择框
      // 开着的时候不登记，等待中的重画不会列出上一次导入的结果；关掉选择框和请求失败都不登记。
      const route = state.route;
      const r = await packAction('import');
      if (!r?.cancelled) { importedHere = route; await packChanged(); }
    }
    else if (what === 'package-cancel') await packAction('cancel');
    else if (what === 'discard') {
      if (!await confirmAction(t('删除已经下载但还没完成的部分？下次要从头下载。'), t('删除未完成的部分'), t('删除'))) return;
      await packAction('discard');
    } else if (what === 'delete') {
      if (!await confirmAction(t('删除角色资料包？已启用的图库保留现有的扩展候选，新识别的图不再检查；再用要重新下载或导入。'), t('删除角色资料包'), t('删除'))) return;
      await packAction('delete');
      toast(t('角色资料包已删除'));
    } else if (what === 'enable-confirm') {
      if (!await confirmAction(enableMessage(), t('启用角色资料包'), t('启用'))) return;
      await enable();
    } else if (what === 'enable') await enable();
    else if (what === 'enable-retry') await enable(retryVersion(current()?.task) || undefined);
    else if (what === 'decline') await declinePack();
    else if (what === 'withdraw') {
      if (!await confirmAction(withdrawMessage(), t('撤回角色资料包'), t('撤回'))) return;
      await withdrawPack();
    } else if (what === 'withdraw-retry') await withdrawPack();
    else if (what === 'cancel') {
      // 取消晚了一步、任务已经进了写入阶段：后端回 409。这是个事实，不是故障，不带错误编号（#559 走查）。
      try { await cancelPack(); } catch (e) {
        if (e.status !== 409) throw e;
        toast(t('已经开始写入，没能取消'));
        loadPack().catch(() => {});
      }
    }
  } catch (e) { toast(e.message); }
}
// 处理函数。页面启动时调用一次。任务面板里的按钮由 ui/tasks.js 分派，设置页的卡片用行，点了都直接调 runPack。
export function wirePacks() {
  onPack(before => {
    const x = current(), task = packFinished(before, x), d = packDownloadFinished(before, x);
    const note = task ? finishedText(task, before) : d ? downloadText(d, before) : '';
    if (note) toast(note);
  });
}
