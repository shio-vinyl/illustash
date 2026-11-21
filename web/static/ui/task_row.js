// 任务面板的一行（#715）：一个任务一个部件 {el, update, dispose}，由 ui/tasks.js 用 keyed 列表按任务的 key 复用。
// 轮询时 update 只写变了的地方（ui/el.js 的写入函数比较旧值）：状态点、名称、状态行、进度条和按钮各管各的，按钮只在
// 按钮的数据变了时才重建，展开的详情、焦点和正在按的按钮不会因为别的字变了被换掉。
//
// 收的任务（ui/tasks.js 的 taskList）：
//   {key, name, line, active, warn, off,
//    progress: {done, total} | null,
//    buttons: [{attr, act, label, primary, disabled, title}],   // attr 是分派用的 data-* 名：task、prov 归 ui/tasks.js，
//                                                                // pack 归 ui/packs.js 的 wirePacks，models 归 ui/models.js 的 wireModels
//    body: HTML,              // 详情：损坏的图片、找回画师的分步进度、出错时的诊断行（已转义、翻译），没有时为空串
//    log: [标题, HTML] | null} // 收起的记录表（同步记录、运行记录）
import {redraw, fold, wireFolds} from './dom.js';
import {attr, cls, css, make, refill, show, text, tpl} from './el.js';

const ROW = tpl`<div class="task-item"><div class="task-main"><span class="local-dot" data-ref="dot"></span><span class="task-text"><b data-ref="name"></b>
  <small data-ref="line"></small></span><span class="task-acts" data-ref="acts"></span></div>
  <span class="task-bar" data-ref="bar" hidden><span data-ref="fill"></span></span><div class="task-body" data-ref="body" hidden></div></div>`;

function button(b) {
  const node = make('button', `${b.primary ? 'primary' : 'quiet'} small`, b.label);
  node.dataset[b.attr || 'task'] = b.act;
  if (b.disabled) node.disabled = true;
  if (b.title) node.title = b.title;
  return node;
}

/** 一行任务的部件。 */
export function taskRow(task) {
  const {el, refs} = ROW();
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
    refill(refs.acts, JSON.stringify(list), () => list.map(button));
    const body = (x.body || '') + (x.log ? fold('task-' + x.key, x.log[0], x.log[1]) : '');
    show(refs.body, body !== '');
    if (redraw(refs.body, body)) wireFolds(refs.body);
  }
  update(task);
  return {el, update, dispose() {}};
}
