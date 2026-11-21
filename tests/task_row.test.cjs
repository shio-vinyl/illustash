// 任务面板的一行（ui/task_row.js，#715）：轮询时 update 只写变了的地方。按钮的数据没变时按钮节点留着（正在按的按钮、焦点不丢），
// 数据变了才重建；进度条只改 transform；分派用的 data-* 名照按钮数据写（task、prov、pack、models）。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const dom = require('./dom.cjs');
const web = require('./web.cjs');
let taskRow;
before(async () => {
  await web.ready();
  dom.install();
  ({taskRow} = await web.load('ui/task_row.js'));
});
const task = (over = {}) => ({key: 'sync', name: '文件夹同步', line: '上次同步：11-20', active: false, warn: false, off: false, progress: null,
  buttons: [{attr: 'task', act: 'sync', label: '立即同步'}], body: '', log: null, ...over});

test('值没变不动按钮；按钮数据变了才重建，data-* 名照 attr 写', () => {
  const row = taskRow(task());
  const first = row.el.querySelector('button');
  assert.equal(first.getAttribute('data-task'), 'sync'); assert.equal(first.textContent, '立即同步');
  row.update(task({line: '正在同步 3 / 10'}));
  assert.equal(row.el.querySelector('button'), first, '只有状态行变了，按钮还是原来那个');
  assert.equal(row.el.querySelector('small').textContent, '正在同步 3 / 10');
  row.update(task({buttons: [{attr: 'pack', act: 'cancel', label: '取消', disabled: true, title: '正在写入'}]}));
  const next = row.el.querySelector('button');
  assert.notEqual(next, first);
  assert.equal(next.getAttribute('data-pack'), 'cancel'); assert.equal(next.disabled, true); assert.equal(next.title, '正在写入');
});

test('同一个动作只改文字、disabled 和 title，按钮节点留着（焦点不掉，#422）', () => {
  const row = taskRow(task());
  const first = row.el.querySelector('button');
  row.update(task({buttons: [{attr: 'task', act: 'sync', label: '同步中', disabled: true, title: '正在同步'}]}));
  const same = row.el.querySelector('button');
  assert.equal(same, first); assert.equal(same.textContent, '同步中'); assert.equal(same.disabled, true); assert.equal(same.title, '正在同步');
  assert.equal(same.className, 'btn is-small');
});

test('进度条只改 transform，没有总数时藏起来；状态点和进行中的底跟着状态', () => {
  const row = taskRow(task({active: true, progress: {done: 3, total: 12}}));
  const bar = row.el.querySelector('.task-bar');
  assert.equal(bar.hidden, false); assert.equal(bar.getAttribute('aria-valuenow'), '3');
  assert.equal(bar.querySelector('span').style.getPropertyValue('transform'), 'scaleX(0.250)');
  assert.ok(row.el.classList.contains('on')); assert.equal(row.el.querySelector('.local-dot').getAttribute('class'), 'local-dot busy');
  row.update(task({warn: true}));
  assert.equal(bar.hidden, true); assert.ok(!row.el.classList.contains('on'));
  assert.equal(row.el.querySelector('.local-dot').getAttribute('class'), 'local-dot warn');
});

test('详情和记录表收在行里，没有时藏起来', () => {
  const row = taskRow(task({body: '<p class="task-side">错误编号：D-1</p>'}));
  const body = row.el.querySelector('.task-body');
  assert.equal(body.hidden, false); assert.match(body.innerHTML, /错误编号：D-1/);
  row.update(task());
  assert.equal(body.hidden, true);
});
