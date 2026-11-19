// 生命周期（0.4 渲染底座，#704）：一个部件或页面挂上的监听、计时器、观察器、帧任务和子部件都登记在一个作用域里，
// dispose() 一次清掉。dispose() 可以重复调用，第二次起什么也不做。
//
//   const s = scope('detail-card');                 // 或 parent.child('名')：父作用域释放时子作用域跟着释放
//   s.on(button, 'click', () => …);                  // addEventListener，释放时 removeEventListener
//   s.timeout(fn, 300); s.interval(fn, 1000);        // 释放时 clear
//   s.observe(new ResizeObserver(…)).observe(el);    // 任何带 disconnect() 的观察器
//   s.add(child);                                    // 子部件（带 dispose()）或者一个清理函数，例如 frame.write() 的返回值
//   s.signal                                         // AbortSignal，释放时 abort（交给 fetch 之类）
//   s.alive                                          // 还没释放
//   s.dispose();
//
// 每个页面一个作用域：page('名') 释放上一页的作用域、建一个新的返回（由页面框架在换页时调用，#706）。
// 全应用一直在的部件（活页条、浮层管理器）挂在 app 上，不随换页释放。部件的作用域应当是这两者之一的后代。
//
// 开发模式（docs/WEB_V2.md「渲染底座」：在开发者工具里运行 localStorage['pi.dev'] = 'true' 后重新载入；测试里用
// setDev(true)）下记着所有还活着的作用域：换页后，上一页期间建的、不属于 app 也不属于新一页的根作用域还没释放，
// 就在控制台提示（多半是忘了挂到页面作用域上，或者忘了 dispose）。live() 给出还活着的作用域数和各名字的数量，测试读它。
import {prefs} from '../core/prefs.js';

let dev = prefs.get('dev', false) === true;
const living = new Set();
let current = null;   // 当前页面的作用域

class Scope {
  constructor(name, parent = null) {
    this.name = name; this.parent = parent; this.alive = true; this.cleanups = [];
    this.page = parent ? parent.page : current?.name ?? '';
    if (dev) living.add(this);
  }
  add(thing) {
    if (!thing) return thing;
    const clean = typeof thing === 'function' ? thing : () => thing.dispose();
    if (!this.alive) { clean(); return thing; }   // 已经释放的作用域：登记即释放，不留下孤儿
    this.cleanups.push(clean);
    return thing;
  }
  on(target, type, fn, options) {
    target.addEventListener(type, fn, options);
    this.add(() => target.removeEventListener(type, fn, options));
    return fn;
  }
  timeout(fn, ms) { const id = setTimeout(fn, ms); this.add(() => clearTimeout(id)); return id; }
  interval(fn, ms) { const id = setInterval(fn, ms); this.add(() => clearInterval(id)); return id; }
  observe(observer) { this.add(() => observer.disconnect()); return observer; }
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
    for (const clean of this.cleanups.splice(0).reverse()) {
      try { clean(); } catch (error) { console.error(error); }
    }
  }
}

export const scope = name => new Scope(name);
export const app = new Scope('app');

export function page(name) {
  const before = current;
  before?.dispose();
  current = new Scope('page:' + name);
  if (dev && before) {
    const left = [...living].filter(s => !s.parent && s !== app && s !== current && s.page === before.name);
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
