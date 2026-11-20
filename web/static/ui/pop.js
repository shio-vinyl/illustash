// 弹出层的开合（#715）：视图选项、任务面板、页面说明、图库切换、上手清单这类 hidden 开关的浮层，开的时候显示出来
// （进场是 css/overlays.css 里 .menu 的动画，取消显示时 display 变回来就会重播），关的时候先播一段短的退场再 hidden。
// 浮层管理器（ui/overlay.js）照旧管栈和 Esc；这里只管样子，各处原来写 `el.hidden = !open` 的地方改调 showPop。
import {cancelExit, exit, exitingNow} from './motion.js';

/**
 * 开或关一个弹出层。关：播完退场才 hidden（期间再开会接手，不闪）；开：取消没播完的退场、去掉 hidden。
 * to 是退场的样子（ui/motion.js 的 exit）；靠 transform 定位的（提示条居中用 translateX）传 'fade'，免得缩放把位置冲掉。
 */
export function showPop(el, open, {to = 'scale'} = {}) {
  if (!el) return;
  if (open) { cancelExit(el); el.hidden = false; return; }
  if (el.hidden || exitingNow(el)) return;
  exit(el, {to, remove: false});
}
/** 开着、而且不在关的路上。拿来决定「再点一下是开还是关」。 */
export const popOpen = el => !!el && !el.hidden && !exitingNow(el);
