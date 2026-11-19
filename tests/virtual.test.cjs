// ui/virtual.js（#704）：只挂视口上下约一屏；节点按键复用；重新布局以视口顶部那张为锚；滚远的图片节点摘掉、去掉 src。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

let V, frame, document, frames;
const flush = () => { while (frames.length) frames.shift()(); };
test.before(async () => {
  await web.ready(); document = dom.install();
  frame = await web.load('ui/frame.js'); V = await web.load('ui/virtual.js');
});
test.after(() => dom.uninstall());
test.beforeEach(() => { frames = []; frame.setScheduler(fn => frames.push(fn)); });

// 视口 500 高；content 在滚动内容里从 100px 开始（上面是页头）。
function page() {
  const viewport = document.createElement('div'), content = document.createElement('div');
  viewport.appendChild(content); viewport.clientHeight = 500; viewport.clientWidth = 400;
  content.getBoundingClientRect = () => ({top: 100 - viewport.scrollTop, left: 0});
  return {viewport, content};
}
// 一列 100 张，每张 100 高。
const column = (n = 100, h = 100) => Array.from({length: n}, (_, i) => ({key: 'k' + i, x: 0, y: i * h, w: 100, h}));

function cards() {
  const made = [], disposed = [];
  const create = box => {
    const el = document.createElement('div'), img = document.createElement('img');
    img.setAttribute('src', `/t/${box.key}.jpg`); img.setAttribute('srcset', `/t/${box.key}@2x.jpg 2x`); el.appendChild(img);
    made.push(box.key);
    return {el, img, update: next => { el.dataset.y = next.y; }, dispose: () => disposed.push(box.key)};
  };
  return {create, made, disposed};
}

test('只挂视口上下各一屏里的框，定位只写 transform 和宽高', () => {
  const {viewport, content} = page(), c = cards();
  const list = V.virtual(viewport, content, {create: c.create});
  list.set(column(), 10000); flush();
  assert.equal(content.style.height, '10000px');
  // 视口顶部在 content 里是 -100：范围 [-600, 900)，即第 0–8 张。
  assert.deepEqual(c.made, ['k0', 'k1', 'k2', 'k3', 'k4', 'k5', 'k6', 'k7', 'k8']);
  assert.equal(list.mounted, 9);
  const el = list.comp('k3').el;
  assert.equal(el.style.transform, 'translate(0px, 300px)');
  assert.deepEqual([el.style.position, el.style.width, el.style.height], ['absolute', '100px', '100px']);
  list.dispose();
});

test('滚远了才摘：超出两屏的节点摘掉，摘之前去掉图片的 src 和 srcset', () => {
  const {viewport, content} = page(), c = cards();
  const list = V.virtual(viewport, content, {create: c.create});
  list.set(column(), 10000); flush();
  const first = list.comp('k0');
  viewport.scrollTop = 300; viewport.dispatchEvent(dom.event('scroll')); flush();
  assert.ok(list.comp('k0'), 'within two screens: kept');
  viewport.scrollTop = 3000; viewport.dispatchEvent(dom.event('scroll')); flush();
  assert.equal(list.comp('k0'), null);
  assert.equal(first.el.parentNode, null);
  assert.equal(first.img.hasAttribute('src'), false);
  assert.equal(first.img.hasAttribute('srcset'), false);
  assert.ok(c.disposed.includes('k0'));
  // 视口顶部 2900：想要 [2400, 3900)，即第 24–38 张；第 19–23、39–43 张还在保留范围里。
  for (let i = 24; i <= 38; i++) assert.ok(list.comp('k' + i), 'k' + i);
  assert.equal(list.comp('k60'), null);
  list.dispose();
  assert.equal(content.childNodes.length, 0);
});

test('一帧里滚好几次只量一次、挂一次', () => {
  const {viewport, content} = page(), c = cards();
  const list = V.virtual(viewport, content, {create: c.create});
  list.set(column(), 10000); flush();
  for (const y of [500, 900, 1300]) { viewport.scrollTop = y; viewport.dispatchEvent(dom.event('scroll')); }
  assert.equal(frames.length, 1);
  flush();
  list.dispose();
});

test('重新布局：同一个键复用同一个部件，视口顶部那张保持在原处', () => {
  const {viewport, content} = page(), c = cards();
  const list = V.virtual(viewport, content, {create: c.create});
  list.set(column(), 10000); flush();
  viewport.scrollTop = 3050; viewport.dispatchEvent(dom.event('scroll')); flush();   // 视口顶部在 k29 往下 50px
  const anchor = list.comp('k29');
  assert.ok(list.comp('k39'));
  list.set(column(100, 200), 20000); flush();
  // k29 的新位置是 5800：滚到 5800 + 50 + 页头 100。
  assert.equal(viewport.scrollTop, 5950);
  assert.equal(list.comp('k29'), anchor);
  assert.equal(anchor.el.dataset.y, '5800');
  assert.equal(anchor.el.style.transform, 'translate(0px, 5800px)');
  // k39 的新位置 7800 超出了两屏，摘掉。
  assert.equal(list.comp('k39'), null);
  // 锚的键不在新布局里：滚动位置不动。
  list.set(column(10), 1000); flush();
  assert.equal(viewport.scrollTop, 5950);
  list.dispose();
});

test('框不按 y 排序（瀑布）也能取范围', () => {
  const {viewport, content} = page(), c = cards();
  const boxes = [{key: 'low', x: 0, y: 5000, w: 50, h: 100}, {key: 'top', x: 50, y: 0, w: 50, h: 100}, {key: 'tall', x: 100, y: 0, w: 50, h: 4000}];
  const list = V.virtual(viewport, content, {create: c.create});
  list.set(boxes, 5100); flush();
  assert.deepEqual(c.made.sort(), ['tall', 'top']);
  list.dispose();
});

test('挂在作用域下：父作用域释放时一起释放', async () => {
  const S = await web.load('ui/scope.js'), parent = S.scope('page'), {viewport, content} = page(), c = cards();
  const list = V.virtual(viewport, content, {create: c.create, scope: parent});
  list.set(column(5), 500); flush();
  parent.dispose();
  assert.equal(content.childNodes.length, 0);
  assert.equal(viewport.listenerCount('scroll'), 0);
});

test('部件根节点就是 <img>、或者用 <picture> 时，摘掉前同样去掉地址', () => {
  const {viewport, content} = page(), roots = {};
  const list = V.virtual(viewport, content, {create: box => {
    if (box.key === 'k0') { const img = document.createElement('img'); img.setAttribute('src', '/a.jpg'); img.setAttribute('srcset', '/a@2x.jpg 2x'); roots.img = img; return {el: img}; }
    const pic = document.createElement('picture'), source = document.createElement('source'), img = document.createElement('img');
    source.setAttribute('srcset', '/b.avif'); img.setAttribute('src', '/b.jpg'); pic.append(source, img); roots.source = source; return {el: pic};
  }});
  list.set([{key: 'k0', x: 0, y: 0, w: 100, h: 100}, {key: 'k1', x: 0, y: 100, w: 100, h: 100}], 200); flush();
  list.dispose();
  assert.equal(roots.img.hasAttribute('src'), false);
  assert.equal(roots.img.hasAttribute('srcset'), false);
  assert.equal(roots.source.hasAttribute('srcset'), false);
});

test('DOM 次序按框的先后：从下往上滚回来时新挂的节点插在前面', () => {
  const {viewport, content} = page(), c = cards();
  const list = V.virtual(viewport, content, {create: c.create});
  list.set(column(), 10000); flush();
  viewport.scrollTop = 3000; viewport.dispatchEvent(dom.event('scroll')); flush();
  viewport.scrollTop = 1500; viewport.dispatchEvent(dom.event('scroll')); flush();
  const order = content.childNodes.map(el => Number(el.querySelector('img').getAttribute('src').match(/k(\d+)/)[1]));
  assert.deepEqual(order, [...order].sort((a, b) => a - b));
  list.dispose();
});

test('焦点在里面的节点滚远了先不摘，焦点离开后再摘；键从数据里没了照摘', () => {
  const {viewport, content} = page(), c = cards();
  document.body.appendChild(viewport);
  const list = V.virtual(viewport, content, {create: box => { const comp = c.create(box), b = document.createElement('button'); comp.el.appendChild(b); comp.button = b; return comp; }});
  list.set(column(), 10000); flush();
  const first = list.comp('k0');
  first.button.focus();
  viewport.scrollTop = 5000; viewport.dispatchEvent(dom.event('scroll')); flush();
  assert.equal(list.comp('k0'), first, 'kept while focused');
  assert.equal(document.activeElement, first.button);
  first.button.blur();
  viewport.scrollTop = 5100; viewport.dispatchEvent(dom.event('scroll')); flush();
  assert.equal(list.comp('k0'), null);
  const again = list.comp('k55'); again.el.querySelector('button').focus();
  list.set(column(10), 1000); flush();
  assert.equal(list.comp('k55'), null, 'its key is gone: removed anyway');
  assert.equal(document.activeElement, document.body);
  list.dispose(); viewport.remove();
});

test('content 是 static 时第一次量完给它写上 position: relative', () => {
  const {viewport, content} = page(), saved = globalThis.getComputedStyle;
  globalThis.getComputedStyle = el => ({position: el.style.position || 'static'});
  try {
    const list = V.virtual(viewport, content, {create: cards().create});
    list.set(column(3), 300); flush();
    assert.equal(content.style.position, 'relative');
    list.dispose();
    const other = page(); other.content.style.position = 'absolute';
    const kept = V.virtual(other.viewport, other.content, {create: cards().create});
    kept.set(column(3), 300); flush();
    assert.equal(other.content.style.position, 'absolute');
    kept.dispose();
  } finally { globalThis.getComputedStyle = saved; }
});
