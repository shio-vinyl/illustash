// 设置的几种行与配置表（0.4 渲染底座，#704；设置页由 #714 用它重写，几种行来自 #676）。
//
// 一张配置表声明一组行，rows() 画一次，之后 update(state) 按状态只改变了的地方。轮询刷新时整组重新 update 也不会
// 重建节点，所以输入框的焦点、打了一半的字、下拉框展开的状态都不丢（焦点在输入框上时它的值不被覆盖，见 ui/el.js 的 value）。
//
//   const panel = rows(container, [
//     {kind: 'switch', key: 'updates', label: t('自动检查更新'), hint: t('启动时和之后每天查一次'),
//      get: s => s.autoUpdate, set: (on, s) => saveUpdates(on), disabled: s => s.busy},
//     {kind: 'action', key: 'check', label: t('检查新角色'), button: t('检查…'), run: s => startCheck()},
//     {kind: 'info', key: 'version', label: t('版本'), value: s => s.version},
//     {kind: 'select', key: 'lang', label: t('界面语言'), options: s => s.languages, get: s => s.lang, set: (v, s) => …},
//     {kind: 'field', key: 'mirror', label: t('镜像地址'), placeholder: 'https://…', get: s => s.mirror, set: (v, s) => …},
//     {kind: 'list', key: 'libs', label: t('图库'), items: s => s.libraries, itemKey: x => x.id, itemText: x => x.name,
//      itemHint: x => x.path, actions: [{label: t('移除'), run: (x, s) => …, hidden: (x, s) => x.current}]},
//   ], {scope: pageScope});
//   panel.update(state);   // 建好后立刻 update 一次（在那之前点控件，set / run 收到的 state 是 null），之后每次状态变了
//   panel.dispose();
//
// 几种行：
//   switch  开关行：标题 + 说明，右边开关。改动时调 set(checked, state)。
//   action  操作行：标题 + 说明，右边一个按钮（button 是按钮上的字），点了调 run(state)。
//   info    信息行：标题 + 说明，右边是值（value(state) 的文字）。
//   select  选择行：右边下拉框，options 是 [{value, label}] 或者 state => […]；改动时调 set(value, state)。
//   field   输入行：右边输入框（type 默认 text），离开输入框或回车（change 事件）时调 set(value, state)。
//   list    列表行：标题下面一项一行（按 itemKey 复用），每项右边可以有几个按钮（actions），点了调 run(item, state)。
// 每行都可以带 hidden(state)、disabled(state)（list 的按钮各自带）；hidden 为真时这一行别的值都不取。配置表在建任何节点之前整张校验，写错 kind 时
// 直接抛错，容器里不会留下半张表。
//
// 设置页要的几个选项（#714，不加新的 kind）：
//   select 的 seg: true   画成分段控件（css/base.css 的 .seg，选中的一段 aria-pressed），两三个选项的用它。
//   action 的 buttons     state => [{act, label, primary, disabled, title}]：一行几个按钮（按 act 复用节点），点了调
//                         run(state, act)。run 返回 promise 时，等它落定之前这个按钮不能再点。ui/models.js 的
//                         modelsButtons、ui/packs.js 的 packButtons 就是这个形状。
//   dot                   state => '' | 'busy' | 'warn' | 'off' | null：标题前的状态圆点（null 不画）。
//   progress              state => {done, total} | null：说明下面的进度条，随 update 走（焦点在别处的输入框上也动）。
//   href                  文字或 state => 地址：标题是链接（链到资料库、别的设置节）。info 行带 href 时整行是链接，右边的
//                         value 写「3 部 ›」这类；只有链接的行用 info，action 必须有 button 或 buttons（配置校验拦住）。
//   link                  state => {href, label} | null：说明后面的一个小链接（「手动下载说明」），在新窗口打开。
//   warn                  state => 文字：说明下面一行醒目的小字（「这次启动已用命令行参数关闭」）。
//   value（action 也有）   右边按钮之前的一段文字（「3 部 ›」）。
//   field 的 min / max / step / unit / wide / plain   数字框的范围和后面的单位（「张」），wide 是长输入框（地址），plain
//                         关掉拼写检查和自动填充；placeholder 也可以是 state => 文字；enter(state) 是在框里按回车时做的事
//                         （先把框里的值交给 set，输入法组字时不算）。
//   list 的 itemHref      item => 地址 | null：这一项的文字是链接；itemTitle 是这一项的悬停提示（完整路径、SHA-256）；
//                         actions 的 label、title 可以是 (item, state) => 文字，primary 是主按钮。
// action 带 buttons 时，行级的 disabled 不起作用，每个按钮自己带 disabled。
//
// 改了还没存好的值（#469）：开关、下拉框、输入框改动时，这一行记下「待提交的值」，再调 set。set 返回 promise 时，
// 等它落定再清掉；不返回 promise 时当作已经同步存好，立刻清掉。待提交的值还在、而 state 里的值还是旧的，update 不覆盖
// 控件（离开输入框以后、新状态回来之前来一次轮询，打的字不会被冲回旧值）；state 里的值等于它时提前清掉。清掉的同时
// 按最新的 state 重画这一行：保存失败、确认框取消时控件立刻回到原值，各节的 set 不用自己重画。
// 焦点在输入框、下拉框上时 update 也不覆盖它们的值（ui/el.js 的 value），下拉框的选项在焦点上时也不换，免得展开着的
// 列表被换掉。这一条的前提是点开下拉框时它拿到焦点：Chromium 是这样；macOS 的 Safari 点按钮、复选框不给焦点，点
// <select> 给焦点（Safari 17.0 也是）。
// label、hint、value 可以写成文字或函数（state => 文字，每次 update 取值，用来带上数字）；button、placeholder、选项和
// 按钮的 label 是文字。文字都由调用方用 t('…') 翻译好再传进来（键要写成字面量，tests/i18n.test.cjs 才收集得到）。
//
// 行的样式在 css/settings.css（.row 一组）。
import {composing} from '../core/keys.js';
import {attr, cls, css, make, prop, show, text, tpl, value} from './el.js';
import {keyed} from './keyed.js';
import {scope} from './scope.js';

// 每种行左边都是同一段文字区：圆点、标题、说明、小链接、醒目的小字、进度条。只写一份（TEXT），各行种的外壳里留一个
// <span data-ref="text">，建行时换成它，两边的 refs 合在一起。
const TEXT = tpl`<span class="row-text"><span class="row-title"><span class="row-dot" data-ref="dot" hidden></span><b data-ref="label"></b></span>
  <small class="row-hint" data-ref="hint"></small><a class="row-link" data-ref="link" target="_blank" rel="noreferrer noopener" hidden></a>
  <em class="row-warn" data-ref="warn" hidden></em><span class="row-bar" data-ref="bar" hidden><span data-ref="fill"></span></span></span>`;
const SWITCH = tpl`<label class="row"><span data-ref="text"></span><input type="checkbox" class="switch" data-ref="input"></label>`;
const ACTION = tpl`<div class="row"><span data-ref="text"></span>
  <span class="row-ctrl" data-ref="ctrl"><span class="row-value" data-ref="value" hidden></span><button type="button" class="btn is-small" data-ref="input"></button></span></div>`;
const INFO = tpl`<div class="row"><span data-ref="text"></span><span class="row-value" data-ref="value"></span></div>`;
const SELECT = tpl`<label class="row"><span data-ref="text"></span><select class="row-select" data-ref="input"></select></label>`;
const SEG = tpl`<div class="row"><span data-ref="text"></span><span class="seg" role="group" data-ref="input"></span></div>`;
const FIELD = tpl`<label class="row"><span data-ref="text"></span>
  <span class="row-ctrl"><input class="row-input" data-ref="input"><span class="row-unit" data-ref="unit" hidden></span></span></label>`;
const LIST = tpl`<div class="row-list"><div class="row is-head" data-ref="head"><span data-ref="text"></span></div>
  <div class="row-items" data-ref="items"></div></div>`;
// 外壳加上文字区。
const shell = make_ => () => {
  const {el, refs} = make_(), text = TEXT();
  refs.text.replaceWith(text.el);
  return {el, refs: {...refs, ...text.refs, text: text.el}};
};
const ITEM = tpl`<div class="row is-item"><span class="row-text"><a class="row-to" data-ref="to" hidden></a><span data-ref="text"></span><small class="row-hint" data-ref="hint"></small></span>
  <span class="row-ctrl" data-ref="actions"></span></div>`;
const KINDS = {switch: shell(SWITCH), action: shell(ACTION), info: shell(INFO), select: shell(SELECT), field: shell(FIELD), list: shell(LIST)};
const SEG_ROW = shell(SEG);

// 文字原样用，函数每次按状态取值。
const read = (v, s) => typeof v === 'function' ? v(s) : v;
const button = (label, primary) => {
  const b = make('button', 'btn is-small' + (primary ? ' is-primary' : ''), label);
  b.type = 'button';
  return b;
};
// 点了以后 run 返回 promise 的，等它落定之前不能再点（结束后一定恢复，见 ui/dom.js 的 busy）。
function press(b, run) {
  if (b.dataset.busy) return;
  const result = run();
  if (!result || typeof result.then !== 'function') return;
  b.dataset.busy = '1'; b.disabled = true;
  const done = () => { delete b.dataset.busy; b.disabled = Boolean(b.dataset.off); };
  result.then(done, done);
}
const setOff = (b, off) => { if (off) b.dataset.off = '1'; else delete b.dataset.off; prop(b, 'disabled', off || Boolean(b.dataset.busy)); };

export function rows(container, table, {scope: parent = null} = {}) {
  for (const spec of table) {
    if (!KINDS[spec.kind]) throw new Error(`ui/rows.js: 没有 ${spec.kind} 这种行`);
    // 只有链接、没有按钮的行用 info（带 href），action 必须有按钮。
    if (spec.kind === 'action' && !spec.button && !spec.buttons) throw new Error(`ui/rows.js: action 行 ${spec.key} 既没有 button 也没有 buttons`);
  }
  const s = parent ? parent.child('rows') : scope('rows');
  let state = null;
  const built = table.map(spec => build(spec));

  function build(spec) {
    const make_ = spec.kind === 'select' && spec.seg ? SEG_ROW : KINDS[spec.kind];
    const {el, refs} = make_(), label = spec.label, hint = spec.hint ?? '';
    const row = {spec, el, refs, label, hint, list: null, options: '', pending: null, buttons: null, to: null};
    if (spec.href && spec.kind === 'info') {
      // 链接行：整行是一个 <a>（右边的值写「3 部 ›」这类），点哪里都跳。
      const a = make('a', 'row is-link');
      a.append(...el.childNodes);
      row.el = a; row.to = a;
    } else if (spec.href) {
      // 标题是链接：把 <b> 包进 <a>。
      const a = make('a', 'row-to');
      refs.label.replaceWith(a); a.append(refs.label);
      row.to = a;
    }
    row.el.dataset.row = spec.key;
    if (spec.kind === 'switch') s.on(refs.input, 'change', () => submit(row, refs.input.checked));
    if (spec.kind === 'action' && spec.buttons) {
      refs.input.remove(); refs.input = null;
      row.buttons = keyed(refs.ctrl, {key: b => b.act, create: b => actButton(spec, b), scope: s});
    } else if (spec.kind === 'action') {
      text(refs.input, spec.button);
      s.on(refs.input, 'click', () => press(refs.input, () => spec.run?.(state)));
    }
    if (spec.kind === 'select' && !spec.seg || spec.kind === 'field') s.on(refs.input, 'change', () => submit(row, refs.input.value));
    if (spec.kind === 'field') {
      attr(refs.input, 'type', spec.type || 'text');
      for (const k of ['min', 'max', 'step']) attr(refs.input, k, spec[k] ?? null);
      if (spec.wide) cls(refs.input, 'is-wide', true);
      if (spec.unit) { text(refs.unit, spec.unit); show(refs.unit, true); }
      // 回车：先把框里的值交给 set，再调 enter（名称翻译的三个框回车保存）；输入法组字时的回车不算。
      if (spec.enter) s.on(refs.input, 'keydown', e => {
        if (e.key !== 'Enter' || composing(e)) return;
        e.preventDefault?.();
        submit(row, refs.input.value);
        spec.enter(state);
      });
      if (spec.type === 'password' || spec.plain) { attr(refs.input, 'spellcheck', 'false'); attr(refs.input, 'autocomplete', 'off'); }
    }
    if (spec.kind === 'list') row.list = keyed(refs.items, {key: spec.itemKey, create: item => listItem(spec, item), scope: s});
    container.append(row.el);
    return row;
  }

  // 一行几个按钮里的一个：节点按 act 复用，字、主次、能不能点随 update 变。
  function actButton(spec, first) {
    const b = button(first.label, first.primary), own = s.child('row-button');
    let current = first;
    own.on(b, 'click', () => press(b, () => spec.run?.(state, current.act)));
    const update = x => {
      current = x;
      text(b, x.label);
      cls(b, 'is-primary', Boolean(x.primary));
      setOff(b, Boolean(x.disabled));
      attr(b, 'title', x.title || null);
    };
    update(first);
    return {el: b, update, dispose: () => own.dispose()};
  }

  // 记下待提交的值再调 set；set 的 promise 落定（或者不是 promise）时清掉，前提是这期间没有再改过。
  function submit(row, v) {
    const pending = {value: v};
    row.pending = pending;
    const result = row.spec.set?.(v, state);
    // 落定以后（保存成功、失败或取消）按最新的 state 重画这一行：失败或取消时控件回到原值，不用等别处来一次 update。
    // 这期间又改过（row.pending 换了）或者这张表已经释放，就不动。
    const clear = () => { if (row.pending !== pending) return; row.pending = null; if (s.alive && state) paint(row); };
    if (result && typeof result.then === 'function') result.then(clear, clear); else clear();
  }
  // 控件该不该照 state 写：没有待提交的值，或者 state 已经是它了。
  function settled(row, v) {
    if (!row.pending) return true;
    if (String(row.pending.value) !== String(v)) return false;
    row.pending = null;
    return true;
  }

  function listItem(spec, first) {
    const {el, refs} = ITEM(), item = {current: first}, own = s.child('row-item');
    const buttons = (spec.actions || []).map(action => {
      const b = button(typeof action.label === 'function' ? '' : action.label, action.primary);
      own.on(b, 'click', () => press(b, () => action.run(item.current, state)));
      refs.actions.append(b);
      return {b, action};
    });
    const update = next => {
      item.current = next;
      const words = spec.itemText(next), href = spec.itemHref?.(next) || null;
      text(refs.to, href ? words : ''); attr(refs.to, 'href', href); show(refs.to, href);
      // 外部链接在新窗口打开（桌面 App 里不把设置页换掉）。
      const away = /^https?:/.test(href || '');
      attr(refs.to, 'target', away ? '_blank' : null); attr(refs.to, 'rel', away ? 'noreferrer noopener' : null);
      text(refs.text, href ? '' : words); show(refs.text, !href);
      const h = spec.itemHint?.(next) ?? ''; text(refs.hint, h); show(refs.hint, h);
      attr(el, 'title', spec.itemTitle?.(next) || null);
      for (const {b, action} of buttons) {
        if (typeof action.label === 'function') text(b, action.label(next, state));
        show(b, !action.hidden?.(next, state)); setOff(b, Boolean(action.disabled?.(next, state)));
        attr(b, 'title', action.title?.(next, state) || null);
      }
    };
    update(first);
    return {el, update, dispose: () => own.dispose()};
  }

  function paint(row) {
    const {spec, refs} = row, el = row.el;
    // 藏起来的行不再取别的值：它要的数据这时可能还没有。
    const hidden = Boolean(spec.hidden?.(state));
    show(el, !hidden);
    if (hidden) return;
    text(refs.label, read(row.label, state));
    if (spec.kind === 'select' && spec.seg) attr(refs.input, 'aria-label', read(row.label, state));
    if (row.to) attr(row.to, 'href', read(spec.href, state) || null);
    const h = read(row.hint, state) || ''; text(refs.hint, h); show(refs.hint, h);
    const dot = spec.dot ? spec.dot(state) : null;
    show(refs.dot, dot !== null && dot !== undefined); attr(refs.dot, 'data-dot', dot || null);
    const link = spec.link?.(state) || null;
    show(refs.link, link); attr(refs.link, 'href', link?.href || null); text(refs.link, link?.label || '');
    const warn = spec.warn?.(state) || ''; text(refs.warn, warn); show(refs.warn, warn);
    const bar = spec.progress?.(state) || null, total = bar?.total || 0;
    show(refs.bar, total > 0);
    if (total > 0) {
      attr(refs.bar, 'role', 'progressbar'); attr(refs.bar, 'aria-valuemax', String(total)); attr(refs.bar, 'aria-valuenow', String(bar.done || 0));
      css(refs.fill, 'transform', `scaleX(${Math.min(1, (bar.done || 0) / total).toFixed(3)})`);
    }
    const off = Boolean(spec.disabled?.(state));
    if (refs.input && spec.kind === 'action') setOff(refs.input, off);
    else if (refs.input && !(spec.kind === 'select' && spec.seg)) prop(refs.input, 'disabled', off);
    if (spec.kind === 'switch') { const on = Boolean(spec.get?.(state)); if (settled(row, on)) prop(refs.input, 'checked', on); }
    if (spec.kind === 'info' || spec.kind === 'action') {
      const v = read(spec.value, state) ?? '';
      text(refs.value, v);
      if (spec.kind === 'action') show(refs.value, v !== '');
    }
    if (spec.kind === 'action' && row.buttons) row.buttons.set(spec.buttons(state) || []);
    if (spec.kind === 'select') {
      const options = read(spec.options, state) || [], key = JSON.stringify(options.map(o => [o.value, o.label]));
      // 选项变了才重建（焦点在下拉框上时也不动，免得展开着的列表被换掉）。
      if (key !== row.options && document.activeElement !== refs.input) {
        row.options = key;
        refs.input.textContent = '';
        // 分段控件的每一段各挂一个监听，选项换了连同监听一起换。
        row.segScope?.dispose(); row.segScope = spec.seg ? s.child('row-seg') : null;
        for (const o of options) {
          const opt = spec.seg ? make('button', '', o.label) : document.createElement('option');
          if (spec.seg) {
            opt.type = 'button'; opt.dataset.value = o.value;
            row.segScope.on(opt, 'click', () => { if (!opt.disabled) { submit(row, o.value); paint(row); } });
          } else { opt.value = o.value; opt.textContent = o.label; }
          refs.input.append(opt);
        }
      }
      const v = spec.get?.(state) ?? '';
      if (spec.seg) {
        const shown = row.pending ? row.pending.value : v;
        settled(row, v);
        for (const b of refs.input.children) { attr(b, 'aria-pressed', String(String(b.dataset.value) === String(shown))); prop(b, 'disabled', off); }
      } else if (settled(row, v)) value(refs.input, v);
    }
    if (spec.kind === 'field') attr(refs.input, 'placeholder', read(spec.placeholder, state) || null);
    if (spec.kind === 'field') { const v = spec.get?.(state); if (settled(row, v ?? '')) value(refs.input, v); }
    if (spec.kind === 'list') { show(refs.head, read(row.label, state) || h); row.list.set(read(spec.items, state) || []); }
  }

  s.add(() => { for (const row of built) { row.list?.dispose(); row.buttons?.dispose(); row.el.remove(); } });
  return {
    update(next) { state = next; for (const row of built) paint(row); },
    row: key => built.find(row => row.spec.key === key)?.el ?? null,
    dispose: () => s.dispose(),
  };
}
