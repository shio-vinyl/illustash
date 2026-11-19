// ui/frame.js（#704）：每帧先跑完所有读再跑所有写；按键去重；读里排的写同一帧跑，写里排的读挪到下一帧。
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

test('按键去重：同一个键只跑最后排进来的，位置是第一次的位置', () => {
  const log = [];
  frame.read(() => log.push('a1'), 'a');
  frame.read(() => log.push('b'));
  frame.read(() => log.push('a2'), 'a');
  tick();
  assert.deepEqual(log, ['a2', 'b']);
});

test('读里排的写在同一帧跑；写里排的读到下一帧；写里排的写也在这一帧', () => {
  const log = [];
  frame.read(() => { log.push('read'); frame.write(() => { log.push('write'); frame.read(() => log.push('next read')); frame.write(() => log.push('write again')); }); });
  tick();
  assert.deepEqual(log, ['read', 'write', 'write again']);
  assert.equal(frames.length, 1);
  tick();
  assert.deepEqual(log, ['read', 'write', 'write again', 'next read']);
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
