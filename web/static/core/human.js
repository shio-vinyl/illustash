// 人工结论快照（#441 的接口，界面 #502）：自动快照的状态、导出、导入与从自动快照恢复、报告分页，以及这些状况的文字。
// 不碰 DOM。GET /api/human-conclusions 的回应在 state.human，只由这里写入；导入与恢复走现有的导入槽，进度和取消沿用
// core/jobs.js 的 pollImports。onHuman(fn) 在每次读到新状态（或读取失败）后调用 fn({ended, error})，ended 表示一次导出刚结束。
import {t, tb} from './i18n.js';
import {number} from './format.js';
import {session, api} from './session.js';
import {state} from './state.js';
import {listeners} from './events.js';
import {setLatest} from './edits.js';
import {importStarted} from './jobs.js';
const changed = listeners();
export const onHuman = changed.on;

// 导入槽里人工结论任务的 kind；导出有自己的槽，kind 为 human_export。
export const HUMAN_IMPORT = 'human_conclusions';
const EXPORT_ACTIVE = ['running', 'stopping'];
export const exportActive = () => EXPORT_ACTIVE.includes(state.human?.export?.status);
// RFC3339 的时间（快照清单的 created_at）换成 Unix 秒；读不出时为 null。
export const rfcSeconds = s => { const ms = Date.parse(s || ''); return Number.isNaN(ms) ? null : ms / 1000; };

// 没有副作用的 POST：不经 post()，免得启动或取消一次任务就被当成图库写入（afterWrite 会作废列表）。
const send = (path, body) => api(path, {method: 'POST', body: JSON.stringify(body)});

// 几次读取会重叠（设置页渲染、导出轮询、导入结束）：只采用最后发出的那次；换了会话的回应丢掉（#34）。
let timer = 0, poll = 0;
export async function loadHuman() {
  clearTimeout(timer);
  const mine = ++poll, token = session.token;
  let error = null, ended = false;
  try {
    const value = await api('/api/human-conclusions');
    if (mine !== poll || token !== session.token) return;
    const was = state.human?.export;
    state.human = value;
    ended = EXPORT_ACTIVE.includes(was?.status) && was.id === value.export?.id && !EXPORT_ACTIVE.includes(value.export?.status);
  } catch (e) {
    if (mine !== poll || token !== session.token) return;
    error = e;
  }
  await changed.emit({ended, error});
  // 导出或自动快照正在进行时继续读；读取失败时按上次知道的状态决定，间隔放长。
  if (mine === poll && (exportActive() || state.human?.automatic?.running)) timer = setTimeout(loadHuman, error ? 5000 : 1000);
}

// ---------- 导出 ----------
export async function startExport() {
  const task = await send('/api/human-conclusions/export', {});
  state.human = {...state.human, export: task};
  await changed.emit({ended: false, error: null});
  loadHuman();
}
// 只取消这一次导出；编号已变（另一次导出）时后端回 409。
export async function cancelExport() {
  const id = state.human?.export?.id; if (!id) return;
  try { await send('/api/human-conclusions/export/cancel', {id}); } finally { loadHuman(); }
}

// ---------- 导入与恢复 ----------
// 哪次导入任务是「从自动快照恢复」开始的（按任务编号）；重新载入页面后不知道的按导入算。
const restores = new Set();
export const isRestore = task => restores.has(task?.id);
// 导入文件：不传 path，由后端打开原生选择器；用户在选择器里取消解析为 null，不算错误。
export async function startHumanImport() {
  const r = await send('/api/human-conclusions/import', {});
  if (r?.cancelled) return null;
  const task = r?.task || r; importStarted(task); return task;
}
export async function restoreSnapshot(backupId) {
  const r = await send('/api/human-conclusions/restore', {backup_id: backupId});
  const task = r?.task || r; restores.add(task.id); importStarted(task); return task;
}

// 报告：导入报告保存在图库里（重启、撤销之后仍能读），导出报告只有本会话最近一次。明细按 offset/limit 分页（1–500）。
export const REPORT_PAGE = 50;
export const readReport = (id, offset = 0, limit = REPORT_PAGE) =>
  api('/api/human-conclusions/report?' + new URLSearchParams({id, offset: String(offset), limit: String(limit)}));

// ---------- 撤销 ----------
// 导入有有效改动时，结果里的 undo 是这一步的编号：接到现有的撤销控件上。label 与后端 actions 表里这一步的名字相同。
export const IMPORT_LABEL = t('已导入人工结论');
const importSteps = new Set();
export function adoptUndo(result) {
  if (!result?.committed || result.undo == null) return false;
  importSteps.add(result.undo);
  setLatest({id: result.undo, label: IMPORT_LABEL});
  return true;
}
// 撤销失败时说什么：撤销导入返回 409，是后面有改动用到了导入的内容，整步撤销会丢掉它们。
export const undoRefusal = (e, step) => (e?.status === 409 && importSteps.has(step?.id)
  ? t('这次导入之后，又有改动用到了导入的内容，不能整步撤销。要撤销，先撤销后面的改动。') : e?.message || String(e));

// ---------- 文字 ----------
// 400、413、409、500 各一句话；committed 表示错误发生在发布之后（结论已经写进图库）。reason 是后端的原话，附在后面。
export function humanError(code, op = 'import', committed = false, reason = '') {
  // 500 时后端的原话是固定的一句（让人去复制诊断信息），和这里的说法重复，不再附上。
  const tail = reason && code !== 500 ? t('（{reason}）', {reason: tb(reason)}) : '';
  if (committed) return t('人工结论已经导入，但之后的处理出错了，不要重复导入。可以在「设置 › 关于」复制诊断信息。') + tail;
  const text = op === 'export' ? {
    409: t('现在不能导出：可能已有导出或导入在进行，或者图库刚被切换。请稍后再试。'),
    500: t('写导出文件时出错，没有生成文件。可以在「设置 › 关于」复制诊断信息。'),
  }[code] : {
    400: op === 'restore' ? t('这份快照已经不在了，或没有通过完整性检查；图库没有改动。') : t('这个文件不是可用的人工结论文件，或者格式版本不受支持；图库没有改动。'),
    413: t('文件太大：超过 256 MiB、有一行超过 4 MiB，或超过 200 万条记录。图库没有改动。'),
    409: t('现在不能导入：可能有别的导入或导出在进行、图库刚被切换、图库需要先升级，或者快照在这期间变了。图库没有改动，请稍后再试。'),
    500: t('读写文件或数据库时出错，图库没有改动。可以在「设置 › 关于」复制诊断信息。'),
  }[code];
  return (text || t('操作失败。')) + tail;
}

// 报告里的四类计数，口径不同，分开显示、不相加。各类的结果名见 #441 的接口定稿。
export const COUNT_GROUPS = [
  ['counts', t('人工结论')], ['asset_counts', t('涉及的图片')], ['definition_counts', t('依赖的角色和作品')], ['definition_fields', t('已有角色和作品的名字、链接')]];
const RESULTS = {
  exported: t('已导出'),
  applied: t('补上了'),
  already_present: t('这个图库已经有了'),
  conflict: t('和这个图库已有的结论冲突，保留了已有的'),
  blocked_dependency: t('依赖的角色或作品没能建立'),
  unsupported_dependency: t('依赖的内容暂不支持导入'),
  unmatched_asset: t('这个图库里找不到对应的图'),
  ambiguous_asset: t('对应的图不止一张，无法确定'),
  unverified_asset: t('没能核验图片内容'),
  content_changed: t('图片内容已经变了'),
  matched: t('认回了'),
  matched_dependency: t('对上了已有的'),
  unneeded_dependency: t('用不上，没有新建'),
  name_group_applied: t('补上的名字组'),
  name_group_already_present: t('已经有的名字组'),
  name_group_conflict: t('冲突的名字组，保留了已有的'),
  name_group_unsupported_dependency: t('暂不支持的名字组'),
  name_rows_unsupported_dependency: t('新建时省略的名字'),
  link_applied: t('补上的链接'),
  link_already_present: t('已经有的链接'),
  link_conflict: t('冲突的链接，保留了已有的'),
};
// 依赖定义的 applied 是新建了这个角色或作品，不是补上一条结论。
export const resultText = (result, group = 'counts') =>
  group === 'definition_counts' && result === 'applied' ? t('新建了') : RESULTS[result] || t('其他（{code}）', {code: result});
const KINDS = {
  asset: t('图片'), entity: t('角色或作品'), image_verdict: t('图里是谁'), relation_verdict: t('角色之间的关系'),
  identity_context: t('身份的适用范围'), curation: t('收录或排除'), deferred: t('暂不判断'), uncertain: t('不确定'),
  review: t('已检查'), identity_skip: t('跳过的配对'), identity_keep_apart: t('分成两个，不再问'), anchor_decision: t('参考图'),
};
export const kindText = kind => KINDS[kind] || kind;
const REASONS = {
  same_id_unverified: t('同一张图，但这次没能读到它的内容来核验'),
  duplicate_content: t('图库里有几张内容相同的图，不知道是哪一张'),
  candidate_set_incomplete: t('图库里有些图暂时读不到，不能排除是它们'),
  source_hash_missing: t('导出时没能读到这张图，文件里没有它的内容摘要'),
  multiple_source_assets: t('文件里有几张图都对应到同一张'),
  opaque_or_dangling_pair: t('关系另一端的角色无法识别'),
  reference_version_unverified: t('参考图的版本无法核验'),
};
export const reasonText = reason => !reason ? '' : REASONS[reason] || t('其他原因（{code}）', {code: reason});
// 实体明细里的名字组、链接计数：「补上的名字组 2 · 冲突的链接，保留了已有的 1」。
export const fieldText = counts => Object.entries(counts || {}).filter(([, n]) => n).map(([k, n]) => `${resultText(k, 'definition_fields')} ${number(n)}`).join(' · ');
// 结论里的冲突（不含名字组、链接，它们另算）。
export const conflictCount = summary => Number(summary?.counts?.conflict || 0);
