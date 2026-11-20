// 视图选项浮层（右上的「视图」按钮；#708 审核从 ui/grid.js 拆出）：画开关、接点击，改了以后让网格和各页重画。选项的值在
// core/view_opts.js。这里导入 ui/grid.js（重画网格、缩放滑块），ui/grid.js 不反过来导入这里。
import {t} from '../core/i18n.js';
import {pageOf} from '../core/pages.js';
import {state} from '../core/state.js';
import {resetView, saveView, viewOpts} from '../core/view_opts.js';
import {$} from './dom.js';
import {attr, make} from './el.js';
import {inspect} from './detail.js';
import {renderGrid, zoomFromSlider, zoomSlider} from './grid.js';
import {layout} from './grid_rows.js';
import {createLayer} from './overlay.js';

function applyViewClasses() {
  const b = document.body.classList;
  b.toggle('hide-marks', !viewOpts.marks); b.toggle('hide-footer', !viewOpts.footer);
}
// 一个开关：key 是 core/view_opts.js 里的键；hint 是开关下面的一行小字。
function viewSwitch(key, label, hint = '') {
  const b = make('button', 'view-opt'), text = make('span', '', label);
  b.dataset.view = key; attr(b, 'role', 'switch'); attr(b, 'aria-checked', String(viewOpts[key]));
  if (hint) text.append(make('small', '', hint));
  b.append(text, make('i', `view-sw${viewOpts[key] ? ' on' : ''}`));
  return b;
}
// 一组：小标题和它下面的几行。
const group = (title, rows) => { const box = make('div', 'view-group'); box.append(...rows); return [make('h6', '', title), box]; };
// 首页缩略图的大小：小、中、大三档的分段按钮。
function homeSizeRow() {
  const row = make('div', 'view-row'), seg = make('div', 'segmented');
  seg.id = 'homeSizeSeg';
  for (const [v, label] of [[0, t('小')], [1, t('中')], [2, t('大')]]) {
    const b = make('button', viewOpts.homeSize === v ? 'on' : '', label); b.dataset.hsize = String(v); seg.append(b);
  }
  row.append(make('span', '', t('缩略图')), seg);
  return row;
}
// 网格的缩放滑块（和 ⌘＋／⌘－、捏合同一个缩放）。
function zoomRow() {
  const row = make('label', 'view-row'), input = make('input');
  for (const [k, v] of [['type', 'range'], ['id', 'zoom'], ['min', '0'], ['max', '100'], ['step', '1'], ['aria-label', t('缩略图大小')]]) attr(input, k, v);
  input.value = String(zoomSlider());
  input.addEventListener('input', e => zoomFromSlider(Number(e.target.value)));
  row.append(make('span', '', t('缩略图大小')), input);
  return row;
}
// 导出给样式清单页调用（tools/styleguide/live.js，#352）：改名或改参数时，tools/check_styleguide.py 会报出来。
export function renderViewPop() {
  const page = pageOf(state.route), nodes = [];
  if (page.area === 'homePage') {
    nodes.push(...group(t('按天'), [homeSizeRow(), viewSwitch('people', t('分组显示角色数')), viewSwitch('marks', t('图上显示副本数'))]));
  }
  if (!page.panel) {
    nodes.push(...group(t('图片网格'), [zoomRow(), viewSwitch('marks', t('图上显示副本数与造型')),
      viewSwitch('hints', t('未识别的图上显示「可能是」'), t('点一下就设为该角色'))]));
  }
  nodes.push(...group(t('提示'), [viewSwitch('footer', t('底部状态栏'), t('已加载张数与操作提示'))]),
    ...group(t('详情面板'), [viewSwitch('chart', t('每月加入张数'), t('这个角色每月收了多少张')), viewSwitch('same', t('同一角色的其它图')),
      viewSwitch('meta', t('文件信息与画面标签')), viewSwitch('ask', t('偶尔问一句「是 TA 吗？」'), t('在详情和按天视图'))]));
  const foot = make('div', 'view-foot'), reset = make('button', 'link-button strong', t('恢复默认'));
  reset.id = 'viewReset';
  reset.onclick = () => { resetView(); afterViewChange(true); renderViewPop(); pageOf(state.route).refresh?.('view'); };
  foot.append(make('span', '', t('设置会自动保存')), reset);
  $('viewPopover').replaceChildren(...nodes, foot);
}
function afterViewChange(relayout) {
  applyViewClasses();
  if (relayout && !pageOf(state.route).panel) { layout(); renderGrid(true); }
}
// 视图选项是非模态浮层：Esc、点外关闭由浮层管理器（ui/overlay.js）统一处理。
// 打开时焦点进第一项，↑ ↓ 在各项之间移动（#422）。
const viewLayer = createLayer({kind: 'popover', id: 'view-options', el: () => $('viewPopover'), anchor: () => $('viewButton'), menu: true, focusIn: 'first',
  onDismiss: reason => toggleViewPop(false, reason)});
function toggleViewPop(open = $('viewPopover').hidden, reason) {
  $('viewPopover').hidden = !open; $('viewButton').classList.toggle('on', open); $('viewButton').setAttribute('aria-expanded', open);
  if (open) { renderViewPop(); viewLayer.open(); } else viewLayer.close(reason);
}

/** 接上「视图」按钮和浮层里的点击，并按当前选项给页面加上类名（app.js 启动时调一次）。 */
export function wireViewOptions() {
  $('viewButton').onclick = e => { e.stopPropagation(); toggleViewPop(); };
  $('viewPopover').addEventListener('click', e => {
    e.stopPropagation();
    const hs = e.target.closest('[data-hsize]');
    if (hs) {
      viewOpts.homeSize = Number(hs.dataset.hsize); saveView();
      for (const x of $('homeSizeSeg').children) x.classList.toggle('on', x === hs);
      pageOf(state.route).refresh?.('view');
      return;
    }
    const b = e.target.closest('[data-view]'); if (!b) return;
    const key = b.dataset.view; viewOpts[key] = !viewOpts[key]; saveView();
    b.setAttribute('aria-checked', viewOpts[key]); b.querySelector('.view-sw').classList.toggle('on', viewOpts[key]);
    afterViewChange(key === 'hints');
    if (['chart', 'same', 'meta'].includes(key) && state.focused && !$('inspector').hidden) inspect(state.focused, true);
    if (['people', 'marks'].includes(key)) pageOf(state.route).refresh?.('view');
    if (key === 'ask') { if (state.focused && !$('inspector').hidden) inspect(state.focused, true); pageOf(state.route).refresh?.('ask'); }
  });
  applyViewClasses();
}
