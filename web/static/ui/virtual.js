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
// - 重新布局时以视口顶部那一张为锚：set() 前视口顶部是哪个键、往下滚过它多少，set() 后滚到同一个键的新位置加同样的
//   偏移（照现在网格的做法，docs/WEB_V2.md「浏览界面」）。那个键不在新框里时滚动位置不动。
// - 摘掉节点前先去掉里面（包括根节点自己）<img> 的 src、srcset 和 <picture> 里 <source> 的 srcset：浏览器随之放掉
//   解码后的位图，不等垃圾回收。
// - 滚动、尺寸变化只在自己的作用域上排 ui/frame.js 的读（量 scrollTop、视口高、content 在视口里的偏移）和写（挂、摘、定位），
//   一帧最多一次。帧任务的令牌每份列表各取各的（#751）：同一帧里有好几份列表时各排各的。
// - 框的顺序随意（瀑布不按 y 排序）：按 512px 一段建索引，取范围时只看相交的几段。
//
// 调用方要知道的四件事：
// 1. content 要是定位元素：节点是 position: absolute; left: 0; top: 0，相对 content 摆。content 的计算样式是 static 时，
//    第一次量的时候它给 content 写上 position: relative。content 的高度也由它写。
// 2. 部件根节点的 transform、width、height 归它管（定位只写这三项），部件自己不要用 css(el, 'transform', …) 写根节点；
//    动画（FLIP）用 ui/motion.js 的 WAAPI 动根节点没问题，WAAPI 不改内联样式。要在根节点上另做位移，包一层子节点。
// 3. DOM 里的先后按框在 set() 里的先后：新挂的节点插到已挂节点里它该在的位置，所以 Tab 和读屏按框的先后走。重新布局
//    换了先后时，已经挂着的节点不挪（挪动会丢焦点），DOM 次序可能暂时和新的先后不一致，直到它们滚出去重挂。
//    键盘导航（方向键在网格里走）由调用方按布局算，像 ui/grid.js 那样，不靠 DOM 次序。
// 4. 焦点：节点滚出保留范围时，如果焦点在它里面，先不摘，等焦点离开后下一次滚动或 set() 再摘；它的键从数据里没了
//    （图被删、换了列表）或整个列表释放时照摘，焦点随之回到 body，由调用方决定要不要把焦点挪到别处。
import {scope} from './scope.js';

const BAND = 512;

// 节点（含根节点自己）里的图：去掉地址，放掉解码后的位图。
export function releaseImages(el) {
  const imgs = [...el.querySelectorAll('img')], sources = [...el.querySelectorAll('source')];
  if (el.localName === 'img') imgs.push(el);
  for (const source of sources) source.removeAttribute('srcset');
  for (const img of imgs) { img.removeAttribute('srcset'); img.removeAttribute('src'); }
}

export function virtual(viewport, content, {create, margin = 1, scope: parent = null}) {
  const s = parent ? parent.child('virtual') : scope('virtual'), mounted = new Map();   // 键 -> {comp, box}
  const READ = s.key('virtual:read'), WRITE = s.key('virtual:write'), MEASURE = s.key('virtual:measure'), SET = s.key('virtual:set');
  let boxes = [], byKey = new Map(), bands = [], height = 0, positioned = null;
  let view = {top: 0, height: 0, offset: 0};

  // 量：滚动位置、视口高度，以及 content 顶部在滚动内容里的位置（content 上面可能还有页头）。
  function measure() {
    const vr = viewport.getBoundingClientRect(), cr = content.getBoundingClientRect();
    view = {top: viewport.scrollTop, height: viewport.clientHeight, offset: cr.top - vr.top + viewport.scrollTop};
    if (positioned === null) positioned = typeof getComputedStyle !== 'function' || getComputedStyle(content).position !== 'static';
  }
  function refresh() { s.read(measure, READ); s.write(apply, WRITE); }

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

  const focusedIn = el => { const active = document.activeElement; return Boolean(active && active !== document.body && el.contains(active)); };

  function apply() {
    if (!s.alive) return;
    if (positioned === false) { content.style.position = 'relative'; positioned = true; }
    const top = view.top - view.offset, screen = Math.max(view.height, 1);
    const want = within(top - margin * screen, top + view.height + margin * screen);
    const keep = within(top - 2 * margin * screen, top + view.height + 2 * margin * screen);
    for (const [key, entry] of mounted) {
      if (!byKey.has(key)) unmount(key, entry);
      else if (!keep.has(byKey.get(key)) && !focusedIn(entry.comp.el)) unmount(key, entry);
    }
    for (const k of [...want].sort((a, b) => a - b)) {
      const box = boxes[k];
      let entry = mounted.get(box.key);
      if (!entry) {
        const comp = create(box);
        comp.el.style.position = 'absolute'; comp.el.style.left = '0px'; comp.el.style.top = '0px';
        entry = {comp, box: null}; mounted.set(box.key, entry); content.insertBefore(comp.el, after(k));
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

  // 第 k 个框的节点该插在谁前面：已挂节点里框的序号比 k 大的、最小的那个。
  function after(k) {
    let best = null, at = Infinity;
    for (const [key, entry] of mounted) {
      const j = byKey.get(key);
      if (j > k && j < at && entry.comp.el.parentNode === content) { at = j; best = entry.comp.el; }
    }
    return best;
  }

  function position(el, box, before) {
    if (!before || before.x !== box.x || before.y !== box.y) el.style.transform = `translate(${box.x}px, ${box.y}px)`;
    if (!before || before.w !== box.w) el.style.width = `${box.w}px`;
    if (!before || before.h !== box.h) el.style.height = `${box.h}px`;
  }

  function unmount(key, entry) {
    releaseImages(entry.comp.el);
    entry.comp.el.remove(); entry.comp.dispose?.(); mounted.delete(key);
  }

  function set(next, total) {
    s.read(() => {
      if (!s.alive) return;
      measure();
      // 锚：视口顶部那条线穿过的第一个框（没有就取顶部以下最近的一个），以及视口顶部在它里面的偏移。
      const top = view.top - view.offset;
      let anchor = null;
      for (const k of within(top, top + 1)) if (!anchor || boxes[k].y < anchor.y) anchor = boxes[k];
      if (!anchor) for (const k of within(top, top + view.height)) if (!anchor || boxes[k].y < anchor.y) anchor = boxes[k];
      const shift = anchor ? top - anchor.y : 0, anchorKey = anchor?.key;
      s.write(() => {
        if (!s.alive) return;
        boxes = next; height = total; byKey = new Map(boxes.map((box, k) => [box.key, k])); index();
        content.style.height = `${height}px`;
        const moved = anchorKey !== undefined && byKey.has(anchorKey) ? boxes[byKey.get(anchorKey)] : null;
        if (moved && view.top > 0) {
          const scrollTop = Math.max(0, moved.y + shift + view.offset);
          if (Math.abs(scrollTop - view.top) > .5) { viewport.scrollTop = scrollTop; view.top = scrollTop; }
        }
        apply();
      }, SET);
    }, MEASURE);
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
