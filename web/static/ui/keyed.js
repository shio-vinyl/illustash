// 按键复用的列表（0.4 渲染底座，#704）：一组数据变了，按键找回已有的部件，只调换次序和增删，不重建没变的节点。
//
//   const list = keyed(container, {
//     key: item => item.uuid,
//     create: (item, index) => chip(item),        // 返回部件 {el, update(item, index), dispose()}（ui/el.js 的约定）
//     enter: (el, item, index) => …,              // 可选：新节点插进去以后调（第一次有数据的那次 set 不调，见下）
//     exit: (el, item) => promise,                // 可选：节点要摘掉时调；返回的 promise 落定后才摘
//     move: (el, item, from, to) => …,            // 可选：留下来的节点序号变了时调（DOM 已经调换好）
//     appear: false,                              // 可选：true 时第一次有数据的那次 set 也调 enter
//     scope: pageScope,                           // 可选：挂在哪个作用域下（ui/scope.js），父作用域释放时它跟着释放
//   });
//   list.set(items);   // 之后每次数据变了再 set
//   list.dispose();    // 立刻释放全部部件，包括还在退场的，并摘掉它们的节点
//
// - container 归这个列表管：里面只放列表的节点（以及还在退场的节点），不要往里塞别的。
// - 复用的部件每次 set 都会收到 update(item, index)；部件用 ui/el.js 的写入函数，值没变不动 DOM。
// - 初次填充不播进场：以「第一次有数据的 set」为准，不是第一次调 set。数据还没到时先 set([]) 很常见，数据到了那一批
//   是首屏，不该一张张飞进来；之后新来的项才调 enter。要首屏也播，传 appear: true。
// - 节点挪位置用最少的移动（最长递增子序列以外的才挪）：没动的节点不会被拔出来再插回去。被挪的节点在浏览器里会丢
//   焦点，所以焦点原来在被挪的节点里时，挪完把焦点放回原处（不滚动页面）。
// - create 中途抛错时，这一轮已经建好的新部件都释放掉再抛，列表保持原样（复用的部件也还没收到 update）。
// - 退场：节点先设 inert（不能点、不能聚焦、读屏跳过），留在原处，exit 的 promise 落定后再摘掉、dispose。
//   退场中的键又出现时建一个新部件，旧节点照常退场。exit 要保证 promise 会落定（ui/motion.js 的退场按时长兜底）。
// - 钩子只收函数，这里不导入 ui/motion.js；进场、退场、FLIP 由调用方传进来。
import {scope} from './scope.js';

export function keyed(container, {key, create, enter, exit, move, appear = false, scope: parent = null}) {
  const s = parent ? parent.child('keyed') : scope('keyed');
  let live = new Map();         // 键 -> {item, comp, index}
  const leaving = new Map();    // 还在退场的节点 -> 它的部件
  let filled = false;           // 有过一次带数据的 set

  function set(items) {
    if (!s.alive) return;
    const keys = items.map(key);
    // 先查键重复，再动部件：抛错时列表保持原样。
    if (new Set(keys).size !== keys.length) throw new Error(`ui/keyed.js: 键重复：${String(keys.find((k, i) => keys.indexOf(k) !== i))}`);
    // 先建新部件；中途抛错就把这一轮建好的释放掉，复用的部件还没动。
    const made = new Map();
    try {
      items.forEach((item, index) => { if (!live.has(keys[index])) made.set(keys[index], create(item, index)); });
    } catch (error) {
      for (const comp of made.values()) { try { comp.dispose?.(); } catch (e) { console.error(e); } }
      throw error;
    }
    const next = new Map(), order = [];
    items.forEach((item, index) => {
      const k = keys[index];
      let entry = live.get(k);
      if (entry) { entry.from = entry.index; entry.item = item; entry.index = index; entry.comp.update?.(item, index); }
      else entry = {item, index, from: -1, comp: made.get(k), fresh: true};
      next.set(k, entry); order.push(entry);
    });
    // 离开的：退场或者直接摘掉。
    for (const [k, entry] of live) if (!next.has(k)) leave(entry);
    live = next;
    place(order);
    const quiet = !filled && !appear;
    for (const entry of order) {
      if (entry.fresh) { entry.fresh = false; if (enter && !quiet) enter(entry.comp.el, entry.item, entry.index); }
      else if (move && entry.from !== entry.index) move(entry.comp.el, entry.item, entry.from, entry.index);
    }
    if (items.length) filled = true;
  }

  function leave(entry) {
    const el = entry.comp.el;
    const done = () => { if (!leaving.has(el)) return; leaving.delete(el); el.remove(); entry.comp.dispose?.(); };
    if (!exit) { el.remove(); entry.comp.dispose?.(); return; }
    el.inert = true; leaving.set(el, entry.comp);
    Promise.resolve().then(() => exit(el, entry.item)).then(done, done);
  }

  // 按 order 排好节点：旧节点在 DOM 里的先后取最长递增子序列，这些不动，其余的从后往前插到下一个节点前面。
  function place(order) {
    const position = new Map();
    let n = 0;
    for (let node = container.firstChild; node; node = node.nextSibling) if (!leaving.has(node)) position.set(node, n++);
    const old = order.map(entry => entry.fresh ? -1 : position.get(entry.comp.el) ?? -1);
    const stay = increasing(old), active = document.activeElement;
    let before = null, movedFocus = false;
    for (let i = order.length - 1; i >= 0; i--) {
      const el = order[i].comp.el;
      if (!stay.has(i)) {
        if (old[i] >= 0 && active && el.contains(active)) movedFocus = true;
        container.insertBefore(el, before && before.parentNode === container ? before : null);
      }
      before = el;
    }
    if (movedFocus && document.activeElement !== active && active.isConnected) active.focus({preventScroll: true});
  }

  s.add(() => {
    for (const entry of live.values()) { entry.comp.el.remove(); entry.comp.dispose?.(); }
    for (const [el, comp] of leaving) { el.remove(); comp.dispose?.(); }
    live = new Map(); leaving.clear();
  });
  return {
    set,
    get: k => live.get(k)?.comp ?? null,
    get size() { return live.size; },
    dispose: () => s.dispose(),
  };
}

// a 里最长严格递增子序列（忽略 -1）的下标集合。O(n log n)。
export function increasing(a) {
  const tails = [], prev = new Array(a.length).fill(-1);
  for (let i = 0; i < a.length; i++) {
    if (a[i] < 0) continue;
    let lo = 0, hi = tails.length;
    while (lo < hi) { const mid = (lo + hi) >>> 1; if (a[tails[mid]] < a[i]) lo = mid + 1; else hi = mid; }
    if (lo > 0) prev[i] = tails[lo - 1];
    tails[lo] = i;
  }
  const keep = new Set();
  for (let i = tails.length ? tails[tails.length - 1] : -1; i >= 0; i = prev[i]) keep.add(i);
  return keep;
}
