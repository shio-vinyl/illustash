// ui/scope.js（#704）：登记的监听、计时器、观察器、子部件一次释放；dispose 可重复调用；开发模式下数还活着的作用域，
// 换页后没释放的在控制台提示。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

let S, document;
test.before(async () => { await web.ready(); document = dom.install(); S = await web.load('ui/scope.js'); S.setDev(true); });
test.after(() => dom.uninstall());

test('dispose 释放全部登记的东西，后登记的先释放，重复调用不再做事', async () => {
  const s = S.scope('card'), button = document.createElement('button'), log = [];
  let clicks = 0;
  s.on(button, 'click', () => clicks++);
  let fired = false; s.timeout(() => { fired = true; }, 5);
  s.observe({disconnect: () => log.push('observer')});
  s.add({dispose: () => log.push('child component')});
  s.add(() => log.push('cleanup'));
  const signal = s.signal;
  button.dispatchEvent(dom.event('click'));
  assert.equal(clicks, 1);
  s.dispose(); s.dispose();
  assert.deepEqual(log, ['cleanup', 'child component', 'observer']);
  assert.equal(button.listenerCount('click'), 0);
  assert.equal(signal.aborted, true);
  assert.equal(s.alive, false);
  await new Promise(r => setTimeout(r, 15));
  assert.equal(fired, false);
  // 释放以后再登记：立刻释放，不留下孤儿。
  let late = false; s.add(() => { late = true; });
  assert.equal(late, true);
});

test('子作用域随父作用域释放；自己先释放时从父作用域里摘掉', () => {
  const parent = S.scope('list'), log = [];
  const a = parent.child('item a'), b = parent.child('item b');
  a.add(() => log.push('a')); b.add(() => log.push('b'));
  a.dispose();
  assert.equal(parent.children.size, 1);
  parent.dispose();
  assert.deepEqual(log, ['a', 'b']);
  assert.equal(b.alive, false);
});

test('开发模式：live() 数还活着的作用域；换页后上一页建的根作用域没释放就提示', () => {
  const warnings = [], saved = console.warn;
  console.warn = (...args) => warnings.push(args.join(' '));
  try {
    const before = S.live().count;
    const first = S.page('grid');
    const kept = first.child('card');            // 挂在页面上的：随页面释放
    const forgotten = S.scope('forgotten-popup'); // 忘了挂上、也没释放的
    const global = S.app.child('rail');           // 全应用的：不算
    assert.equal(S.live().names['page:grid'], 1);
    assert.ok(S.live().count >= before + 4);
    const second = S.page('home');
    assert.equal(first.alive, false);
    assert.equal(kept.alive, false);
    assert.equal(S.currentPage(), second);
    assert.equal(warnings.length, 1);
    assert.match(warnings[0], /forgotten-popup/);
    assert.match(warnings[0], /page:grid/);
    assert.equal(S.live().names['forgotten-popup'], 1);
    forgotten.dispose(); global.dispose();
    assert.equal(S.live().names['forgotten-popup'], undefined);
    S.page('settings');
    assert.equal(warnings.length, 1, 'nothing left behind this time');
  } finally { console.warn = saved; }
});

test('触发过的 timeout、跑完的帧任务自己摘掉，长寿的作用域不越积越多；off() 提前摘监听', async () => {
  const frame = await web.load('ui/frame.js'), frames = [];
  frame.setScheduler(fn => frames.push(fn));
  const s = S.scope('rail'), button = document.createElement('button');
  for (let i = 0; i < 50; i++) s.timeout(() => {}, 1);
  await new Promise(r => setTimeout(r, 20));
  assert.equal(s.cleanups.size, 0);
  let ran = 0;
  for (let i = 0; i < 20; i++) s.write(() => ran++, 'rail:paint');   // 同一个 key：只记一项、只跑一次
  for (let i = 0; i < 5; i++) s.read(() => ran++);
  assert.equal(s.cleanups.size, 6);
  while (frames.length) frames.shift()();
  assert.equal(ran, 6);
  assert.equal(s.cleanups.size, 0);
  // 释放时取消还没跑的帧任务
  s.write(() => ran++, 'late');
  const off = s.on(button, 'click', () => {});
  off();
  assert.equal(button.listenerCount('click'), 0);
  assert.equal(s.cleanups.size, 1);
  s.dispose();
  while (frames.length) frames.shift()();
  assert.equal(ran, 6);
  assert.equal(s.write(() => ran++)(), undefined, 'a disposed scope schedules nothing');
});

test('开发模式：同一个漏掉的作用域只提示一次，同名页面来回切不重复报', () => {
  const warnings = [], saved = console.warn;
  console.warn = (...args) => warnings.push(args.join(' '));
  try {
    S.page('grid');
    const leaked = S.scope('leaked-once');
    S.page('home'); S.page('grid'); S.page('home'); S.page('grid');
    assert.equal(warnings.filter(w => w.includes('leaked-once')).length, 1);
    leaked.dispose();
  } finally { console.warn = saved; }
});
