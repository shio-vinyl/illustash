// 一次写入或一个后台任务的回应会改动页面别处的什么（桌面布局，#199）：图库的计数和图片、显示任务的设置卡片、向导。
// app.js 把这些交给 onPost() 和 onJob()。
import {SYNC_ACTIVE, importLine} from '../core/jobs.js';
import {packFinished} from '../core/packs.js';
import {HUMAN_IMPORT} from '../core/human.js';
import {metaWritten, refreshMeta} from '../core/meta.js';
import {onPage, pageOf} from '../core/pages.js';
import {loadLibraries} from '../core/settings.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {catalogChanged} from '../core/catalog.js';
import {error, toast} from './dom.js';
import {afterSync, listChanged, reload, selCount} from './grid.js';
import {inspect} from './detail.js';
import {guideAfterPost, guideRefresh} from './guide.js';
import {humanImportEnded, renderHumanBlock} from './human.js';

// 不改图片列表的写入（#452）：引导进度、画廊、收藏（不让图离开本页的那条路径在本地改心形；会离开的走 ui/grid.js 的
// mutate，自己重读）、设置，以及启动后台任务（任务结束时由 afterJob 处理）。这些写入之后已读到的列表照旧有效。
const KEEPS_LIST = new Set(['/api/onboarding', '/api/galleries', '/api/favorite', '/api/startup', '/api/cache', '/api/backup',
  '/api/names', '/api/sync', '/api/index', '/api/provenance', '/api/similar', '/api/regroup', '/api/integrity', '/api/entities', '/api/oc/shared']);
// 后端接受了一次写入：可能变了的计数和读取，以及向导的下一步。
export function afterWrite(path, body) {
  guideAfterPost(path, body);
  if (KEEPS_LIST.has(path)) return;
  listChanged();
  // 防止更早的元数据读取覆盖刚完成的改动。
  metaWritten(path === '/api/tag');
}
// 角色资料包（#602）的启用、换包、重新检查、自动检查或撤回改了图库（result.changed 不为 0）：候选、归属和计数都可能
// 变了，详情栏开着的那张图重新读取。
export async function afterPack(before) {
  const task = packFinished(before, state.pack);
  if (task?.status !== 'done' || !task.result?.changed) return;
  listChanged();
  await refreshMeta().catch(e => error(readError(e)));
  if (state.focused) inspect(state.focused, true);
}
// 后台任务的回应在任务面板之外改动什么：图库的计数和图片、显示这个任务的设置卡片，以及向导。
export async function afterJob(job, change) {
  if (job === 'sync') {
    if (change.arrived) {
      listChanged();
      await refreshMeta().catch(e => error(readError(e)));
      // 面板页自己重画（按天视图的分组和计数来自它自己的读取，同步落定后重画一次）。
      if (!pageOf(state.route).panel && !selCount()) reload();
      else if (!SYNC_ACTIVE.includes(state.sync.status)) pageOf(state.route).refresh?.('sync');
    }
    if (change.ended && state.libraries?.folder_changes?.some(c => c.state === 'pending')) await loadLibraries();
    guideRefresh(change.ended);
    // 同步期间撤销或确认的角色，同步结束后研究才学（#404）：落定后重读一次。
    if (change.ended) afterSync().catch(e => error(readError(e)));
  } else if (job === 'index') {
    const s = state.index.state;
    if (change.ended && s.status === 'done') { listChanged(); await refreshMeta().catch(e => error(readError(e))); }
    if (s.status !== change.was) catalogChanged.emit('index');
  } else if (job === 'names') {
    if (change.ended) refreshMeta();
  }
  else if (job === 'import') {
    const task = state.importTask;
    if (change.ended && task.kind === HUMAN_IMPORT) { if (task.status === 'done' && (task.committed ?? task.result?.committed)) listChanged(); humanImportEnded(task); }
    else if (change.ended) { toast(importLine(task)); if (task.status === 'done' && task.kind === 'archive') loadLibraries(); }
    renderHumanBlock();
  }
}
