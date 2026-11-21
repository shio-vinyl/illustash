// openDialog（ui/dialog.js，#319）：页面所有运行时模态对话框的唯一生成函数。关闭途径（×／[data-close]、Esc、点背景），
// 点背景默认不关（只读或确认类对话框显式声明，#343），关闭时先 onClose 再移除，以及焦点钩子。
// Esc 和点背景经过浮层管理器（ui/overlay.js）。
const {test, before, afterEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let dialog, overlay;
before(async () => { await web.ready(); dialog = await web.load('ui/dialog.js'); overlay = await web.load('ui/overlay.js'); });
afterEach(() => { delete globalThis.document; });

function page() {
  const dialogs = [], focused = [];
  const field = {focus() { focused.push('field'); }};
  const handlers = {};
  globalThis.document = {body: {append(d) { dialogs.push(d); }}, addEventListener(type, fn) { (handlers[type] ||= []).push(fn); },
    dispatch(type, e) { for (const fn of handlers[type] || []) fn(e); }, createElement() {
    const nodes = {'#name': field};
    return {returnValue: '', attributes: {}, setAttribute(k, v) { this.attributes[k] = v; }, querySelector(selector) { return nodes[selector] ||= {}; },
      showModal() { this.open = true; }, addEventListener(type, fn) { if (type === 'close') this.afterClose = fn; else (this.on ||= {})[type] = fn; },
      close(value = '') { this.returnValue = value; this.open = false; this.onclose(); this.afterClose?.(); },
      remove() { this.removed = true; }, getBoundingClientRect: () => ({left: 10, right: 100, top: 10, bottom: 100})};
  }};
  overlay.resetLayers(); overlay.wireOverlays();
  return {dialogs, focused};
}
const inside = {clientX: 50, clientY: 50}, outside = {clientX: 0, clientY: 0};

test('builds a titled modal with the shared class, aria-label and shell', () => {
  const {dialogs} = page(), d = dialog.openDialog({title: '标题', cls: 'work-dialog', body: '<p>正文</p>', actions: '<button class="primary">好</button>'});
  assert.equal(dialogs[0], d); assert.equal(d.open, true);
  assert.equal(d.className, 'modal glass-scrim work-dialog'); assert.equal(d.attributes['aria-label'], '标题');
  assert.equal(d.querySelector('h2').textContent, '标题');
  assert.match(d.innerHTML, /<form method="dialog">/); assert.match(d.innerHTML, /<p>正文<\/p>/);
  assert.match(d.innerHTML, /<div class="dialog-actions"><button class="primary">好<\/button><\/div>/);
  assert.match(d.innerHTML, /class="icon" data-close/);
});
test('without actions there is no action row', () => {
  page(); assert.doesNotMatch(dialog.openDialog({title: 'x'}).innerHTML, /dialog-actions/);
});
test('点背景只关声明过的对话框；点框内、点子元素、默认的表单都不关', () => {
  page();
  const click = e => document.dispatch('click', e);
  const d = dialog.openDialog({title: 'x', backdropClose: true});
  click({target: {closest: () => null}, ...outside}); assert.equal(d.open, true, '点子元素不算点背景');
  click({target: d, ...inside}); assert.equal(d.open, true, '点在框内');
  click({target: d, ...outside}); assert.equal(d.open, false); assert.equal(d.removed, true);
  const form = dialog.openDialog({title: 'x'});
  click({target: form, ...outside}); assert.equal(form.open, true, '默认不关，带输入的表单不会被误点关掉');
  const explicit = dialog.openDialog({title: 'x', backdropClose: false});
  click({target: explicit, ...outside}); assert.equal(explicit.open, true);
});
test('Esc 只关最上面的对话框，返回值与原生取消相同', () => {
  page();
  const esc = () => { const e = {key: 'Escape', target: {}, prevented: false, stopped: false, preventDefault() { this.prevented = true; }, stopImmediatePropagation() { this.stopped = true; }}; document.dispatch('keydown', e); return e; };
  const below = dialog.openDialog({title: 'a'}), top = dialog.openDialog({title: 'b'});
  const e = esc(); assert.equal(top.open, false); assert.equal(below.open, true); assert.equal(top.returnValue, ''); assert.equal(e.prevented && e.stopped, true);
  esc(); assert.equal(below.open, false);
  assert.equal(esc().stopped, false, '什么都没开时 Esc 留给页面');
});
test('any [data-close] inside closes it, also one drawn later', () => {
  page();
  const d = dialog.openDialog({title: 'x'});
  d.onclick({target: {closest: selector => (selector === '[data-close]' ? {} : null)}});
  assert.equal(d.open, false);
});
test('onClose sees the return value before the dialog is removed, and the dialog is removed even if it throws', () => {
  page();
  let seen;
  const d = dialog.openDialog({title: 'x', onClose: dlg => { seen = {value: dlg.returnValue, removed: dlg.removed}; }});
  d.close('ok'); assert.deepEqual(seen, {value: 'ok', removed: undefined}); assert.equal(d.removed, true);
  const bad = dialog.openDialog({title: 'x', onClose: () => { throw new Error('boom'); }});
  assert.throws(() => bad.close(), /boom/); assert.equal(bad.removed, true);
});
test('focus hands the focus to the named control', () => {
  const {focused} = page();
  dialog.openDialog({title: 'x'}); assert.deepEqual(focused, []);
  dialog.openDialog({title: 'x', focus: '#name'}); assert.deepEqual(focused, ['field']);
});
test('isBackdropClick is true only for the dialog itself outside its box', () => {
  const d = {getBoundingClientRect: () => ({left: 10, right: 100, top: 10, bottom: 100})};
  assert.equal(overlay.isBackdropClick(d, {target: d, ...outside}), true);
  assert.equal(overlay.isBackdropClick(d, {target: d, ...inside}), false);
  assert.equal(overlay.isBackdropClick(d, {target: {}, ...outside}), false);
});

test('表单提交播完退场再关，returnValue 是按下的按钮；调用方拦下的提交不关（#715）', () => {
  page();
  const d = dialog.openDialog({title: 'x'});
  const blocked = {defaultPrevented: true, submitter: {value: 'ok'}, preventDefault() {}};
  d.on.submit(blocked); assert.equal(d.open, true, '校验没过，调用方 preventDefault 了');
  let prevented = false;
  d.on.submit({defaultPrevented: false, submitter: {value: 'ok'}, preventDefault() { prevented = true; }});
  assert.equal(prevented, true); assert.equal(d.open, false); assert.equal(d.returnValue, 'ok');
});
test('closeModal：退场中再关一次不重复关；openStatic 打开静态对话框，Esc 同样走退场（#715）', async () => {
  page();
  const d = document.createElement('dialog'); d.onclose = () => {};
  let closes = 0; const close = d.close.bind(d); d.close = v => { closes++; close(v); };
  dialog.openStatic(d, {id: 'static'}); assert.equal(d.open, true); assert.deepEqual(overlay.layerIds(), ['static']);
  await Promise.all([dialog.closeModal(d, 'a'), dialog.closeModal(d, 'b')]);
  assert.equal(closes, 1); assert.equal(d.returnValue, 'a'); assert.deepEqual(overlay.layerIds(), []);
  dialog.openStatic(d, {id: 'static'});
  document.dispatch('keydown', {key: 'Escape', target: {}, preventDefault() {}, stopImmediatePropagation() {}});
  assert.equal(d.open, false); assert.equal(closes, 2); assert.deepEqual(overlay.layerIds(), []);
});
