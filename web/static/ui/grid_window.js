// 图库网格的可见范围、预读行和快滚时的缩略图策略（#769）。
import {PAGE} from '../core/assets.js';
import {flowing, rowAt, rowReach, rowStart} from './grid_rows.js';

// 从第 first 张到第 last 张所在的几页（最多 4 页）。
export function pagesBetween(first, last) {
  const offsets = [];
  for (let o = Math.floor(first / PAGE) * PAGE; o < Math.max(last, 1); o += PAGE) offsets.push(o);
  return offsets.slice(0, 4);
}
export const visibleEnd = v => rowStart(rowAt(v.scrollTop + v.clientHeight) + 2);
export const visibleOffsets = v => pagesBetween(rowStart(rowAt(v.scrollTop)), visibleEnd(v));

// 快滚时新卡先用小图，限制连续上传的位图量。速度降下来后升级此时可见的卡片。
const PREVIEW_SPEED = 1.6, PREVIEW_SETTLE = 300;  // 像素/毫秒，毫秒
export function gridWindow(onSettled) {
  let fastScroll = false, lastPaintTop = 0, lastPaintAt = 0, previewTimer, top = 0, bottom = 0;

  function reset() {
    clearTimeout(previewTimer);
    fastScroll = false;
    lastPaintAt = 0;
  }

  function track(scrollTop) {
    const now = performance.now(), elapsed = now - lastPaintAt;
    if (lastPaintAt && elapsed > 0 && elapsed < 100 && Math.abs(scrollTop - lastPaintTop) / elapsed > PREVIEW_SPEED) {
      fastScroll = true;
      clearTimeout(previewTimer);
      previewTimer = setTimeout(() => {
        fastScroll = false;
        onSettled();
      }, PREVIEW_SETTLE);
    }
    lastPaintTop = scrollTop;
    lastPaintAt = now;
  }

  function measure(viewport, rows) {
    top = viewport.scrollTop;
    bottom = top + viewport.clientHeight;
    const first = Math.max(0, rowAt(top - rowReach()) - (flowing() ? 1 : 2));
    const last = Math.min(rows, rowAt(bottom) + 2);
    // 先读挂载范围，再预读视口上下约一屏。尺寸提早到，减少眼前的行重排。
    const ahead = rowStart(Math.min(rows, rowAt(top + 2 * viewport.clientHeight) + 1));
    const behind = rowStart(rowAt(top - viewport.clientHeight));
    // 可见范围变了就更新小图，即使预挂载的整段卡片的位置还没变。
    const key = fastScroll ? 'fast' : `${rowAt(top)}:${rowAt(bottom)}`;
    return {first, last, ahead, behind, key};
  }

  function preview(box) {
    if (fastScroll) return 'fast';
    return box.y + box.h > top && box.y < bottom ? 'visible' : 'offscreen';
  }

  return {reset, track, measure, preview};
}
