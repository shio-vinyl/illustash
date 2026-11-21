// inert 锁（#715）：一个元素可能同时被几处设成 inert：浮层管理器（盖满页面的层盖住的元素，ui/overlay.js）、出场动画
// （退场中的浮层，ui/motion.js 的 exit）。各处按自己的名字加锁、解锁，最后一把锁解开时才去掉 inert，谁加的谁解，
// 不会把别人的锁清掉。第一次加锁前元素本来就是 inert 的（keyed 列表直接设的、页面里写死的），全部解锁后还原成那样。
// 这个模块不 import 别的模块，可以单独在 Node 里测试。
const locks = new WeakMap();   // 元素 → {who: Set, base: 加锁前的 inert}

/** 以 `who` 的名义锁住（设成 inert）。同一个名字重复加锁只算一次。 */
export function lockInert(el, who) {
  if (!el) return;
  let entry = locks.get(el);
  if (!entry) { entry = {who: new Set(), base: !!el.inert}; locks.set(el, entry); }
  entry.who.add(who);
  if (!el.inert) el.inert = true;
}
/** 解开 `who` 的锁；没有别的锁时 inert 回到第一次加锁前的样子。没加过锁的不动。 */
export function unlockInert(el, who) {
  const entry = el && locks.get(el);
  if (!entry || !entry.who.delete(who)) return;
  if (entry.who.size) return;
  locks.delete(el);
  if (el.inert !== entry.base) el.inert = entry.base;
}
