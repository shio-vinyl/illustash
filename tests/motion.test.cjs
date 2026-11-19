// 动效工具（ui/motion.js，#705）：「减少动态效果」时直接到终态、结束的计时兜底、FLIP 和共享元素过渡的起止计算、
// 出场摘节点、数字滚动、缩略图位置登记表。节点和 Web Animations 都是假的，记下调用参数，由测试决定动画何时结束。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let motion;

// 假的 Web Animations：finished 由测试调 end() / cancel() 决定；hang 为真时永远不结束（模拟 finished 不来）。
let animations, hang;
class FakeAnimation {
  constructor(el, keyframes, options) {
    Object.assign(this, {el, keyframes, options, state: 'running'});
    this.finished = new Promise((resolve, reject) => { this.end = () => { this.state = 'finished'; resolve(this); }; this.fail = reject; });
    this.finished.catch(() => {});
    animations.push(this);
    if (!hang && options.duration === 0) queueMicrotask(this.end);
  }
  finish() { this.state = 'finished'; this.end(); }
  // 和真的一样：已结束的动画 cancel() 也会撤掉 fill 留下的终态，只是 finished 不再变。
  cancel() { const was = this.state; this.state = 'cancelled'; if (was === 'running') this.fail(new Error('AbortError')); }
}
function node({rect = {left: 0, top: 0, width: 100, height: 100}} = {}) {
  const el = new web.FakeElement();
  el.isConnected = true;
  el.animate = (keyframes, options) => new FakeAnimation(el, keyframes, options);
  el.getBoundingClientRect = () => ({...rect, x: rect.left, y: rect.top, right: rect.left + rect.width, bottom: rect.top + rect.height});
  return el;
}
let reduce;
before(async () => {
  await web.ready();
  globalThis.matchMedia = () => ({get matches() { return reduce; }});
  globalThis.getComputedStyle = () => ({getPropertyValue: name => ({'--dur-1': '140ms', '--dur-2': '.24s', '--dur-3': '380ms',
    '--ease': 'cubic-bezier(.2,.8,.2,1)', '--ease-spring': 'cubic-bezier(.34,1.42,.5,1)'}[name] || '')});
  motion = await web.load('ui/motion.js');
});
beforeEach(() => {
  animations = []; hang = false; reduce = false;
  web.fakePage();
  globalThis.innerWidth = 1200; globalThis.innerHeight = 800;
  motion.thumbs.clear();
});

test('时长和缓动取令牌：1 / 2 / 3 档、秒和毫秒都认，直接给的毫秒原样用', () => {
  assert.equal(motion.duration(1), 140);
  assert.equal(motion.duration(2), 240);
  assert.equal(motion.duration(3), 380);
  assert.equal(motion.duration(90), 90);
  assert.equal(motion.easing(), 'cubic-bezier(.2,.8,.2,1)');
  assert.equal(motion.easing('spring'), 'cubic-bezier(.34,1.42,.5,1)');
  assert.equal(motion.easing('linear'), 'linear');
});

test('减少动态效果：Web Animations 的时长和延迟都是 0，fill 照旧，所以直接到终态', async () => {
  reduce = true;
  const el = node();
  assert.equal(motion.duration(3), 0);
  assert.equal(await motion.animate(el, [{opacity: 0}, {opacity: 1}], {dur: 3, delay: 200, fill: 'forwards'}), true);
  assert.deepEqual([animations[0].options.duration, animations[0].options.delay, animations[0].options.fill], [0, 0, 'forwards']);
  assert.equal(await motion.enter(el), true);
  assert.equal(animations[1].options.duration, 0);
  // 出场同样立刻结束，节点照样摘掉。
  const parent = new web.FakeElement(); const child = node(); parent.append(child);
  assert.equal(await motion.exit(child), true);
  assert.equal(parent.children.length, 0);
});

test('减少动态效果：数字直接写成新值，不排帧', async () => {
  reduce = true;
  let frames = 0;
  globalThis.requestAnimationFrame = () => { frames++; return 0; };
  const el = node();
  el.textContent = '3';
  assert.equal(await motion.countTo(el, 40, {from: 3}), true);
  assert.equal(el.textContent, '40');
  assert.equal(frames, 0);
});

test('减少动态效果：共享元素过渡不用 View Transitions', async () => {
  reduce = true;
  let started = 0;
  document.startViewTransition = () => { started++; return {finished: Promise.resolve()}; };
  const thumb = node({rect: {left: 10, top: 10, width: 80, height: 80}}), big = node({rect: {left: 100, top: 50, width: 600, height: 400}});
  let drawn = false;
  assert.equal(await motion.share({from: thumb, to: big, update: () => { drawn = true; }}), true);
  assert.equal(started, 0);
  assert.equal(drawn, true);
  assert.equal(animations.at(-1).options.duration, 0);
});

test('结束的计时兜底：finished 一直不来时，最晚在时长之后 FALLBACK_MS 结束，并把动画推到终点', async () => {
  hang = true;
  const el = node();
  const began = Date.now();
  const ok = await motion.animate(el, [{opacity: 0}, {opacity: 1}], {dur: 30});
  const waited = Date.now() - began;
  assert.equal(ok, true);
  assert.ok(waited >= 30 + motion.FALLBACK_MS - 5, `waited ${waited}ms`);
  assert.ok(waited < 30 + motion.FALLBACK_MS + 1000, `waited ${waited}ms`);
  assert.equal(animations[0].state, 'finished');
});

test('结束：finished 先来就不等兜底；被取消时是 false', async () => {
  const el = node();
  const done = motion.animate(el, [{opacity: 0}, {opacity: 1}], {dur: 5000});
  animations[0].end();
  const began = Date.now();
  assert.equal(await done, true);
  assert.ok(Date.now() - began < 1000);
  const cancelled = motion.animate(el, [{opacity: 0}, {opacity: 1}], {dur: 5000});
  animations[1].cancel();
  assert.equal(await cancelled, false);
});

test('出场比进场短；结束后摘掉节点，remove: false 时藏起来，被取消时留在原处', async () => {
  const parent = new web.FakeElement();
  const a = node(), b = node(), c = node();
  parent.append(a, b, c);
  motion.enter(a);
  assert.ok(animations[0].options.duration > motion.duration(1));
  const gone = motion.exit(a);
  assert.equal(animations[1].options.duration, 140);
  assert.equal(animations[1].options.fill, 'forwards');
  animations[1].end();
  assert.equal(await gone, true);
  assert.deepEqual(parent.children, [b, c]);
  const hidden = motion.exit(b, {remove: false});
  animations[2].end();
  assert.equal(await hidden, true);
  assert.equal(b.hidden, true);
  assert.equal(animations[2].state, 'cancelled', '藏起来以后撤掉 fill 的终态');
  const kept = motion.exit(c);
  motion.cancelExit(c);
  assert.equal(await kept, false);
  assert.ok(parent.children.includes(c));
});

test('FLIP 的起止：起点是从新框回到旧框的平移和缩放，终点是 none；起止相同不动画', async () => {
  assert.deepEqual(motion.flipDelta({x: 10, y: 20, w: 50, h: 40}, {x: 110, y: 60, w: 100, h: 80}), {x: -100, y: -40, sx: 0.5, sy: 0.5});
  // DOMRect 的写法同样认；宽高为 0 的方向不缩放。
  assert.deepEqual(motion.flipDelta({left: 0, top: 0, width: 30, height: 30}, {left: 5, top: 5, width: 0, height: 60}), {x: -5, y: -5, sx: 1, sy: 0.5});
  const frames = motion.flipFrames({x: 10, y: 20, w: 50, h: 40}, {x: 110, y: 60, w: 100, h: 80});
  assert.deepEqual(frames, [
    {transformOrigin: '0 0', transform: 'translate(-100px, -40px) scale(0.5, 0.5)'},
    {transformOrigin: '0 0', transform: 'none'},
  ]);
  assert.equal(motion.flipFrames({x: 1, y: 2, w: 3, h: 4}, {x: 1, y: 2, w: 3, h: 4}), null);
  const el = node();
  assert.equal(await motion.flip(el, {x: 1, y: 2, w: 3, h: 4}, {x: 1, y: 2, w: 3, h: 4}), true);
  assert.equal(animations.length, 0);
  motion.flip(el, {x: 10, y: 20, w: 50, h: 40}, {x: 110, y: 60, w: 100, h: 80}, {ease: 'spring', dur: 3});
  assert.deepEqual([animations[0].options.duration, animations[0].options.easing], [380, 'cubic-bezier(.34,1.42,.5,1)']);
  assert.deepEqual(animations[0].keyframes, frames);
});

test('共享元素的起止：等比放大盖满缩略图，裁出缩略图那一块；终点回到自己的框', () => {
  // 大图 400×200 在 (100, 100)；缩略图是 (20, 30) 的 50×50 方块。
  const [first, last] = motion.shareFrames({x: 20, y: 30, w: 50, h: 50}, {x: 100, y: 100, w: 400, h: 200}, {radius: [8, 0]});
  // 等比缩放 s = max(50/400, 50/200) = 0.25，缩放后 100×50，中心对准缩略图中心 (45, 55)。
  const s = 0.25, x = 45 - 50 - 100, y = 55 - 25 - 100;
  assert.equal(first.transform, `translate(${x}px, ${y}px) scale(${s}, ${s})`);
  // 裁掉左右各 (100 - 50) / 2 / s = 100px（缩放前的尺寸），上下不裁；圆角按缩放前写，缩完正好 8px。
  assert.equal(first.clipPath, 'inset(0px 100px round 32px)');
  assert.deepEqual(last, {transformOrigin: '0 0', transform: 'none', clipPath: 'inset(0px 0px round 0px)'});
  // 起点画出来的可见区域正好是缩略图：左边 = 100 + x + 100 * s。
  assert.equal(100 + x + 100 * s, 20);
  assert.equal(motion.shareFrames({x: 0, y: 0, w: 10, h: 10}, {x: 0, y: 0, w: 0, h: 10}), null);
});

test('共享元素：有 View Transitions 时用它，在过渡里画大图并交接名字；没有时先画再用 Web Animations', async () => {
  const thumb = node({rect: {left: 20, top: 30, width: 50, height: 50}}), big = node({rect: {left: 100, top: 100, width: 400, height: 200}});
  let names = [];
  document.startViewTransition = update => {
    names.push(['before', thumb.style.viewTransitionName, big.style.viewTransitionName]);
    update();
    names.push(['after', thumb.style.viewTransitionName, big.style.viewTransitionName]);
    return {finished: Promise.resolve()};
  };
  const order = [];
  assert.equal(await motion.share({from: thumb, to: big, update: () => order.push('draw')}), true);
  assert.deepEqual(names, [['before', 'share', undefined], ['after', '', 'share']]);
  assert.equal(big.style.viewTransitionName, '', '结束后去掉名字');
  assert.equal(animations.length, 0);
  delete document.startViewTransition;
  const done = motion.share({from: thumb, to: big, update: () => order.push('draw again')});
  assert.deepEqual(order, ['draw', 'draw again']);
  assert.equal(animations.length, 1);
  assert.equal(animations[0].el, big);
  assert.equal(animations[0].options.duration, 380);
  assert.deepEqual(animations[0].keyframes, motion.shareFrames({x: 20, y: 30, w: 50, h: 50}, {x: 100, y: 100, w: 400, h: 200}));
  animations[0].end();
  assert.equal(await done, true);
});

test('关看图器：缩回缩略图的框并停在那里；没有框时淡出', async () => {
  const big = node({rect: {left: 100, top: 100, width: 400, height: 200}});
  const back = motion.shareBack(big, {x: 20, y: 30, w: 50, h: 50});
  const [first, last] = animations[0].keyframes;
  assert.equal(first.transform, 'none');
  assert.equal(last.transform, motion.shareFrames({x: 20, y: 30, w: 50, h: 50}, {x: 100, y: 100, w: 400, h: 200}).at(0).transform);
  assert.equal(animations[0].options.fill, 'forwards');
  animations[0].end();
  assert.equal(await back, true);
  motion.shareBack(big, null);
  assert.equal(animations[1].keyframes.at(-1).opacity, 0);
});

test('数字滚动：逐帧写到终值；又滚时接着新目标走', async () => {
  const queue = [];
  globalThis.requestAnimationFrame = fn => { queue.push(fn); return queue.length; };
  const el = node();
  const first = motion.countTo(el, 100, {from: 0, dur: 100});
  queue.shift()(0);
  assert.equal(el.textContent, '0');
  queue.shift()(50);
  const middle = Number(el.textContent);
  assert.ok(middle > 50 && middle < 100, `缓出：一半时间走过一大半，实际 ${middle}`);
  const second = motion.countTo(el, 10, {dur: 100});
  assert.equal(await first, false, '被新目标接走');
  queue.shift();
  queue.shift()(0); queue.shift()(100);
  assert.equal(await second, true);
  assert.equal(el.textContent, '10');
  // 没有起点时直接写。
  const other = node();
  assert.equal(await motion.countTo(other, 7), true);
  assert.equal(other.textContent, '7');
});

test('缩略图登记表：取最后登记、还在页面上、在视口里的那个；节点换图时旧登记去掉', () => {
  const a = node({rect: {left: 0, top: 0, width: 100, height: 100}});
  const b = node({rect: {left: 200, top: 0, width: 100, height: 100}});
  const below = node({rect: {left: 0, top: 2000, width: 100, height: 100}});
  const offA = motion.thumbs.add('img-1', a);
  motion.thumbs.add('img-1', b);
  motion.thumbs.add('img-1', below);
  assert.equal(motion.thumbs.find('img-1'), b, '视口外的不算');
  assert.equal(motion.thumbs.rect('img-1').left, 200);
  b.isConnected = false;
  assert.equal(motion.thumbs.find('img-1'), a, '摘掉的节点不算');
  motion.thumbs.add('img-2', a);
  assert.equal(motion.thumbs.find('img-1'), null, '节点改画 img-2 以后不再是 img-1');
  assert.equal(motion.thumbs.find('img-2'), a);
  offA();
  assert.equal(motion.thumbs.find('img-2'), a, '撤销的是 img-1 的登记，img-2 照旧');
  motion.thumbs.remove('img-2', a);
  assert.equal(motion.thumbs.rect('img-2'), null);
  assert.equal(motion.thumbs.find('missing'), null);
});
