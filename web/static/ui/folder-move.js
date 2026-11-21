// 文件夹来源的位置操作（设置 › 图库的「这个文件夹搬到了…」「认回原图…」「撤销」，d 是 {folderMove} / {folderRecognize} /
// {folderUndo, token}）。所有请求都带选择文件夹或确认候选之前的会话令牌。
import {t} from '../core/i18n.js';
import {esc, home, number} from '../core/format.js';
import {session} from '../core/session.js';
import {loadLibraries} from '../core/settings.js';
import {state} from '../core/state.js';
import {pollSync} from '../core/jobs.js';
import {confirmAction, openDialog} from './dialog.js';
import {toast} from './dom.js';

// 认回或搬家之后的一句结果（设置 › 图库里那个文件夹下面的小字），没有时为空。
export function folderResult(change) {
  if (!change) return '';
  return change.state === 'pending' ? t('等待核对新位置的文件') :
    change.from ? t('已认回 {kept} 张，新增 {added} 张，旧记录未认回 {missing} 张',
      {kept: number(change.kept), added: number(change.new), missing: number(change.missing)}) : t('已认回 {n} 张', {n: number(change.kept)});
}

function confirmMatches(files) {
  return new Promise(resolve => {
    const options = files.map(f => `<option>${esc(home(f.old))} → ${esc(home(f.new))}</option>`).join('');
    openDialog({title: t('认回原图'), backdropClose: true,
      body: `<p>${t('这 {n} 张图片将沿用旧位置的 ID 和整理记录。可以在来源设置里撤销。', {n: number(files.length)})}</p>
        <select class="work-choice" size="6" aria-label="${t('旧位置和新位置')}">${options}</select>`,
      actions: `<button value="cancel" class="quiet" autofocus>${t('取消')}</button><button value="ok" class="primary">${t('认回原图')}</button>`,
      onClose: d => resolve(d.returnValue === 'ok')});
  });
}

export async function changeFolder(d, pick, request) {
  const token = session.token, route = state.route;
  const current = () => token === session.token && route === state.route;
  let result;
  if (d.folderMove) {
    const [path] = await pick(false);
    if (!path || !current()) return;
    result = await request('sources/move', {source: d.folderMove, path}, token);
  } else if (d.folderRecognize) {
    const source = d.folderRecognize, preview = await request('sources/recognize', {source}, token);
    if (!preview || !current()) return;
    if (!preview.files.length) { toast(t('没有可认回的图片')); return; }
    if (!await confirmMatches(preview.files) || !current()) return;
    result = await request('sources/recognize', {source, digest: preview.digest}, token);
  } else {
    if (!await confirmAction(t('恢复认回前的文件位置和图片 ID。原文件不会移动。'), t('撤销认回')) || !current()) return;
    result = await request('sources/undo', {source: d.folderUndo, token: d.token}, token);
  }
  if (!current() || !result) return;
  await loadLibraries();
  if (result.sync) await pollSync();
  if (current()) await loadLibraries();
}
