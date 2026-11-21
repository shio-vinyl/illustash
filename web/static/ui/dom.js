// 桌面布局里覆盖整个页面的部件（#199）：按 id 找元素、带撤销按钮的 toast、错误横幅和工具栏里的撤销按钮。它们的状态在
// core/session.js 和 core/edits.js。
import {t, tb} from '../core/i18n.js';
import {esc, isMac, platformText, shortcut} from '../core/format.js';
import {prefs} from '../core/prefs.js';
import {session, STALE, onStale, readError} from '../core/session.js';
import {onLatest, setLatest} from '../core/edits.js';
import {workLabel} from '../core/labels.js';
import {AREAS, pageOf} from '../core/pages.js';
import {state} from '../core/state.js';
import {createLayer} from './overlay.js';
import {popOpen, showPop} from './pop.js';
export const $ = id => document.getElementById(id);
// 主区切换：显示 area（core/pages.js 登记的主区，默认是当前页的），隐藏其余几个。网格的空状态跟着网格：换到别的主区时
// 收起，回到网格时由 reload 按读到的结果显示。
export function showArea(area = pageOf(state.route).area) {
  for (const a of AREAS) $(a).hidden = a !== area;
  if (area !== 'grid') $('empty').hidden = true;
}
// 名称联网查询还没授权时，搜索结果的说明后面接的「允许查询…」。点击由 ui/settings.js 的 [data-names-allow] 处理。
export const namesAllowButton = `<button type="button" class="link-button" data-names-allow>${t('允许查询…')}</button>`;
// 网格空了时空状态下面的一个去处（core/pages.js 的 emptyNext）。
export const emptyLink = (href, label) => `<a class="quiet link" href="${href}">${label}</a>`;
// ---------- 页面说明「?」（#527） ----------
// 页面标题旁的「?」：这一页是什么、怎么操作，点开（键盘聚焦后 Enter／Space）才显示，标题下面只留数量、状态和
// 必须一眼看到的警告。说明是非模态浮层，Esc、点外关闭由浮层管理器处理，关了焦点回到按钮。标题重画时元素换掉，
// 层按 id 找元素。lines 是几行说明（已转义的 HTML），一行一句。返回 [按钮, 浮层]：按钮放进 h1，浮层放在
// .heading-main 里（标题可能截断溢出，浮层不能放在 h1 里面）。
const helpLayer = createLayer({kind: 'popover', id: 'page-help', el: () => $('pageHelpPop'), anchor: () => $('pageHelp'), focusIn: 'self',
  onDismiss: reason => showHelp(false, reason)});
function showHelp(open, reason) {
  showPop($('pageHelpPop'), open);
  $('pageHelp')?.setAttribute('aria-expanded', open); $('pageHelp')?.classList.toggle('on', open);
  if (open) helpLayer.open(); else helpLayer.close(reason);
}
export function helpTip(lines) {
  if (!lines?.length) return ['', ''];
  const label = t('这一页的说明');
  return [`<button class="icon small" id="pageHelp" title="${label}" aria-label="${label}" aria-expanded="false" aria-controls="pageHelpPop">?</button>`,
    `<div id="pageHelpPop" class="menu glass menu-drop" role="dialog" aria-label="${label}" tabindex="-1" hidden>${lines.map(x => `<p>${x}</p>`).join('')}</div>`];
}
// 标题画好之后调一次：给「?」挂上开关。
export function wireHelpTip() { const b = $('pageHelp'); if (b) b.onclick = () => showHelp(!popOpen($('pageHelpPop'))); }
// 帮助文字和提示用 ⌘ 写；其他系统显示 Ctrl。
export function platformHints(root, mac = isMac) {
  if (mac) return;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  while (walker.nextNode()) walker.currentNode.nodeValue = platformText(walker.currentNode.nodeValue, mac);
  for (const el of root.querySelectorAll('[title]')) el.title = platformText(el.title, mac);
}
let toastTimer;
// 提示条靠 translateX(-50%) 居中，退场只淡出（#715）。
const hideToast = () => showPop($('toast'), false, {to: 'fade'});
export function toast(message, withUndo = false) {
  $('toastText').textContent = message; $('toastUndo').hidden = !withUndo; $('toastAct').hidden = true; showPop($('toast'), true);
  clearTimeout(toastTimer); toastTimer = setTimeout(hideToast, withUndo ? 6000 : 3500);
}
// 给正在显示的 toast 追加一点：再补几个字，可选再加一个按钮（如对刚设好的角色所属作品「关注」，#293）。toast 随后会多
// 停留一会儿。
// 一次可撤销的写入成功之后（#432 ③-7）：撤销指向这一步，toast 写明做了什么并带「撤销」。text 默认是后端给这一步的名字。
export function written(undo, text = tb(undo.label)) { setLatest(undo); toast(text, true); }
export function toastMore(text, label = '', run = null) {
  if (!popOpen($('toast'))) return;
  $('toastText').textContent += text;
  const b = $('toastAct'); b.hidden = !label; b.textContent = label; b.onclick = () => { hideToast(); run(); };
  clearTimeout(toastTimer); toastTimer = setTimeout(hideToast, 9000);
}
// 指针停在 toast 上时它不消失；它的撤销按钮运行 undo()。
export function wireToast(undo) {
  $('toast').addEventListener('mouseenter', () => clearTimeout(toastTimer));
  $('toast').addEventListener('mouseleave', () => { toastTimer = setTimeout(hideToast, 2500); });
  $('toastUndo').onclick = () => undo();
}
// 角色名字后面用灰色小字写的作品（core/labels.js workLabel）。
export const workSmall = c => { const w = workLabel(c); return w ? `<small class="char-work">${esc(w)}</small>` : ''; };
// 角色选项里的名字（#294、#678）：名字后面跟作品小字。「跳到角色」等命令面板（ui/palette.js）和筛选条「＋ 条件」
// （ui/filters.js）都用它，以后新加的角色选择器也用它，不各自拼。work 默认按 workLabel 算；Danbooru 上的候选还不在
// 图库里，由调用处给它在 D 站的作品名。
export const charOption = (c, work = workLabel(c)) => esc(c.name) + (work ? `<small class="char-work">${esc(work)}</small>` : '');
export function error(message) { if (session.stale) return; $('errorBanner').textContent = message; $('errorBanner').hidden = !message; }
export function readFailure(node, e) { if (node) { node.hidden = false; node.innerHTML = `<p class="empty" role="alert">${esc(readError(e))}</p>`; } }
// 可折叠的块，展开或收起的状态在重新渲染和重新载入后保留。
export const fold = (key, summary, body) => `<details class="more" data-fold-key="${key}"${prefs.get('settingsOpen', {})[key] ? ' open' : ''}><summary>${summary}</summary><div class="more-body">${body}</div></details>`;
export function wireFolds(root) {
  for (const d of root.querySelectorAll('details[data-fold-key]')) {
    if (d.dataset.foldWired) continue;
    d.dataset.foldWired = '1';
    d.ontoggle = () => { const o = prefs.get('settingsOpen', {}); if (d.open) o[d.dataset.foldKey] = true; else delete o[d.dataset.foldKey]; prefs.set('settingsOpen', o); };
  }
}
// 内容变了才重画（#429）：记下每个元素上次由这里写进去的 HTML，相同就不动 DOM。轮询期间按钮不会在按下的那一刻被
// 换掉，展开的折叠块和滚动位置也不丢。keepInput：焦点在元素里的输入框上时也不重画，免得抹掉正在填的内容。
// 返回是否重画了。按钮的忙碌状态不归这里管，用下面的 busy()。
const drawn = new WeakMap();
export function redraw(el, html, {keepInput = false} = {}) {
  if (drawn.get(el) === html) return false;
  const active = document.activeElement;
  if (keepInput && active && el.contains(active) && active.matches?.('input,textarea,select')) return false;
  el.innerHTML = html; drawn.set(el, html);
  return true;
}
// 元素被别处整个改写过：传 html 时记下它现在的内容，不传时忘掉，下一次 redraw 一定重画。
export function setDrawn(el, html = null) { if (html === null) drawn.delete(el); else drawn.set(el, html); }
// 上次记下的 HTML；还没画过是 undefined。要在重画前后做别的事（如换场动画）时用。
export const drawnHTML = el => drawn.get(el);
// 按钮忙碌（#429）：等待期间禁用，结束后一定恢复，不管是成功、取消（关掉系统选择框或确认框）还是失败。不能靠重画来
// 恢复：内容没变时 redraw 不会重画，按钮就一直是灰的。等待中按钮被重画换掉的，恢复的是已经不在页面上的旧按钮。
export async function busy(buttons, run) {
  const list = [buttons].flat();
  for (const b of list) b.disabled = true;
  try { return await run(); } finally { for (const b of list) b.disabled = false; }
}
// 按钮的数据（[{act, label, primary, disabled, title}]，ui/rows.js 的 buttons 也是这个形状）拼成 HTML，给还在拼 HTML 的
// 任务面板和向导横条用；attr 是放动作名的属性（data-models、data-pack…）。不能点时 title 写原因。
export const buttonsHTML = (list, attr) => list.map(b => `<button class="${b.primary ? 'primary' : 'quiet'} small" ${attr}="${esc(b.act)}"`
  + `${b.disabled ? ` disabled${b.title ? ` title="${esc(b.title)}"` : ''}` : ''}>${esc(b.label)}</button>`).join('');
// 只替换 HTML 变了的部分，没变的封面不会重新加载。
export function reconcileSections(container, sections, empty = t('没有匹配的角色')) {
  if (!sections.length) { container.innerHTML = `<p class="empty">${empty}</p>`; return; }
  const old = new Map([...container.children].map(el => [el._sectionKey, el]));
  let cursor = container.firstElementChild;
  const keep = new Set();
  for (const [key, html] of sections) {
    let el = old.get(key);
    if (!el || el._sectionHTML !== html) {
      const previous = el;
      const template = document.createElement('template'); template.innerHTML = html;
      el = template.content.firstElementChild; el._sectionKey = key; el._sectionHTML = html;
      if (previous) { previous.replaceWith(el); if (cursor === previous) cursor = el; }
    }
    if (el !== cursor) container.insertBefore(el, cursor);
    else cursor = cursor.nextElementSibling;
    keep.add(el);
  }
  for (const el of [...container.children]) if (!keep.has(el)) el.remove();
}
// 搜索框里的放大镜。
export const searchIcon = () => '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><circle cx="10" cy="10" r="6.5"/><path d="m15 15 5 5"/></svg>';
// 页面停下时的横幅，以及工具栏的撤销按钮。页面启动时调用一次。
export function wireDom() {
  // App 无法解码的图片（损坏的文件）：用一个朴素的占位，而不是浏览器的破图图标。
  // 看图器和详情卡片的大图带 id，各自写「读不出来」（ui/viewer.js、ui/detail_card.js）。
  // <img> 不换掉，只藏起来、在它后面放占位：部件里的图是建一次、之后只换 src 的（详情卡片的头像、按天视图的图），
  // 换掉节点的话部件手里拿的就是摘下来的那个。同一个 <img> 之后读好了别的图（load），占位撤掉。
  document.addEventListener('error', e => {
    const img = e.target;
    if (!(img instanceof HTMLImageElement) || img.id || !img.getAttribute('src')?.startsWith('/image?')) return;
    if (img.classList.contains('is-broken')) return;
    const ph = document.createElement('span'); ph.className = 'img-broken'; ph.textContent = t('无法打开');
    img.classList.add('is-broken'); img.after(ph);
  }, true);
  document.addEventListener('load', e => {
    const img = e.target;
    if (!(img instanceof HTMLImageElement) || !img.classList.contains('is-broken')) return;
    img.classList.remove('is-broken');
    if (img.nextElementSibling?.classList.contains('img-broken')) img.nextElementSibling.remove();
  }, true);
  // 停下的页面在错误横幅里说明，并带一个重新载入的按钮。
  onStale(() => {
    const banner = $('errorBanner'), reload = document.createElement('button');
    banner.textContent = t('{reason}。这个页面已停止读写，未保存的输入不会提交。', {reason: STALE});
    reload.type = 'button'; reload.textContent = t('重新载入'); reload.onclick = () => location.reload();
    banner.append(reload); banner.hidden = false;
  });
  onLatest(({latest, busy}) => {
    $('undoButton').disabled = !latest || busy;
    $('undoButton').title = latest ? t('撤销：{label}（{key}）', {label: tb(latest.label), key: shortcut('Z')}) : t('暂无可撤销操作');
  });
}
