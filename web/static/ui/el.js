// 模板与节点（0.4 渲染底座，#704）：建节点、往节点里写数据的唯一入口。规则见 docs/WEB_V2.md「渲染底座」。
//
// 1. 模板是不带插值的静态 HTML，用 tpl`…` 声明在模块顶层。第一次用到时解析一次（按模板字符串缓存），之后每次只克隆：
//      const CARD = tpl`<div class="card"><img data-ref="img" alt=""><b data-ref="name"></b>
//        <button data-ref="fav" title="收藏" data-t-attr="title"><span data-t>收藏</span></button></div>`;
//      const {el, refs} = CARD();          // refs.img、refs.name、refs.fav 是这一份克隆里的节点
//    - 模板里写 `${…}` 会直接抛错：数据从不经过 HTML 解析，转义也就不靠调用处记得（tools/check_frontend.py 规则 10）。
//    - data-ref="名" 的节点放进 refs（名字不能重复）；模板只能有一个根元素。
//    - data-t 的文字、data-t-attr 列出的特性按 web/static/i18n.js 的写法在解析时翻译一次（语言在页面载入时定，改了会重新
//      载入，所以缓存的模板不用重译）。带占位符的文字（「{n} 张」）不写在模板里，用 text(node, t('{n} 张', {n})) 写。
// 2. 数据只用下面这些函数写：text、attr、prop、value、cls、show、css。它们记着上一次写进去的值，值没变就不动 DOM，
//    所以 update() 里可以每次把全部字段都写一遍，不用自己比较。只经这些函数写同一个节点的同一项；绕开它们直接改了 DOM，
//    缓存就不准了，下一次同值的写入会被跳过。
// 3. 带链接、加粗的译文用 rich() 拼节点：rich(t('到 {where} 里打开'), {where: link})，不用 HTML 字符串。
//    小的动态节点（列表里的一枚芯片）用 make('b', 'chip', 文字)。
// 4. 部件的约定是 {el, update(props), dispose()}：el 建一次；update 只经第 2 条的函数改变了的地方；dispose 释放监听、
//    计时器和子部件（交给 ui/scope.js 的作用域登记，dispose 里调 scope.dispose()）。列表用 ui/keyed.js，长列表用
//    ui/virtual.js，它们都按这个约定调用部件。
import {t} from '../core/i18n.js';

const parsed = new WeakMap();   // 模板字符串数组 -> {root, paths}
const LAST = Symbol('el.last');   // 节点上记着的上一次写入：{键: 值}

// 声明一个模板（标签函数）；返回的函数每调用一次克隆一份，得到 {el, refs}。
export function tpl(strings, ...values) {
  if (values.length) throw new Error('ui/el.js: 模板不带插值；数据用 text() / attr() 写进 data-ref 的节点');
  return () => {
    let entry = parsed.get(strings);
    if (!entry) { entry = parse(strings.join('')); parsed.set(strings, entry); }
    const el = document.importNode(entry.root, true), refs = {};   // 模板里的节点属于惰性文档，importNode 直接拷进本页
    for (const [name, path] of entry.paths) { let n = el; for (const k of path) n = n.childNodes[k]; refs[name] = n; }
    return {el, refs};
  };
}

function parse(source) {
  const holder = document.createElement('template');
  holder.innerHTML = source.trim();
  const roots = [...holder.content.childNodes].filter(n => n.nodeType === 1 || n.textContent.trim());
  if (roots.length !== 1 || roots[0].nodeType !== 1) throw new Error('ui/el.js: 模板要有且只有一个根元素');
  const root = roots[0], paths = new Map();
  // 记下每个 data-ref 从根走下去的子节点序号，克隆后按路径取，不再查选择器。
  const walk = (node, path) => {
    if (node.nodeType !== 1) return;
    translate(node);
    const name = node.getAttribute('data-ref');
    if (name) {
      if (paths.has(name)) throw new Error(`ui/el.js: data-ref="${name}" 重复`);
      paths.set(name, path);
    }
    node.childNodes.forEach((child, k) => walk(child, [...path, k]));
  };
  walk(root, []);
  return {root, paths};
}

// 与 i18n.js 的 translateMarkup 相同的约定：data-t 翻译它的文字，data-t-attr="title aria-label" 翻译列出的特性。
// 这些键由 tests/i18n.test.cjs 从各模块的 tpl`…` 模板里收集（和 index.html 的 data-t 一样），所以这里按变量翻译。
const markup = t;
function translate(node) {
  if (node.hasAttribute('data-t')) node.textContent = markup(node.textContent);
  const names = node.getAttribute('data-t-attr');
  if (names) for (const name of names.split(' ')) if (node.hasAttribute(name)) node.setAttribute(name, markup(node.getAttribute(name)));
}

// 上一次经这里写进 node 的 key 是不是 value；不是就记下新值，返回 true（要写）。
function changed(node, key, value) {
  const last = node[LAST] || (node[LAST] = {});
  if (key in last && Object.is(last[key], value)) return false;
  last[key] = value;
  return true;
}

// 文字。null / undefined 写成空串。
export function text(node, value) {
  const s = value == null ? '' : String(value);
  if (changed(node, 'text', s)) node.textContent = s;
}
// 特性。null / undefined / false 去掉这个特性，true 写成空值（hidden、disabled 这类），其余转成字符串。
export function attr(node, name, value) {
  const v = value == null || value === false ? null : value === true ? '' : String(value);
  if (!changed(node, '@' + name, v)) return;
  if (v === null) node.removeAttribute(name); else node.setAttribute(name, v);
}
// 属性（checked、disabled 之类按属性写的）。和节点上现在的值比，不和缓存比：checked 这类用户点一下就会变。
// 不要用它写 scrollTop、offsetWidth 这类读一下就要排版的属性（那些放进 ui/frame.js 的 write）。
export function prop(node, name, value) {
  if (node[name] !== value) node[name] = value;
}
// 输入框、下拉框的值：焦点在它上面时不覆盖（用户正在改；轮询刷新不冲掉打了一半的字），离开焦点后下一次 update 再写。
export function value(node, v) {
  const s = v == null ? '' : String(v);
  if (document.activeElement === node) return;
  if (node.value !== s) node.value = s;
}
// 类。
export function cls(node, name, on) {
  on = Boolean(on);
  if (changed(node, 'class:' + name, on)) node.classList.toggle(name, on);
}
// 显隐（hidden 特性）。
export function show(node, on) { attr(node, 'hidden', !on); }
// 样式属性，包括自定义属性（css(el, '--tint', color)、css(el, 'transform', `translate(${x}px, ${y}px)`)）。null 去掉。
export function css(node, name, v) {
  const s = v == null ? null : String(v);
  if (!changed(node, 'css:' + name, s)) return;
  if (s === null) node.style.removeProperty(name); else node.style.setProperty(name, s);
}

// 把一句已经翻译好的文字拼成节点：{名} 换成 parts[名]（节点，或文字），其余是文字节点。返回 DocumentFragment。
//   rich(t('在 {settings} 里打开'), {settings: make('a', 'set-link', t('设置'))})
export function rich(sentence, parts = {}) {
  const out = document.createDocumentFragment();
  String(sentence).split(/(\{\w+\})/).forEach((piece, k) => {
    if (!piece) return;
    const name = k % 2 ? piece.slice(1, -1) : null, part = name && Object.hasOwn(parts, name) ? parts[name] : piece;
    out.append(typeof part === 'object' && part ? part : document.createTextNode(String(part)));
  });
  return out;
}

// 一个小节点：make('b', 'chip', 文字)。className、文字都可以不给。
export function make(tag, className = '', content = '') {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== '') node.textContent = content;
  return node;
}
