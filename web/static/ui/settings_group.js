// 设置页的分组（#714）：一节由几组组成，每组一块浅底（css/base.css 的 .group），有标题、范围标签、说明，下面是行
// （ui/rows.js）、小标题、折叠区和少数自己画的小块，最后一行小字。ui/settings_*.js 各节只写配置，不拼节点。
//
//   const view = groups(main, [
//     {key: 'sync', title: t('同步'), scope: 'library', note: t('…'), hidden: s => !s.settings,
//      parts: [{kind: 'switch', …}, {kind: 'sub', label: t('来源')}, {kind: 'fold', key: 'apps', summary: s => …, parts: […]},
//              {kind: 'block', key: 'tiles', mount: (el, scope) => ({update(s) {…}})}],
//      foot: t('…')},
//   ], {scope: pageScope});
//   view.update(state);
//
// 组和部分的 title、note、foot、summary、label 可以是文字或 state => 文字。相邻的几行合成一张配置表（rows()）。
// 折叠区的开合记在 prefs 的 settingsOpen（键是 fold 的 key），重画、重开页面后照旧。
import {t} from '../core/i18n.js';
import {prefs} from '../core/prefs.js';
import {saveStartup} from '../core/settings.js';
import {state as app} from '../core/state.js';
import {attr, make, show, text, tpl} from './el.js';
import {toast} from './dom.js';
import {rows} from './rows.js';
import {openTasks} from './tasks.js';

// 改了以后重画当前这一节（ui/settings.js 登记进来，免得各节反过来导入外壳）。
let refresh = () => {};
export const onRefresh = fn => { refresh = fn; };
export const refreshSection = () => refresh();

const GROUP = tpl`<section class="group set-group"><h2 class="set-group-title"><span data-ref="title"></span><span class="chip set-scope" data-ref="scope" hidden></span></h2>
  <p class="set-group-note" data-ref="note" hidden><span data-ref="noteText"></span><span class="set-more" data-ref="more" hidden></span></p>
  <div class="set-body" data-ref="body"></div><p class="set-group-foot" data-ref="foot" hidden></p></section>`;
const FOLD = tpl`<details class="set-fold"><summary data-ref="summary"></summary><div class="set-body" data-ref="body"></div></details>`;

// 设置保存在哪里，让用户知道哪些设置会跟着他们跨图库、跨浏览器。按浏览器保存的偏好在桌面 App 里存在 App 自己的网页存储
// 里，标签改说「本 App」（#423）：/api/updates 带着外壳的安装状态（install 不为 null）时就是在桌面 App 里。
const SCOPES = {
  browser: () => [t('仅本浏览器'), t('保存在这个浏览器里，换浏览器或清除网站数据会恢复默认')],
  app: () => [t('仅本 App'), t('保存在 Illustash App 里；在浏览器里打开图库时，浏览器另存一份')],
  library: () => [t('本图库'), t('每个图库各自保存')],
  shared: () => [t('所有图库共用'), t('保存在这台电脑上，所有图库共用')],
};
export const scopeLabel = key => SCOPES[key === 'browser' && app.updates?.install ? 'app' : key]();

const read = (v, s) => typeof v === 'function' ? v(s) : v;
const ROW_KINDS = new Set(['switch', 'action', 'info', 'select', 'field', 'list']);

// 一串部分画进 body：返回 update(state)。
function parts(body, list, scope) {
  const updates = [];
  let batch = [];
  const flush = () => {
    if (!batch.length) return;
    const panel = rows(body, batch, {scope});
    updates.push(s => panel.update(s));
    batch = [];
  };
  for (const part of list) {
    if (ROW_KINDS.has(part.kind)) { batch.push(part); continue; }
    flush();
    if (part.kind === 'sub') {
      const h = make('h3', 'set-sub'); body.appendChild(h);
      updates.push(s => { show(h, !part.hidden?.(s)); text(h, read(part.label, s)); });
    } else if (part.kind === 'note') {
      const p = make('p', 'set-sub-note'); body.appendChild(p);
      updates.push(s => { const v = read(part.text, s) || ''; show(p, v && !part.hidden?.(s)); text(p, v); });
    } else if (part.kind === 'fold') {
      const {el, refs} = FOLD(), own = scope.child('fold');
      el.open = Boolean(prefs.get('settingsOpen', {})[part.key]);
      own.on(el, 'toggle', () => {
        const open = prefs.get('settingsOpen', {});
        if (el.open) open[part.key] = true; else delete open[part.key];
        prefs.set('settingsOpen', open);
      });
      body.appendChild(el);
      const inner = parts(refs.body, part.parts, own);
      updates.push(s => {
        const hidden = Boolean(part.hidden?.(s));
        show(el, !hidden);
        if (hidden) return;
        text(refs.summary, read(part.summary, s));
        inner(s);
      });
    } else if (part.kind === 'block') {
      const el = make('div', part.className || ''); el.dataset.block = part.key;
      body.appendChild(el);
      const own = scope.child('block'), view = part.mount(el, own);
      updates.push(s => { const hidden = Boolean(part.hidden?.(s)); show(el, !hidden); if (!hidden) view.update?.(s); });
    } else throw new Error(`ui/settings_group.js: 没有 ${part.kind} 这种部分`);
  }
  flush();
  return s => { for (const u of updates) u(s); };
}

export function groups(container, defs, {scope}) {
  const built = defs.map(def => {
    const {el, refs} = GROUP(), own = scope.child('group');
    el.dataset.group = def.key;
    container.appendChild(el);
    if (def.more) { text(refs.more, t('了解详情')); show(refs.more, true); }
    const inner = parts(refs.body, def.parts || [], own);
    own.add(() => el.remove());
    return s => {
      const hidden = Boolean(def.hidden?.(s));
      show(el, !hidden);
      if (hidden) return;
      text(refs.title, read(def.title, s));
      if (def.scope) {
        const [label, hint] = scopeLabel(read(def.scope, s));
        text(refs.scope, label); attr(refs.scope, 'title', hint); attr(refs.scope, 'data-scope', read(def.scope, s)); show(refs.scope, true);
      }
      const note = read(def.note, s) || '';
      text(refs.noteText, note); show(refs.note, note || def.more);
      if (def.more) attr(refs.more, 'title', read(def.more, s));
      const foot = read(def.foot, s) || ''; text(refs.foot, foot); show(refs.foot, foot);
      inner(s);
    };
  });
  return {update(s) { for (const u of built) u(s); }};
}

// 几节都用的行。
// 启动时的开关；本次启动仍以 --no-* 参数为准。还没读到 /api/settings 时不显示。
export const startupRow = (key, label, hint) => ({
  kind: 'switch', key: 'startup-' + key, label, hint: () => `${hint} · ${t('下次启动时生效')}`,
  hidden: s => !s.settings?.startup,
  warn: s => (s.settings?.forced_off || []).includes(key) ? t('这次启动已用命令行参数关闭') : '',
  get: s => Boolean(s.settings?.startup?.[key]),
  set: async on => {
    try { await saveStartup({[key]: on}); toast(t('已保存，下次启动时生效')); } catch (e) { toast(e.message); }
    refresh();
  },
});
// 任务的状态行：圆点 + 一句话，右边转到任务面板（运行在那里启动、停止和确认）。
export const taskRow = (key, line, dot, extra = {}) => ({
  kind: 'action', key, label: line, dot, button: t('在任务面板查看'), run: () => openTasks(), ...extra,
});
// 只给开发者的入口（测试图库的标记、粘贴 Pixiv cookie）要带 ?dev 打开过一次页面后才显示。
export const devMode = (() => { try { if (new URLSearchParams(location.search).has('dev')) prefs.set('dev', true); } catch {} return !!prefs.get('dev', false); })();
