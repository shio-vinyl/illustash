// 不带识别模型的安装包在第一次使用时下载的模型（#168）：向导横条、任务面板和「设置 › 数据」共用的文字和按钮（设置页的
// 卡片用这里的数据画成行，在 ui/settings_data.js，#714）。用户点
// 「下载」之前什么都不下载；按钮上写明有多大。拒绝的话还有「从文件导入」和手动下载页面。
import {t, tb} from '../core/i18n.js';
import {esc, bytes, home, number} from '../core/format.js';
import {state} from '../core/state.js';
import {SYNC_ACTIVE, INDEX_ACTIVE, startSync, pollSync, pollIndex, onJob, indexEndText} from '../core/jobs.js';
import {loadModels, modelsPost, modelsWaiting, modelsFinished, modelsArrived, onModels, saveAcceleration, fallbacks,
  loadCharacterBase, characterBasePost, characterBasePath, optionalGaps, baseDownloadStatus} from '../core/models.js';
import {toast, busy, buttonsHTML} from './dom.js';
import {confirmAction} from './dialog.js';
import {loadPack} from '../core/packs.js';

const running = m => m?.job?.status === 'running';
const left = m => Math.max(0, (m?.total || 0) - (m?.have || 0));
const fileSize = n => n < 10000 ? t('{n} 字节', {n: number(n)}) : bytes(n);
// 一行：正在发生什么，或缺了什么。
export function modelsLine(m = state.models) {
  if (!m) return '…';
  const j = m.job;
  if (running(m) && j.kind === 'import') return t('正在核对所选文件 {done} / {total}', {done: bytes(j.done), total: bytes(j.total)});
  const progress = running(m) ? {done: bytes(j.done), total: bytes(j.total)} : null;
  if (running(m)) return (j.file && j.file === characterBasePath()
    ? t('正在下载基础角色索引 {done} / {total}', progress) : t('正在下载 {done} / {total}', progress))
    + (j.speed ? ' · ' + t('{speed}/秒', {speed: bytes(j.speed)}) : '') + (j.source ? ' · ' + t('来自 {source}', {source: j.source}) : '')
    + (j.proxy ? ' · ' + t('经代理 {proxy}', {proxy: j.proxy}) : '');
  if (m.status === 'ready') {
    const gaps = optionalGaps(m);
    if (!m.optional_missing) return t('识别模型已就位');
    if (gaps.base && gaps.wd) return t('识别模型已就位；WD 加速文件和基础角色索引尚未就位。');
    return gaps.base ? t('识别模型已就位；基础角色索引尚未就位，只认得 WD 词表里的角色。') : t('识别模型已就位；WD 加速文件尚未就位，WD 暂用 CPU。');
  }
  if (j?.status === 'failed' && j.failed_file) return t('{path} 没能下载，每个来源都试过了', {path: j.failed_file});
  if (j?.status === 'failed') return t('下载没有完成：{reason}', {reason: tb(j.message || '')});
  if (j?.status === 'cancelled') return t('已取消，下好的部分会保留，下次接着下');
  return m.have ? t('还缺 {list}（已有 {have}）', {list: m.missing.join(t('、')), have: bytes(m.have)}) : t('还没下载（约 {size}）', {size: bytes(m.total)});
}
export function modelsBar(m = state.models) {
  const j = m?.job; if (!running(m) || !j.total) return '';
  return `<div class="progress" role="progressbar" aria-valuemin="0" aria-valuemax="${j.total}" aria-valuenow="${j.done}"><span style="width:${Math.min(100, j.done / j.total * 100).toFixed(1)}%"></span></div>`;
}
// 模型已就位后还能补的（#375）：缺 WD 加速文件时下载，开着加速而上次 WD 没用上时修复。只在会自己下载模型的安装里；
// 自带模型的安装，模型在 App 包里，不往里写。
const extra = m => {
  if (m?.status !== 'ready' || !m.on_demand) return '';
  return m.optional_missing ? 'download' : m.acceleration?.backend === 'coreml' && fallbacks(m).wd ? 'repair' : '';
};
// 当前状态下的按钮（[{act, label, primary, disabled, title}]，#714）：设置页的卡片用 ui/rows.js 的 buttons 接，任务面板和
// 向导横条用 modelsActions 拼成 HTML（data-models="动作"）；点了都走 runModels(act)。`compact` 省掉不常用的那些（任务面板）。
export function modelsButtons(m = state.models, compact = false) {
  if (!m) return [];
  if (running(m)) return [{act: 'cancel', label: t('取消')}];
  if (m.status === 'ready') {
    const want = extra(m); if (!want) return [];
    const gaps = optionalGaps(m), size = bytes(left(m));
    const label = want !== 'download' ? t('修复 WD 加速文件')
      : gaps.base && gaps.wd ? t('下载 WD 加速文件和基础角色索引（约 {size}）', {size})
      : gaps.base ? t('下载基础角色索引（约 {size}）', {size}) : t('下载 WD 加速文件（约 {size}）', {size});
    return [{act: 'download', label}, {act: 'import', label: t('从文件导入…')}];
  }
  const download = {act: 'download', primary: true,
    label: m.have ? t('继续下载（还差 {left}）', {left: fileSize(left(m))}) : t('下载模型（约 {size}）', {size: bytes(m.total)})};
  if (compact) return [download];
  return [download, {act: 'import', label: t('从文件导入…')}, ...(m.partial && m.have ? [{act: 'discard', label: t('删除未完成的部分')}] : [])];
}
export const modelsActions = (m = state.models, compact = false) => buttonsHTML(modelsButtons(m, compact), 'data-models');
// 下载进度 {done, total}，没在下载时 null（ui/rows.js 的 progress）。
export const modelsProgress = (m = state.models) => running(m) && m.job.total ? {done: m.job.done, total: m.job.total} : null;
// 设置卡片里列出的几项 {key, text, hint}：每个来源各自失败的原因，导入对每个选中的文件做了什么。
export function modelsResultItems(m = state.models) {
  const fails = m?.job?.status === 'failed' ? (m.job.failures || []).map((f, i) => ({key: 'f' + i, text: t('{source}：{why}', {source: f.source, why: tb(f.why)})})) : [];
  const job = m?.job, results = job?.kind === 'import' ? job.results || [] : [];
  return [...fails, ...results.map((r, i) => ({key: 'r' + i, text: r.name, hint: RESULT[r.result]?.() || r.result}))];
}
// 下载卡在某个文件上时，每个来源各自失败的原因。
export function modelsFailures(m = state.models) {
  const list = m?.job?.status === 'failed' ? m.job.failures || [] : [];
  return list.length ? `<ul class="models-results">${list.map(f => `<li class="mismatch">${esc(t('{source}：{why}', {source: f.source, why: tb(f.why)}))}</li>`).join('')}</ul>` : '';
}
// 在会自己下载模型的安装里已经就位（用户 2026-10-19 定，#168）：可以删除，正在使用时不行。
export function modelsDeleteButtons(m = state.models) {
  if (!m?.on_demand || m.status !== 'ready') return [];
  const working = running(m) || SYNC_ACTIVE.includes(state.sync?.status) || INDEX_ACTIVE.includes(state.index?.state?.status);
  return [{act: 'delete', label: t('删除已下载的模型'), disabled: working, title: working ? t('正在识别或下载，等它结束再删除') : ''}];
}
// 手动下载说明的链接；已就位、又没有要补下的文件时不给。
export const modelsManual = (m = state.models) => m?.manual && !(m.status === 'ready' && extra(m) !== 'download') ? {href: m.manual, label: t('手动下载说明')} : null;
const manualLink = (m = state.models) => m?.manual ? `<a class="link" href="${esc(m.manual)}" target="_blank" rel="noreferrer noopener">${t('手动下载说明')}</a>` : '';
// 导入对每个选中的文件做了什么。
const RESULT = {accepted: () => t('已接收'), mismatch: () => t('SHA-256 不符，已拒绝'), unknown: () => t('不是需要的模型文件'), already: () => t('已经有了')};
export function importSummary(job) {
  if (job?.kind !== 'import' || !job.results?.length) return '';
  return `<ul class="models-results">${job.results.map(r => `<li class="${r.result}"><bdi>${esc(r.name)}</bdi> · ${RESULT[r.result]?.() || esc(r.result)}</li>`).join('')}</ul>`;
}

// 识别等待模型期间向导的横条，出现在这个安装的每个图库里。
export function modelsNotice() {
  const m = state.models;
  const baseDownload = SYNC_ACTIVE.includes(state.sync?.status) ? null : baseDownloadStatus(m);
  if (!modelsWaiting(m) && !baseDownload) return '';
  // 有事发生之后，这一行才说明现状：下载在进行、已停止，或者已经有一部分在这里。
  const line = running(m) || m.job || m.have ? esc(modelsLine(m)) : '';
  const note = running(m) ? '' : t('识别要用的模型还没下载（约 {size}）。图片会先加入图库，可以浏览和手动整理；首次准备结束后会自动开始识别。', {size: bytes(m.total)});
  const wait = baseDownload === 'waiting' ? t('下载结束后开始识别。') : '';
  return `<div class="models-notice"><span>${[esc(note), line, esc(wait)].filter(Boolean).join(' ')}</span>
    ${modelsBar(m)}${running(m) ? '' : modelsFailures(m)}<div class="models-acts">${modelsActions(m)}${running(m) ? '' : manualLink(m)}</div>${running(m) ? '' : importSummary(m.job)}</div>`;
}

// 「使用 GPU 加速识别」（#270）：只在这个构建带加速器的地方出现。全应用共用（preferences.json）；worker 启动时读取。
// 上一个 worker 没能用上加速器时，说明原因，以及它改用 CPU 继续了。
export const accelerationHint = (m = state.models) => {
  const a = m?.acceleration; if (!a?.available) return '';
  const hint = a.backend === 'coreml'
    ? t('用 Core ML 加速标签与角色特征计算。首次需要准备编译缓存；加速不可用时自动用 CPU。')
    : t('用显卡计算，识别更快，角色判定和只用 CPU 时一致。');
  return `${hint} ${t('关掉就全部用 CPU。')} · ${t('下次识别时生效')}`;
};
export const accelerationFell = (m = state.models) => {
  const fell = m?.acceleration?.available ? Object.values(fallbacks(m))[0] : null;
  return fell ? t('上次没能用上加速，已改用 CPU：{reason}', {reason: tb(fell.reason)}) : '';
};

// 下载或导入刚结束时提示的一句（#375），没什么要说时为空。模型刚到（`arrived`）时说开始识别；首次下载在同一个任务里
// 接着下 WD 加速文件，它没下成时并进同一句（提示条只有一个，后一条会盖掉前一条）。没下成的是可选文件时写「WD 加速文件」。
export function finishedNote(job, arrived, m = state.models) {
  const optional = !!job.failed_file && !!m?.files?.find(f => f.path === job.failed_file)?.optional;
  const base = optional && job.failed_file === characterBasePath();
  const failure = job.status !== 'failed' ? ''
    : base ? t('基础角色索引没能下载，可以在「设置 › 数据」里重试。')
    : optional ? t('WD 加速文件没能下载，WD 暂用 CPU。')
    : job.failed_file ? t('{path} 没能下载，每个来源都试过了', {path: job.failed_file})
    : t('下载没有完成：{reason}', {reason: tb(job.message || '')});
  if (arrived) return base ? t('识别模型已就位，开始识别；基础角色索引没能下载，可以在「设置 › 数据」里重试。')
    : optional ? t('识别模型已就位，开始识别；WD 加速文件没能下载，WD 暂用 CPU。')
    : failure ? t('识别模型已就位，开始识别。{problem}', {problem: failure}) : t('识别模型已就位，开始识别');
  if (failure) return failure;
  return job.status === 'done' && job.kind === 'import' && m?.status !== 'ready' ? t('还缺 {n} 个文件', {n: m.missing.length}) : '';
}
// 镜像说明：清单里有项目自己发布的可选文件时（WD 加速文件只在 Apple 芯片的 macOS 上，基础角色索引在每个平台上），
// 说明它们从哪里下载。
export const mirrorNote = (m = state.models) => {
  const hf = {hf: 'https://huggingface.co'}, base = characterBasePath();
  const wd = m.files.some(f => f.optional && f.path !== base), index = !!base && m.files.some(f => f.path === base);
  return wd && index ? t('原始模型先从这个地址下载，不行再换 ModelScope。填 {hf} 就是直连 Hugging Face。WD 加速文件和基础角色索引从项目的 GitHub Release 下载。', hf)
    : index ? t('原始模型先从这个地址下载，不行再换 ModelScope。填 {hf} 就是直连 Hugging Face。基础角色索引从项目的 GitHub Release 下载。', hf)
    : wd ? t('原始模型先从这个地址下载，不行再换 ModelScope。填 {hf} 就是直连 Hugging Face。WD 加速文件从项目的 GitHub Release 下载。', hf)
    : t('先从这个地址下载，不行再换 ModelScope。填 {hf} 就是直连 Hugging Face。', hf);
};
// 基础角色索引（#607）在这个图库里的状态和按钮；文件还没下载时由上面的下载那一行负责。
export function characterBaseText(b = state.characterBase) {
  if (!b?.included || !b.writable || (!b.available && !b.enabled)) return '';
  const n = number(b.characters);
  return b.running ? t('基础角色索引正在更新')
    : b.enabled ? t('基础角色索引：已启用，{n} 个角色', {n})
    : b.withdrawn ? t('基础角色索引：已撤回') : t('基础角色索引：已下载，这个图库还没启用');
}
export const characterBaseButtons = (b = state.characterBase) => !characterBaseText(b) || b.running ? []
  : b.enabled ? [{act: 'base-withdraw', label: t('撤回')}] : [{act: 'base-enable', label: t('启用')}];
// 文件清单（设置卡片的折叠区）：存放位置和每个文件的大小、SHA-256 开头。
export const modelsFileItems = (m = state.models) => !m ? [] : [{key: 'location', text: t('存放位置'), hint: home(m.location), title: m.location},
  ...m.files.map(f => ({key: f.path, text: f.path, hint: `${fileSize(f.size)} · ${f.sha256.slice(0, 12)}…`, title: 'SHA-256 ' + f.sha256}))];

export async function runModels(what, mirror = null) {
  try {
    if (what === 'download') {
      await modelsPost({action: 'download'});
      // 宿主随后在后台读资料包目录、下载角色资料包（#630）：马上读一次它的状态，卡片接着轮询，不停在旧的版本上（#648）。
      loadPack().catch(() => {});
    }
    else if (what === 'cancel') await modelsPost({action: 'cancel'});
    else if (what === 'import') { const r = await modelsPost({action: 'import'}); if (r.cancelled) return; }
    else if (what === 'base-enable') { await characterBasePost(true); pollIndex(); }
    else if (what === 'base-withdraw') {
      if (!await confirmAction(t('撤回基础角色索引？由它给出的角色候选会删掉，你确认过和移出的结论都保留。撤回对这台电脑上的所有图库生效，之后可以再启用。'), t('撤回基础角色索引'), t('撤回'))) return;
      await characterBasePost(false); pollIndex();
    }
    else if (what === 'delete') {
      if (!await confirmAction(t('删除已下载的识别模型（约 {size}）？删除后要重新下载或从文件导入，才能识别新加入的图片；已经识别的结果都保留。', {size: bytes(state.models.total)}), t('删除识别模型'), t('删除'))) return;
      await modelsPost({action: 'delete'});
      toast(t('识别模型已删除'));
    } else if (what === 'discard') {
      if (!await confirmAction(t('删除已经下载但还没完成的部分？下次要从头下载。'), t('删除未完成的部分'), t('删除'))) return;
      await modelsPost({action: 'discard'});
    } else if (what === 'mirror' || what === 'mirror-reset') {
      await modelsPost({action: 'mirror', mirror: what === 'mirror' ? (mirror ?? '').trim() || null : null});
      toast(t('设置已保存'));
    }
  } catch (e) { toast(e.message); }
}

// 处理函数。页面启动时调用一次。
export function wireModels() {
  document.addEventListener('click', e => {
    const button = e.target.closest('[data-models]'); if (!button || button.disabled) return;
    busy(button, () => runModels(button.dataset.models));
  });
  onModels((before, start) => {
    const job = modelsFinished(before); if (!job) return;
    // 基础角色索引刚下载好时，宿主在下一次恢复检查里为这个图库启用它；稍等再读状态和索引任务。
    const wasEnabled = state.characterBase?.enabled;
    setTimeout(async () => {
      const was = state.index?.state?.status;
      await Promise.all([loadCharacterBase().catch(() => {}), pollIndex()]);
      // 新图库启用得很快，2 秒后第一次读到的已经是结束状态，任务面板等不到它「结束」：在这里说一次（#607）。
      const b = state.characterBase, s = state.index?.state;
      if (wasEnabled === false && b?.enabled && !b.running && !INDEX_ACTIVE.includes(was))
        toast(s?.kind === 'base' && s.status === 'done' ? indexEndText(s) : t('基础角色索引已启用'));
    }, 2000);
    const arrived = modelsArrived(before, start), note = finishedNote(job, arrived);
    if (note) toast(note);
    // 图库里已有的图片等的就是这个：现在就识别它们。
    if (arrived && !SYNC_ACTIVE.includes(state.sync?.status) && state.sync?.status !== 'off') startSync().then(pollSync).catch(e => toast(e.message));
  });
  // 一次同步结束：重新读取模型状态，卡片上的加速回退原因跟着最近一次识别变，不用等打开任务面板。
  onJob((job, change) => {
    if (job === 'sync' && change.ended) { loadModels().catch(() => {}); loadCharacterBase().catch(() => {}); }
    if (job === 'index' && change.ended) loadCharacterBase().catch(() => {});
  });
  loadModels().catch(() => {});
  loadCharacterBase().catch(() => {});
}
