// 按住拖动的手势（#450 的拖到别的应用、#710 的拖进画廊共用）：按下左键（不是触控），移动超过 SLOP 像素算开始拖。
//   wirePress(el, {when, start})  el 上按下时 when(down) 为假就不算；开始拖时调一次 start(down, move)（down 是按下时的
//                                 pointerdown 事件，move 是越过阈值的那次 pointermove），它返回真值（真的拖起来了）时吞掉
//                                 松手后的那次点击，免得点开大图、详情。返回 off()，交给作用域。
// 用在 ui/files.js 的 wireDragOut、ui/drag.js 的 wireLift。改这个手势（指针捕获、触控）只改这里。
export const SLOP = 6;

export function wirePress(el, {when = () => true, start}) {
  let down = null, dragged = false;
  const on = {
    pointerdown: e => { dragged = false; down = e.button === 0 && e.pointerType !== 'touch' && when(e) ? e : null; },
    pointermove: e => {
      if (!down || !(e.buttons & 1) || Math.hypot(e.clientX - down.clientX, e.clientY - down.clientY) < SLOP) return;
      const from = down; down = null;
      dragged = !!start(from, e);
    },
    pointerup: () => { down = null; },
    pointercancel: () => { down = null; },
  };
  const swallow = e => { if (dragged) { e.preventDefault(); e.stopImmediatePropagation(); } dragged = false; };
  for (const [type, fn] of Object.entries(on)) el.addEventListener(type, fn);
  el.addEventListener('click', swallow, true);
  return () => {
    for (const [type, fn] of Object.entries(on)) el.removeEventListener(type, fn);
    el.removeEventListener('click', swallow, true);
  };
}
