// 页面自己的模态对话框（桌面布局，#199、#319）。openDialog 是唯一生成对话框的地方：标题行加 × 按钮、正文、操作区，
// ×、[data-close]、Esc 和点背景都能关（Esc 与点背景由浮层管理器 ui/overlay.js 统一处理），关闭后从 DOM 移除。
// confirmAction 和 chooseWork 是建在它上面的两个现成对话框；ui/catalog.js 和 ui/palette.js 自己的对话框也用它。
// 开合（#715）：进场是 css/overlays.css 里 dialog[open] 的动画；关掉先播一段短的退场（ui/motion.js 的 exit），播完才 close()。
// 页面里打开对话框一律经过这里：生成的用 openDialog，index.html 里静态的用 openStatic；关一律走 closeModal（×、[data-close]、
// 表单提交、Esc、点背景都已经走它）。浮层管理器的 openModal 只在这个文件里调。例外：换页时作用域清理（ui/day_ask.js）
// 直接 close()，页面马上被换掉，不播退场。
// 退场期间：对话框已经 inert（exit 设的），控件点不动、焦点出去了，表单里的值就是提交那一刻的值；returnValue 一开始就写好；
// 这一层留在浮层管理器的栈里直到真的 close()，所以第二下 Esc 还落在它身上（忽略），不会落到下面一层，也不会变成浏览器自己的
// cancel 把「确认」改成取消。
import {t} from '../core/i18n.js';
import {esc} from '../core/format.js';
import {session} from '../core/session.js';
import {state} from '../core/state.js';
import {cancelExit, exit, exitingNow} from './motion.js';
import {openModal} from './overlay.js';
// 生成并显示一个模态对话框，返回它。`title` 是纯文本；`body` 和 `actions` 是 HTML（调用方已转义、翻译），分别是中间和底部一行
// （为空则省略）。`cls` 加在 `modal` 之后。`backdropClose` 是点背景是否关闭，默认不关：只读或确认类的对话框传 true，
// 带输入或选项的表单保持不关，免得误点丢掉填的东西；每个调用点都要写明属于哪一类。
// `focus` 是拿焦点的控件的选择器（默认：浏览器的第一个可聚焦控件，或 [autofocus]）。
// `onClose(d)` 在关闭时运行（在这里读 d.returnValue 和表单）；随后对话框被移除。
// `dirty()` 返回真时（表单填了还没保存），Esc 先问一句「放弃改动？」，确认了才关（#422）；×、取消照常直接关。
/** 播完退场再关掉对话框，`value` 成为 returnValue。退场中再关一次直接忽略；没开着的不动。 */
export function closeModal(d, value) {
  if (!d?.open || exitingNow(d)) return Promise.resolve(false);
  if (value !== undefined) d.returnValue = value;
  return exit(d, {to: 'scale', done: () => { if (d.open) d.close(d.returnValue); }});
}
// <form method="dialog"> 的提交（按钮、输入框里回车）：调用方自己的处理函数拦下了（校验没过）就不关，否则播完退场再关，
// returnValue 是按下的那个按钮的 value，和浏览器自己关时一样。监听挂在 <dialog> 上，冒泡时才收到：调用方要拦下提交，
// 处理函数挂在 <form> 上（form.onsubmit 调 preventDefault）。
const wired = new WeakSet();
function wireSubmit(d) {
  if (wired.has(d)) return;
  wired.add(d);
  d.addEventListener('submit', e => {
    if (e.defaultPrevented) return;
    e.preventDefault(); closeModal(d, e.submitter?.value ?? '');
  });
}
// Esc、点背景：先跑调用方的 onDismiss（逻辑上关掉：让在途的操作作废），再播退场；返回 false 让这一层留在栈里，
// 等 close 事件才出栈（见文件头）。
const dismisser = (d, before) => () => { before?.(); closeModal(d); return false; };
/**
 * 打开页面里已有的 <dialog>（命令面板、快捷键表这类静态的），Esc、点背景、表单提交也播退场。`onDismiss` 在 Esc、点背景
 * 时先运行（逻辑上关掉）。别的参数同 openModal。还在退场就又要开（选完一个马上再选）：接着开着，不关。
 */
export function openStatic(d, {backdropClose = false, id = '', onDismiss} = {}) {
  cancelExit(d);
  wireSubmit(d);
  const layer = openModal(d, {backdropClose, id, onDismiss: dismisser(d, onDismiss)});
  layer.onDismiss = dismisser(d, onDismiss);   // 退场中重开时层还在栈里：换成这一次的回调，不沿用上一次的
  return layer;
}
export function openDialog({title, body = '', actions = '', cls = '', backdropClose = false, focus = '', onClose, dirty} = {}) {
  const d = document.createElement('dialog'); d.className = ('modal glass-scrim ' + cls).trim();
  d.setAttribute('aria-label', title);
  d.innerHTML = `<form method="dialog">
    <div class="dialog-top"><h2 class="title-section"></h2><button type="button" class="icon" data-close aria-label="${t('关闭')}">×</button></div>
    ${body}${actions ? `<div class="dialog-actions">${actions}</div>` : ''}</form>`;
  d.querySelector('h2').textContent = title;
  d.onclick = e => { if (e.target !== d && e.target.closest('[data-close]')) closeModal(d); };
  wireSubmit(d);
  d.onclose = () => { try { return onClose?.(d); } finally { d.remove(); } };
  const onDismiss = dirty ? () => {
    if (exitingNow(d)) return false;   // 已经提交、正在退场：不再问「放弃改动？」
    if (!dirty()) { closeModal(d); return false; }
    confirmAction(t('填写的内容还没有保存，关掉后不会保留。'), t('放弃改动？'), t('放弃改动')).then(yes => { if (yes) closeModal(d); });
    return false;
  } : dismisser(d);
  document.body.append(d); openModal(d, {backdropClose, id: 'dialog', onDismiss});
  if (focus) d.querySelector(focus)?.focus?.();
  return d;
}
// 图库页面没有 Tauri 对话框权限。确认框留在页面内；window.confirm 可能被桌面外壳换成一个异步的插件函数。
let confirmationOpen = false;
// `ok` 是确认按钮的名字；没有 `cancel` 时对话框只作告知。
export function confirmAction(message, title = t('确认操作'), ok = t('确认'), cancel = true) {
  if (confirmationOpen) return Promise.resolve(false);
  confirmationOpen = true;
  const token = session.token, route = state.route;
  return new Promise(resolve => {
    const d = openDialog({title, backdropClose: true, body: '<p class="muted-note dialog-message"></p>',
      actions: `${cancel ? `<button value="cancel" class="quiet" autofocus>${t('取消')}</button>` : ''}<button value="ok" class="primary"></button>`,
      onClose: d => { confirmationOpen = false; resolve(d.returnValue === 'ok' && token === session.token && route === state.route); }});
    d.querySelector('[value="ok"]').textContent = ok;
    d.querySelector('p').textContent = message;
  });
}
const workOption = (w, selected) => `<option value="${esc(w.id)}" ${w.id === selected ? 'selected' : ''}>${esc(w.name)}${w.qualifier ? ' · ' + esc(w.qualifier) : ''}</option>`;
// 挑一个本地作品（或空选项）；取消时解析为 null。选择类表单，点背景不关。
export function chooseWork(title, works, selected = '', emptyLabel = t('未归类 / 无上级')) {
  return new Promise(resolve => {
    openDialog({title, cls: 'work-dialog', backdropClose: false,
      body: `<select name="work" class="work-choice"><option value="">${esc(emptyLabel)}</option>${works.map(w => workOption(w, selected)).join('')}</select>`,
      actions: `<button value="cancel" class="quiet">${t('取消')}</button><button value="ok" class="primary">${t('确认')}</button>`,
      onClose: d => resolve(d.returnValue === 'ok' ? d.querySelector('select').value : null)});
  });
}
