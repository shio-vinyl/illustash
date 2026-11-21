// 运行：node --test tests/viewer_slots.test.cjs
// 看图器的三张轮换（ui/viewer.js，#709）：翻几页以后只有当前那张槽有原图，翻走的槽放掉原图；关掉以后三张槽都没有原图。
// 守住 #683 量到的显存问题（三张槽都留着原图时 Chromium GPU 进程多出约 150 MiB）。用 tests/dom.cjs 的真树，帧任务手动推。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

test('翻几页以后只有当前槽有原图，关掉以后都没有', async () => {
  await web.ready();
  const document = dom.install();
  try {
    for (const id of ['viewer', 'viewerStage', 'viewerBar', 'viewerCount', 'viewerSize', 'viewerClose', 'viewerPrev', 'viewerNext']) {
      const el = document.createElement('div'); el.id = id; document.body.append(el);
    }
    document.getElementById('viewer').hidden = true;
    const frame = await web.load('ui/frame.js');
    let frames = [];
    frame.setScheduler(fn => frames.push(fn));
    const flush = () => { while (frames.length) { const now = frames; frames = []; for (const f of now) f(); } };
    globalThis.fetch = () => new Promise(() => {});   // 底栏的详情不读回：这里只看槽
    const viewer = await web.load('ui/viewer.js'), state = web.core().state;
    state.items = Array.from({length: 6}, (_, i) => ({uuid: `u${i}`, width: 800, height: 1200}));
    state.total = 6;
    viewer.bindViewer({loadPage: async () => {}, PAGE: 100, setCursor() {}, previewList() {}, endPreview: () => false, previewing: () => false,
      kept: {}, ensureVisible() {}, inspect() {}, step() {}, markDetailKeys() {}, askEnabled: () => false, act() {}});
    viewer.wireViewer();
    const slots = () => document.getElementById('viewerStage').children.filter(x => x.classList.contains('vw-slot'));
    const high = slot => slot.querySelector('img').nextSibling;   // .vw-low 后面就是 .vw-high
    const fulls = () => slots().filter(x => high(x).getAttribute('src'));
    assert.equal(slots().length, 3);
    for (const i of [0, 1, 2, 3]) { viewer.showViewer(i); await Promise.resolve(); flush(); }
    assert.equal(fulls().length, 1, '只有当前那张有原图');
    assert.ok(fulls()[0].classList.contains('is-current'));
    assert.match(high(fulls()[0]).getAttribute('src'), /id=u3&size=full/);
    viewer.closeViewer('silent');
    flush();
    assert.equal(fulls().length, 0, '关掉以后都放掉原图');
  } finally { dom.uninstall(); }
});
