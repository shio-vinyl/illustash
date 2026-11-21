// ui/frame.js（#704）：每帧先跑完所有读再跑所有写；按令牌去重（#751）；读里排的写同一帧跑，写里排的读挪到下一帧。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');

let frame, frames;
const tick = () => { const run = frames.shift(); assert.ok(run, 'no frame was scheduled'); run(); };
test.before(async () => { frame = await web.load('ui/frame.js'); });
test.beforeEach(() => { frames = []; frame.setScheduler(fn => frames.push(fn)); });

test('同一帧里先读后写，不按排进来的先后交替', () => {
  const log = [];
  frame.write(() => log.push('w1'));
  frame.read(() => log.push('r1'));
  frame.write(() => log.push('w2'));
  frame.read(() => log.push('r2'));
  assert.equal(frames.length, 1, 'one frame for all four');
  tick();
  assert.deepEqual(log, ['r1', 'r2', 'w1', 'w2']);
  assert.equal(frames.length, 0);
});

test('按令牌去重：同一个令牌只跑最后排进来的，位置是第一次的位置', () => {
  const log = [], a = frame.key('a');
  frame.read(() => log.push('a1'), a);
  frame.read(() => log.push('b'));
  frame.read(() => log.push('a2'), a);
  tick();
  assert.deepEqual(log, ['a2', 'b']);
});

test('读里排的写在同一帧跑；写里排的读和写都挪到下一帧', () => {
  const log = [];
  frame.read(() => { log.push('read'); frame.write(() => { log.push('write'); frame.read(() => log.push('next read')); frame.write(() => log.push('next write')); }); });
  tick();
  assert.deepEqual(log, ['read', 'write']);
  assert.equal(frames.length, 1);
  tick();
  assert.deepEqual(log, ['read', 'write', 'next read', 'next write']);
  assert.equal(frames.length, 0);
});

test('总在再排自己的写停得下来：每帧跑一次', () => {
  let n = 0;
  const loop = frame.key('loop'), again = () => { n++; frame.write(again, loop); };
  frame.write(again, loop);
  tick(); tick(); tick();
  assert.equal(n, 3);
  assert.equal(frames.length, 1);
});

test('取消还没跑的任务；一个任务抛错不挡住别的', () => {
  const log = [], errors = [], saved = globalThis.reportError;
  globalThis.reportError = e => errors.push(e.message);
  try {
    const cancel = frame.write(() => log.push('cancelled'));
    frame.read(() => { throw new Error('boom'); });
    frame.read(() => log.push('after'));
    cancel();
    tick();
  } finally { globalThis.reportError = saved; }
  assert.deepEqual(log, ['after']);
  assert.deepEqual(errors, ['boom']);
});

test('同名的两个令牌各跑各的：两份部件同一帧排同一种用途，两个都跑', () => {
  const log = [], first = frame.key('day:measure'), second = frame.key('day:measure');
  assert.notEqual(first, second);
  assert.equal(String(first), 'key(day:measure)');
  frame.read(() => log.push('first'), first);
  frame.read(() => log.push('second'), second);
  tick();
  assert.deepEqual(log, ['first', 'second']);
});

test('同一个令牌读、写各算各的：同一帧里既排读又排写，两个都跑', () => {
  const log = [], k = frame.key('edges');
  frame.read(() => log.push('read 1'), k);
  frame.write(() => log.push('write'), k);
  frame.read(() => log.push('read 2'), k);
  tick();
  assert.deepEqual(log, ['read 2', 'write']);
});

test('键只能是 key() 发的令牌：字符串、数字、Symbol、普通对象都抛 TypeError，也不排帧', () => {
  for (const bad of ['grid:render', 7, Symbol('x'), {label: 'x'}]) {
    assert.throws(() => frame.read(() => {}, bad), TypeError);
    assert.throws(() => frame.write(() => {}, bad), TypeError);
  }
  assert.throws(() => frame.write(() => {}, 'grid:render'), /key\('名字'\) 发的令牌.*字符串 'grid:render'/);
  assert.equal(frames.length, 0, 'nothing was scheduled');
  // 不带键（undefined、null）照旧：不去重
  const log = [];
  frame.write(() => log.push(1)); frame.write(() => log.push(2), null);
  tick();
  assert.deepEqual(log, [1, 2]);
});
