// 每帧一次的读写调度（0.4 渲染底座，#704）：先量后改，一帧里不来回交替读布局和改 DOM（那会让浏览器一帧排好几次版）。
//
//   read(fn, key?)   排一个「读」：量尺寸、读 scrollTop、getBoundingClientRect。下一帧先跑完所有读。
//   write(fn, key?)  排一个「写」：改 DOM、样式、scrollTop。同一帧里所有读跑完以后再跑所有写。
//   两个都返回取消函数（交给 ui/scope.js 的作用域登记，页面释放时取消还没跑的）。
//
// - 带 key 的按键去重：同一帧里同一个 key 只跑最后排进来的那个函数（位置还是第一次排进来时的位置）。滚动、缩放这类
//   一帧里来好几次的事件，处理函数只排 read(measure, 'grid:scroll')，不直接量和改。
// - 读阶段里排的写在同一帧的写阶段跑；写阶段里排的写也在这一帧跑；写阶段里排的读挪到下一帧（这一帧已经改过 DOM，
//   再量就要重新排版）。
// - 一个任务抛错不影响同一帧的其他任务：错误照常报到控制台。
// - 这里和 ui/motion.js 是仅有的两处可以调 requestAnimationFrame 的地方（tools/check_frontend.py 规则 11）。
//
// 测试：Node 里没有 requestAnimationFrame，用 setScheduler(fn) 换成自己的（例如把回调存起来、手动 flush()）。

let reads = new Map(), writes = new Map(), later = new Map();
let phase = 'idle', pending = false, unique = 0;
let schedule = typeof requestAnimationFrame === 'function' ? fn => requestAnimationFrame(fn) : fn => setTimeout(fn, 16);

function queue(map, fn, key) {
  const k = key ?? Symbol(unique++);
  map.set(k, fn);
  if (!pending && phase === 'idle') { pending = true; schedule(flush); }
  return () => { if (map.get(k) === fn) map.delete(k); };
}

export function read(fn, key) {
  // 写阶段里排的读：等下一帧。
  if (phase === 'write') return queue(later, fn, key);
  return queue(reads, fn, key);
}

export function write(fn, key) { return queue(writes, fn, key); }

function run(map) {
  // 逐个取出再跑：跑的过程中同一张表里新排进来的（写阶段里再排的写）也在这一轮跑掉。
  while (map.size) {
    const [k, fn] = map.entries().next().value;
    map.delete(k);
    try { fn(); } catch (error) { report(error); }
  }
}

function report(error) {
  if (typeof reportError === 'function') reportError(error); else console.error(error);
}

// 跑一帧：所有读，然后所有写。由调度器调用；测试里也可以直接调。
export function flush() {
  pending = false;
  phase = 'read'; run(reads);
  phase = 'write'; run(writes);
  phase = 'idle';
  if (later.size) { reads = later; later = new Map(); }
  if (reads.size || writes.size) { pending = true; schedule(flush); }
}

// 测试用：换掉调度器（默认是 requestAnimationFrame），并清空还没跑的任务。
export function setScheduler(fn) {
  schedule = fn; reads = new Map(); writes = new Map(); later = new Map(); phase = 'idle'; pending = false;
}
