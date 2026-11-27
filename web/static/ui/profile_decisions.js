// 编辑资料中可以拆开词表关系，或解除一对角色的「不再追问」偏好。
import {roleName} from '../core/labels.js';
import {readProfile, resumePair, splitCatalog} from '../core/character.js';
import {exclusive} from '../core/edits.js';
import {t} from '../core/i18n.js';
import {charHash} from '../core/route.js';
import {toast, written} from './dom.js';
import {attr, show, text, tpl} from './el.js';

const SECTION = tpl`<div class="field"><span data-ref="label"></span><div class="link-list" data-ref="list"></div></div>`;
const CATALOG = tpl`<div class="link-row"><span><b data-ref="name"></b><small data-ref="note"></small>
  <small data-ref="tags"><code data-ref="from"></code> → <code data-ref="to"></code></small></span>
  <button type="button" class="quiet" data-ref="split" title="拆开后角色词表不会再合并这一对" data-t-attr="title" data-t>拆开</button></div>`;
const APART = tpl`<div class="link-row"><span><span><a data-ref="a" data-close></a> ↔ <a data-ref="b" data-close></a></span></span>
  <button type="button" class="quiet" data-ref="resume" data-t>允许再次询问</button></div>`;

export function profileDecisions(container, initial, refresh) {
  const id = initial.id;
  const action = (button, write) => async () => {
    button.disabled = true;
    try {
      await exclusive(async () => {
        const saved = await write();
        written(saved.undo);
        await Promise.all([readProfile(id).then(p => { if (container.isConnected) render(p); }), refresh()]);
      });
    } catch (e) { toast(e.message); }
    finally { button.disabled = false; }
  };
  const section = (label, items, row) => {
    const {el, refs} = SECTION();
    text(refs.label, label);
    refs.list.replaceChildren(...items.map(row));
    return el;
  };
  const catalogRow = item => {
    const {el, refs} = CATALOG();
    text(refs.name, item.name);
    text(refs.note, item.kind === 'merged' ? t('D 站改名前后的 tag，已合成一个角色') : t('已挂成这个角色的造型'));
    const from = item.tags?.join(' / '), to = item.owner_tags?.join(' / ');
    show(refs.tags, !!(from && to));
    text(refs.from, from); text(refs.to, to);
    refs.split.onclick = action(refs.split, () => splitCatalog(id, item));
    return el;
  };
  const apartRow = pair => {
    const {el, refs} = APART();
    for (const side of ['a', 'b']) {
      const role = pair[side];
      text(refs[side], roleName(role));
      attr(refs[side], 'href', role.family ? charHash(role.family) : null);
    }
    refs.resume.onclick = action(refs.resume, () => resumePair(pair));
    return el;
  };
  function render(p) {
    const sections = [];
    if (p.catalog?.length) sections.push(section(t('角色词表'), p.catalog, catalogRow));
    if (p.kept_apart?.length) sections.push(section(t('不再追问的角色对'), p.kept_apart, apartRow));
    container.replaceChildren(...sections);
    show(container, sections.length !== 0);
  }
  render(initial);
}
