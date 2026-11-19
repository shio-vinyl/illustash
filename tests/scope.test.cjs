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
