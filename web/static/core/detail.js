// 单张图的数据（0.4 第 D 步，#709）：详情卡片（ui/detail.js）和看图器（ui/viewer.js）共用的读取、角色每月张数、
// 「偶尔问一句是 TA 吗」的节流，以及把归属证据写成文字的纯函数。不碰 DOM，可以在 Node 里直接测（tests/detail_core.test.cjs）。
//
// - readDetail(uuid)：读 /api/detail（带收藏和主色）。同一张图、同一个读取纪元（state.readEpoch，写入后加一）里正在读的
//   那次直接复用，所以详情卡片和大图底栏同时要同一张时只发一个请求。读完写进 state.detail（本文件是它的写入者）。
// - 回复是旧的（读的过程中又要了别的图）由调用方判断：readDetail 返回 {d, uuid}，调用方比对自己记着的当前图。
// - state.question（详情或大图里正显示的那条提问）也只在这里写：setQuestion。
import {t} from './i18n.js';
import {cleanup} from './route.js';
import {api} from './session.js';
import {state} from './state.js';

// 读详情、大图底栏时带上的标记：收藏（#447）和主色、小调色板（#717）。
export const DETAIL_MARKS = {marks: 'fav,color'};

// ---- 读取 ----
const reading = new Map();   // `${uuid}|${纪元}` -> 还没落定的 promise
/** 读一张图的详情；同一张图、同一纪元里还在读的那次直接复用。读到后写进 state.detail。 */
export function readDetail(uuid) {
  const key = `${uuid}|${state.readEpoch}`;
  let p = reading.get(key);
  if (!p) {
    p = api('/api/detail?' + new URLSearchParams({id: uuid, ...DETAIL_MARKS})).finally(() => reading.delete(key));
    reading.set(key, p);
  }
  return p;
}
/** 详情卡片、大图底栏读到并画出来的那份：单图快捷键（1–9、C、A）按它。 */
export function setDetail(d) { state.detail = d; }
/** 正显示的提问（{q, ident}），没有时是 null。 */
export function setQuestion(q) { state.question = q; }

// 这张图的角色每月加入多少张：按角色、NSFW 设置和读取纪元缓存，到下一次写入或切换 NSFW 为止。
const statsCache = new Map();
/** 角色卡的数字：{count, first, months: [{month, count}], recent: [uuid]}；读不到时是 null。exclude 是这张图自己。 */
export async function readCharStats(id, exclude) {
  const key = `${id}|${state.nsfw}|${state.readEpoch}`;
  if (statsCache.has(key)) return statsCache.get(key);
  let st;
  try { st = await api('/api/character/stats?' + new URLSearchParams({id, nsfw: state.nsfw, exclude})); } catch { return null; }
  statsCache.set(key, st);
  return st;
}

// ---- 归属与证据 ----
/** 一个操作所替换的身份：当前角色页的那个身份（如果这张图上有），否则是第一个。 */
export const primaryOf = (d, route = state.route) => d.memberships.find(m => m.id === route.char) || d.memberships[0];

// PROTO = 从用户自己的裁决学到的原型（native/terms/src/research_compute/learn.rs）。WD_EXT、CCIP_LONGTAIL 是角色数据包
// （#550 WD 角色扩展的线性头、#559 CCIP 长尾包的锚点），后面跟它的分数（max_score），和「WD 0.91」并列。
const SOURCE_NAMES = {CCIP: t('外部索引'), PROTO: t('你的标注'), WD_EXT: t('WD 扩展'), CCIP_LONGTAIL: t('CCIP 长尾')};
// 数据包的来源和分数的小数位：WD 扩展是概率，CCIP 长尾是和外部索引同一种相似度。
const PACK_DIGITS = {WD_EXT: 2, CCIP_LONGTAIL: 3};
const sourceName = (s, e) => !(s in PACK_DIGITS) ? SOURCE_NAMES[s] || s : SOURCE_NAMES[s] + (e.max_score != null ? ` ${Number(e.max_score).toFixed(PACK_DIGITS[s])}` : '');
/** 只由数据包给出的项：名字后面常显小字「扩展」（#550 待决定 4），提示里只写数据包和它的分数。 */
export const fromExt = e => !!e?.sources?.length && e.sources.every(s => s in PACK_DIGITS);
/** 归属行和「也可能是」里的证据文字（alternatives 带 wd、ccip 两个键，归属行不带）：WD、外部索引或你的标注的分数，数据包的分数。 */
export const evidence = e => !e ? '' : fromExt(e) ? e.sources.map(s => sourceName(s, e)).join(' + ') : [e.wd != null ? `WD ${e.wd.toFixed(2)}` : '',
  e.ccip != null ? `${e.sources?.includes('PROTO') && !e.sources.includes('CCIP') ? SOURCE_NAMES.PROTO : SOURCE_NAMES.CCIP} ${e.ccip.toFixed(3)}` : '',
  !('wd' in e) && e.sources?.length ? e.sources.map(s => sourceName(s, e)).join(' + ') : '',
  'wd' in e ? (e.sources || []).filter(s => s in PACK_DIGITS).map(s => sourceName(s, e)).join(' + ') : ''].filter(Boolean).join(' · ');
/** 认出方式：机器识别、相簿、人工确认。 */
export const howFound = m => m.status === 'machine' ? t('机器识别') : m.origin === 'album' ? t('相簿') : t('人工确认');

// ---- 偶尔一问「是 TA 吗？」 ----
// 浏览时每隔几张图最多问一次，被略过三次后本次访问不再问；整理页（某个角色、未识别…）总是显示。回答发到
// /api/ask/answer，供学习使用（ui/detail.js 的 answerAsk）。
const ask = {cool: 0, ignored: 0, off: false, pending: null};
/** 这张图要不要显示提问。enabled 是视图选项里的「偶尔问一句」。同一张图再画（翻回来、写入后重读）时结论不变。 */
export function shouldAsk(d, ident, enabled) {
  if (!enabled || !d.question) return false;
  if (ask.pending === ident) return true;
  if (ask.pending) { ask.ignored++; ask.pending = null; if (ask.ignored >= 3) ask.off = true; }
  if (!cleanup()) {
    if (ask.off) return false;
    if (ask.cool > 0) { ask.cool--; return false; }
    ask.cool = 3;
  }
  ask.pending = ident; return true;
}
/** 回答过了：不算略过。 */
export function askAnswered() { ask.pending = null; ask.ignored = 0; }
/** 测试用：回到刚打开页面时的状态。 */
export function resetAsk() { Object.assign(ask, {cool: 0, ignored: 0, off: false, pending: null}); }
/** 提问那一句的说明：随机抽查、像你确认过的 TA、和谁分不清。 */
export const askWhy = q => q.kind === 'audit' ? t('随机抽查') : q.kind === 'hint' ? t('和你确认过的 TA 很像') + (q.current_name ? ' · ' + t('现在算作 {name}', {name: q.current_name}) : '')
  : q.kind === 'ambiguous' ? t('和 {name} 分不太清', {name: q.rival_name}) : '';
/** 回答发给 /api/ask/answer 的内容。how 是 'yes'、'no' 或 'switch'（是对手那个角色）。 */
export const askBody = ({q, ident}, how) => ({ids: [ident], entity: q.entity, kind: q.kind, ...(how === 'yes' ? {yes: [ident]} : {no: [ident]}), ...(how === 'switch' ? {switch: q.rival} : {})});
