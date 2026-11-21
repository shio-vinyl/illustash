// 「⋯」菜单（画廊页头 #710、角色页横幅 #712）：一个按钮和它下面的下拉菜单。菜单是非模态浮层，Esc、点外关闭和焦点由浮层管理器
// （ui/overlay.js）管；打开时焦点进第一项，↑ ↓ 在各项之间移动（menu，#422）。菜单项每次打开时按 items() 现排：
// [文字, 执行, 提示?]，假值跳过；点一项先关菜单再执行。
import {attr, make, tpl} from './el.js';
import {createLayer} from './overlay.js';

const MORE = tpl`<div class="tag-control"><button type="button" data-ref="button" title="更多操作" aria-label="更多操作" data-t-attr="title aria-label"
  aria-haspopup="menu" aria-expanded="false">⋯</button><div class="menu menu-drop more-menu" data-ref="menu" hidden></div></div>`;

/** 「⋯」菜单：{el, close(reason), dispose()}。id 是浮层的名字；menuId 给菜单元素的 id（走查按它找）；button 是按钮的类。 */
export function moreMenu({id, menuId, items, button = 'btn is-quiet'}) {
  const {el, refs} = MORE();
  attr(refs.button, 'class', button);
  if (menuId) attr(refs.menu, 'id', menuId);
  const layer = createLayer({kind: 'popover', id, el: () => refs.menu, anchor: () => refs.button, menu: true, focusIn: 'first',
    onDismiss: reason => set(false, reason)});
  function set(open, reason) {
    refs.menu.hidden = !open;
    attr(refs.button, 'aria-expanded', open ? 'true' : 'false');
    if (open) layer.open(); else layer.close(reason);
  }
  function item([label, run, title]) {
    const b = make('button', 'menu-item', label);
    attr(b, 'type', 'button');
    if (title) attr(b, 'title', title);
    b.onclick = () => { set(false); run(); };
    return b;
  }
  refs.button.onclick = () => {
    if (refs.menu.hidden) refs.menu.replaceChildren(...items().filter(Boolean).map(item));
    set(refs.menu.hidden);
  };
  return {el, close: reason => set(false, reason), dispose() { set(false); el.remove(); }};
}
