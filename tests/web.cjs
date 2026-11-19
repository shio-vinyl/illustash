// 在 Node 里加载页面的 ES 模块（web/static/core、web/static/ui；#199）。web/static/i18n.js 是页面上先于它们运行的
// 经典脚本，在这里也先作为本进程的脚本运行，与页面上一样：除非 ready() 之前用 language() 选了别的语言，否则是
// 中文页面；每个测试文件一种语言。fetch、document、localStorage 之类由测试自己提供：调用前设在 globalThis 上。
const path = require('node:path');
const vm = require('node:vm');
const {readFileSync} = require('node:fs');
const {pathToFileURL} = require('node:url');
const STATIC = path.join(__dirname, '../web/static');
const load = name => import(pathToFileURL(path.join(STATIC, name)).href);
let page = {code: 'zh-CN', tables: {}}, loaded;
// 用这种语言和这些表（{code: table}；默认取 locales/en.json 的英文）的页面。
function language(code, tables = {en: JSON.parse(readFileSync(path.join(STATIC, 'locales/en.json'), 'utf8'))}) {
  if (loaded) throw new Error('language() comes before ready()');
  page = {code, tables};
}
// 运行 i18n.js 并加载 core/，每个测试文件一次：传给 node:test 的 before()。
async function ready() {
  const names = Object.fromEntries(Object.entries(page.tables).map(([c, table]) => [c, table['@name'] || c]));
  globalThis.self = {ILLUSTASH_LANGUAGE: page.code, ILLUSTASH_LANGUAGES: names, ILLUSTASH_TABLES: page.tables};
  globalThis.document = {documentElement: {}, querySelectorAll: () => []};
  vm.runInThisContext(readFileSync(path.join(STATIC, 'i18n.js'), 'utf8'), {filename: path.join(STATIC, 'i18n.js')});
  loaded = Object.assign({}, ...await Promise.all(['i18n', 'format', 'prefs', 'session', 'edits', 'state', 'labels', 'jobs', 'settings', 'meta', 'route'].map(name => load(`core/${name}.js`))));
}
// core/ 导出的内容，会话是新的，没有可撤销的操作。core/state.js 的其余部分由测试自己设置。
function core() {
  if (!loaded) throw new Error('call before(web.ready) first');
  Object.assign(loaded.session, {token: '', stale: false});
  Object.assign(loaded.edits, {latest: null, busy: false});
  return loaded;
}

// 用普通对象拼成的页面，够桌面布局的模块往里画（#199）：getElementById() 在第一次用到时建出元素；元素保存
// children、style、dataset、classes 和 listeners，innerHTML 按文本保存。
// 返回 {document, el(id)}；同时设置 globalThis.document。
class FakeElement {
  constructor(tag = 'div', id = '') {
    Object.assign(this, {tagName: tag.toUpperCase(), id, hidden: false, textContent: '', innerHTML: '', value: '', title: '', className: '',
      style: fakeStyle(), dataset: {}, children: [], parent: null, listeners: {}, attributes: {}, scrollTop: 0, clientWidth: 1000, clientHeight: 800});
    const classes = new Set();
    this.classList = {add: (...c) => c.forEach(x => classes.add(x)), remove: (...c) => c.forEach(x => classes.delete(x)),
      toggle: (c, on = !classes.has(c)) => (on ? classes.add(c) : classes.delete(c), on), contains: c => classes.has(c)};
  }
  get firstElementChild() { return this.children[0] || null; }
  get nextElementSibling() { return this.parent?.children[this.parent.children.indexOf(this) + 1] || null; }
  append(...els) { for (const el of els) if (typeof el === 'object') this.insertBefore(el, null); }
  replaceChildren(...els) { for (const el of [...this.children]) el.remove(); this.append(...els); }
  get previousElementSibling() { return this.parent?.children[this.parent.children.indexOf(this) - 1] || null; }
  insertBefore(el, before) {
    el.remove?.(); const i = before ? this.children.indexOf(before) : this.children.length;
    this.children.splice(i < 0 ? this.children.length : i, 0, el); el.parent = this; return el;
  }
  insertAdjacentHTML(where, html) { this.innerHTML += html; }
  remove() { if (this.parent) { const a = this.parent.children; a.splice(a.indexOf(this), 1); this.parent = null; } }
  replaceWith(el) { const p = this.parent; if (p) { p.insertBefore(el, this); this.remove(); } }
  addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }
  removeEventListener() {}
  setAttribute(k, v) { this.attributes[k] = String(v); }
  getAttribute(k) { return this.attributes[k] ?? null; }
  hasAttribute(k) { return k in this.attributes; }
  removeAttribute(k) { delete this.attributes[k]; }
  querySelector() { return new FakeElement(); }
  querySelectorAll() { return []; }
  closest() { return null; }
  contains(el) { return el === this; }
  matches() { return false; }
  focus() {} blur() {} scrollIntoView() {} showModal() { this.open = true; } close() { this.open = false; } click() {}
  getBoundingClientRect() { return {left: 0, top: 0, right: this.clientWidth, bottom: this.clientHeight, width: this.clientWidth, height: this.clientHeight}; }
}
// style 上的 setProperty 一类方法不可枚举：测试用 deepEqual 比较 style 时只比写进去的值。
function fakeStyle() {
  const style = {};
  Object.defineProperties(style, {
    setProperty: {value: (k, v) => { style[k] = String(v); }}, removeProperty: {value: k => { delete style[k]; }},
    getPropertyValue: {value: k => style[k] ?? ''},
  });
  return style;
}
// ui/el.js 的静态模板（tpl`…`）要真的解析：交给 tests/dom.cjs 的假 DOM，解析出来的节点照样能放进这里的元素。
let parser = null;
function templateDocument() {
  if (parser) return parser;
  const saved = {document: globalThis.document, Node: globalThis.Node, HTMLElement: globalThis.HTMLElement};
  parser = require('./dom.cjs').install();
  Object.assign(globalThis, saved);
  return parser;
}
function fakePage() {
  const nodes = new Map();
  const document = {
    getElementById: id => { if (!nodes.has(id)) nodes.set(id, new FakeElement('div', id)); return nodes.get(id); },
    // <template>：ui/el.js 读 content.childNodes（真的解析），ui/dom.js 的 reconcileSections 读 content.firstElementChild.html（原文）。
    createElement: tag => tag === 'template' ? {set innerHTML(html) {
      const real = templateDocument().createElement('template');
      try { real.innerHTML = html; } catch {}
      this.content = {childNodes: real.content?.childNodes || [], firstElementChild: Object.assign(new FakeElement(), {html})};
    }} : new FakeElement(tag),
    importNode: (node, deep) => templateDocument().importNode(node, deep),
    createDocumentFragment: () => templateDocument().createDocumentFragment(),
    createTextNode: text => templateDocument().createTextNode(text),
    querySelector: () => null, querySelectorAll: () => [], addEventListener() {}, activeElement: null,
    body: new FakeElement('body'), documentElement: new FakeElement('html'),
  };
  globalThis.document = document;
  return {document, el: id => document.getElementById(id)};
}
// ui/empty.js 画进 box 的空状态：{fact, next: [{label, href} | {label, act}]}；没有空状态时是 null。
function emptyState(box) {
  const kids = n => n.childNodes || n.children || [];
  const find = (n, name) => (n.className || '').split(' ').includes(name) ? n : kids(n).map(c => find(c, name)).find(Boolean);
  const root = box.children?.[0]; if (!root) return null;
  const next = kids(find(root, 'empty-next') || {}).map(b => b.href ? {label: b.textContent, href: b.href} : {label: b.textContent, act: b.dataset.empty});
  return {fact: find(root, 'empty-fact')?.textContent, next};
}
// 页头（ui/shell.js renderHeading）画出来的文字：新写法（head）是节点，旧写法（headingHTML）是 innerHTML 文本。
function headingText(el) {
  const kids = el.children || [];
  return kids.length ? kids.map(c => c.textContent ?? '').join(' ') : String(el.innerHTML || '').replace(/<[^>]+>/g, '');
}
module.exports = {load, language, ready, core, fakePage, FakeElement, emptyState, headingText};
