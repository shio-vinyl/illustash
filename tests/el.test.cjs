// ui/el.js（#704）：静态模板解析一次、data-ref、data-t 翻译、值没变不动 DOM、用节点拼的译文。英文页面。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

web.language('en', {en: {'收藏': 'Favorite', '加入收藏': 'Add to favorites', '在 {where} 里打开': 'Open in {where}'}});
let el;
test.before(async () => { await web.ready(); dom.install(); el = await web.load('ui/el.js'); });
test.after(() => dom.uninstall());

test('模板解析一次，每次调用克隆一份，data-ref 按路径取到这一份里的节点', () => {
  const CARD = el.tpl`<div class="card"><img data-ref="img" alt=""><p><b data-ref="name"></b></p></div>`;
  const a = CARD(), b = CARD();
  assert.notEqual(a.el, b.el);
  assert.equal(a.el.className, 'card');
  assert.equal(a.refs.img.localName, 'img');
  assert.equal(a.refs.name.parentNode.parentNode, a.el);
  assert.notEqual(a.refs.name, b.refs.name);
  el.text(a.refs.name, 'Hatsune');
  assert.equal(b.refs.name.textContent, '');
});

test('data-t 和 data-t-attr 在解析时翻译', () => {
  const {el: node, refs} = el.tpl`<button title="加入收藏" data-t-attr="title"><span data-ref="label" data-t>收藏</span></button>`();
  assert.equal(node.getAttribute('title'), 'Add to favorites');
  assert.equal(refs.label.textContent, 'Favorite');
});

test('模板带插值、多个根元素或重复的 data-ref 都报错', () => {
  const name = 'x';
  assert.throws(() => el.tpl`<b>${name}</b>`, /不带插值/);
  assert.throws(() => el.tpl`<b></b><i></i>`(), /一个根元素/);
  assert.throws(() => el.tpl`<p><b data-ref="a"></b><i data-ref="a"></i></p>`(), /重复/);
});

test('写入函数：值没变不动 DOM', () => {
  const {refs} = el.tpl`<div><span data-ref="s"></span></div>`(), s = refs.s;
  el.text(s, 'a'); el.attr(s, 'title', 'x'); el.cls(s, 'on', true); el.css(s, '--tint', '#123'); el.show(s, true);
  dom.resetWrites();
  el.text(s, 'a'); el.attr(s, 'title', 'x'); el.cls(s, 'on', true); el.css(s, '--tint', '#123'); el.show(s, true);
  assert.equal(dom.writes(), 0);
  el.text(s, 'b'); el.cls(s, 'on', false); el.show(s, false); el.css(s, '--tint', null);
  assert.equal(s.textContent, 'b');
  assert.equal(s.classList.contains('on'), false);
  assert.equal(s.hidden, true);
  assert.equal(s.style.getPropertyValue('--tint'), '');
  el.attr(s, 'title', null); el.attr(s, 'disabled', true);
  assert.equal(s.hasAttribute('title'), false);
  assert.equal(s.getAttribute('disabled'), '');
  el.text(s, null);
  assert.equal(s.textContent, '');
});

test('数据只进 textContent：带尖括号的文字不会变成节点', () => {
  const {refs} = el.tpl`<div><span data-ref="s"></span></div>`();
  el.text(refs.s, '<img src=x onerror=alert(1)>');
  assert.equal(refs.s.childNodes.length, 1);
  assert.equal(refs.s.firstChild.nodeType, 3);
});

test('prop 和节点上现在的值比；value 在焦点上时不覆盖', () => {
  const {refs} = el.tpl`<div><input type="checkbox" data-ref="box"><input data-ref="field"></div>`();
  el.prop(refs.box, 'checked', false);
  refs.box.checked = true;                 // 用户点了一下
  el.prop(refs.box, 'checked', false);     // 状态回来还是 false：要改回去
  assert.equal(refs.box.checked, false);
  el.value(refs.field, 'saved');
  refs.field.focus(); refs.field.value = 'typing';
  el.value(refs.field, 'saved');
  assert.equal(refs.field.value, 'typing');
  refs.field.blur();
  el.value(refs.field, 'saved');
  assert.equal(refs.field.value, 'saved');
});

test('rich 把译文里的占位符换成节点，其余是文字节点', () => {
  const link = el.make('a', 'set-link', 'Settings');
  const frag = el.rich('Open in {where}', {where: link});
  assert.deepEqual(frag.childNodes.map(n => n.nodeType === 3 ? n.data : n.localName), ['Open in ', 'a']);
  assert.equal(frag.childNodes[1], link);
  // 没给的占位符原样留成文字；给的是文字就是文字节点。
  const plain = el.rich('{n} of {missing}', {n: 3});
  assert.deepEqual(plain.childNodes.map(n => n.data), ['3', ' of ', '{missing}']);
  assert.equal(el.make('b').className, '');
});
