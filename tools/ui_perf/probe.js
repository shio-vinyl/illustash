// 界面性能测量（#683）的页内探针。两个内核注入同一份：Chromium 经 DevTools 协议（chromium.mjs），WebKit 经 WKWebView
// （webkit.swift）。场景的每一步都由这里执行，所以两边的滚动方式、计时窗口和指标口径一致。
//
// 依赖页面结构的部分（入口地址、页面就绪的标志、滚动容器、怎么开大图和翻页、怎么触发同步、同步时盯着哪一块）全在
// 下面的 PAGES 里。界面改版（#716）后只改这一段，场景和指标不动。
//
// 指标：
//   帧        requestAnimationFrame 的间隔。两个内核都有；长帧是间隔超过 50ms 的帧。
//   长任务    PerformanceObserver('longtask')，只有 Chromium 有（WebKit 没有这个 API，结果里写 null）。
//   DOM       测量窗口里新增、删掉的元素数；删掉的已显示图片（<img> 已解码完）数，以及删的时候它在视野里的数——
//             同步导入时「可见的那一天整块重建」就是后者不为零。「在视野里」只数内容区（#viewport 里的图）：活页条（#706：
//             同步完成时二级层顶上挤进新的一天，最早那天收进按月的一摞，它的封面节点照设计会摘掉）和浮层不算，预算第 2、3 条
//             管的是内容区（#683 评论；2026-11-20 C1 #757 单跑定位到同步时拆掉的那一张是活页条的时间轴封面）。
//   快照      元素总数、<img> 数和已加载的数、滚动位置和总高度；Chromium 另有 JS 堆（performance.memory）。
(() => {
  if (window.__perf) return;
  const $ = id => document.getElementById(id);
  const viewport = () => $('viewport');
  const PAGES = {
    // 应用启动完、能接受跳转的标志。
    // 新前端认活页条，改前的前端（main dfbabc62）认旧侧栏，同一份探针量两边。
    booted: () => !!viewport() && !$('splash') && !!document.querySelector('#rail .rail-entry[data-entry], .nav[data-page]'),
    // 图库的按天视图（#707，原来的首页）。场景名仍叫 home，和改前的数字对照：地址写 #/，新前端落在按天视图、改前（main
    // dfbabc62）的前端落在首页，就绪标志两种都认，所以改前改后用同一份探针量。
    home: {hash: '#/', ready: () => !!document.querySelector('#homePage .day-tile img, #homePage .home-day img'), scroller: viewport,
      // 同步导入时盯着的「最上面那一天」：新图都进这一块。
      watch: () => $('homePage')},
    library: {hash: '#/library', ready: () => !!document.querySelector('#grid .card[data-id] img'), scroller: viewport, watch: () => $('grid')},
    // 画廊（#710）的瀑布、色彩：一个放进全部图的手选画廊，网格和图库同一套，只换排法和排序。prepare 在跳过去之前建好
    // 画廊（每次运行的图库都是新恢复的）、记好展示方式，返回要去的地址。
    galleryMasonry: {prepare: () => perfGallery('masonry'), ready: () => !!document.querySelector('#grid .card[data-id] img'), scroller: viewport,
      watch: () => $('grid')},
    galleryColor: {prepare: () => perfGallery('color'), ready: () => !!document.querySelector('#grid .card[data-id] img'), scroller: viewport,
      watch: () => $('grid')},
    characters: {hash: '#/characters', ready: () => !!document.querySelector('#charIndex .cast-poster img, #charIndex .cat-poster img'), scroller: viewport,
      watch: () => $('charIndex')},
    viewer: {
      hash: '#/library', ready: () => !!document.querySelector('#grid .card[data-id] img'), scroller: viewport, watch: () => $('viewer'),
      // 双击网格里的第一张打开大图，点「下一张」翻页。看图器三张轮换（#709）：image 给一个固定的对象，src、complete、
      // naturalWidth 读当前那一张的预览图，换图时 src 会变。改前的前端（main dfbabc62）只有一张 #viewerImg，没有轮换的槽时
      // 读它，同一份探针量两边（#716）。
      open: () => document.querySelector('#grid .card[data-id]').dispatchEvent(new MouseEvent('dblclick', {bubbles: true, cancelable: true})),
      opened: () => !$('viewer').hidden,
      next: () => $('viewerNext').click(),
      image: () => {
        const shown = () => document.querySelector('#viewer .vw-slot.is-current .vw-low') || $('viewerImg') || {};
        return {get src() { return shown().src; }, get complete() { return shown().complete; }, get naturalWidth() { return shown().naturalWidth; }};
      },
    },
    // 页面自己的同步入口：和上手指引里「同步」按钮一样，先 startSync 再 pollSync（同步进行中每 2 秒读一次，有新图就重画）。
    // 上一次还没结束时不发新的。返回这次有没有发出。
    sync: async () => {
      const jobs = await import('/core/jobs.js'), {state} = await import('/core/state.js');
      if (jobs.SYNC_ACTIVE.includes(state.sync?.status)) return false;
      await jobs.startSync();
      jobs.pollSync();
      return true;
    },
    // 同步落定后读到的「这次新加入」：只用来核对同步确实在导入。
    imported: async () => (await import('/core/state.js')).state.sync?.imported || 0,
  };

  const sleep = ms => new Promise(r => setTimeout(r, ms));
  // 下一帧。窗口被挡住或在后台时 requestAnimationFrame 不来：1 秒后照样往下走，记一次 stall（结果里的 rafStalls）。
  let stalls = 0;
  const frame = () => new Promise(r => {
    let done = false;
    const timer = setTimeout(() => { if (!done) { done = true; stalls++; r(performance.now()); } }, 1000);
    requestAnimationFrame(t => { if (!done) { done = true; clearTimeout(timer); r(t); } });
  });
  async function until(test, limit, what) {
    const end = performance.now() + limit;
    while (performance.now() < end) {
      try { if (await test()) return true; } catch { /* 页面在换 */ }
      await sleep(50);
    }
    throw new Error('等' + what + '超时');
  }
  const quantile = (xs, q) => {
    if (!xs.length) return null;
    const s = [...xs].sort((a, b) => a - b);
    return s[Math.min(s.length - 1, Math.floor(q * s.length))];
  };
  const round = x => x == null ? null : Math.round(x * 10) / 10;

  // ---------- 记录器：一个测量窗口里的帧、长任务和 DOM 变化 ----------
  let current = null;
  const longtasks = 'PerformanceObserver' in window && PerformanceObserver.supportedEntryTypes?.includes('longtask');
  if (longtasks) new PerformanceObserver(list => { if (current) for (const e of list.getEntries()) current.long.push(e.duration); }).observe({type: 'longtask', buffered: false});
  const visible = new Set();
  const seen = new IntersectionObserver(es => { for (const e of es) e.isIntersecting ? visible.add(e.target) : visible.delete(e.target); });
  const loaded = img => img.complete && img.naturalWidth > 0;
  function track(node) {
    if (node.nodeType !== 1) return;
    if (node.tagName === 'IMG') seen.observe(node);
    for (const img of node.getElementsByTagName('img')) seen.observe(img);
  }
  const mutations = new MutationObserver(records => {
    for (const r of records) {
      for (const n of r.addedNodes) { if (n.nodeType !== 1) continue; track(n); if (current) current.added += 1 + n.getElementsByTagName('*').length; }
      for (const n of r.removedNodes) {
        if (n.nodeType !== 1) continue;
        const imgs = n.tagName === 'IMG' ? [n] : [...n.getElementsByTagName('img')];
        if (current) {
          current.removed += 1 + n.getElementsByTagName('*').length;
          const inContent = viewport()?.contains(r.target);   // 从内容区里摘下来的（见开头「DOM」）
          for (const img of imgs) { if (loaded(img)) { current.removedImages++; if (inContent && visible.has(img)) current.removedVisible++; } }
        }
        for (const img of imgs) { seen.unobserve(img); visible.delete(img); }
      }
    }
  });
  // 应用启动完（boot）才开始盯 DOM：注入时页面可能还在跳转，没有 body。
  let watching = false;
  function watch() { if (watching) return; watching = true; mutations.observe(document.body, {childList: true, subtree: true}); track(document.body); }

  function begin() {
    const rec = {start: performance.now(), frames: [], long: [], added: 0, removed: 0, removedImages: 0, removedVisible: 0, stop: false};
    current = rec;
    let last = null;
    const loop = t => { if (rec.stop) return; if (last != null) rec.frames.push(t - last); last = t; requestAnimationFrame(loop); };
    requestAnimationFrame(loop);
    return rec;
  }
  function end(rec) {
    rec.stop = true;
    if (current === rec) current = null;
    const f = rec.frames, ms = performance.now() - rec.start;
    return {
      ms: Math.round(ms),
      frames: {count: f.length, fps: round(f.length / (ms / 1000)), p50: round(quantile(f, .5)), p95: round(quantile(f, .95)),
        p99: round(quantile(f, .99)), max: round(f.length ? Math.max(...f) : null), over50: f.filter(x => x > 50).length,
        over100: f.filter(x => x > 100).length},
      longtasks: longtasks ? {count: rec.long.length, total: Math.round(rec.long.reduce((a, b) => a + b, 0)),
        max: round(rec.long.length ? Math.max(...rec.long) : 0), over100: rec.long.filter(x => x > 100).length} : null,
      dom: {added: rec.added, removed: rec.removed, removedImages: rec.removedImages, removedVisible: rec.removedVisible},
    };
  }
  function snapshot(page) {
    const imgs = [...document.getElementsByTagName('img')], s = page && PAGES[page]?.scroller();
    return {elements: document.getElementsByTagName('*').length, images: imgs.length, imagesLoaded: imgs.filter(loaded).length,
      imagesVisible: visible.size, scrollTop: s ? Math.round(s.scrollTop) : null, scrollHeight: s ? s.scrollHeight : null,
      jsHeap: performance.memory ? Math.round(performance.memory.usedJSHeapSize / 1048576) : null};
  }

  async function perfGallery(show) {
    // 展示方式照应用里那样记进偏好（先读一次，免得页面第一次 loadShows() 用存着的偏好盖掉它）。
    const galleries = await import('/core/galleries.js'), shows = await import('/core/gallery_shows.js');
    await shows.loadShows();
    let g = (await galleries.loadGalleries()).find(x => x.name === '性能测量');
    if (!g) {
      await galleries.createManual('性能测量', {filter: {order: 'newest'}, confirm: true});
      g = (await galleries.loadGalleries()).find(x => x.name === '性能测量');
    }
    shows.chooseShow(g.id, show);
    return galleries.galleryHref(g.id, show);
  }

  // ---------- 场景的每一步 ----------
  let syncLoop = null;
  const ops = {
    // 启动后先查样式表都加载上了：宿主不发某个样式表（例如 0.4 之前的宿主不发 /css/，#683）时量出来的不是这个前端，直接报错。
    async boot() {
      await until(PAGES.booted, 60000, '应用启动'); watch();
      const links = [...document.querySelectorAll('link[rel=stylesheet]')], missing = links.filter(l => !l.sheet).map(l => l.getAttribute('href'));
      if (missing.length) throw new Error('样式表没有加载上：' + missing.join(', '));
      return {dpr: devicePixelRatio, width: innerWidth, height: innerHeight, ua: navigator.userAgent, sheets: links.length};
    },
    // 跳到一页，等它的第一批图显示出来，再等 settle 毫秒让后续的读取和解码落定。
    async go({page, settle = 1500}) {
      const p = PAGES[page], hash = p.prepare ? await p.prepare() : p.hash;
      if (location.hash !== hash) location.hash = hash;
      await until(p.ready, 60000, page + ' 就绪');
      await sleep(settle);
      return snapshot(page);
    },
    // 以 speed 像素/秒匀速往下滚 screens 屏（每帧按经过的时间推进 scrollTop），记录这一段。滚到底时等更多内容（首页的
    // 更早几天）最多 wait 毫秒，还没有就提前结束（reachedEnd）。页面重画把滚动位置往回拉时记一次 jumpsBack；整段最多用
    // 预计时间的三倍加 wait，到了就停（timedOut），免得被一直往回拉时停不下来。
    async scroll({page, screens = 8, speed = 2400, wait = 5000}) {
      const s = PAGES[page].scroller(), distance = screens * s.clientHeight, rec = begin(), stalled = stalls;
      const limit = performance.now() + distance / speed * 3000 + wait;
      let last = performance.now(), stuckSince = null, reachedEnd = false, timedOut = false, moved = 0, jumpsBack = 0, expected = s.scrollTop;
      while (moved < distance - 1) {
        const t = await frame(), before = s.scrollTop;
        if (before < expected - 2) jumpsBack++;
        const step = Math.min(distance - moved, speed * Math.min(100, t - last) / 1000);
        s.scrollTop = before + step;
        last = t; expected = s.scrollTop;
        if (s.scrollTop > before) { moved += s.scrollTop - before; stuckSince = null; }
        else { stuckSince ??= t; if (t - stuckSince > wait) { reachedEnd = true; break; } }
        if (t > limit) { timedOut = true; break; }
      }
      const out = end(rec);
      return {...out, reachedEnd, timedOut, jumpsBack, rafStalls: stalls - stalled, after: snapshot(page)};
    },
    // 不动，记录 ms 毫秒（空闲时或同步导入时盯着页面看）。
    async idle({page, ms = 5000}) { const rec = begin(); await sleep(ms); return {...end(rec), after: snapshot(page)}; },
    // 打开大图，然后每 every 毫秒翻一张，共 flips 张；记录整段，外加每次翻页到新图显示出来（load）的时间。
    async flip({flips = 30, every = 400}) {
      const p = PAGES.viewer;
      p.open();
      await until(p.opened, 10000, '大图打开');
      await until(() => loaded(p.image()), 30000, '大图显示');
      await sleep(800);
      const rec = begin(), waits = [];
      for (let i = 0; i < flips; i++) {
        const img = p.image(), was = img.src, t0 = performance.now();
        p.next();
        try { await until(() => img.src !== was && loaded(img), 10000, '下一张'); waits.push(performance.now() - t0); } catch { waits.push(null); }
        const left = every - (performance.now() - t0);
        if (left > 0) await sleep(left);
      }
      const shown = waits.filter(x => x != null);
      return {...end(rec), flip: {count: flips, missed: waits.length - shown.length, p50: round(quantile(shown, .5)), p95: round(quantile(shown, .95)),
        max: round(shown.length ? Math.max(...shown) : null)}, after: snapshot('viewer')};
    },
    // 同步导入：每 every 毫秒经页面自己的入口发一次同步（上一次没结束就不发）。来源文件夹里的新图由测量脚本另行放进去。
    async syncStart({every = 4000}) {
      const loop = {stop: false, started: 0, imported: 0};
      syncLoop = loop;
      (async () => {
        while (!loop.stop) {
          try {
            const before = loop.started ? await PAGES.imported() : 0;
            if (await PAGES.sync()) { loop.started++; loop.imported += before; }
          } catch { /* 下一次再试 */ }
          await sleep(every);
        }
      })();
      return {};
    },
    // 停下同步循环；imported 是各次同步新加入的张数之和（最后一次没落定的不算）。
    async syncStop() {
      const loop = syncLoop; syncLoop = null;
      if (!loop) return {started: 0, imported: 0};
      loop.stop = true;
      const {state} = await import('/core/state.js'), jobs = await import('/core/jobs.js');
      const last = jobs.SYNC_ACTIVE.includes(state.sync?.status) ? 0 : await PAGES.imported();
      return {started: loop.started, imported: loop.imported + last};
    },
    async snapshot({page}) { return snapshot(page); },
  };

  window.__perf = {
    async call(op, args = {}) {
      try { return {ok: true, value: await ops[op](args)}; } catch (e) { return {ok: false, error: String(e?.message || e)}; }
    },
  };
})();
