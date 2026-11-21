// 空状态（#706，样子是 css/base.css 的 .empty-state，#705）：大图标 + 一句事实 + 去处。网格页的空状态由注册表的 empty、
// emptyIcon、emptyNext 声明（core/pages.js），各面板页自己的空状态也用这里的 drawEmpty。
//
//   drawEmpty(box, {icon: 'check', fact: t('没有未识别的图。'), next: [{label: t('回到按组整理'), href: '#/groups'}]});
//   next 的每一项是链接（href）或按钮（act：写进 data-empty，点击由所在的页面接，网格页的由 ui/shell.js 接）。
//   note 是事实下面可选的一句说明（资料库画师页签的「还没有画师」，#711）。
import {make, text, tpl} from './el.js';

const ICONS = {
  image: tpl`<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3.5" y="4.5" width="17" height="15" rx="3"/><path d="m4.5 17 5-5 4 4 2.5-2.5 4 4"/><circle cx="15.5" cy="9" r="1.6"/></svg>`,
  check: tpl`<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8.5"/><path d="m8.2 12.3 2.6 2.6 5-5.4"/></svg>`,
  calendar: tpl`<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3.5" y="5" width="17" height="15" rx="3"/><path d="M3.5 10h17M8 3v4M16 3v4"/></svg>`,
  gallery: tpl`<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3.5" y="6" width="13" height="13" rx="2.5"/><path d="M7.5 3.5h10a3 3 0 0 1 3 3v10"/></svg>`,
  person: tpl`<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="8.5" r="3.8"/><path d="M4.8 20c.8-3.8 3.6-5.8 7.2-5.8s6.4 2 7.2 5.8"/></svg>`,
  heart: tpl`<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 19.6S4 14.9 4 9.4A4 4 0 0 1 12 7.5a4 4 0 0 1 8 1.9c0 5.5-8 10.2-8 10.2Z"/></svg>`,
  search: tpl`<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/></svg>`,
};
const EMPTY = tpl`<div class="empty-state"><span class="empty-icon" data-ref="icon"></span><p class="empty-fact" data-ref="fact"></p><p class="empty-note"
  data-ref="note"></p><div class="empty-next"
  data-ref="next"></div></div>`;

// 把 box 的内容换成一个空状态。同一个 box 再画时整块换掉（空状态不常变，也没有输入框）。
export function drawEmpty(box, {icon = 'image', fact = '', note = '', next = []} = {}) {
  const {el, refs} = EMPTY();
  refs.icon.append((ICONS[icon] || ICONS.image)().el);
  text(refs.fact, fact);
  text(refs.note, note); refs.note.hidden = !note;
  for (const x of next || []) {
    const b = x.href ? make('a', 'btn', x.label) : make('button', 'btn', x.label);
    if (x.href) b.href = x.href; else b.dataset.empty = x.act;
    refs.next.append(b);
  }
  refs.next.hidden = !next?.length;
  box.replaceChildren(el);
}
