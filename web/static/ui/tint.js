// 图的颜色（#705）：把一张图的主色写到元素上，是写 --tint 的唯一入口（tools/check_frontend.py 规则 20 拦别处直接写 --tint）。
//
// 怎么用：
//   import {setTint} from './tint.js';
//   setTint(card, item.color);   // 写 --tint，并给元素加 .tint，派生的底色（--tint-wash/-soft/-glow/-shadow）在这个元素上重算
//   setTint(card, null);         // 没有颜色：两样都去掉，退回 :root 上的中性色
//
// 为什么要加 .tint：自定义属性里的 var() 在声明它的元素上就算好了，:root 上的 --tint-wash 只认得 :root 的 --tint；
// css/tokens.css 在 .tint 上把派生色重新算一遍，写了 --tint 却漏了 .tint，就会静悄悄地退回中性色。所以两样一起写，只在这里写。
// 颜色由后端给（#717，十六进制）；格式不对的当作没有颜色。界面上不显示色值。

const COLOR = /^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/i;

/**
 * 给元素写上图的主色。
 * @param {HTMLElement} el
 * @param {string|null|undefined} color 十六进制颜色；空或格式不对时去掉
 */
export function setTint(el, color) {
  if (!el?.style) return;
  if (typeof color === 'string' && COLOR.test(color)) {
    if (el.style.getPropertyValue?.('--tint') !== color) el.style.setProperty('--tint', color);
    el.classList.add('tint');
  } else {
    el.style.removeProperty?.('--tint');
    el.classList.remove('tint');
  }
}
