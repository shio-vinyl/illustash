// 用图动作的界面（#450）：把图拖到别的应用、选择栏的「导出…」对话框。详情卡片的「打开」「在访达中显示」在 ui/detail_card.js。
// 请求和导出状态在 core/files.js；导出的进度在任务面板（ui/tasks.js）。
import {t} from '../core/i18n.js';
import {number} from '../core/format.js';
import {DRAG_LIMIT, canDrag, dragFiles, startExport} from '../core/files.js';
import {openDialog} from './dialog.js';
import {toast} from './dom.js';

// 鼠标按下后移动超过这么多像素，就算开始拖。
const SLOP = 6;
// 在 el 上按住拖动时，把 ids(down) 给的图拖到别的应用（外壳起系统的拖放，只允许复制）。down 是按下时的 pointerdown 事件：
// 在容器上委托时，用 down.target.closest(…) 找按的是哪张。when(down) 为假时（比如大图放大了在平移）不拖。拖出之后吞掉
// 松手时的那次点击，免得点开大图。只在桌面 App 里有效。返回 off()，取下这些监听（交给作用域：s.add(wireDragOut(…))）。
export function wireDragOut(el, ids, when = () => true) {
  let down = null, dragged = false;
  const on = {
    pointerdown: e => {
      dragged = false;
      down = e.button === 0 && e.pointerType !== 'touch' && canDrag() && when(e) ? e : null;
    },
    pointermove: e => {
      if (!down || !(e.buttons & 1) || Math.hypot(e.clientX - down.clientX, e.clientY - down.clientY) < SLOP) return;
      const from = down; down = null; dragged = true;
      startDrag(ids(from));
    },
    pointerup: () => { down = null; },
    pointercancel: () => { down = null; },
  };
  const swallow = e => { if (dragged) { e.preventDefault(); e.stopImmediatePropagation(); } dragged = false; };
  for (const [type, fn] of Object.entries(on)) el.addEventListener(type, fn);
  el.addEventListener('click', swallow, true);
  return () => {
    for (const [type, fn] of Object.entries(on)) el.removeEventListener(type, fn);
    el.removeEventListener('click', swallow, true);
  };
}
// 拖出这几张。超过上限时只提示，不拖。
export function startDrag(ids) {
  if (!ids.length) return;
  if (ids.length > DRAG_LIMIT) { toast(t('一次最多拖出 {n} 张', {n: number(DRAG_LIMIT)})); return; }
  dragFiles(ids).catch(e => toast(e.message));
}
// 选择栏的「导出…」：选子文件夹的分法，再弹系统的选目录窗口。selection 是 {ids} 或 {filter, exclude}；n 是张数。
// 带选项的表单，点背景不关。
export function openExport(selection, n) {
  const groups = [['none', t('不分')], ['character', t('按角色')], ['artist', t('按画师')]];
  let group = 'none';
  const d = openDialog({title: t('导出 {n} 张', {n: number(n)}), backdropClose: false,
    body: `<p class="muted-note dialog-message">${t('把原图复制到选的文件夹，不覆盖已有的文件。')}</p>
      <div class="setting-line"><span>${t('子文件夹')}</span><div class="segmented">${groups.map(([v, label]) =>
        `<button type="button" class="${v === group ? 'on' : ''}" data-group="${v}">${label}</button>`).join('')}</div></div>`,
    actions: `<button value="cancel" class="quiet">${t('取消')}</button><button value="ok" class="primary" autofocus>${t('选择文件夹…')}</button>`,
    onClose: d => {
      if (d.returnValue !== 'ok') return;
      startExport(selection, group).then(started => {
        if (started) toast(t('开始导出 {n} 张，进度在「任务」里', {n: number(n)}));
      }).catch(e => toast(e.message));
    }});
  for (const b of d.querySelectorAll('[data-group]')) b.onclick = () => {
    group = b.dataset.group;
    for (const x of d.querySelectorAll('[data-group]')) x.classList.toggle('on', x === b);
  };
  return d;
}
