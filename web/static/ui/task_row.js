// 任务面板的一行（#715）：一个任务一个部件 {el, update, dispose}，由 ui/tasks.js 用 keyed 列表按任务的 key 复用。
// 轮询时 update 只写变了的地方（ui/el.js 的写入函数比较旧值）：状态点、名称、状态行、进度条、按钮、详情和记录各管各的。
// 按钮按动作（attr + act）复用，只更新文字、disabled、title，动作变了才换节点；详情（每次轮询都可能变的进度）和收起的
// 记录表是两个节点，详情变了不重画记录表，展开的记录里的焦点和选中的字留着。
//
// 收的任务（ui/tasks.js 的 taskList）：
//   {key, name, line, active, warn, off,
//    progress: {done, total} | null,
//    buttons: [{attr, act, label, primary, disabled, title}],   // attr 是分派用的 data-* 名（task、prov、pack、models），
//                                                                // 点了由 ui/tasks.js 在面板上统一分派
//    body: HTML,              // 详情：损坏的图片、找回画师的分步进度、出错时的诊断行（已转义、翻译），没有时为空串
//    log: [标题, HTML] | null} // 收起的记录表（同步记录、运行记录）
// body 和 log 还是拼好的 HTML，经 redraw 写进 innerHTML：内容都由 ui/tasks.js 转义过，换成节点是 #716 的过渡项。
import {redraw, fold, wireFolds} from './dom.js';
import {attr, cls, css, make, refill, show, text, tpl} from './el.js';

const ROW = tpl`<div class="task-item"><div class="task-main"><span class="local-dot" data-ref="dot"></span><span class="task-text"><b data-ref="name"></b>
  <small data-ref="line"></small></span><span class="task-acts" data-ref="acts"></span></div>
  <span class="task-bar" data-ref="bar" hidden><span data-ref="fill"></span></span><div class="task-body" data-ref="body" hidden></div>
  <div class="task-body" data-ref="log" hidden></div></div>`;

const actKey = b => `${b.attr || 'task'}:${b.act}`;
function button(b) {
  const node = make('button', '');
  node.type = 'button';
  node.dataset[b.attr || 'task'] = b.act;
  return node;
}
// 文字、样子、disabled、title 和上一次写的比，变了才写：busy() 在点下后设的 disabled 不会被下一次轮询改回去。
const painted = new WeakMap();
function paint(node, b) {
  const was = painted.get(node) || {}, now = {cls: `btn is-small${b.primary ? ' is-primary' : ''}`, label: b.label, disabled: !!b.disabled, title: b.title || ''};
  if (was.cls !== now.cls) node.className = now.cls;
  if (was.label !== now.label) node.textContent = now.label;
  if (was.disabled !== now.disabled) node.disabled = now.disabled;
  if (was.title !== now.title) node.title = now.title;
  painted.set(node, now);
}

/** 一行任务的部件。 */
export function taskRow(task) {
  const {el, refs} = ROW();
  let buttons = [];
  function update(x) {
    cls(el, 'on', x.active);
    attr(refs.dot, 'class', 'local-dot' + (x.active ? ' busy' : x.warn ? ' warn' : x.off ? ' off' : ''));
    text(refs.name, x.name); text(refs.line, x.line); attr(refs.line, 'title', x.line);
    const total = x.progress?.total || 0;
    show(refs.bar, total > 0);
    if (total > 0) {
      attr(refs.bar, 'role', 'progressbar'); attr(refs.bar, 'aria-valuemax', String(total)); attr(refs.bar, 'aria-valuenow', String(x.progress.done || 0));
      css(refs.fill, 'transform', `scaleX(${Math.min(1, (x.progress.done || 0) / total).toFixed(3)})`);
    }
    const list = x.buttons || [];
    refill(refs.acts, list.map(actKey).join('|'), () => (buttons = list.map(button)));
    list.forEach((b, i) => paint(buttons[i], b));
    const body = x.body || '', log = x.log ? fold('task-' + x.key, x.log[0], x.log[1]) : '';
    show(refs.body, body !== '');
    if (redraw(refs.body, body)) wireFolds(refs.body);   // 详情里也有折叠块（损坏的图片）
    show(refs.log, log !== '');
    if (redraw(refs.log, log)) wireFolds(refs.log);
  }
  update(task);
  return {el, update, dispose() {}};
}
