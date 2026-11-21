// 横幅左上的返回（#711 作品页、#712 角色页）：写「回到哪里」，按来路走。去处和名字由 core/rail.js 的 backTo 按上一条地址
// （core/route.js 的 cameFrom）算；后退能回去就一次退 backSteps() 步（同一页上换过造型、筛选的那几条一起退掉），没有来路时去
// 调用方给的 fallback（{label, href}）。样子是浮在图上的玻璃按钮（css/base.css 的 .btn.glass）。
import {backTo} from '../core/rail.js';
import {backSteps, cameFrom, stepHistory} from '../core/route.js';
import {attr, text, tpl} from './el.js';

const BACK = tpl`<a class="btn glass"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M12 4.5 6.5 10l5.5 5.5"/></svg><span data-ref="label"></span></a>`;

/** 返回按钮：{el, update(), dispose()}。点击登记在 scope 上；地址变了（同一页又画一次）调 update 重写去处。 */
export function backLink(scope, fallback) {
  const {el, refs} = BACK();
  scope.on(el, 'click', e => { if (backTo(cameFrom(), fallback).step) { e.preventDefault(); stepHistory(-backSteps()); } });
  function update() {
    const back = backTo(cameFrom(), fallback);
    text(refs.label, back.label); attr(el, 'href', back.href);
  }
  update();
  return {el, update, dispose: () => el.remove()};
}
