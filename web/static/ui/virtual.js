// 只挂视口附近的节点（0.4 渲染底座，#704）：给它布局算好的框和滚动容器，它只把视口上下各约一屏里的框挂成节点。
// macOS 14 的 WKWebView 没有 content-visibility，长页面（按天视图、画廊、资料库长列表）靠它控制节点数和图形内存（#683）。
//
//   const list = virtual(viewport, content, {
//     create: box => card(box),     // 返回部件 {el, update(box), dispose()}；box 是下面 set() 给的框
//     margin: 1,                    // 视口上下各挂几屏（默认 1）；超出 2 × margin 屏的才摘，来回小幅滚动不反复建
//     scope: pageScope,             // 可选：挂在哪个作用域下（ui/scope.js），父作用域释放时它跟着释放
//   });
//   list.set(boxes, height);        // 框：{key, x, y, w, h, …}，坐标相对 content 左上角；height 是 content 的总高
//   list.dispose();
//
// - 节点按键复用：重新布局（窗口变宽、分页到达、数据变了）后同一个键还在范围里，就是同一个部件，收到 update(box)。
// - 定位只写 transform、width、height（节点的 position: absolute 由它设上），content 的高度由它写。
// - 重新布局时以视口顶部那一张为锚：set() 前视口顶部是哪个键、往下滚过它多少，set() 后滚到同一个键的新位置加同样的
//   偏移（照现在网格的做法，docs/WEB_V2.md「浏览界面」）。那个键不在新框里时滚动位置不动。
// - 摘掉节点前先去掉里面 <img> 的 src、srcset：浏览器随之放掉解码后的位图，不等垃圾回收。
// - 滚动、尺寸变化只排 ui/frame.js 的 read（量 scrollTop、视口高、content 在视口里的偏移）和 write（挂、摘、定位），
//   一帧最多一次。
// - 框的顺序随意（瀑布不按 y 排序）：按 512px 一段建索引，取范围时只看相交的几段。
import {read, write} from './frame.js';
import {scope} from './scope.js';

const BAND = 512;
let ids = 0;

export function virtual(viewport, content, {create, margin = 1, scope: parent = null}) {
  const s = parent ? parent.child('virtual') : scope('virtual'), id = `virtual:${ids++}`, mounted = new Map();   // 键 -> {comp, box}
  let boxes = [], byKey = new Map(), bands = [], height = 0;
  let view = {top: 0, height: 0, offset: 0};

  // 量：滚动位置、视口高度，以及 content 顶部在滚动内容里的位置（content 上面可能还有页头）。
  function measure() {
    const vr = viewport.getBoundingClientRect(), cr = content.getBoundingClientRect();
    view = {top: viewport.scrollTop, height: viewport.clientHeight, offset: cr.top - vr.top + viewport.scrollTop};
  }
  function refresh() { read(measure, id + ':read'); write(apply, id + ':write'); }

  function index() {
    bands = [];
    boxes.forEach((box, k) => {
      for (let b = Math.floor(box.y / BAND), end = Math.floor((box.y + box.h) / BAND); b <= end; b++) (bands[b] ||= []).push(k);
    });
  }
  function within(top, bottom) {
    const found = new Set();
    for (let b = Math.max(0, Math.floor(top / BAND)), end = Math.floor(bottom / BAND); b <= end && b < bands.length; b++) {
      for (const k of bands[b] || []) { const box = boxes[k]; if (box.y < bottom && box.y + box.h > top) found.add(k); }
    }
    return found;
  }

  function apply() {
    if (!s.alive) return;
    const top = view.top - view.offset, screen = Math.max(view.height, 1);
    const want = within(top - margin * screen, top + view.height + margin * screen);
    const keep = within(top - 2 * margin * screen, top + view.height + 2 * margin * screen);
    for (const [key, entry] of mounted) if (!byKey.has(key) || !keep.has(byKey.get(key))) unmount(key, entry);
    for (const k of want) {
      const box = boxes[k];
      let entry = mounted.get(box.key);
      if (!entry) {
        const comp = create(box);
        comp.el.style.position = 'absolute'; comp.el.style.left = '0px'; comp.el.style.top = '0px';
        entry = {comp, box: null}; mounted.set(box.key, entry); content.appendChild(comp.el);
      } else if (entry.box !== box) entry.comp.update?.(box);
      if (entry.box !== box) position(entry.comp.el, box, entry.box);
      entry.box = box;
    }
    // 已挂着、在保留范围里、但不在这次想要的范围里的，位置也跟上新布局。
    for (const entry of mounted.values()) {
      const box = boxes[byKey.get(entry.box.key)];
      if (box && box !== entry.box) { entry.comp.update?.(box); position(entry.comp.el, box, entry.box); entry.box = box; }
    }
  }

  function position(el, box, before) {
    if (!before || before.x !== box.x || before.y !== box.y) el.style.transform = `translate(${box.x}px, ${box.y}px)`;
    if (!before || before.w !== box.w) el.style.width = `${box.w}px`;
    if (!before || before.h !== box.h) el.style.height = `${box.h}px`;
  }

  function unmount(key, entry) {
    for (const img of entry.comp.el.querySelectorAll('img')) { img.removeAttribute('srcset'); img.removeAttribute('src'); }
    entry.comp.el.remove(); entry.comp.dispose?.(); mounted.delete(key);
  }

  function set(next, total) {
    read(() => {
      measure();
      // 锚：视口顶部那条线穿过的第一个框（没有就取顶部以下最近的一个），以及视口顶部在它里面的偏移。
      const top = view.top - view.offset;
      let anchor = null;
      for (const k of within(top, top + 1)) if (!anchor || boxes[k].y < anchor.y) anchor = boxes[k];
      if (!anchor) for (const k of within(top, top + view.height)) if (!anchor || boxes[k].y < anchor.y) anchor = boxes[k];
      const shift = anchor ? top - anchor.y : 0, anchorKey = anchor?.key;
      write(() => {
        boxes = next; height = total; byKey = new Map(boxes.map((box, k) => [box.key, k])); index();
        content.style.height = `${height}px`;
        const moved = anchorKey !== undefined && byKey.has(anchorKey) ? boxes[byKey.get(anchorKey)] : null;
        if (moved && view.top > 0) {
          const scrollTop = Math.max(0, moved.y + shift + view.offset);
          if (Math.abs(scrollTop - view.top) > .5) { viewport.scrollTop = scrollTop; view.top = scrollTop; }
        }
        apply();
      }, id + ':set');
    }, id + ':measure');
  }

  s.add(() => { for (const [key, entry] of [...mounted]) unmount(key, entry); });
  s.on(viewport, 'scroll', refresh, {passive: true});
  if (typeof ResizeObserver === 'function') s.observe(new ResizeObserver(refresh)).observe(viewport);

  return {
    set,
    refresh,
    get mounted() { return mounted.size; },
    comp: key => mounted.get(key)?.comp ?? null,
    dispose: () => s.dispose(),
  };
}
