// 按住拖动的手势（#450 的拖到别的应用、#710 的拖进画廊共用）：按下左键（不是触控），移动超过 SLOP 像素算开始拖。
//   wirePress(el, {when, start})  el 上按下时 when(down) 为假就不算；开始拖时调一次 start(down, move)（down 是按下时的
//                                 pointerdown 事件，move 是越过阈值的那次 pointermove），它返回真值（真的拖起来了）时吞掉
//                                 松手后的那次点击，免得点开大图、详情。返回 off()，交给作用域。
//   onPress(el, fn)               el 上按下时调 fn(e)：平时是 pointerdown；没有 pointerdown 的那次按下由 mousedown 补上（见下）。返回 off()
//   handedOff()                   拖到别的应用以后调一次（ui/drag.js 的 handOff）
// 用在 ui/files.js 的 wireDragOut、ui/drag.js 的 wireLift、ui/grid.js 的按下。改这个手势（指针捕获、触控）只改这里。
//
// 拖到别的应用以后（#710 虚拟机走查）：松手被系统拖放接走，页面收不到 pointerup，WebKit 还当左键按着。于是指针一动就开始选字，
// 下一次真按下也不派 pointerdown（只派 mousedown），要先单击一次才恢复。所以：交出去的时候清掉选区，到下一次按下或松开之前
// 页面不可选（body 的 .handed-off，样式在 css/base.css）；按下由 onPress 用 mousedown 补上。
export const SLOP = 6;

export function onPress(el, fn) {
  let viaPointer = false;
  const pointer = e => { viaPointer = true; fn(e); }, mouse = e => { if (!viaPointer) fn(e); viaPointer = false; };
  el.addEventListener('pointerdown', pointer); el.addEventListener('mousedown', mouse);
  return () => { el.removeEventListener('pointerdown', pointer); el.removeEventListener('mousedown', mouse); };
}

let handed = false;
const SETTLE = ['mousedown', 'pointerup', 'mouseup', 'focus'];
export function handedOff() {
  document.getSelection?.()?.removeAllRanges();
  if (handed) return;
  handed = true;
  document.body.classList.add('handed-off');
  for (const type of SETTLE) addEventListener(type, settle, true);
}
function settle() {
  handed = false;
  document.body.classList.remove('handed-off');
  for (const type of SETTLE) removeEventListener(type, settle, true);
  document.getSelection?.()?.removeAllRanges();
}
// 还没缓过来（测试用）。
export const handedOffNow = () => handed;

export function wirePress(el, {when = () => true, start}) {
  let down = null, dragged = false;
  const offPress = onPress(el, e => { dragged = false; down = e.button === 0 && e.pointerType !== 'touch' && when(e) ? e : null; });
  const on = {
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
    offPress();
    for (const [type, fn] of Object.entries(on)) el.removeEventListener(type, fn);
    el.removeEventListener('click', swallow, true);
  };
}
