// 作品卡（#711）：作品名大字 + 作品配色做底（样图的主色，#717；没有时中性色）+ 一排角色圆头像 + 几条不同图的竖切片，左下是
// 「关注新角色」开关，右上是钉到活页条的图钉。一眼和角色海报区分开：海报是一个人，作品卡是一组人。
// 悬停时卡片浮起、投影带作品配色，切片依次轻轻上浮，头像散开一点（css/catalog.css）。点卡片进作品页，开关和图钉在卡上直接点。
//
//   const make = workCards(s, {open});   // open(key, el)：点了卡片、要进作品页之前（作品页据此从卡片展开）
//   keyed(box, {key: x => x.id, create: make, scope: s}).set([{id, name, count, chars, covers, watched, watchable, tag}, …]);
import {workColor} from '../core/catalog.js';
import {imageUrl, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {hashFor} from '../core/route.js';
import {setWatch} from './catalog.js';
import {attr, make, prop, refill, show, text, tpl} from './el.js';
import {pinButton} from './pin_button.js';
import {setTint} from './tint.js';

const CARD = tpl`<article class="cat-work"><a class="cat-work-link" data-ref="link"><span class="cat-work-text"><b class="cat-work-name" data-ref="name"></b><small
  data-ref="meta"></small><span class="cat-faces" data-ref="faces"></span></span><span class="cat-slices" data-ref="slices"></span></a><label class="cat-watch"
  data-ref="watch"><input type="checkbox" class="switch" data-ref="box"><span data-t>关注新角色</span></label></article>`;
const FACES = 6;

export function workCards(parent, {open}) {
  return first => card(first, parent.child('work-card'), open);
}

function card(first, s, open) {
  const {el, refs} = CARD(), pinned = pinButton(s);
  el.append(pinned.el);
  let w = first;
  s.on(refs.link, 'click', e => { if (!e.metaKey && !e.ctrlKey && !e.shiftKey) open?.(w.id, el); });
  s.on(refs.box, 'change', () => { const on = refs.box.checked; refs.box.checked = !!w.watched; setWatch(w, on); });
  function update(next) {
    w = next;
    attr(refs.link, 'href', hashFor({page: 'work', id: w.id}));
    attr(el, 'data-work', w.id);
    setTint(el, workColor(w.covers));
    text(refs.name, w.name);
    text(refs.meta, [w.chars.length ? t('{n} 位角色', {n: number(w.chars.length)}) : '', t('{n} 张', {n: number(w.count || 0)})].filter(Boolean).join(' · '));
    const faces = w.chars.slice(0, FACES), more = w.chars.length - faces.length;
    refill(refs.faces, faces.map(c => c.id + c.cover).join('|') + '|' + more, () => [
      ...faces.map(c => { const img = make('img', 'avatar is-small'); img.alt = ''; img.loading = 'lazy'; img.title = c.name; if (c.cover) img.src = imageUrl(c.cover); setTint(img, c.color); return img; }),
      ...(more > 0 ? [make('span', 'cat-faces-more', '+' + number(more))] : [])]);
    refill(refs.slices, w.covers.map(x => x.uuid).join('|'), () => w.covers.map(x => {
      const img = make('img'); img.alt = ''; img.loading = 'lazy'; img.decoding = 'async'; img.src = imageUrl(x.uuid); setTint(img, x.color); return img;
    }));
    show(refs.watch, !w.pseudo);
    prop(refs.box, 'checked', !!w.watched);
    attr(refs.watch, 'title', w.watchable ? t('定期检查 Danbooru 上这部作品还没收录的角色（只发送作品名）') : t('先在「编辑作品」里核对 Danbooru 作品 tag，才能关注'));
    pinned.update(w.pseudo ? '' : 'w:' + w.id);
  }
  update(first);
  return {el, update, dispose: () => s.dispose()};
}
