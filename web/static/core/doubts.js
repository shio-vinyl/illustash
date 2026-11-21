// 整理 › 可能认错（#787）的数据：机器已经算给角色 A、但另一个角色 B 的证据几乎一样强的图（ambiguous_claims），按（A，B）分组。
// 读取、切成卡、各种回答的请求体。不碰 DOM，Node 里能直接测（tests/doubts.test.cjs）。画在 ui/doubts.js。
//
// 接口（#786，PR #789）：GET /api/ambiguous?nsfw=… → {groups: [{a: {id, name}, b: {id, name}, count, items: [uuid…]}]}。组按张数从多到少，
// 组内新加入的在前、给全（count === items.length）；只有看得见的图，7 天里对（图，A）答过「以后再说」的不列出。张数另在
// /api/review-pools 末尾的 kind=ambiguous 一项里（core/rail.js 的 DOUBT_POOL，活页条用）。
// 回答沿用 /api/ask/answer（kind=ambiguous，entity 是 A），和认一认一样一步一个撤销，见 doubtAnswer。提交时 A、B 已经不是这几张的
// 候选对（卡片过期）时后端回 400，页面重读队列（ui/doubts.js）。
import {askBody, askLater} from './ask.js';
import {api} from './session.js';

/** 一张卡最多放几张图：图多的一对分成几张卡，回答只作用于卡上露出来的图。 */
export const CARD_MAX = 16;

/** 读这一批（失败时抛错）。 */
export async function loadDoubts(nsfw, signal) {
  return (await api('/api/ambiguous?' + new URLSearchParams({nsfw}), {signal})).groups || [];
}

/**
 * 切成卡：每对（A，B）的图按次序每 max 张一张卡，同一对的卡排在一起。卡的键是这一对加上卡上第一张图：答掉一张卡、重读以后，
 * 其余的卡键不变（ui/keyed.js 按键复用，不重新加载图）。
 * 返回 [{key, a, b, items: [{uuid}], parts, total}]：parts 是这一对共几张卡，total 是这一对的张数（tally 算）。
 */
export function doubtCards(groups, max = CARD_MAX) {
  const out = [];
  for (const g of groups || []) {
    const ids = g.items || [], parts = Math.ceil(ids.length / max);
    for (let i = 0; i < parts; i++) {
      const items = ids.slice(i * max, (i + 1) * max).map(uuid => ({uuid}));
      out.push({key: `${g.a.id}|${g.b.id}|${items[0].uuid}`, a: g.a, b: g.b, items});
    }
  }
  return tally(out);
}
/**
 * 按现在的卡重算每张卡上这一对的 parts（共几张卡）、total（共几张图）：答掉同一对的一张卡以后，其余卡上「这一对共 N 张」跟着变。
 * 返回新的卡（对象换了，ui/keyed.js 照常按键复用节点）。接口给的 items 是全量，所以各卡的张数加起来就是这一对的张数。
 */
export function tally(cards) {
  const pairs = new Map();
  for (const c of cards) {
    const k = c.a.id + '|' + c.b.id, p = pairs.get(k) || {parts: 0, total: 0};
    p.parts++; p.total += c.items.length; pairs.set(k, p);
  }
  return cards.map(c => ({...c, ...pairs.get(c.a.id + '|' + c.b.id)}));
}

/**
 * 一张卡的回答（交给 /api/ask/answer，一步、一个撤销）：
 *   'a'      其余是 A，点掉的（off）不是 A（和认一认的「其余 N 张是」一样）
 *   'b'      卡上全部是 B：不是 A、是 B（switch）
 *   'none'   都不是：卡上全部对 A、对 B 都记「不是」（also_no，#786）
 *   'other'  点掉的是另一个角色 other：只提交点掉的那几张（不是 A、是 other）
 */
export function doubtAnswer(card, how, off, other = '') {
  const batch = {entity: card.a.id, kind: 'ambiguous', items: card.items}, all = new Set(card.items.map(x => x.uuid));
  if (how === 'a') return askBody(batch, 'ok', off);
  if (how === 'b') return askBody(batch, 'other', all, card.b.id);
  if (how === 'none') return {...askBody(batch, 'none', off), also_no: card.b.id};
  return askBody(batch, 'other', off, other);
}
/** 以后再说：记下卡上这几张问过了（7 天里不再列出），不改图库，不进撤销。 */
export const doubtLater = card => askLater({entity: card.a.id, kind: 'ambiguous', items: card.items});
