// 标志上的图库切换浮层（ui/library_pop.js，#706）：打开时先按已有的列表画，/api/libraries 读回来再画一次。各行按路径留着、
// 只增删和更新文字，有焦点的那一行不会被删掉重建（虚拟机走查：重建后焦点掉到 body 上）。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const dom = require('./dom.cjs');
const web = require('./web.cjs');
let pop, state, document, libraries;
const tick = () => new Promise(r => setTimeout(r, 0));
const $ = id => document.getElementById(id);
const rows = () => $('libraryPop').querySelectorAll('button').filter(x => x.classList.contains('library-row'));
const lib = (path, name = path.slice(1)) => ({path, name});

before(async () => {
  await web.ready();
  document = dom.install();
  document.body.innerHTML = '<button id="brand"></button><div id="libraryPop" hidden></div>';
  document.getElementById = id => { const walk = n => { for (const c of n.children || []) { if (c.id === id) return c; const f = walk(c); if (f) return f; } return null; }; return walk(document.body); };
  // 浮层管理器找第一个可聚焦的节点用的复杂选择器和 closest，假 DOM 不认：在这个容器上换成按钮和链接（都看得见）
  const box = document.getElementById('libraryPop'), all = box.querySelectorAll.bind(box);
  const visible = x => Object.assign(x, {closest: () => null});
  box.querySelectorAll = sel => sel.includes(':not(') ? [...all('button'), ...all('a')].map(visible) : all(sel);
  globalThis.location = {hash: ''};
  globalThis.fetch = async () => ({status: 200, ok: true, headers: {get: () => null}, json: async () => libraries});
  pop = await web.load('ui/library_pop.js');
  ({state} = web.core());
  pop.wireLibraryPop();
});

test('读回列表后各行不重建：有焦点的那一行还在、焦点不丢；去掉的图库删掉、新的加在后面', async () => {
  state.libraries = {current: '/a', info: {name: 'A'}, libraries: [lib('/a'), lib('/b'), lib('/c')]};
  libraries = {current: '/a', info: {name: 'A'}, libraries: [lib('/a'), lib('/b', 'B2'), lib('/c'), lib('/d')]};
  $('brand').dispatchEvent({type: 'click'});
  const first = rows()[0];
  assert.equal(document.activeElement, first);   // 打开时焦点进第一项
  for (let i = 0; i < 4; i++) await tick();
  assert.equal(rows()[0], first);
  assert.equal(document.activeElement, first);
  assert.deepEqual(rows().map(r => r.getAttribute('title')), ['/b', '/c', '/d']);
  assert.equal(first.querySelector('b').textContent, 'B2');
  // 再开一次，这回 /b 不在了
  $('brand').dispatchEvent({type: 'click'});   // 关
  libraries = {current: '/a', info: {name: 'A'}, libraries: [lib('/a'), lib('/d'), lib('/c')]};
  $('brand').dispatchEvent({type: 'click'});   // 开
  for (let i = 0; i < 4; i++) await tick();
  assert.deepEqual(rows().map(r => r.getAttribute('title')), ['/d', '/c']);
});
