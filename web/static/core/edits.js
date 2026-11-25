// 编辑和撤销（#199）。每次写入都可以撤销：后端对每次写入都回应它能撤销的最近一步；同一时间只进行一项改动——一次写入，
// 或撤销一次写入。不碰 DOM：布局按 onLatest() 画撤销控件。
import {t, tb, I18N} from './i18n.js';
import {post} from './session.js';
import {listeners} from './events.js';
// latest：撤销会还原的那一步的 {id, label, removal?}，或 null。busy：正在进行一项改动或撤销。
export const edits = {latest: null, busy: false};
const changed = listeners();
// 每次改动后调用 fn(edits)；返回一个再把这个监听去掉的函数。
export const onLatest = changed.on;
// busy 改变后也会调用，让控件跟着变。
export function setLatest(step) {
  edits.latest = step;
  changed.emit(edits);
}
// 同一时间只进行一项改动（#432 ③-7）：已经有改动在进行时什么也不做，返回 undefined；否则加锁执行 fn，返回它的结果。
// 开始和结束都通知撤销控件（busy 时撤销按钮不可用）。写入或撤销之后的公共收尾放在这里或 ui/dom.js 的 written，
// 不在各处另写。
export async function exclusive(fn) {
  if (edits.busy) return undefined;
  edits.busy = true; changed.emit(edits);
  try { return await fn(); } finally { edits.busy = false; setLatest(edits.latest); }
}
// 双角色的「都不是」带独立的名字，保留名字本身的标点。
export const editLabel = ({label, removal}) => removal
  ? t('已将 {n} 张移出 {name}', {n: removal.count, name: removal.names.join(t('、'))})
  : tb(label);
// 还原最近一步，返回被撤销的步骤；下一步已经设好。
export async function undoLatest() {
  const step = edits.latest, result = await post('/api/undo', {id: step.id});
  setLatest(result.undo);
  return step;
}
// 撤销这一步之后，toast 显示的文字。
export function undoneText(step) {
  const label = editLabel(step);
  return t('已撤销 · {label}', {label: I18N.language === 'zh-CN' ? label.replace(/^已/, '') : label});
}
