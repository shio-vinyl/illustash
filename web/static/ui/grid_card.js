// 图库网格的卡片（#708，0.4 第 C2 步）：一张图一个部件，节点建一次，之后只改变了的地方（ui/el.js 的写入函数）。
// 网格（ui/grid.js）管行坐标、挂哪些卡片和选择；卡片只按给它的数据画，不发请求、不读 state。
//
//   const card = gridCard(scope);
//   card.update({item, index, box: {x, y, w, h}, ctx});   // ctx 是这一轮所有卡片共用的上下文（gridContext 的结果）
//   card.mark({selected, focused, cursor});                // 选中、详情栏里那张、键盘光标：只改这三样，滚动和选择时用
//   card.dispose();
//
// 样子（css/library.css「网格」一段）：图满格、圆角 8，下面没有标题条。图读完之前是图的主色（--tint-soft），悬停时上浮、
// 投影带图的颜色（css/tokens.css 的 .tint）；悬停、键盘光标在它上面时，底部压暗一条，写角色名：一位写名字，几位写第一位
// 加「另外 N 位」，窄卡只写「N 位」。选中、光标、详情栏里那张用 A2 的「选中」画法。
//
// 角上的东西和原来的卡片一样，按页面和视图选项出现：左上的勾（选择）、右上的副本角标、右下的心形和「机器识别」的点、
// 左下一排芯片（造型、「现为 TA」、排除原因、「可能是」的建议、随机抽查的角色标签）。按钮带着 data-fav、data-hint、
// data-tagact，点击由网格在容器上统一接（ui/grid.js wireGrid），卡片自己不挂点击。
//
// 节点：根节点是 .card，带 data-id（图的 uuid）、data-index（第几张）；位置写 transform 和宽高（只动合成层）。
// 缩略图在 ui/motion.js 的登记表里标上 uuid（看图器从这里展开、关掉时回到这里，#709）；摘掉时放掉解码后的位图（#683）。
import {imageUrl, number, short, thumbSize} from '../core/format.js';
import {t} from '../core/i18n.js';
import {REASONS, nameWithWork} from '../core/labels.js';
import {clampRatio, aspect} from '../core/layout.js';
import {attr, cls, css, make, show, text, tpl} from './el.js';
import {heartTip} from './favorites.js';
import {icon} from './icons.js';
import {flip, thumbs} from './motion.js';
import {setTint} from './tint.js';
import {releaseImages} from './virtual.js';

const CARD = tpl`<div class="card" role="button"><div class="card-stage" data-ref="stage"><img data-ref="img" alt="" decoding="async" loading="lazy"
  draggable="false"><span class="card-info" data-ref="info"><b data-ref="who"></b><small data-ref="more"></small></span><span class="card-chips"
  data-ref="chips"></span><span class="dup-badge" data-ref="dup"></span><span class="machine-dot" data-ref="dot" title="机器识别，未检查"
  data-t-attr="title"></span><span class="card-missing" data-t>图片暂不可读</span><button type="button" class="fav-mark" data-ref="fav"></button><span class="check" aria-hidden="true"></span></div></div>`;
const SKELETON = tpl`<div class="card skeleton" aria-hidden="true"></div>`;

// 角上这些东西各自要多宽才出现（px）：芯片（CHIPS_MIN）、抽查标签（SMALL）按格宽 ctx.cellW；悬停时写不写名字（INFO_MIN）、写名字
// 还是只写「N 位」（NAMES_MIN）按这张卡片自己的宽 box.w（原始比例里各张不一样宽）。
const CHIPS_MIN = 120, SMALL = 70, INFO_MIN = 72, NAMES_MIN = 130;

/**
 * 这一轮所有卡片共用的上下文：页面、视图选项和格子大小。网格每次画之前算一次，传给每张卡片的 update。
 * @param {object} o {route, view（core/pages.js 的 viewOf）, fit（原始比例）, cellW, hints（视图选项「可能是」）, hue(i)（重复组的色调）}
 */
export function gridContext({route, view, fit, cellW, hints, hue}) {
  const card = view?.card || {};
  return {
    fit, cellW, hue,
    char: route.char || '', maybe: !!route.maybe,
    reason: !!card.reason && !route.reason,   // 排除原因：已经按原因筛选时不写
    audit: !!card.audit, dup: !!card.dup,
    hints: !!hints, hintMin: card.hintMin ?? CHIPS_MIN,
  };
}

// 重复页的角标颜色（#386 的 2-14）：同一组同色，列表里相邻的两组一定不同色。每组先按组的键取 12 种色相之一，和前一组撞色时
// 往后错开 5 档；前一组要看它自己的颜色，所以从这一段连续读到的第一张往后推。items 是网格的列表，按 version（数据版本）缓存，
// 新读到一页时重算。返回色相的角度，写进 --dup-hue。
const hueOf = key => { let h = 0; for (const ch of key) h = (h * 31 + ch.charCodeAt(0)) >>> 0; return h % 12; };
const hues = {version: -1, at: []};
export function dupHue(items, version, i) {
  if (hues.version !== version) { hues.version = version; hues.at = []; }
  const at = hues.at, grp = k => items[k]?.dup?.grp;
  let j = i; while (j > 0 && at[j] === undefined && grp(j - 1) !== undefined) j--;
  for (let k = j; k <= i; k++) {
    if (at[k] !== undefined) continue;
    const prev = k > 0 && grp(k - 1) !== undefined ? at[k - 1] : undefined, own = hueOf(grp(k));
    at[k] = grp(k) === grp(k - 1) && prev !== undefined ? prev : own === prev ? (own + 5) % 12 : own;
  }
  return at[i] * 30;
}
// 卡片的说明：读屏和悬停提示用。
export const cardLabel = (item, i) => t('{names}，第 {n} 张', {names: item.memberships.map(m => m.name).join(t('、')) || t('未识别图片'), n: i + 1});

// 左下的芯片：随卡片内容变，个数少，整排重建（只在内容变了时）。
function chips(item, ctx) {
  const out = [], chip = (label, title = '') => { const b = make('span', 'card-chip', label); if (title) b.title = title; return b; };
  if (ctx.char && ctx.cellW >= CHIPS_MIN) {
    const m = item.memberships.find(x => x.id === ctx.char);
    if (!ctx.maybe) for (const v of m?.variants || []) out.push(chip(v.name));
    else out.push(chip(item.memberships.length ? t('现为 {name}', {name: short(item.memberships[0].name)}) : t('未识别')));
  }
  if (ctx.reason && item.excluded && ctx.cellW >= CHIPS_MIN) out.push(chip(REASONS[item.excluded] || ''));
  // 随机抽查是在小格子里快速过的：角色标签一直留到 70px 的格子。
  if (ctx.audit && item.audit && ctx.cellW >= SMALL) out.push(...auditTags(item));
  if (ctx.hints && !item.memberships.length && item.hint && ctx.cellW >= ctx.hintMin) {
    const b = make('button', 'hint-add', `+ ${short(item.hint.name)}`);
    b.type = 'button'; b.dataset.hint = ''; b.title = t('可能是 {name}，点一下设为 TA', {name: item.hint.name});
    out.push(b);
  }
  return out;
}
// 随机抽查：图上的角色标签，正在抽查的那个排在最前并着色。悬停在标签上出现 ✓ / ✗：在抽查的标签上回答抽查，在别的标签上
// 确认或去掉那个角色（点击由网格接，看 data-tagact、data-entity）。
function auditTags(item) {
  const a = item.audit, mark = {yes: '✓', no: '✗'}[a.answer] || '';
  const tag = (id, name, kind, title, checked) => {
    const box = make('span', `atag ${kind}`), label = make('b');
    box.title = title;
    if (checked && mark) label.append(make('i', '', mark));
    label.append(short(name));
    box.append(label);
    for (const [act, sign, tip] of [['yes', '✓', t('是 {name}', {name})], ['no', '✗', t('不是 {name}', {name})]]) {
      const b = make('button', '', sign); b.type = 'button'; b.dataset.tagact = act; b.dataset.entity = id; b.title = tip; box.append(b);
    }
    return box;
  };
  const asked = a.answer ? (a.answer === 'yes' ? t('抽查已答：是') : t('抽查已答：不是')) : t('抽查：是 TA 吗？');
  return [tag(a.id, a.name, `checked ${a.answer || 'open'}`, asked, true),
    ...item.memberships.filter(m => m.id !== a.id).map(m => m.status === 'confirmed' ? tag(m.id, m.name, 'confirmed', t('已确认')) : tag(m.id, m.name, '', t('机器识别')))];
}
// 芯片的内容由这些决定；一样就不重建。
const chipsKey = (item, ctx) => JSON.stringify([ctx.char, ctx.maybe, ctx.reason, ctx.audit, ctx.hints, ctx.cellW >= CHIPS_MIN, ctx.cellW >= SMALL, ctx.cellW >= ctx.hintMin,
  item.memberships.map(m => [m.id, m.name, m.status, (m.variants || []).map(v => v.name)]), item.excluded || '', item.audit || null, item.hint || null]);

/**
 * 一张卡片。scope 是这张卡片自己的作用域（网格给每张卡片建一个子作用域，摘掉时释放）。
 * @returns {{el: HTMLElement, update: Function, mark: Function, flip: Function, leave: (on: boolean) => void, park: Function, dispose: Function}}
 */
export function gridCard(scope) {
  const {el, refs} = CARD();
  refs.fav.append(icon('heart'));
  let item = null, key = '', tint, thumb;   // 上一次写的芯片内容、主色、登记的图：没变就不再写
  // 图读坏了：记在根节点上，样式藏起图、露出「图片暂不可读」。换了别的图再去掉。
  scope.on(refs.img, 'error', () => attr(el, 'data-broken', true));

  let wantedImage = '', previewImage = '';
  function showImage(src) {
    if (wantedImage === src) return;
    wantedImage = src;
    const upgrading = previewImage && src !== previewImage && refs.img.getAttribute('src') === previewImage;
    if (!upgrading || !refs.img.complete || !refs.img.naturalWidth) {
      attr(refs.img, 'src', src);
      if (src !== previewImage) previewImage = '';
      return;
    }
    // 标准档解码好才替换已显示的小图。卡片换图或回备用池后丢弃晚到的结果，读取失败时仍显示小图。
    const full = new Image();
    full.src = src;
    full.decode().then(() => {
      if (wantedImage !== src) return;
      previewImage = '';
      attr(refs.img, 'src', src);
    }, () => { if (wantedImage === src) wantedImage = ''; });
  }

  function update({item: next, index, box, ctx, preview}) {
    const changed = next !== item;
    item = next;
    attr(el, 'data-id', item.uuid);
    attr(el, 'data-index', index);
    attr(el, 'aria-label', cardLabel(item, index));
    css(el, 'transform', `translate(${box.x}px, ${box.y}px)`);
    css(el, 'width', `${box.w}px`);
    css(el, 'height', `${box.h}px`);
    if (item.color !== tint) { tint = item.color; setTint(el, tint); }
    // 原始比例：宽高比超出上下限的图在方框里整张显示（留底色），其余裁满。方格总是裁满。
    const raw = aspect(item);
    cls(el, 'is-whole', ctx.fit && raw > 0 && clampRatio(raw) !== raw);
    const standardSrc = imageUrl(item.uuid, thumbSize(box.w)), previewSrc = imageUrl(item.uuid, 'thumb');
    const shown = refs.img.getAttribute('src');
    if (previewSrc === standardSrc) previewImage = '';
    const startPreview = preview === 'fast' && shown !== standardSrc;
    const keepPreview = preview === 'offscreen' && previewImage === previewSrc;
    const usePreview = previewSrc !== standardSrc && (startPreview || keepPreview);
    if (usePreview) previewImage = previewSrc;
    const src = usePreview ? previewSrc : standardSrc;
    if (changed || shown !== src) attr(el, 'data-broken', null);
    showImage(src);
    if (item.uuid !== thumb) { thumb = item.uuid; thumbs.mark(refs.img, thumb); }
    const k = chipsKey(item, ctx);
    if (k !== key) { key = k; refs.chips.replaceChildren(...chips(item, ctx)); }
    // 悬停时写的名字：一位写名字；几位写第一位和「另外 N 位」，窄卡只写「N 位」。左下有芯片时不写（芯片也在底边，
    // 而且已经写了这一页要看的信息）。
    const names = item.memberships, many = names.length > 1, wide = box.w >= NAMES_MIN;
    show(refs.info, names.length > 0 && box.w >= INFO_MIN && !refs.chips.children.length);
    text(refs.who, !names.length ? '' : many && !wide ? t('{n} 位', {n: number(names.length)}) : names[0].name);
    text(refs.more, many && wide ? t('另外 {n} 位', {n: number(names.length - 1)}) : '');
    attr(refs.info, 'title', names.length ? names.map(nameWithWork).join(t('、')) : null);
    // 副本：重复页上同组同色、写「第几份 / 共几份」；别的页上折叠的副本写「×N」。
    const dup = item.dup, dupShown = !!dup && (ctx.dup || ctx.cellW >= SMALL);
    show(refs.dup, dupShown);
    cls(refs.dup, 'folded', dupShown && !ctx.dup);
    text(refs.dup, !dupShown ? '' : ctx.dup ? `${dup.pos}/${dup.size}` : `×${dup.size}`);
    css(refs.dup, '--dup-hue', dupShown && ctx.dup ? ctx.hue(index) : null);
    attr(refs.dup, 'title', !dupShown ? null : ctx.dup ? t('同一组共 {n} 份副本', {n: dup.size}) : t('共 {n} 份相同副本，操作会同步到全部', {n: dup.size}));
    // 角色页上，这张图上的这个角色还是机器识别的、没人看过。
    show(refs.dot, !!ctx.char && item.memberships.find(m => m.id === ctx.char)?.status === 'machine');
    // 心形：收藏过的一直显示；格子太小时只显示实心、不能点。
    const fav = !!item.fav, small = ctx.cellW < SMALL;
    attr(refs.fav, 'data-fav', item.uuid);
    cls(refs.fav, 'on', fav); cls(refs.fav, 'still', small);
    attr(refs.fav, 'aria-pressed', fav ? 'true' : 'false');
    attr(refs.fav, 'title', small ? null : heartTip(fav));
    attr(refs.fav, 'aria-label', small ? null : t('收藏', {context: 'verb'}));
    attr(refs.fav, 'aria-hidden', small ? 'true' : null);
    attr(refs.fav, 'tabindex', small ? '-1' : null);
    show(refs.fav, !small || fav);
  }

  return {
    el, update,
    mark({selected, focused, cursor}) {
      attr(el, 'aria-pressed', selected ? 'true' : 'false');
      attr(el, 'aria-current', focused ? 'true' : null);
      cls(el, 'is-cursor', cursor);
    },
    // 缩放时从旧的框滑到新的框（屏幕坐标）：动里面的 .card-stage，根节点的 transform 是网格写的位置。
    flip(from, to) { return flip(refs.stage, from, to); },
    // 写入后要离开本页：先淡出（写入失败时 leave(false) 收回）。
    leave(on) { cls(el, 'leaving', on); },
    // 滚出视野、留着下次复用（ui/grid.js 的备用卡片）：去掉图片地址（放掉解码后的位图），下一次 update 当作新的一张来画。
    park() { wantedImage = previewImage = ''; attr(refs.img, 'src', null); item = null; cls(el, 'leaving', false); },
    dispose() { wantedImage = previewImage = ''; releaseImages(el); scope.dispose(); },
  };
}

// 还没读到的那几张：只是一块占位，按序号复用。
export function skeletonCard() {
  const {el} = SKELETON();
  return {el, update({index, box}) {
    attr(el, 'data-index', index);
    css(el, 'transform', `translate(${box.x}px, ${box.y}px)`);
    css(el, 'width', `${box.w}px`);
    css(el, 'height', `${box.h}px`);
  }, mark() {}, leave() {}, park() {}, dispose() {}};
}
