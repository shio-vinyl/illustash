// inert 锁（ui/inert.js，#715）和按钮的 disabled（ui/dom.js 的 busy、setDisabled）：几处同时锁、同时禁用时谁加的谁解。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const dom = require('./dom.cjs');
const web = require('./web.cjs');
let inert, ui;
before(async () => {
  await web.ready();
  dom.install();
  inert = await web.load('ui/inert.js');
  ui = await web.load('ui/dom.js');
});

test('两处都锁着时，解开一处还是 inert；全解开回到加锁前的样子', () => {
  const el = {inert: false};
  inert.lockInert(el, 'overlay'); inert.lockInert(el, 'exit');
  inert.unlockInert(el, 'exit'); assert.equal(el.inert, true, '大图的锁还在');
  inert.unlockInert(el, 'overlay'); assert.equal(el.inert, false);
  const fixed = {inert: true};   // keyed 列表直接设的
  inert.lockInert(fixed, 'exit'); inert.unlockInert(fixed, 'exit'); assert.equal(fixed.inert, true);
  inert.unlockInert({inert: true}, 'exit');   // 没加过锁的不动、不报错
});

test('busy 收尾时回到数据说的样子：任务在点的时候开始跑了，按钮留着禁用', async () => {
  const b = {disabled: false};
  let release; const run = ui.busy(b, () => new Promise(r => { release = r; }));
  assert.equal(b.disabled, true);
  ui.setDisabled(b, true); ui.setDisabled(b, true);   // 轮询读到 scanning
  release(); await run;
  assert.equal(b.disabled, true, '不变回可点');
  ui.setDisabled(b, false); assert.equal(b.disabled, false);
  const plain = {disabled: false}; await ui.busy(plain, async () => {}); assert.equal(plain.disabled, false, '没有数据的照旧启用');
});
