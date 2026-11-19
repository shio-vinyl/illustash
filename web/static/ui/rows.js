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
//   panel.update(state);   // 第一次和之后每次状态变了
//   panel.dispose();
//
// 几种行：
//   switch  开关行：标题 + 说明，右边开关。改动时调 set(checked, state)。
//   action  操作行：标题 + 说明，右边一个按钮（button 是按钮上的字），点了调 run(state)。
//   info    信息行：标题 + 说明，右边是值（value(state) 的文字）。
//   select  选择行：右边下拉框，options 是 [{value, label}] 或者 state => […]；改动时调 set(value, state)。
//   field   输入行：右边输入框（type 默认 text），离开输入框或回车（change 事件）时调 set(value, state)。
//   list    列表行：标题下面一项一行（按 itemKey 复用），每项右边可以有几个按钮（actions），点了调 run(item, state)。
// 每行都可以带 hidden(state)、disabled(state)（list 的按钮各自带）。
// label、hint 可以写成文字或函数（state => 文字，每次 update 取值，用来带上数字）；button、placeholder、选项和按钮的
// label 是文字。文字都由调用方用 t('…') 翻译好再传进来（键要写成字面量，tests/i18n.test.cjs 才收集得到）。
//
// 行的样式沿用设置页现有的 .switch-line、.setting-line（tools/styleguide/「设置卡」）；#714 换新样式时改这里的模板。
import {attr, prop, show, text, tpl, value} from './el.js';
import {keyed} from './keyed.js';
import {scope} from './scope.js';

const SWITCH = tpl`<label class="switch-line"><span><b data-ref="label"></b><small data-ref="hint"></small></span>
  <input type="checkbox" class="switch" data-ref="input"></label>`;
const ACTION = tpl`<div class="setting-line"><span><b data-ref="label"></b><small class="line-hint" data-ref="hint"></small></span>
  <button type="button" class="quiet small" data-ref="input"></button></div>`;
const INFO = tpl`<div class="setting-line"><span><b data-ref="label"></b><small class="line-hint" data-ref="hint"></small></span>
  <span data-ref="value"></span></div>`;
const SELECT = tpl`<label class="setting-line"><span><b data-ref="label"></b><small class="line-hint" data-ref="hint"></small></span>
  <select data-ref="input"></select></label>`;
const FIELD = tpl`<label class="setting-line"><span><b data-ref="label"></b><small class="line-hint" data-ref="hint"></small></span>
  <input data-ref="input"></label>`;
const LIST = tpl`<div><div class="setting-line"><span><b data-ref="label"></b><small class="line-hint" data-ref="hint"></small></span></div>
  <div data-ref="items"></div></div>`;
const ITEM = tpl`<div class="setting-line"><span><span data-ref="text"></span><small class="line-hint" data-ref="hint"></small></span>
  <span data-ref="actions"></span></div>`;
const KINDS = {switch: SWITCH, action: ACTION, info: INFO, select: SELECT, field: FIELD, list: LIST};

// 文字原样用，函数每次按状态取值。
const read = (v, s) => typeof v === 'function' ? v(s) : v;

export function rows(container, table, {scope: parent = null} = {}) {
  const s = parent ? parent.child('rows') : scope('rows');
  let state = null;
  const built = table.map(spec => build(spec));

  function build(spec) {
    const make = KINDS[spec.kind];
    if (!make) throw new Error(`ui/rows.js: 没有 ${spec.kind} 这种行`);
    const {el, refs} = make(), label = spec.label, hint = spec.hint ?? '';
    el.dataset.row = spec.key;
    const row = {spec, el, refs, label, hint, list: null, options: ''};
    if (spec.kind === 'switch') s.on(refs.input, 'change', () => spec.set?.(refs.input.checked, state));
    if (spec.kind === 'action') { text(refs.input, spec.button); s.on(refs.input, 'click', () => spec.run?.(state)); }
    if (spec.kind === 'select' || spec.kind === 'field') s.on(refs.input, 'change', () => spec.set?.(refs.input.value, state));
    if (spec.kind === 'field') { attr(refs.input, 'type', spec.type || 'text'); attr(refs.input, 'placeholder', spec.placeholder || null); }
    if (spec.kind === 'list') row.list = keyed(refs.items, {key: spec.itemKey, create: item => listItem(spec, item)});
    container.appendChild(el);
    return row;
  }

  function listItem(spec, first) {
    const {el, refs} = ITEM(), item = {current: first}, own = s.child('row-item');
    const buttons = (spec.actions || []).map(action => {
      const b = document.createElement('button');
      b.type = 'button'; b.className = 'quiet small'; b.textContent = action.label;
      own.on(b, 'click', () => action.run(item.current, state));
      refs.actions.appendChild(b);
      return {b, action};
    });
    const update = next => {
      item.current = next;
      text(refs.text, spec.itemText(next));
      const h = spec.itemHint?.(next) ?? ''; text(refs.hint, h); show(refs.hint, h);
      for (const {b, action} of buttons) { show(b, !action.hidden?.(next, state)); prop(b, 'disabled', Boolean(action.disabled?.(next, state))); }
    };
    update(first);
    return {el, update, dispose: () => own.dispose()};
  }

  function paint(row) {
    const {spec, el, refs} = row;
    show(el, !spec.hidden?.(state));
    text(refs.label, read(row.label, state));
    const h = read(row.hint, state) || ''; text(refs.hint, h); show(refs.hint, h);
    const off = Boolean(spec.disabled?.(state));
    if (refs.input) prop(refs.input, 'disabled', off);
    if (spec.kind === 'switch') prop(refs.input, 'checked', Boolean(spec.get?.(state)));
    if (spec.kind === 'info') text(refs.value, spec.value?.(state));
    if (spec.kind === 'select') {
      const options = read(spec.options, state) || [], key = JSON.stringify(options.map(o => [o.value, o.label]));
      // 选项变了才重建（焦点在下拉框上时也不动，免得展开着的列表被换掉）。
      if (key !== row.options && document.activeElement !== refs.input) {
        row.options = key;
        refs.input.textContent = '';
        for (const o of options) { const opt = document.createElement('option'); opt.value = o.value; opt.textContent = o.label; refs.input.appendChild(opt); }
      }
      value(refs.input, spec.get?.(state));
    }
    if (spec.kind === 'field') value(refs.input, spec.get?.(state));
    if (spec.kind === 'list') row.list.set(read(spec.items, state) || []);
  }

  s.add(() => { for (const row of built) { row.list?.dispose(); row.el.remove(); } });
  return {
    update(next) { state = next; for (const row of built) paint(row); },
    row: key => built.find(row => row.spec.key === key)?.el ?? null,
    dispose: () => s.dispose(),
  };
}
