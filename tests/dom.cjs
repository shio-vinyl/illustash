// 渲染底座（#704）测试用的最小假 DOM：tests/web.cjs 的 fakePage() 只记 innerHTML 文本，测不了 ui/el.js 解析模板、
// ui/keyed.js 调换节点次序这类事，所以这里另写一份真正有树的。只实现底座用到的部分：元素和文字节点、特性、
// classList、style（含 setProperty）、childNodes 的增删挪、cloneNode、<template> 的 innerHTML 解析（不带插值的静态
// 模板：标签、带引号的特性、文字、自闭合和空元素、几个常用实体）、事件的登记与派发（不冒泡），以及记录写入次数
// （writes），测「值没变不动 DOM」。
//
// 焦点：focus() / blur() 改 document.activeElement；摘掉或挪动含焦点的节点时焦点回到 body（和浏览器一样）。
//
// 没有模拟的（真浏览器里有，测试过了不代表真浏览器也过）：排版和尺寸（clientWidth / clientHeight / scrollTop 是测试自己
// 设的普通属性，scrollTop 不按内容高度截断，getBoundingClientRect 只回测试设的 rect）；样式计算（没有 getComputedStyle）；
// 事件冒泡、捕获和默认动作（dispatchEvent 只调这个节点上的监听）；<select>.value 不随选项变、checked / value 只是普通
// 属性；inert、hidden 只记特性，不挡焦点和点击；HTML 解析只认不带插值的静态模板，不处理注释、<script> 和容错。
//
// 用法：const dom = require('./dom.cjs'); const document = dom.install(); … dom.uninstall();
// install() 把 document、Node、HTMLElement 设到 globalThis 上，返回 document。
const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']);
const ENTITIES = {amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' '};
const decode = s => s.replace(/&(#\d+|#x[0-9a-f]+|\w+);/gi, (all, e) => e[0] === '#'
  ? String.fromCodePoint(e[1] === 'x' || e[1] === 'X' ? parseInt(e.slice(2), 16) : Number(e.slice(1))) : ENTITIES[e] ?? all);

let writes = 0;
class Node {
  constructor(type) { this.nodeType = type; this.parentNode = null; this.childNodes = []; this.ownerDocument = current; }
  get firstChild() { return this.childNodes[0] || null; }
  get lastChild() { return this.childNodes[this.childNodes.length - 1] || null; }
  get nextSibling() { const p = this.parentNode; return p ? p.childNodes[p.childNodes.indexOf(this) + 1] || null : null; }
  get previousSibling() { const p = this.parentNode; return p ? p.childNodes[p.childNodes.indexOf(this) - 1] || null : null; }
  get parentElement() { return this.parentNode?.nodeType === 1 ? this.parentNode : null; }
  get isConnected() { let n = this; while (n.parentNode) n = n.parentNode; return n === current?.documentElement; }
  insertBefore(node, before) {
    if (node.nodeType === 11) { for (const child of [...node.childNodes]) this.insertBefore(child, before); return node; }
    if (before && before.parentNode !== this) throw new Error('insertBefore: reference is not a child');
    node.remove(); writes++;
    const at = before ? this.childNodes.indexOf(before) : this.childNodes.length;
    this.childNodes.splice(at, 0, node); node.parentNode = this; return node;
  }
  appendChild(node) { return this.insertBefore(node, null); }
  append(...nodes) { for (const n of nodes) this.appendChild(typeof n === 'string' ? current.createTextNode(n) : n); }
  prepend(...nodes) { const first = this.firstChild; for (const n of nodes) this.insertBefore(typeof n === 'string' ? current.createTextNode(n) : n, first); }
  replaceChildren(...nodes) { for (const c of [...this.childNodes]) c.remove(); this.append(...nodes); }
  removeChild(node) { node.remove(); return node; }
  // 和浏览器一样：摘掉或挪动（insertBefore 先摘再插）含焦点的节点时，焦点回到 body。
  remove() {
    const p = this.parentNode; if (!p) return; writes++;
    if (current && this.contains(current.activeElement) && current.activeElement !== current.body) current.activeElement = current.body;
    p.childNodes.splice(p.childNodes.indexOf(this), 1); this.parentNode = null;
  }
  replaceChildren(...nodes) { for (const c of [...this.childNodes]) c.remove(); this.append(...nodes); }
  replaceWith(node) { const p = this.parentNode; if (p) { p.insertBefore(node, this); this.remove(); } }
  contains(node) { for (let n = node; n; n = n.parentNode) if (n === this) return true; return false; }
  get textContent() { return this.nodeType === 3 ? this.data : this.childNodes.map(c => c.textContent).join(''); }
  set textContent(value) {
    writes++;
    if (this.nodeType === 3) { this.data = String(value); return; }
    for (const c of this.childNodes) c.parentNode = null;
    this.childNodes = [];
    if (value !== '' && value != null) { const text = current.createTextNode(String(value)); this.childNodes.push(text); text.parentNode = this; }
  }
  addEventListener(type, fn) { ((this.listeners ||= {})[type] ||= new Set()).add(fn); }
  removeEventListener(type, fn) { this.listeners?.[type]?.delete(fn); }
  dispatchEvent(event) { event.target ??= this; for (const fn of [...(this.listeners?.[event.type] || [])]) fn.call(this, event); return true; }
  listenerCount(type) { return this.listeners?.[type]?.size || 0; }
}
class Text extends Node {
  constructor(data) { super(3); this.data = data; }
  cloneNode() { return new Text(this.data); }
}
class Fragment extends Node {
  constructor() { super(11); }
  get children() { return this.childNodes.filter(c => c.nodeType === 1); }
  get firstElementChild() { return this.children[0] || null; }
}
class Element extends Node {
  constructor(tag) {
    super(1); this.tagName = tag.toUpperCase(); this.localName = tag.toLowerCase(); this.attrs = new Map();
    const classes = () => new Set((this.getAttribute('class') || '').split(/\s+/).filter(Boolean));
    const write = set => this.setAttribute('class', [...set].join(' '));
    this.classList = {
      contains: c => classes().has(c),
      add: (...c) => { const s = classes(); c.forEach(x => s.add(x)); write(s); },
      remove: (...c) => { const s = classes(); c.forEach(x => s.delete(x)); write(s); },
      toggle: (c, on) => { const s = classes(); on ??= !s.has(c); if (on) s.add(c); else s.delete(c); write(s); return on; },
    };
    const props = {};
    this.style = new Proxy(props, {
      get: (o, k) => k === 'setProperty' ? (n, v) => { writes++; o[n] = String(v); } : k === 'getPropertyValue' ? n => o[n] ?? ''
        : k === 'removeProperty' ? n => { writes++; delete o[n]; } : o[k] ?? '',
      set: (o, k, v) => { writes++; o[k] = String(v); return true; },
    });
    this.dataset = new Proxy({}, {
      get: (o, k) => this.getAttribute('data-' + String(k).replace(/[A-Z]/g, c => '-' + c.toLowerCase())) ?? undefined,
      set: (o, k, v) => { this.setAttribute('data-' + String(k).replace(/[A-Z]/g, c => '-' + c.toLowerCase()), v); return true; },
    });
    this.scrollTop = 0; this.clientWidth = 0; this.clientHeight = 0; this.rect = null;
  }
  get children() { return this.childNodes.filter(c => c.nodeType === 1); }
  get firstElementChild() { return this.children[0] || null; }
  get nextElementSibling() { let n = this.nextSibling; while (n && n.nodeType !== 1) n = n.nextSibling; return n; }
  get className() { return this.getAttribute('class') || ''; }
  set className(v) { this.setAttribute('class', v); }
  get id() { return this.getAttribute('id') || ''; }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(v) { if (v) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  get inert() { return this.hasAttribute('inert'); }
  set inert(v) { if (v) this.setAttribute('inert', ''); else this.removeAttribute('inert'); }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  setAttribute(k, v) { writes++; this.attrs.set(k, String(v)); }
  hasAttribute(k) { return this.attrs.has(k); }
  removeAttribute(k) { if (this.attrs.has(k)) writes++; this.attrs.delete(k); }
  getAttributeNames() { return [...this.attrs.keys()]; }
  cloneNode(deep) {
    const el = current.createElement(this.localName);
    for (const [k, v] of this.attrs) el.attrs.set(k, v);
    if (deep) for (const c of this.childNodes) { const copy = c.cloneNode(true); el.childNodes.push(copy); copy.parentNode = el; }
    return el;
  }
  querySelectorAll(selector) {
    const m = /^\[([\w-]+)\]$/.exec(selector) || /^([a-z]+)$/.exec(selector);
    if (!m) throw new Error('fake querySelectorAll supports [attr] and tag only: ' + selector);
    const out = [], test = selector[0] === '[' ? el => el.hasAttribute(m[1]) : el => el.localName === m[1];
    const walk = n => { for (const c of n.children) { if (test(c)) out.push(c); walk(c); } };
    walk(this); return out;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  getBoundingClientRect() { return this.rect || {left: 0, top: 0, width: this.clientWidth, height: this.clientHeight, right: this.clientWidth, bottom: this.clientHeight}; }
  focus() { current.activeElement = this; }
  blur() { if (current.activeElement === this) current.activeElement = current.body; }
  get innerHTML() { return this.childNodes.map(serialize).join(''); }
  set innerHTML(html) {
    writes++;
    const target = this.localName === 'template' ? (this.content = new Fragment()) : this;
    if (target === this) { for (const c of this.childNodes) c.parentNode = null; this.childNodes = []; }
    parseInto(target, html);
  }
}
function serialize(n) {
  if (n.nodeType === 3) return n.data.replace(/&/g, '&amp;').replace(/</g, '&lt;');
  const attrs = [...n.attrs].map(([k, v]) => v === '' ? ` ${k}` : ` ${k}="${v.replace(/"/g, '&quot;')}"`).join('');
  return `<${n.localName}${attrs}>` + (VOID.has(n.localName) ? '' : n.childNodes.map(serialize).join('') + `</${n.localName}>`);
}
function parseInto(root, html) {
  const stack = [root], re = /<\/([a-zA-Z][\w-]*)\s*>|<([a-zA-Z][\w-]*)((?:\s+[^\s=>/]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*(\/?)>|([^<]+)/g;
  let m;
  while ((m = re.exec(html))) {
    const top = stack[stack.length - 1];
    if (m[1]) { if (stack.length > 1) stack.pop(); continue; }
    if (m[5] !== undefined) { const text = new Text(decode(m[5])); top.childNodes.push(text); text.parentNode = top; continue; }
    const el = current.createElement(m[2]);
    for (const a of m[3].matchAll(/([^\s=>/]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)) el.attrs.set(a[1], decode(a[2] ?? a[3] ?? a[4] ?? ''));
    top.childNodes.push(el); el.parentNode = top;
    if (!m[4] && !VOID.has(el.localName)) stack.push(el);
  }
}

let current = null;
class Document extends Node {
  constructor() {
    super(9); current = this;
    this.documentElement = new Element('html'); this.body = new Element('body');
    this.documentElement.appendChild(this.body); this.activeElement = this.body;
  }
  createElement(tag) { return new Element(tag); }
  createTextNode(data) { return new Text(String(data)); }
  createDocumentFragment() { return new Fragment(); }
  importNode(node, deep) { return node.cloneNode(deep); }
}
let saved = null;
function install() {
  saved = {document: globalThis.document, Node: globalThis.Node, HTMLElement: globalThis.HTMLElement};
  const document = new Document();
  Object.assign(globalThis, {document, Node: {ELEMENT_NODE: 1, TEXT_NODE: 3}, HTMLElement: Element});
  writes = 0;
  return document;
}
function uninstall() { if (saved) Object.assign(globalThis, saved); saved = null; current = null; }
const event = (type, extra = {}) => ({type, ...extra, preventDefault() {}, stopPropagation() {}});
module.exports = {install, uninstall, event, writes: () => writes, resetWrites: () => { writes = 0; }};
