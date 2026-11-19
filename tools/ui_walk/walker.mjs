// 走查的步骤记录（#453）：每一步做一个动作，等页面静下来，取一份快照（焦点、打开的层、选中、光标、详情栏、大图、
// 地址、滚动、提示条……），可选截图；检查项（check）记下通过与否。一次运行的结果是一份 JSON，两次运行逐步比较
// （python3 -m tools.ui_walk compare）。快照里的图片一律写成合成图库里的文件名，不写 uuid：每次运行重新建库，uuid 会变。

import {writeFileSync} from 'node:fs';
import {join} from 'node:path';

// 在页面里执行的快照函数。只读 DOM 和 core/state.js；旧版本没有的模块或字段记为 null，不报错，
// 这样同一套走查可以在任意一份 web/static 上跑（#469 新旧对比）。
const SNAPSHOT = `(async () => {
  const S = (await import('/core/state.js')).state;
  let layers = null;
  try { const o = await import('/ui/overlay.js'); layers = o.layerIds ? o.layerIds() : null; } catch {}
  const names = window.__walkNames || {};
  const nameOf = uuid => uuid ? (names[uuid] || ('?' + String(uuid).slice(0, 6))) : null;
  const visible = el => !!el && !el.hidden && !el.closest('[hidden]') && el.getClientRects().length > 0
    && getComputedStyle(el).visibility !== 'hidden' && getComputedStyle(el).display !== 'none';
  const text = (el, n = 80) => el ? (el.innerText || el.textContent || '').replace(/\\s+/g, ' ').trim().slice(0, n) : null;
  const describe = el => {
    if (!el || el === document.body) return 'body';
    let s = el.tagName.toLowerCase();
    if (el.id) s += '#' + el.id;
    else if (el.classList.length) s += '.' + [...el.classList].slice(0, 2).join('.');
    const label = el.getAttribute('aria-label') || el.getAttribute('data-act') || el.getAttribute('name');
    if (label) s += '[' + label.slice(0, 40) + ']';
    const where = el.closest('dialog[open], #viewer, #inspector, #tour, #taskPanel, #rail, #toolbar, #selectionBar, #grid, #pageHead, [role=dialog], .popover');
    if (where && where !== el) s += ' in ' + (where.id ? '#' + where.id : where.tagName.toLowerCase() + (where.className ? '.' + String(where.className).split(' ')[0] : ''));
    const card = el.closest('.card[data-id]');
    if (card) s += ' card ' + nameOf(card.dataset.id);
    return s;
  };
  const OVERLAYS = ['viewer', 'inspector', 'taskPanel', 'viewPopover', 'tagPopover', 'morePopover', 'castPop', 'tour', 'nudge',
    'checklist', 'selectionBar', 'toast', 'splash', 'errorBanner'];
  const shown = OVERLAYS.filter(id => visible(document.getElementById(id)));
  const dialogs = [...document.querySelectorAll('dialog[open]')].map(d => d.id || d.getAttribute('aria-label') || text(d.querySelector('h2,h3,header'), 30) || 'dialog');
  const sel = S.sel || {ids: new Set(), exclude: new Set()};
  const viewport = document.getElementById('viewport');
  const search = document.getElementById('search');
  const toast = document.getElementById('toast');
  const picker = document.getElementById('picker');
  return {
    hash: decodeURIComponent(location.hash),
    heading: text(document.getElementById('heading'), 120),
    active: describe(document.activeElement),
    layers, overlays: shown, dialogs,
    // 命令面板：底栏那句话说明这次选了会做什么（「把这张从 A 换成 B」「给这张加上 B」）。
    picker: picker && picker.open ? {query: document.getElementById('pickerSearch')?.value ?? null,
      foot: text(document.getElementById('pickerFoot'), 120),
      rows: [...picker.querySelectorAll('#pickerResults [role=option]')].slice(0, 5).map(r => text(r, 40))} : null,
    toast: visible(toast) ? text(toast, 120) : null,
    selected: sel.all ? 'all-' + sel.exclude.size : [...sel.ids].map(nameOf).sort(),
    selecting: !!S.selecting,
    cursor: S.cursor >= 0 ? nameOf(S.items?.[S.cursor]?.uuid) ?? ('#' + S.cursor) : null,
    focused: nameOf(S.focused),
    detail: nameOf(S.detail?.uuid ?? S.detail?.asset?.uuid ?? null),
    viewer: S.viewer >= 0 ? nameOf(S.viewerItem?.uuid) ?? ('#' + S.viewer) : null,
    // 网格上画着高亮的卡片：同时有两张「当前」的样子就是指向不一致（#386 的 1-4）。
    marked: {focused: [...document.querySelectorAll('#grid .card.focused')].map(c => nameOf(c.dataset.id)),
      cursor: [...document.querySelectorAll('#grid .card.cursor')].map(c => nameOf(c.dataset.id))},
    total: S.total ?? null,
    items: (S.items || []).slice(0, 12).map(x => nameOf(x?.uuid)),
    search: search ? search.value : null,
    scroll: viewport ? Math.round(viewport.scrollTop) : null,
    count: text(document.getElementById('resultCount'), 40),
    inspectorName: visible(document.getElementById('inspector')) ? text(document.querySelector('#inspector .insp-title, #inspector h2, #inspector header'), 60) : null,
  };
})()`;

export class Walker {
  constructor(page, {flow, out, shots = true, lang}) {
    this.page = page; this.flow = flow; this.out = out; this.shots = shots; this.lang = lang;
    this.steps = []; this.checks = []; this.n = 0; this.last = null;
  }

  async snapshot() { return this.page.eval(SNAPSHOT); }

  // 做一步：action 是一个 async 函数；之后等页面静下来，记快照和这一步里冒出的控制台错误。
  async step(label, action, {shot = false, settle} = {}) {
    const index = ++this.n, begun = Date.now();
    let error = null;
    try { if (action) await action(this.page); } catch (e) { error = String(e.message || e); }
    await this.page.settle(settle);
    let snap;
    try { snap = await this.snapshot(); } catch (e) { snap = {error: String(e.message || e)}; }
    if (error) this.check('动作执行成功', false, error);
    if (snap.error) this.check('快照读取成功', false, snap.error);
    const console = this.page.drainConsole();
    const entry = {index, label, ms: Date.now() - begun, snap, console, ...(error ? {error} : {})};
    if (shot && this.shots) {
      const file = `${this.flow}-${String(index).padStart(2, '0')}.png`;
      try { writeFileSync(join(this.out, file), await this.page.screenshot()); entry.shot = file; } catch { /* 截不到就不截 */ }
    }
    this.steps.push(entry);
    this.last = snap;
    return snap;
  }

  // 一条检查：ok 为真算通过；detail 写看到了什么，便于在报告里直接读出是哪里不对。
  check(name, ok, detail = '') {
    this.checks.push({name, ok: !!ok, step: this.n, detail: typeof detail === 'string' ? detail : JSON.stringify(detail)});
    return !!ok;
  }

  result(extra = {}) {
    const pending = this.page.drainConsole();
    if (pending.length) this.steps.push({index: ++this.n, label: '流程收尾', snap: {}, console: pending});
    return {flow: this.flow, lang: this.lang, steps: this.steps, checks: this.checks, ...extra};
  }
}
