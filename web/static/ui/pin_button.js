// 钉到活页条的按钮（#711，接 #684）：资料库的海报、作品卡上是右上角的小圆钮（悬停出现，钉住后一直显示），作品页横幅上是带字的
// 胶囊「钉到活页条／已钉住」。存取和顺序在 core/rail.js（pin、unpin，按图库存在浏览器本地），活页条二级层由 ui/rail.js 画。
//
//   const b = pinButton(s, {wide: true});   // s 是所在部件的作用域；wide：带字的胶囊
//   b.update('w:' + 作品键);                  // 换成另一项（'c:<角色 id>'、'w:<作品键>'）；'' 时藏起来
//   onChange(on)：钉住状态画完后调（海报据此把「+N」挪开）
// 按钮在卡片里面：点它不跟着打开卡片（preventDefault、stopPropagation）。
import {t} from '../core/i18n.js';
import {isPinned, loadPins, pin, pins, unpin} from '../core/rail.js';
import {attr, show, text, tpl} from './el.js';
import {icon} from './icons.js';

const PIN = tpl`<button type="button" class="cat-pin" aria-pressed="false"><span data-ref="label"></span></button>`;

export function pinButton(s, {wide = false, onChange} = {}) {
  const {el, refs} = PIN();
  el.prepend(icon('pin'));
  let key = '';
  if (wide) el.classList.add('btn', 'is-wide');
  show(refs.label, wide);
  function draw() {
    const on = !!key && isPinned(key);
    attr(el, 'aria-pressed', String(on));
    const label = on ? t('已钉住') : t('钉到活页条');
    text(refs.label, label);
    attr(el, 'title', on ? t('取消钉住') : t('钉到活页条'));
    if (!wide) attr(el, 'aria-label', label);
    onChange?.(on);
  }
  s.on(el, 'click', e => {
    e.preventDefault(); e.stopPropagation();
    if (key) (isPinned(key) ? unpin(key) : pin(key));
  });
  s.add(pins.changed.on(draw));
  loadPins().then(() => { if (s.alive) draw(); });
  return {el, update(next) { key = next || ''; show(el, !!key); draw(); }};
}
