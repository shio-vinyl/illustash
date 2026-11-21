// 详情卡片的部件（0.4 第 D 步，#709）：图片、玻璃按钮组、角色身份、提问、角色卡、属性列表和各小节、底部操作。
// 只画，不发请求、不改 state：数据由 ui/detail.js 经 core/detail.js 读好传进来，点了什么交给 on 里的回调。
// 节点建一次，之后 update(d, opts) 只经 ui/el.js 的写入函数改变了的地方（翻页、写入后重读都不重建卡片）；
// 几个小列表（造型、标签、副本、柱子）按内容的签名在变了时重画那一小块，按钮的点击在卡片根上统一接（data-act）。
//
//   const card = detailCard({act(name, data, event, d), artist: {verdict, find, findMore}});   // 见 ui/detail.js 的 act、mount
//   card.loading(uuid);                         // 换图：先换图片，正文等数据（这期间改数据的按钮不响应）
//   card.update(d, {ask, chart, same, meta, skip, resumeKey});
//   card.stats(st, opts, ident, charId);        // 角色每月张数到了（core/detail.js 的 readCharStats）；st 为 null 时藏起来
//   card.fav(on) / card.failed(message)
//
// 大图底栏（ui/viewer.js）用这里的 identities({compact: true, scope}) 和 askBar()。
import {t} from '../core/i18n.js';
import {imageUrl as img, number, thumbSize} from '../core/format.js';
import {evidence, fromExt, howFound, primaryOf, askWhy} from '../core/detail.js';
import {REASONS, nameWithWork, workLabel} from '../core/labels.js';
import {unreadableText} from '../core/jobs.js';
import {charHash, filterRoute, hashFor} from '../core/route.js';
import {state} from '../core/state.js';
import {artistBlock} from './artists.js';
import {canReveal, revealLabel} from '../core/files.js';
import {attr, cls, make, refill, rich, show, text, tpl} from './el.js';
import {monthBars} from './month_bars.js';
import {keyed} from './keyed.js';
import {app} from './scope.js';
import {setTint} from './tint.js';

// 角色的圆头像：资料库里的封面（/api/characters 的 cover）。
const avatarOf = id => state.charMap.get(id)?.cover || '';
function button(className, label, data = {}, title = '') {
  const b = make('button', className); b.type = 'button';
  if (label) b.append(label);
  for (const [k, v] of Object.entries(data)) b.dataset[k] = v;
  if (title) b.title = title;
  return b;
}
const kbd = key => make('kbd', '', key);
// 详情还没读到时也能用的按钮：只要知道是哪张图（翻页、收藏、放大、关闭、打开文件）。
const BY_ID = new Set(['nav', 'fav', 'expand', 'close', 'file']);
/** 换图后详情还没读回时，等这么久才把正文变淡（读得快时不闪）。和 --dur-1 一档，「减少动态效果」时也照样等。 */
export const DIM_MS = 140;
// 已排除的原因，自动规则排除的加一句。
const excludedNote = d => t('已排除 · {reason}', {reason: REASONS[d.excluded.reason] || ''}) + (d.excluded.origin === 'rule' ? t('（自动规则）') : '');

// ---------- 提问条：「是 TA 吗？」 ----------
const ASK = tpl`<div class="dc-ask"><div class="dc-ask-q"><span data-ref="q"></span><small class="text-meta" data-ref="why"></small></div>
  <div class="ask-btns"><button type="button" class="btn is-small is-primary" data-act="ask" data-how="yes"><span data-t>是</span> <kbd>Y</kbd></button>
  <button type="button" class="btn is-small" data-act="ask" data-how="no"><span data-t>不是</span> <kbd>N</kbd></button>
  <button type="button" class="btn is-small" data-act="ask" data-how="switch" data-ref="rival"></button></div></div>`;
/** 提问条；update(q) 换内容，q 为空时藏起来。点击在外层容器上按 data-act="ask" 接。 */
export function askBar() {
  const {el, refs} = ASK();
  return {el, dispose() {}, update(q) {
    show(el, !!q); if (!q) return;
    const sig = JSON.stringify([q.entity, q.name]);
    refill(refs.q, sig, () => {
      const who = make('b', '', q.name), w = workLabel({id: q.entity, name: q.name});
      if (w) who.append(make('small', 'char-work', w));
      return [rich(t('是 {name} 吗？', {name: '{name}'}), {name: who})];
    });
    text(refs.why, askWhy(q)); show(refs.why, !!askWhy(q));
    show(refs.rival, !!q.rival);
    text(refs.rival, q.rival ? t('是 {name}', {name: q.rival_name}) : '');
    attr(refs.rival, 'title', q.rival ? t('换成 {name}', {name: q.rival_name}) : null);
  }};
}

// ---------- 角色身份 ----------
const WHO = tpl`<div class="dc-who">
  <div class="dc-lead" data-ref="lead"><a class="dc-avatar" data-ref="avatarLink" tabindex="-1"><img class="avatar" data-ref="avatar" alt=""></a>
    <div class="dc-lead-text"><h2 class="dc-name"><a data-ref="name" title="在图库里只看 TA" data-t-attr="title"></a></h2>
      <p class="text-meta" data-ref="meta"></p><p class="text-meta" data-ref="orig"></p></div>
    <button type="button" class="dc-icon" data-act="not" data-ref="not" title="不是 TA（⌫）" data-t-attr="title" aria-label="不是 TA（⌫）">×</button></div>
  <div class="dc-lead is-none" data-ref="none"><span class="avatar dc-avatar-none" aria-hidden="true"></span><p class="text-meta" data-ref="noneText"></p></div>
  <div class="dc-variants" data-ref="variants"></div>
  <p class="text-meta dc-note" data-ref="note"></p>
  <div class="dc-acts" data-ref="acts">
    <button type="button" class="btn is-small is-primary" data-act="confirm" data-ref="confirm"><span data-t>没错</span></button>
    <button type="button" class="btn is-small" data-act="swap" data-ref="swap" title="换成…（C）" data-t-attr="title"><span data-t>换一个…</span> <kbd>C</kbd></button>
    <button type="button" class="btn is-small" data-act="also" data-ref="set" title="设为角色（C）" data-t-attr="title"><span data-t>设为角色…</span> <kbd>C</kbd></button>
    <button type="button" class="btn is-small is-quiet" data-act="like" data-ref="like"><span data-t>更多像这张的</span></button></div>
  <div class="dc-others" data-ref="others"></div>
  <button type="button" class="dc-also" data-act="also" data-ref="also" title="追加角色，原有的保留（A）" data-t-attr="title">＋ <span data-t>也是…</span> <kbd>A</kbd></button>
  <div class="dc-alts" data-ref="altsBox"><h3 class="dc-label" data-t>也可能是</h3><div class="dc-chips" data-ref="alts"></div></div>
  <div class="dc-tagids dc-chips" data-ref="tagIds"></div></div>`;
const OTHER = tpl`<div class="dc-other"><img class="avatar is-small" data-ref="avatar" alt=""><a class="dc-other-name" data-ref="name"></a>
  <span class="dc-other-acts"><button type="button" class="dc-icon" data-act="swap" data-ref="swap">⇄</button>
  <button type="button" class="dc-icon" data-act="not" data-ref="not" title="不是 TA（⌫）" data-t-attr="title">×</button></span>
  <div class="dc-variants" data-ref="variants"></div></div>`;

// 名字后面的「扩展」小字（只由角色数据包给出的项，#550）。
const extMark = e => fromExt(e) ? [make('small', 'dc-ext', t('扩展'))] : [];
// 这个角色的造型芯片和「＋ 造型」：芯片的 × 去掉造型。
function fillVariants(node, m, compact) {
  const own = m.variants || [], all = state.charMap.get(m.id)?.variants || [];
  const more = !compact && all.some(v => !own.some(x => x.id === v.id));
  refill(node, JSON.stringify([m.id, own.map(v => [v.id, v.name]), more, compact]), () => [
    ...(compact ? [] : own.map(v => { const c = make('span', 'chip is-variant', v.name); c.append(button('dc-chip-x', '×', {act: 'unvariant', id: v.id}, t('去掉造型'))); return c; })),
    ...(more ? [button('chip is-add', `＋ ${t('造型')}`, {act: 'addVariant', id: m.id})] : []),
  ]);
  show(node, (!compact && own.length) || more);
}
function setAvatar(node, id) {
  const cover = avatarOf(id);
  attr(node, 'src', cover ? img(cover, 'thumb') : null);
}
// 按位置复用的一组行：只增不删，用不上的用类名藏起来（不切 hidden 特性，见 ui/viewer.js 的 arrange）。
function pool(node, create) {
  const rows = [];
  return {set(items) {
    while (rows.length < items.length) { const row = create(); rows.push(row); node.append(row.el); }
    rows.forEach((row, i) => { cls(row.el, 'is-off', i >= items.length); if (i < items.length) row.update(items[i]); });
  }};
}
// 另一个角色（或大图底栏里的每一个角色）。
function otherRow(compact) {
  const {el, refs} = OTHER();
  return {el, dispose() {}, update({m, primary}) {
    attr(el, 'data-id', m.id);
    setAvatar(refs.avatar, m.id);
    text(refs.name, m.name); attr(refs.name, 'href', charHash(m.id)); attr(refs.name, 'title', `${howFound(m)} · ${evidence(m.evidence)} · ${nameWithWork(m)}`);
    cls(el, 'is-ext', m.status === 'machine' && fromExt(m.evidence));
    cls(el, 'is-machine', m.status === 'machine');
    attr(refs.swap, 'data-id', m.id); attr(refs.not, 'data-id', m.id);
    attr(refs.swap, 'title', primary ? t('换成…（C）') : t('换成…'));
    fillVariants(refs.variants, m, compact);
  }};
}

/**
 * 角色身份块。compact（大图底栏）：不画主角色的大头像和按钮组，所有角色都是一行。
 * 点击都在外层按 data-act 接：confirm、swap、not、also、like、unvariant、addVariant、alt（data-i）。
 */
export function identities({compact = false, scope: parent = app} = {}) {
  const {el, refs} = WHO(), s = parent.child('identities');
  cls(el, 'is-compact', compact);
  // 大图底栏的行只增不删：按位置复用，多出来的藏起来，翻图时行和头像 <img> 留着、只换内容（#683 的翻页测量）。
  // 卡片里按角色复用：点了某一行的按钮以后，焦点不会留在换成别的角色的同一位置上。
  const others = compact ? pool(refs.others, () => otherRow(true))
    : keyed(refs.others, {key: x => x.m.id, create: x => { const row = otherRow(false); row.update(x); return row; }, scope: s});
  return {el, dispose: () => s.dispose(), update(d) {
    const primary = primaryOf(d), pc = primary && (state.charMap.get(primary.id) || primary);
    show(refs.lead, !compact && !!primary); show(refs.none, !primary); show(refs.acts, !compact);
    if (primary && !compact) {
      setAvatar(refs.avatar, primary.id); attr(refs.avatarLink, 'href', charHash(primary.id));
      text(refs.name, primary.name); attr(refs.name, 'href', charHash(primary.id));
      refill(refs.name.parentNode, JSON.stringify([primary.id, primary.name, workLabel(pc), fromExt(primary.evidence) && primary.status === 'machine']), () => {
        const w = workLabel(pc);
        return [refs.name, ...(w ? [make('small', 'char-work', w)] : []), ...(primary.status === 'machine' ? extMark(primary.evidence) : [])];
      });
      const count = state.charMap.get(primary.id)?.count;
      text(refs.meta, [howFound(primary), primary.status === 'machine' ? evidence(primary.evidence) : '', count ? t('共 {n} 张', {n: number(count)}) : ''].filter(Boolean).join(' · '));
      const original = pc?.tag && pc.tag !== pc.name ? pc.tag.replaceAll('_', ' ') : '';
      text(refs.orig, original); show(refs.orig, !!original);
      attr(refs.not, 'data-id', primary.id);
      fillVariants(refs.variants, primary, false);
    } else { show(refs.variants, false); text(refs.name, ''); }   // 名字清掉：藏着的上一张的名字别被读屏或走查读到
    // 还没有角色：卡片上写「还没认出是谁」（不当标题），大图底栏写「还没有角色」。
    text(refs.noneText, compact ? (d.unknown ? t('已跳过，还没有角色') : t('还没有角色')) : d.unknown ? t('已跳过') : t('还没认出是谁'));
    const note = d.excluded ? excludedNote(d) : d.pending ? t('待确认：像截图') : '';
    text(refs.note, note); show(refs.note, !compact && !!note);
    // 主角色是机器认出的才有「没错」；还没有角色时「设为角色…」代替「换一个…」。
    show(refs.confirm, primary?.status === 'machine'); attr(refs.confirm, 'data-id', primary?.id);
    show(refs.swap, !!primary); attr(refs.swap, 'data-id', primary?.id);
    show(refs.set, !primary);
    others.set(d.memberships.filter(m => compact || m !== primary).map(m => ({m, primary: m === primary})));
    show(refs.also, d.memberships.length > 0 || compact);
    refill(refs.also, String(!!d.memberships.length), () => ['＋ ', d.memberships.length ? t('也是…') : t('设为角色…'), ' ', kbd(d.memberships.length ? 'A' : 'C')]);
    attr(refs.also, 'title', d.memberships.length ? t('追加角色，原有的保留（A）') : t('设为角色（C）'));
    const alts = (d.alternatives || []).slice(0, 9);
    show(refs.altsBox, alts.length > 0);
    refill(refs.alts, JSON.stringify(alts.map(a => [a.id, a.name, a.weak, a.evidence])), () => alts.map((a, i) => {
      const b = button(`chip dc-alt${a.weak ? ' is-weak' : ''}`, '', {act: 'alt', i: String(i)},
        `${nameWithWork(a)} · ${a.weak ? t('弱线索（中档相似，不算归属）') + ' · ' : ''}${t('{evidence}。点击：换成这个；⌥ 点击：也是', {evidence: evidence(a.evidence)})}`);
      b.append(kbd(String(i + 1)), a.name, ...extMark(a.evidence));
      return b;
    }));
    const tagIds = compact ? [] : d.tag_identities || [];
    show(refs.tagIds, tagIds.length > 0);
    refill(refs.tagIds, JSON.stringify(tagIds.map(x => [x.id, x.name])), () => [make('span', 'dc-label', t('标签')), ...tagIds.map(x => {
      const a = make('a', 'chip', x.name); a.href = charHash(x.id); a.title = t('只当标签、不算角色的身份'); return a;
    })]);
  }};
}

// ---------- 卡片 ----------
const CARD = tpl`<article class="dc is-empty" aria-busy="false">
  <div class="dc-art"><button type="button" class="dc-preview" data-ref="preview" data-act="expand" title="大图查看" data-t-attr="title">
    <img class="dc-img" id="detailImg" data-ref="img" alt="插画预览" data-t-attr="alt" draggable="false"></button>
    <p class="dc-missing" data-ref="missing" hidden></p>
    <div class="glass-bar dc-bar is-start">
      <button type="button" data-act="nav" data-step="-1" aria-label="上一张" title="上一张（←）" data-t-attr="aria-label title">
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m12 4.5-5.5 5.5 5.5 5.5"/></svg></button>
      <button type="button" data-act="nav" data-step="1" aria-label="下一张" title="下一张（→）" data-t-attr="aria-label title">
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m8 4.5 5.5 5.5L8 15.5"/></svg></button></div>
    <div class="glass-bar dc-bar is-end"><button type="button" class="fav-toggle" data-act="fav" data-ref="fav" aria-pressed="false">
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 16.4S3.4 12.5 3.4 7.9A3.3 3.3 0 0 1 10 6.3a3.3 3.3 0 0 1 6.6 1.6c0 4.6-6.6 8.5-6.6 8.5Z"/></svg></button>
      <button type="button" data-act="expand" aria-label="大图" title="大图（Space）" data-t-attr="aria-label title">
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M12 3.5h4.5V8M8 16.5H3.5V12M16.5 3.5 11 9M3.5 16.5 9 11"/></svg></button>
      <button type="button" data-act="close" aria-label="关闭详情" title="关闭（Esc）" data-t-attr="aria-label title">
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m5 5 10 10M15 5 5 15"/></svg></button></div></div>
  <div class="dc-body" data-ref="body"><p class="dc-status text-meta" data-ref="status" hidden></p>
    <div data-ref="askSlot"></div><div data-ref="whoSlot"></div>
    <div class="dc-stats" data-ref="stats" hidden><p class="dc-stat-line"><span data-ref="total"></span><em data-ref="now"></em></p><div class="dc-bars" data-ref="bars"></div>
      <div class="dc-same" data-ref="same"><h3 class="dc-label"><span data-t>同一角色</span><a data-ref="sameAll"><span data-t>全部</span> ›</a></h3>
        <div class="dc-same-row" data-ref="sameRow"></div></div></div>
    <div data-ref="artistSlot"></div>
    <dl class="dc-props" data-ref="props"><dt data-t>尺寸</dt><dd data-ref="size"></dd><dt data-t>加入</dt><dd data-ref="added"></dd>
      <dt data-t>文件</dt><dd><span class="dc-file" data-ref="file"></span><button type="button" class="dc-link" data-act="file" data-kind="reveal" data-ref="reveal"></button></dd>
      <dt data-t>文件形态</dt><dd data-ref="kind"></dd></dl>
    <section class="dc-section"><h3 class="dc-label" data-t>我的标签</h3><div class="dc-chips" data-ref="utags"></div></section>
    <section class="dc-section" data-ref="dupBox"><h3 class="dc-label" data-ref="dupTitle"></h3><div class="dc-dups" data-ref="dups"></div></section>
    <details class="dc-section dc-fold" data-ref="tagBox"><summary data-t>画面标签</summary><div class="dc-chips" data-ref="tags"></div></details></div>
  <div class="dc-foot" data-ref="foot"></div></article>`;

/** 详情卡片。on 是 {act(name, data, event, d), artist: {verdict, find, findMore}}，见文件头和 ui/detail.js 的 mount。 */
export function detailCard(on) {
  const {el, refs} = CARD(), s = app.child('detail-card');
  const ask = askBar(), who = identities({scope: s}), artists = artistBlock(on.artist, s);
  refs.askSlot.replaceWith(ask.el); refs.whoSlot.replaceWith(who.el); refs.artistSlot.replaceWith(artists.el);
  s.add(ask);   // who、artists 的作用域是 s 的子作用域，跟着释放
  let cur = null, pending = null, wanted = '', shown = '', tint;
  const samePics = [];   // 「同一角色的几张」的四格，见 stats()
  // 点击统一在根上接：按钮写 data-act，其余数据在 data-* 上。
  // 换成另一张、它的详情还没读回来时（cur 为空），只认只要知道是哪张图的按钮；角色、标签这些要等数据到了，
  // 免得按着上一张的数据改这一张。
  s.on(el, 'click', e => {
    const b = e.target.closest?.('[data-act]'); if (!b || !el.contains(b)) return;
    if (b.tagName === 'A') e.preventDefault();
    if (!cur && !(pending && BY_ID.has(b.dataset.act))) return;
    on.act(b.dataset.act, b.dataset, e, cur || {uuid: pending});
  });
  s.on(refs.img, 'error', () => { if (wanted === refs.img.getAttribute('src')) failImage(); });
  function failImage() { show(refs.img, false); text(refs.missing, unreadableText()); show(refs.missing, true); }
  // 预览图：新图解码好了再换上去，换之前一直显示上一张（翻页不闪白）；第一次直接设。
  function preview(uuid) {
    const url = img(uuid, 'preview');
    if (wanted === url) return;
    wanted = url;
    const put = () => { if (wanted !== url) return; show(refs.img, true); show(refs.missing, false); attr(refs.img, 'src', url); shown = url; };
    const probe = shown && typeof Image === 'function' ? new Image() : null;
    if (typeof probe?.decode !== 'function') { put(); return; }
    probe.src = url;
    probe.decode().then(put, put);
  }
  const card = {
    el, scope: s, preview: refs.preview, img: refs.img,
    dispose: () => s.dispose(),
    get uuid() { return cur?.uuid ?? pending; },
    /** 开始读一张图：先换图，正文等数据。 */
    loading(uuid) {
      if (cur?.uuid !== uuid) cur = null;
      pending = uuid; artists.loading(); preview(uuid); attr(el, 'aria-busy', 'true'); show(refs.status, !shown); text(refs.status, t('读取图片详情…'));
      s.timeout(() => { if (pending === uuid) cls(el, 'is-dim', true); }, DIM_MS);
    },
    failed(message) { attr(el, 'aria-busy', 'false'); cls(el, 'is-dim', false); show(refs.status, true); text(refs.status, message); },
    /** 收藏按钮的样子。只由卡片自己写（不带 data-fav，ui/favorites.js 的 paintHearts 不碰它）：收藏写入后 ui/detail.js 的
     *  noteFavorite 调它。 */
    fav(on) {
      attr(refs.fav, 'aria-pressed', String(on)); attr(refs.fav, 'title', on ? t('取消收藏（F）') : t('收藏（F）'));
      attr(refs.fav, 'aria-label', t('收藏', {context: 'verb'}));
    },
    update(d, opts) {
      cur = d; pending = null; preview(d.uuid); attr(el, 'aria-busy', 'false'); show(refs.status, false);
      cls(el, 'is-dim', false); cls(el, 'is-empty', false);
      if (tint !== d.color) { tint = d.color; setTint(el, d.color); }
      card.fav(!!d.fav);
      ask.update(opts.ask ? d.question : null);
      who.update(d);
      artists.update(d);
      // 属性
      show(refs.props, opts.meta);
      text(refs.size, `${number(d.width)} × ${number(d.height)}`);
      text(refs.added, (d.added_at || '').slice(0, 16).replace('T', ' '));
      text(refs.file, d.filename); attr(refs.file, 'title', d.filename);
      show(refs.reveal, canReveal()); text(refs.reveal, canReveal() ? revealLabel() : '');
      text(refs.kind, d.src_kind === 'deriv' ? t('本地副本') : t('原图'));
      // 我的标签
      const utags = d.user_tags || [];
      refill(refs.utags, JSON.stringify(utags), () => [...utags.map(tag => {
        const chip = make('span', 'chip is-variant'), a = make('a', '', `# ${tag}`);
        a.href = hashFor(filterRoute({utag: tag})); a.dataset.act = 'utag'; a.dataset.tag = tag;
        chip.append(a, button('dc-chip-x', '×', {act: 'untag', tag}, t('去掉标签')));
        return chip;
      }), (() => { const b = button('chip is-add', `＋ ${t('标签')} `, {act: 'addTag'}); b.append(kbd('G')); return b; })()]);
      // 重复副本
      const dups = d.duplicates || [];
      show(refs.dupBox, dups.length > 0);
      text(refs.dupTitle, dups.length ? t('重复副本 · 共 {n} 份', {n: dups.length + 1}) : '');
      refill(refs.dups, JSON.stringify(dups.map(x => [x.uuid, x.pos, x.size, x.added_at, x.names])), () => dups.map(x => {
        const b = button('dc-dup', '', {act: 'copy', id: x.uuid}, t('查看这份副本')), pic = make('img');
        pic.src = img(x.uuid, thumbSize(55)); pic.alt = ''; pic.loading = 'lazy';
        const words = make('span'), small = make('small', '', x.names.length ? x.names.join(' · ') : t('未识别'));
        words.append(make('b', '', `${x.pos}/${x.size}`), ` ${(x.added_at || '').slice(0, 10)}`, make('br'), small);
        b.append(pic, words); return b;
      }));
      // 画面标签
      show(refs.tagBox, opts.meta);
      refill(refs.tags, JSON.stringify((d.tags || []).map(x => x.value)), () => d.tags?.length ? d.tags.map(x =>
        button('chip', x.value.replaceAll('_', ' '), {act: 'tag', tag: x.value}, t('按此标签筛选图库 · WD {conf}', {conf: x.conf.toFixed(2)}))) : [make('p', 'text-meta', t('暂无'))]);
      // 底部：打开，加上整张图的那一两个决定（跳过、恢复、排除、恢复收录、保留）
      const foot = [{act: 'file', kind: 'open', label: t('打开'), title: t('用默认应用打开原图')}];
      if (opts.skip && !d.memberships.length && !d.unknown) foot.push({act: 'skip', label: t('跳过'), key: 'S'});
      if (d.unknown) foot.push({act: 'resume', label: t('恢复到未识别'), key: opts.resumeKey ? 'S' : ''});
      if (d.excluded) foot.push({act: 'curate', curate: 'include', label: t('恢复收录'), title: excludedNote(d)});
      else if (d.pending) foot.push({act: 'curate', curate: 'exclude', label: t('排除'), key: 'X'}, {act: 'curate', curate: 'include', label: t('保留')});
      else foot.push({act: 'curate', curate: 'exclude', label: t('排除'), key: 'X', title: t('截图、照片等不是插画的图')});
      refill(refs.foot, JSON.stringify(foot), () => foot.map(f => {
        const data = {act: f.act}; if (f.kind) data.kind = f.kind; if (f.curate) data.curate = f.curate;
        const b = button('btn', f.label, data, f.title || '');
        if (f.key) b.append(' ', kbd(f.key));
        return b;
      }));
    },
    /** 角色卡：每月张数的柱子和同一角色的几张。st 为空时藏起来。 */
    stats(st, opts, ident, charId) {
      show(refs.stats, !!st && (opts.chart || opts.same)); if (!st) return;
      const months = st.months || [], now = months.at(-1)?.count || 0;
      show(refs.total.parentNode, opts.chart); show(refs.bars, opts.chart);
      refill(refs.total, JSON.stringify([st.count, st.first]), () => [rich(t('共 {n} 张', {n: '{n}'}), {n: make('b', '', number(st.count))}),
        ...(st.first ? [' · ' + t('始于 {date}', {date: st.first.replace('-', '.')})] : [])]);
      text(refs.now, now ? t('本月 +{n}', {n: number(now)}) : ''); show(refs.now, !!now);
      monthBars(refs.bars, months, {mark: (m, i) => i === months.length - 1 ? 'is-now' : ''});
      const recent = (st.recent || []).filter(u => u !== ident).slice(0, 4);
      show(refs.same, opts.same && recent.length > 0); attr(refs.sameAll, 'href', charHash(charId));
      // 固定四格，建一次以后只换 src：翻图时不删、不重建已经显示着的缩略图。
      if (!samePics.length) {
        for (let i = 0; i < 4; i++) {
          const b = button('dc-same-pic', '', {act: 'copy'}, t('查看这张')), pic = make('img');
          pic.alt = ''; pic.loading = 'lazy'; b.append(pic); samePics.push({b, pic});
        }
        refs.sameRow.replaceChildren(...samePics.map(x => x.b));
      }
      samePics.forEach(({b, pic}, i) => {
        const u = recent[i]; show(b, !!u); if (!u) return;
        attr(b, 'data-id', u); attr(pic, 'src', img(u, thumbSize(85)));
      });
    },
  };
  return card;
}
