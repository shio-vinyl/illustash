// 生命周期（0.4 渲染底座，#704）：一个部件或页面挂上的监听、计时器、观察器、帧任务和子部件都登记在一个作用域里，
// dispose() 一次清掉。dispose() 可以重复调用，第二次起什么也不做。
//
//   const s = scope('detail-card');                 // 或 parent.child('名')：父作用域释放时子作用域跟着释放
//   const off = s.on(button, 'click', () => …);      // addEventListener；释放时 removeEventListener，也可以提前 off()
//   s.timeout(fn, 300); s.interval(fn, 1000);        // 释放时 clear；timeout 触发后自己从作用域里摘掉
//   const MEASURE = s.key('card:measure');           // 帧任务的令牌：只配这个作用域的 read / write（#751）
//   s.read(fn, MEASURE); s.write(fn, MEASURE);       // ui/frame.js 的 read / write；跑完或释放时取消，跑完后自己摘掉
//   s.observe(new ResizeObserver(…)).observe(el);    // 任何带 disconnect() 的观察器
//   s.add(child);                                    // 子部件（带 dispose()）或一个清理函数；s.remove(child) 摘掉不清
//   s.signal                                         // AbortSignal，释放时 abort（交给 fetch 之类）
//   s.alive                                          // 还没释放
//   s.dispose();
//
// 作用域活得越久，登记进去的东西越要是「一直用到释放」的：timeout、read、write 跑完会自己摘掉，可以反复用；add() 进去的
// 清理函数要等 dispose 才走，不要每次滚动、每次输入都 add 一个（例如 add(frame.write(…)) 只适合一次性的任务，反复排的
// 帧任务用 s.write(fn, 建的时候取好的令牌)）。活页条这类挂在 app 上的部件尤其如此。
//
// 帧任务的键是令牌，不是字符串（#751）：s.key('名字') 每次发一个新的，名字只用于调试和报错文字。同一个令牌同一帧里只跑
// 最后排的那次（读、写各算各的）；不同的令牌互不相干，所以同一帧里挂上好几份的部件（按天视图的每一天、每张海报）各跑各的。
// 部件在建的时候、在自己的作用域上取令牌，同一部件里不同用途各取一个。s.read / s.write 只认这个作用域自己发的令牌：
// 传字符串、ui/frame.js 的 key() 或别的作用域发的令牌都抛 TypeError（不分开发模式），所以模块顶层取一个令牌给每份部件
// 共用是写不出来的（模块顶层没有实例的作用域）。
//
// 每个页面一个作用域：page('名') 释放上一页的作用域、建一个新的返回（由页面框架在换页时调用，#706）。
// 全应用一直在的部件（活页条、浮层管理器）挂在 app 上，不随换页释放。部件的作用域应当是这两者之一的后代。
//
// 开发模式（docs/WEB_V2.md「渲染底座」：在开发者工具里运行 localStorage['pi.dev'] = 'true' 后重新载入；测试里用
// setDev(true)）下记着所有还活着的作用域：换页后，上一页期间建的、不属于 app 也不属于新一页的根作用域还没释放，
// 就在控制台提示一次（多半是忘了挂到页面作用域上，或者忘了 dispose）。live() 给出还活着的作用域数和各名字的数量，测试读它。
import {prefs} from '../core/prefs.js';
import {key, read, write} from './frame.js';

// 作用域发的令牌 -> {owner, read, write}：owner 是发它的作用域；read、write 是它在这个作用域里各占的一格登记
// （同一个令牌既排读又排写时两边各记一项，互不覆盖）。
const issued = new WeakMap();

let dev = prefs.get('dev', false) === true;
const living = new Set();
let current = null, visits = 0;   // 当前页面的作用域；第几次进页面（每次 page() 加一）

class Scope {
  constructor(name, parent = null) {
    this.name = name; this.parent = parent; this.alive = true; this.cleanups = new Map();   // 登记的东西 -> 清理函数
    this.visit = parent ? parent.visit : current?.visit ?? 0;
    if (dev) living.add(this);
  }
  add(thing) {
    if (!thing) return thing;
    const clean = typeof thing === 'function' ? thing : () => thing.dispose();
    if (!this.alive) { clean(); return thing; }   // 已经释放的作用域：登记即释放，不留下孤儿
    this.cleanups.set(thing, clean);
    return thing;
  }
  remove(thing) { this.cleanups.delete(thing); return thing; }
  on(target, type, fn, options) {
    const off = () => { target.removeEventListener(type, fn, options); this.cleanups.delete(off); };
    target.addEventListener(type, fn, options);
    this.add(off);
    return off;
  }
  timeout(fn, ms) {
    const clean = () => clearTimeout(id), id = setTimeout(() => { this.cleanups.delete(clean); fn(); }, ms);
    this.add(clean);
    return id;
  }
  interval(fn, ms) { const id = setInterval(fn, ms); this.add(() => clearInterval(id)); return id; }
  observe(observer) { this.add(() => observer.disconnect()); return observer; }
  // 帧任务的令牌（#751）：部件建的时候取，同一部件里不同用途各取一个。方法体里的 key、read、write 是 ui/frame.js 的。
  key(label) {
    const k = key(label);
    issued.set(k, {owner: this, read: {}, write: {}});
    return k;
  }
  read(fn, k) { return this.frame(read, 'read', fn, k); }
  write(fn, k) { return this.frame(write, 'write', fn, k); }
  // 帧任务：同一个令牌读、写各只记一项（frame.js 按令牌去重，后排的替换先排的）；跑完自己摘掉。
  frame(queue, phase, fn, k) {
    const mine = k == null ? null : issued.get(k);
    if (k != null && mine?.owner !== this) throw foreign(this, k);
    if (!this.alive) return () => {};
    const slot = mine ? mine[phase] : Symbol('task');
    let cancel = null;
    const clean = () => cancel();
    cancel = queue(() => { if (this.cleanups.get(slot) === clean) this.cleanups.delete(slot); fn(); }, k);
    this.cleanups.set(slot, clean);
    return clean;
  }
  // 子作用域单独记着、释放时从父作用域里摘掉：父作用域活得久、子部件来来去去（列表项）时不越积越多。
  child(name) {
    const s = new Scope(name, this);
    if (!this.alive) { s.dispose(); return s; }
    (this.children ||= new Set()).add(s);
    return s;
  }
  get signal() {
    if (!this.controller) { this.controller = new AbortController(); this.add(() => this.controller.abort()); }
    return this.controller.signal;
  }
  dispose() {
    if (!this.alive) return;
    this.alive = false;
    living.delete(this);
    this.parent?.children?.delete(this);
    for (const s of [...(this.children || [])].reverse()) s.dispose();
    // 后登记的先清（子部件先于它用到的监听释放）；一项出错不挡住其余几项。
    const cleans = [...this.cleanups.values()].reverse();
    this.cleanups.clear();
    for (const clean of cleans) {
      try { clean(); } catch (error) { console.error(error); }
    }
  }
}

// 不是这个作用域发的键：说清楚收到的是什么，改法写在文件开头。
function foreign(target, k) {
  const from = issued.get(k)?.owner;
  const got = typeof k === 'string' ? `字符串 '${k}'` : from ? `作用域 ${from.name} 发的令牌 ${k}`
    : typeof k?.label === 'string' ? `ui/frame.js 的 key() 发的令牌 ${k}` : `${typeof k} 类型的 ${String(k)}`;
  return new TypeError(`ui/scope.js: 作用域 ${target.name} 的 read / write 只认它自己 s.key('名字') 发的令牌（不认字符串、ui/frame.js 的 key() `
    + `和别的作用域发的），收到的是${got}。部件在建的时候在自己的作用域上取令牌，每份各取各的（#751）`);
}

export const scope = name => new Scope(name);
export const app = new Scope('app');

export function page(name) {
  const before = current;
  before?.dispose();
  current = new Scope('page:' + name);
  current.visit = ++visits;
  if (dev && before) {
    // 只看上一次进页面期间建的：更早漏掉的已经提示过，不再每次换页都报。
    const left = [...living].filter(s => !s.parent && s !== app && s !== current && s.visit === before.visit);
    if (left.length) console.warn(`[scope] 换页后还有 ${left.length} 个作用域没释放（建于 ${before.name}）：`, left.map(s => s.name).join('、'));
  }
  return current;
}
export const currentPage = () => current;

// 开发模式下还活着的作用域：{count, names: {名字: 个数}}。不在开发模式时是 null。
export function live() {
  if (!dev) return null;
  const names = {};
  for (const s of living) names[s.name] = (names[s.name] || 0) + 1;
  return {count: living.size, names};
}
// 测试用：打开或关掉开发模式（不改 localStorage）。打开时从现在起登记。
export function setDev(on) { dev = Boolean(on); if (!dev) living.clear(); else if (app.alive) living.add(app); }
