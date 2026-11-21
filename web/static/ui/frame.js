// 每帧一次的读写调度（0.4 渲染底座，#704）：先量后改，一帧里不来回交替读布局和改 DOM（那会让浏览器一帧排好几次版）。
//
//   const SCROLL = key('grid:scroll');   // 帧任务的键：一个令牌，每次调用发一个新的；名字只用于调试和报错文字
//   read(fn, k?)     排一个「读」：量尺寸、读 scrollTop、getBoundingClientRect。下一帧先跑完所有读。
//   write(fn, k?)    排一个「写」：改 DOM、样式、scrollTop。同一帧里所有读跑完以后再跑所有写。
//   两个都返回取消函数。部件里用 ui/scope.js 的 s.key / s.read / s.write：作用域释放时取消，跑完自己摘掉。
//
// - 按令牌去重：同一帧里同一个令牌只跑最后排进来的那个函数（位置还是第一次排进来时的位置）；读和写各算各的，同一个令牌
//   可以既排读又排写。滚动、缩放这类一帧里来好几次的事件，处理函数只排 read(measure, SCROLL)，不直接量和改。
// - 键只能是 key() 发的令牌（#751）。字符串键是全局的：同一帧里不同实例（按天视图的好几天、资料库的各张海报）或同一部件的
//   不同用途（焦点进场和滚动）写了同一个字符串，就只剩最后排的那个。传别的东西当键（字符串、数字、自己建的 Symbol）直接
//   抛 TypeError，不分开发模式：哪个测试、哪次走查跑到那一行都会失败。
// - 令牌在部件建的时候取，存在变量里：会同时有好几份的部件每一份各取各的，同一份里不同用途各取一个。部件走作用域的
//   s.key()，它发的令牌只配那个作用域的 s.read / s.write（ui/scope.js），模块顶层取不到实例的作用域，也就写不出几份共用
//   一个令牌。这里的 key() 给不经作用域、直接调 read / write 的单例模块（网格、拖放、导览），可以在模块顶层取。
//   不要在排任务的地方现取：read(fn, key('x')) 每次都是新令牌，等于不去重。
// - 一帧里的先后：读阶段里排的读、写都在这一帧跑（读跟在读后面，写排进这一帧的写阶段）；写阶段里排的读和写都挪到
//   下一帧（这一帧已经改过 DOM，再量要重新排版；再排的写放到下一帧，一个总在再排自己的写也停得下来）。
// - write 里不要无条件再排自己：那样会每帧跑一次、永不停下（不会卡死这一帧，但一直占着每一帧）。要动画用 ui/motion.js。
// - 一个任务抛错不影响同一帧的其他任务：错误照常报到控制台。
// - 这里和 ui/motion.js 是仅有的两处可以调 requestAnimationFrame 的地方（tools/check_frontend.py 规则 11）。
//
// 测试：Node 里没有 requestAnimationFrame，用 setScheduler(fn) 换成自己的（例如把回调存起来、手动 flush()）。

let reads = new Map(), writes = new Map(), nextReads = new Map(), nextWrites = new Map();
let phase = 'idle', pending = false, unique = 0;
let schedule = typeof requestAnimationFrame === 'function' ? fn => requestAnimationFrame(fn) : fn => setTimeout(fn, 16);

// 令牌只能由 key() 造：类不导出，read / write 按对象的身份去重。label 只用于调试和报错文字。
class Key {
  constructor(label) { this.label = label; Object.freeze(this); }
  toString() { return `key(${this.label})`; }
}

/** 发一个帧任务的令牌（#751）：每次调用都是新的。部件里用作用域的 s.key(label)。 */
export const key = label => new Key(String(label ?? ''));

function checked(k) {
  if (k == null || k instanceof Key) return k;
  const got = typeof k === 'string' ? `字符串 '${k}'` : `${typeof k} 类型的 ${String(k)}`;
  throw new TypeError(`ui/frame.js: 帧任务的键要用 key('名字') 发的令牌，收到的是${got}。字符串是全局的，不同实例、不同用途写了同一个就会互相顶掉（#751）`);
}

function queue(map, fn, k) {
  const slot = checked(k) ?? Symbol(unique++);
  map.set(slot, fn);
  if (!pending && phase === 'idle') { pending = true; schedule(flush); }
  return () => { if (map.get(slot) === fn) map.delete(slot); };
}

export function read(fn, k) { return queue(phase === 'write' ? nextReads : reads, fn, k); }

export function write(fn, k) { return queue(phase === 'write' ? nextWrites : writes, fn, k); }

function run(map) {
  // 逐个取出再跑：跑的过程中同一张表里新排进来的（读阶段里再排的读）也在这一轮跑掉。
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
  // 写阶段里排的读、写留到下一帧；读阶段的表这时已经空了，直接换过来（取消函数拿着的就是这两张表）。
  [reads, nextReads] = [nextReads, reads];
  [writes, nextWrites] = [nextWrites, writes];
  if (reads.size || writes.size) { pending = true; schedule(flush); }
}

// 测试用：换掉调度器（默认是 requestAnimationFrame），并清空还没跑的任务。
export function setScheduler(fn) {
  schedule = fn; reads = new Map(); writes = new Map(); nextReads = new Map(); nextWrites = new Map(); phase = 'idle'; pending = false;
}
