// 「设置 › 数据 › 备份与存储」里的人工结论快照（#502）：自动快照的状态和清单、导出、从文件导入、从自动快照恢复，以及完成后
// 的报告对话框和撤销。数据、请求和文字在 core/human.js；导入的进度和取消沿用 core/jobs.js 的导入槽。
import {t, tb} from '../core/i18n.js';
import {esc, home, number, localStamp} from '../core/format.js';
import {state} from '../core/state.js';
import {edits} from '../core/edits.js';
import {onPage} from '../core/pages.js';
import {refreshMeta} from '../core/meta.js';
import {readError} from '../core/session.js';
import {IMPORT_STAGES, importActive, importCount, importLine, cancelImport} from '../core/jobs.js';
import {HUMAN_IMPORT, exportActive, rfcSeconds, loadHuman, onHuman, startExport, cancelExport, startHumanImport, restoreSnapshot, isRestore,
  readReport, REPORT_PAGE, adoptUndo, humanError, COUNT_GROUPS, resultText, kindText, reasonText, fieldText, conflictCount} from '../core/human.js';
import {$, toast, error, fold, wireFolds} from './dom.js';
import {closeModal, openDialog, confirmAction} from './dialog.js';

const when = sec => (sec ? esc(localStamp(sec)) : '—');
const dot = tone => `<span class="local-dot${tone ? ' ' + tone : ''}"></span>`;
// 自动快照的状态行：最近一次成功、有没有还没进快照的改动、失败原因和下次重试。
function autoLine(a) {
  let text, tone = '';
  const hint = [];
  if (a.running) { text = t('正在生成自动快照…'); tone = 'busy'; }
  else if (a.last_error) {
    text = t('自动快照失败：{reason}', {reason: esc(tb(a.last_error))}); tone = 'warn';
    if (a.retry_at) hint.push(t('{time} 重试', {time: when(a.retry_at)}));
    if (a.last_success) hint.push(t('上次成功 {time}', {time: when(a.last_success)}));
  } else if (a.last_success) text = t('最近一次自动快照：{time}', {time: when(a.last_success)});
  else { text = t('还没有自动快照'); tone = 'off'; }
  if (a.pending_revision != null) hint.push(a.next_attempt ? t('有改动还没进快照，预计 {time} 生成', {time: when(a.next_attempt)}) : t('有改动还没进快照'));
  else if (a.last_success && !a.last_error) hint.push(t('之后的改动都已经在快照里'));
  return `<div class="setting-line set-status"><span>${dot(tone)}${text}${hint.length ? `<small class="line-hint">${hint.join(' · ')}</small>` : ''}</span></div>`;
}
const coverage = c => (!c ? '' : c.missing ? t('{n} 张图缺内容摘要', {n: number(c.missing)}) : t('图片摘要齐全'));
function snapshotList(a, busy) {
  const list = [...(a.snapshots || [])].sort((x, y) => (rfcSeconds(y.created_at) || 0) - (rfcSeconds(x.created_at) || 0));
  if (!list.length) return '';
  const row = s => `<tr><td>${when(rfcSeconds(s.created_at))}</td><td>${coverage(s.hash_coverage)}</td>
    <td><button class="quiet small" data-human="restore" data-backup="${esc(s.backup_id)}" data-at="${when(rfcSeconds(s.created_at))}" ${busy ? 'disabled' : ''}>${t('恢复…')}</button></td></tr>`;
  return fold('humanSnapshots', t('最近的自动快照（{n} 份）', {n: number(list.length)}), `<p class="muted-note">${t('恢复只补上这个图库现在缺少的结论；和已有结论冲突的，保留已有的。')}</p>
    <table class="settings-table">${list.map(row).join('')}</table>`);
}
// 导出的状态：进行中可以取消；完成后写明文件在哪，并提醒另存到别处。
function exportLine(x) {
  if (!x || x.status === 'idle') return '';
  if (x.status === 'running') {
    const step = {reading: t('正在读取'), writing: t('正在写文件'), committing: t('正在保存')}[x.stage] || t('正在准备');
    return `<p class="muted-note import-line">${t('正在导出人工结论：{step}…', {step})} <button class="quiet small" data-human="cancel-export">${t('取消导出')}</button></p>`;
  }
  if (x.status === 'stopping') return `<p class="muted-note import-line">${t('正在取消导出…')}</p>`;
  if (x.status === 'cancelled') return `<p class="muted-note import-line">${t('导出已取消，没有生成新文件。')}</p>`;
  if (x.status === 'error') return `<p class="muted-note import-line">${esc(humanError(x.code, 'export', false, x.error))}</p>`;
  if (x.status !== 'done') return '';
  const r = x.result || {}, n = key => number(r[key]?.exported || 0);
  return `<p class="muted-note import-line">${t('已导出到 {path}', {path: `<bdi>${esc(home(r.path))}</bdi>`})}<br>
    ${t('{records} 条人工结论 · {assets} 张图 · {definitions} 个角色和作品', {records: n('counts'), assets: n('asset_counts'), definitions: n('definition_counts')})}
    ${r.hash_coverage?.missing ? '<br>' + t('其中 {n} 张图当时读不到，没能记下内容摘要：导回时这些图上的结论认不回来。', {n: number(r.hash_coverage.missing)}) : ''}</p>`;
}
// 导入或恢复的状态，和图库卡片里的是同一个导入槽。
function importBlock(job) {
  if (job?.kind !== HUMAN_IMPORT || job.status === 'idle') return '';
  const restore = isRestore(job);
  if (job.status === 'error') return `<p class="muted-note import-line">${esc(humanError(job.code, restore ? 'restore' : 'import', job.committed, job.error))}</p>`;
  const button = job.status === 'running' ? ` <button class="quiet small" data-human="cancel-import">${restore ? t('取消恢复') : t('取消导入')}</button>`
    : job.status === 'done' ? ` <button class="quiet small" data-human="report">${t('查看报告')}</button>` : '';
  return `<p class="muted-note import-line">${esc(restore ? restoreLine(job) : importLine(job))}${button}</p>`;
}
// 恢复和导入共用导入槽；状态行写明是恢复。
function restoreLine(job) {
  if (job.status === 'running') {
    const count = job.stage === 'matching' ? importCount(job) : '';
    return t('从自动快照恢复：{step}', {step: IMPORT_STAGES.human_conclusions[job.stage] || t('正在准备')}) + count + '…';
  }
  if (job.status === 'stopping') return t('正在取消恢复…');
  if (job.status === 'done') return job.result?.undo != null ? t('已从自动快照恢复。') : t('从自动快照恢复完成，没有新内容。');
  return importLine(job);
}
// 最近一份快照缺内容摘要时的说明（#516）。快照只用已有摘要，缺的在后台补算：hash_unreadable 是补算后仍读不到的张数，
// 补算还没跑完时为空。把「还没算」和「真的读不到」分开说；refill 表示都补齐了、后台马上再存一份。
function hashNote(a) {
  const n = a.hash_missing, m = a.hash_unreadable;
  if (!n) return '';
  const lost = t('从这份快照恢复时，缺摘要的图上的结论认不回来，会在报告里列出；其余的图不受影响。');
  let text;
  if (m == null) text = t('最近一份快照里有 {n} 张图还没有内容摘要，正在后台读取这些图补算。', {n: number(n)}) + lost;
  else if (m === 0) text = a.refill ? t('最近一份快照里有 {n} 张图还没有内容摘要，已经补算好，马上会再存一份快照。', {n: number(n)})
    : t('最近一份快照里有 {n} 张图还没有内容摘要，已经补算好，下一份快照会补上。', {n: number(n)});
  else if (m >= n) text = t('最近一份快照里有 {n} 张图当时读不到，没能记下内容摘要：从它恢复时，不论是原图库还是换了位置的图库，这些图上的结论都认不回来，会在报告里列出；其余的图不受影响。', {n: number(n)});
  else text = t('最近一份快照里有 {n} 张图还没有内容摘要。', {n: number(n)}) + t('其中 {n} 张图现在读不到，补不上；其余的已经补算好，下一份快照会补上。', {n: number(m)}) + lost;
  return `<p class="muted-note">${text}</p>`;
}
let readFailed = null;
// #humanBlock 的内容；h 是 GET /api/human-conclusions 的回应，job 是导入槽的任务。
export function humanBlock(h = state.human, job = state.importTask) {
  if (!h) return `<p class="muted-note">${readFailed ? esc(readError(readFailed)) : t('正在读取…')}</p>`;
  const a = h.automatic || {}, busy = exportActive() || importActive();
  const off = busy ? ` disabled title="${t('有导入或导出正在进行，等它结束再试')}"` : '';
  return `<p class="set-sub-note">${t('确认、移出、合并、原创角色这些人工整理的结论，单独存成一个小文件：可以导回这个图库，或者换了位置、换了电脑的图库；只补上缺少的部分，冲突时保留目标图库已有的。')}</p>
    ${autoLine(a)}
    ${hashNote(a)}
    ${a.cleanup_error ? `<p class="muted-note">${t('清理旧快照没有成功：{reason}。新快照照常生成。', {reason: esc(tb(a.cleanup_error))})}</p>` : ''}
    ${a.unreadable_snapshots ? `<p class="muted-note">${t('另有 {n} 份快照读不出来，不能用来恢复。', {n: number(a.unreadable_snapshots)})}</p>` : ''}
    ${snapshotList(a, busy)}
    <div class="set-actions"><button class="quiet small" data-human="export"${off}>${t('导出人工结论')}</button><button class="quiet small" data-human="import"${off}>${t('从文件导入…')}</button></div>
    ${exportLine(h.export)}${importBlock(job)}
    <p class="set-foot">${t('自动快照存在图库包里，挡不住整个图库被删。导出的文件默认存到「下载」文件夹，请再另存一份到别的硬盘、U 盘或云盘。')}</p>`;
}
const drawn = new WeakMap();
export function renderHumanBlock() {
  const node = $('humanBlock'); if (!node || !onPage(state.route, 'settings')) return;
  const html = humanBlock();
  if (drawn.get(node) === html) return;
  drawn.set(node, html); node.innerHTML = html;
  wireFolds(node);
}

// ---------- 报告 ----------
// 汇总：每一类计数各列一张表，不相加；冲突写明保留了目标图库的结论；没有改动时说没有新内容、不给撤销。
// extra 是接在后面的明细区。能不能撤销由对话框里的「撤销这次导入」按钮表示，正文不另写（#528）。
// 引导句在上面，其余放进一个可滚动的区域，明细展开后对话框也不会超出窗口。
export function reportSummary(r, extra = '') {
  const lines = [];
  const lead = r.undo == null ? t('没有新内容：图库没有改动，也没有可以撤销的步骤。')
    : t('已经写进这个图库。');
  const note = text => lines.push(`<p class="muted-note">${text}</p>`);
  if (r.committed && r.pending_effects) note(t('收录结果还在后台更新，稍后会自己刷新，不需要重新导入。'));
  if (conflictCount(r)) note(t('有 {n} 条和这个图库已有的结论冲突，都保留了这个图库的结论。', {n: number(conflictCount(r))}));
  const f = r.definition_fields || {};
  if (f.name_group_conflict || f.link_conflict) {
    note(t('另有 {groups} 个名字组、{links} 个链接和已有的不同，同样保留了已有的。', {groups: number(f.name_group_conflict || 0), links: number(f.link_conflict || 0)}));
  }
  if (r.hash_coverage?.missing) note(t('文件里有 {n} 张图没有内容摘要，这些图上的结论认不回来。', {n: number(r.hash_coverage.missing)}));
  // 四类计数放在同一张表里，各有标题行，不出合计。
  const rows = COUNT_GROUPS.map(([key, title]) => {
    const found = Object.entries(r[key] || {}).filter(([, n]) => n);
    const body = found.map(([k, n]) => `<tr><td>${esc(resultText(k, key))}</td><td>${number(n)}</td></tr>`).join('');
    return found.length ? `<tr><th colspan="2">${title}</th></tr>${body}` : '';
  }).join('');
  if (!rows) note(t('文件或快照里没有人工结论。'));
  if (rows) lines.push(`<table class="settings-table">${rows}</table><p class="set-sub-note">${t('这几类分开计数，口径不同，不能相加。')}</p>`);
  return `<p class="dialog-lead">${lead}</p><div class="index-add-list">${lines.join('')}${extra}</div>`;
}
const detailRow = d => `<tr><td>${esc(kindText(d.kind))}</td>
  <td>${esc([reasonText(d.reason), fieldText(d.field_counts)].filter(Boolean).join(' · '))}</td><td class="mono">${esc(d.record_key)}</td></tr>`;
// 一页明细，按 result 分组；reason 写成看得懂的话，实体另附名字组、链接的计数。
export function reportDetails(page, total) {
  const details = page.details || [], groups = new Map();
  for (const d of details) { if (!groups.has(d.result)) groups.set(d.result, []); groups.get(d.result).push(d); }
  const group = d => (d.kind === 'asset' ? 'asset_counts' : d.kind === 'entity' ? 'definition_counts' : 'counts');
  const rows = [...groups].map(([result, list]) => `<tr><th colspan="3">${t('{label}（{n}）', {label: esc(resultText(result, group(list[0]))), n: number(list.length)})}</th></tr>
    ${list.map(detailRow).join('')}`).join('');
  const from = details.length ? page.offset + 1 : 0, to = page.offset + details.length;
  return `<p class="muted-note">${t('第 {from}–{to} 条，共 {n} 条', {from: number(from), to: number(to), n: number(total)})}</p>
    <table class="settings-table">${rows}</table>
    <div class="set-actions"><button type="button" class="quiet small" data-page="${page.offset - REPORT_PAGE}" ${page.offset > 0 ? '' : 'disabled'}>${t('上一页')}</button>
      <button type="button" class="quiet small" data-page="${to}" ${to < total ? '' : 'disabled'}>${t('下一页')}</button></div>`;
}
// 导入或恢复完成后的报告。只读，点背景可以关。
function openReport(r, restore = false) {
  if (!r) return null;
  const undoable = r.committed && r.undo != null && edits.latest?.id === r.undo;
  const d = openDialog({title: restore ? t('从自动快照恢复：结果') : t('导入人工结论：结果'), backdropClose: true,
    body: reportSummary(r, r.detail_count
      ? `<div data-details><button type="button" class="quiet small" data-page="0">${t('查看明细（{n} 条）', {n: number(r.detail_count)})}</button></div>` : ''),
    actions: `${undoable ? `<button type="button" class="quiet" data-human-undo>${t('撤销这次导入')}</button>` : ''}<button value="ok" class="primary" autofocus>${t('完成')}</button>`});
  d.addEventListener('click', async e => {
    const page = e.target.closest('[data-page]'), back = e.target.closest('[data-human-undo]');
    if (page) {
      const box = d.querySelector('[data-details]'); page.disabled = true;
      try { box.innerHTML = reportDetails(await readReport(r.report_id, Math.max(0, Number(page.dataset.page))), r.detail_count); }
      catch (err) { toast(readError(err)); page.disabled = false; }
    } else if (back) {
      // 顶栏的撤销总是撤销最近一步：这之后又有别的改动时，不能从这里跳过它们去撤销导入。
      if (edits.latest?.id !== r.undo) { toast(t('这次导入之后又有别的改动，要先撤销那些改动。')); return; }
      closeModal(d); await hooks.undo?.();
    }
  });
  return d;
}

// 一次人工结论导入或恢复结束（ui/effects.js 的 afterJob 调用，图片列表的作废由它做）。
export function humanImportEnded(task) {
  const restore = isRestore(task), r = task.result || {};
  if (task.status === 'done') {
    adoptUndo({...r, committed: task.committed ?? r.committed});
    if (task.committed ?? r.committed) refreshMeta().catch(e => error(readError(e)));
    openReport(r, restore);
  } else if (task.status === 'error') toast(humanError(task.code, restore ? 'restore' : 'import', task.committed, task.error));
  else toast(importLine(task));
  loadHuman();
}
function exportEnded() {
  const x = state.human?.export; if (!x) return;
  if (x.status === 'done') toast(t('人工结论已导出到 {path}', {path: home(x.result?.path)}));
  else if (x.status === 'error') toast(humanError(x.code, 'export', false, x.error));
}
// 卡片里的按钮；设置页的 click 监听调用它。
export async function onHumanClick(e) {
  const b = e.target.closest('[data-human]'); if (!b || !$('humanBlock')?.contains(b)) return;
  const act = b.dataset.human, op = act === 'export' || act === 'cancel-export' ? 'export' : act === 'restore' ? 'restore' : 'import';
  if (act === 'report') { openReport(state.importTask?.result, isRestore(state.importTask)); return; }
  b.disabled = true;
  try {
    if (act === 'export') await startExport();
    else if (act === 'cancel-export') await cancelExport();
    else if (act === 'import') await startHumanImport();
    else if (act === 'cancel-import') await cancelImport();
    else if (act === 'restore') {
      const note = t('用 {time} 的自动快照补回人工结论？\n\n只补上这个图库现在缺少的结论；和已有结论冲突的，保留已有的。完成后会显示报告。', {time: b.dataset.at});
      if (await confirmAction(note, t('从自动快照恢复'), t('恢复'))) await restoreSnapshot(b.dataset.backup);
    }
  } catch (err) { toast(humanError(err.status, op, false, err.message)); }
  finally { b.disabled = false; renderHumanBlock(); }
}
// 不从 ui/grid.js 导入（它在 ui/ 的循环导入里）：撤销由设置页传进来。
const hooks = {undo: null};
// 设置页启动时调用一次；undo 是顶栏撤销按钮用的那个函数。
export function wireHuman({undo}) {
  hooks.undo = undo;
  onHuman(({ended, error: e}) => {
    readFailed = e && !state.human ? e : null;
    renderHumanBlock();
    if (ended) exportEnded();
  });
}