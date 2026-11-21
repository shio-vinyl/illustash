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
  const paint = s.key('rail:paint');
  for (let i = 0; i < 20; i++) s.write(() => ran++, paint);   // 同一个令牌：只记一项、只跑一次
  for (let i = 0; i < 5; i++) s.read(() => ran++);
  assert.equal(s.cleanups.size, 6);
  while (frames.length) frames.shift()();
  assert.equal(ran, 6);
  assert.equal(s.cleanups.size, 0);
  // 释放时取消还没跑的帧任务
  s.write(() => ran++, s.key('late'));
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

test('帧任务的令牌（#751）：两份部件同一帧排同一种用途，两个都跑；同一份同一个令牌排两次，只跑最后一次', async () => {
  const frame = await web.load('ui/frame.js'), frames = [];
  frame.setScheduler(fn => frames.push(fn));
  const parent = S.scope('days'), log = [];
  // 两份「一天」各在自己的作用域上取同名的令牌
  const days = ['a', 'b'].map(name => { const s = parent.child('day'); return {name, s, measure: s.key('day:measure')}; });
  for (const d of days) d.s.read(() => log.push(d.name + ' 1'), d.measure);
  days[0].s.read(() => log.push('a 2'), days[0].measure);   // 同一份、同一个令牌再排一次：替换上一次
  assert.equal(frames.length, 1);
  frames.shift()();
  assert.deepEqual(log, ['a 2', 'b 1']);
  for (const d of days) assert.equal(d.s.cleanups.size, 0);
  parent.dispose();
});

test('s.read / s.write 只认这个作用域自己发的令牌：字符串、frame.key()、别的作用域的令牌都抛 TypeError', async () => {
  const frame = await web.load('ui/frame.js'), frames = [];
  frame.setScheduler(fn => frames.push(fn));
  const a = S.scope('card a'), b = S.scope('card b'), theirs = b.key('card:measure');
  assert.throws(() => a.read(() => {}, 'card:measure'), /card a.*字符串 'card:measure'/);
  assert.throws(() => a.write(() => {}, frame.key('card:measure')), TypeError);
  assert.throws(() => a.read(() => {}, theirs), /作用域 card b 发的令牌 key\(card:measure\)/);
  assert.equal(frames.length, 0, 'nothing was scheduled');
  // 释放以后用错键照样报错（不因为作用域已经释放就放过）；用对的键什么也不排
  a.dispose();
  assert.throws(() => a.write(() => {}, 'late'), TypeError);
  assert.equal(typeof a.write(() => {}, a.key('late')), 'function');
  assert.equal(frames.length, 0);
  b.dispose();
});

test('同一个令牌既排读又排写：两边各记一项，释放时都取消', async () => {
  const frame = await web.load('ui/frame.js'), frames = [];
  frame.setScheduler(fn => frames.push(fn));
  const s = S.scope('rail'), edges = s.key('rail:edges'), log = [];
  s.read(() => log.push('read'), edges);
  s.write(() => log.push('write'), edges);
  assert.equal(s.cleanups.size, 2);
  s.dispose();
  while (frames.length) frames.shift()();
  assert.deepEqual(log, []);
});
