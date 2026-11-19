// 近看一张图（桌面布局，#199）：网格旁边的检查面板、偶尔出现的「是 TA 吗？」提问，以及全屏查看器。
import {esc, monthText, number, thumbSize} from '../core/format.js';
import {unreadableText} from '../core/jobs.js';
import {t} from '../core/i18n.js';
import {leaves} from '../core/filters.js';
import {REASONS, nameWithWork} from '../core/labels.js';
import {fetchChar} from '../core/meta.js';
import {detailAfterViewer, shownUuid} from '../core/modes.js';
import {keyScope, pageOf, poolIs, viewOf} from '../core/pages.js';
import {charHash, cleanup, filterRoute, go, hashFor} from '../core/route.js';
import {likeRoute} from '../core/like.js';
import {api} from '../core/session.js';
import {state} from '../core/state.js';
import {$, readFailure, workSmall} from './dom.js';
import {PAGE, currentItem, endPreview, favoriteOne, kept, loadPage, markDetailKeys, mutate, openTagPicker, previewList, previewing, renderGrid, rowHeight, rowOf, rowTop, setCursor, syncCards, toggleInfo, viewOpts} from './grid.js';
import {newIds, openPicker, targets} from './palette.js';
import {artistSection, findArtistWorks, findArtists} from './artists.js';
import {markHomeFocus} from './home.js';
import {bindFileActions, fileActions, wireDragOut} from './files.js';
import {createLayer} from './overlay.js';
import {MARKS} from '../core/favorites.js';
import {detailHeart, viewerHeart} from './favorites.js';

// 哪一次检查面板读取是当前的：更慢、更早的那次绝不会画到它上面。
let detailGen = 0;
// ---------- 检查面板 ----------
export function closeInspector() { detailGen++; if (state.viewer < 0) kept.hold = null; state.focused = null; state.detail = null; if (state.viewer < 0) state.question = null; $('inspector').hidden = true; markHomeFocus(); syncCards(); requestAnimationFrame(() => renderGrid()); }
function identityChips(d, compact = false) {
  // C 换掉的是主角色（primaryOf），所以只有它的 ⇄ 写着 C；A 是追加（#399）。
  const primary = primaryOf(d);
  const chips = d.memberships.map(m => `<div class="identity${m.status === 'machine' && fromExt(m.evidence) ? ' ext' : ''}"><span class="dot ${m.status}" title="${m.status === 'machine' ? t('机器识别') : m.origin === 'album' ? t('相簿') : t('人工确认')} · ${esc(evidence(m.evidence))}"></span><a class="identity-name" href="${charHash(m.id)}" title="${esc(nameWithWork(m))}">${esc(m.name)}</a>${m.status === 'machine' ? extMark(m.evidence) : ''}<span class="identity-actions"><button class="icon small" data-swap="${esc(m.id)}" title="${m === primary ? t('换成…（C）') : t('换成…')}">⇄</button><button class="icon small" data-not="${esc(m.id)}" title="${t('不是 TA（⌫）')}">×</button></span>
    ${compact ? '' : `<div class="variant-row">${(m.variants || []).map(v => `<span class="variant-chip">${esc(v.name)}<button data-unvariant="${esc(v.id)}" title="${t('去掉造型')}">×</button></span>`).join('')}${(state.charMap.get(m.id)?.variants || []).some(v => !(m.variants || []).some(x => x.id === v.id)) ? `<button class="variant-add" data-addvariant="${esc(m.id)}">＋ ${t('造型')}</button>` : ''}</div>`}</div>`).join('');
  const alts = (d.alternatives || []).slice(0, 9).map((a, i) => `<button class="alt${a.weak ? ' weak' : ''}${fromExt(a.evidence) ? ' ext' : ''}" data-alt="${i}" title="${esc(nameWithWork(a))} · ${a.weak ? t('弱线索（中档相似，不算归属）') + ' · ' : ''}${t('{evidence}。点击：换成这个；⌥ 点击：也是', {evidence: esc(evidence(a.evidence))})}"><kbd>${i + 1}</kbd>${esc(a.name)}${extMark(a.evidence)}</button>`).join('');
  return `<div class="identities">${chips || `<p class="none-yet">${d.unknown ? t('已跳过，还没有角色') : t('还没有角色')}</p>`}<button class="add-identity" data-also="1" title="${d.memberships.length ? t('追加角色，原有的保留（A）') : t('设为角色（C）')}">＋ ${d.memberships.length ? t('也是…') : t('设为角色…')} <kbd>${d.memberships.length ? 'A' : 'C'}</kbd></button></div>` +
    (alts ? `<div class="alts"><h3>${t('也可能是')}</h3><div class="alt-row">${alts}</div></div>` : '');
}
// PROTO = 从用户自己的裁决学到的原型（native/terms/src/research_compute/learn.rs）。WD_EXT、CCIP_LONGTAIL 是角色数据包
// （#550 WD 角色扩展的线性头、#559 CCIP 长尾包的锚点），后面跟它的分数（max_score），和「WD 0.91」并列。
const SOURCE_NAMES = {CCIP: t('外部索引'), PROTO: t('你的标注'), WD_EXT: t('WD 扩展'), CCIP_LONGTAIL: t('CCIP 长尾')};
// 数据包的来源和分数的小数位：WD 扩展是概率，CCIP 长尾是和外部索引同一种相似度。
const PACK_DIGITS = {WD_EXT: 2, CCIP_LONGTAIL: 3};
const sourceName = (s, e) => !(s in PACK_DIGITS) ? SOURCE_NAMES[s] || s : SOURCE_NAMES[s] + (e.max_score != null ? ` ${Number(e.max_score).toFixed(PACK_DIGITS[s])}` : '');
// 只由数据包给出的项：名字后面常显小字「扩展」（修饰类 .identity.ext、.alt.ext，#550 待决定 4），提示里只写数据包和它的分数。
export const fromExt = e => !!e?.sources?.length && e.sources.every(s => s in PACK_DIGITS);
// 归属行和「也可能是」里的项（alternatives 带 wd、ccip 两个键，归属行不带）：WD、外部索引或你的标注的分数，数据包的分数。
const evidence = e => !e ? '' : fromExt(e) ? e.sources.map(s => sourceName(s, e)).join(' + ') : [e.wd != null ? `WD ${e.wd.toFixed(2)}` : '',
  e.ccip != null ? `${e.sources?.includes('PROTO') && !e.sources.includes('CCIP') ? SOURCE_NAMES.PROTO : SOURCE_NAMES.CCIP} ${e.ccip.toFixed(3)}` : '',
  !('wd' in e) && e.sources?.length ? e.sources.map(s => sourceName(s, e)).join(' + ') : '',
  'wd' in e ? (e.sources || []).filter(s => s in PACK_DIGITS).map(s => sourceName(s, e)).join(' + ') : ''].filter(Boolean).join(' · ');
const extMark = e => fromExt(e) ? `<small>${t('扩展')}</small>` : '';
export const INSP_ICONS = {
  prev: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="m12 4.5-5.5 5.5 5.5 5.5"/></svg>',
  next: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="m8 4.5 5.5 5.5L8 15.5"/></svg>',
  expand: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="M12 3.5h4.5V8M8 16.5H3.5V12M16.5 3.5 11 9M3.5 16.5 9 11"/></svg>',
  close: '<svg viewBox="0 0 20 20" aria-hidden="true"><path d="m5 5 10 10M15 5 5 15"/></svg>',
};
// 这张图的角色每月加入多少张：按角色缓存，直到下一次写入或 NSFW 切换。
const statsCache = new Map();
// 详情栏里小图的显示高度（CSS 像素，app.css 的 .cc-same 四列和 .dup-copy img），按它选缩略图档位（#452）。
const SAME_H = 85, DUP_H = 55;
async function loadCharStats(id, ident, gen) {
  const box = $('charStats'); if (!box || (!viewOpts.chart && !viewOpts.same)) return;
  const key = `${id}|${state.nsfw}|${state.readEpoch}`;
  let st = statsCache.get(key);
  if (!st) { try { st = await api('/api/character/stats?' + new URLSearchParams({id, nsfw: state.nsfw, exclude: ident})); } catch { return; } statsCache.set(key, st); }
  if (gen !== detailGen || !$('charStats')) return;
  const months = st.months || [], max = Math.max(1, ...months.map(m => m.count)), now = months.at(-1)?.count || 0;
  const recent = (st.recent || []).filter(u => u !== ident).slice(0, 4);
  $('charStats').innerHTML = (viewOpts.chart ? `<div class="cc-line"><span>${t('共 {n} 张', {n: `<b>${number(st.count)}</b>`})}${st.first ? ' · ' + t('始于 {date}', {date: st.first.replace('-', '.')}) : ''}</span>${now ? `<em>${t('本月 +{n}', {n: number(now)})}</em>` : ''}</div>
    <div class="cc-bars">${months.map((m, i) => `<i style="height:${Math.max(8, m.count / max * 100)}%" class="${i === months.length - 1 ? 'now' : ''}" title="${monthText(m.month.slice(0, 4), m.month.slice(5, 7)) + ' · ' + t('{n} 张', {n: number(m.count)})}"></i>`).join('')}</div>` : '') +
    (viewOpts.same && recent.length ? `<div class="cc-same"><h5>${t('同一角色')}<a href="${charHash(id)}">${t('全部')} ›</a></h5><div>${recent.map(u => `<button data-copy="${esc(u)}" title="${t('查看这张')}"><img src="/image?id=${encodeURIComponent(u)}&size=${thumbSize(SAME_H)}" alt="" loading="lazy"></button>`).join('')}</div></div>` : '');
  for (const b of $('charStats').querySelectorAll('[data-copy]')) b.onclick = () => inspect(b.dataset.copy);
}
export async function inspect(ident, quiet = false) {
  const gen = ++detailGen; state.focused = ident; $('inspector').hidden = false; markHomeFocus();
  if (!quiet) $('inspector').innerHTML = `<div class="loading-line">${t('读取图片详情…')}</div>`;
  syncCards(); requestAnimationFrame(() => renderGrid());
  try {
    const d = await api('/api/detail?' + new URLSearchParams({id: ident, ...MARKS})); if (gen !== detailGen) return;
    state.detail = d;
    const primary = primaryOf(d), pc = primary && (state.charMap.get(primary.id) || primary);
    const original = pc?.tag && pc.tag !== pc.name ? pc.tag.replaceAll('_', ' ') : '';
    const path = pc ? (primary.variants || []).map(v => v.name) : [];
    const curate = d.excluded ? `<button class="secondary large" data-curate="include" title="${t('已排除 · {reason}', {reason: esc(REASONS[d.excluded.reason] || '')})}${d.excluded.origin === 'rule' ? t('（自动规则）') : ''}">${t('恢复收录')}</button>`
      : d.pending ? `<button class="secondary large" data-curate="exclude">${t('排除')} <kbd>X</kbd></button><button class="secondary large" data-curate="include">${t('保留')}</button>`
      : `<button class="secondary large" data-curate="exclude" title="${t('截图、照片等不是插画的图')}">${t('排除')} <kbd>X</kbd></button>`;
    const pendingNote = d.excluded ? `<div class="cc-note">${t('已排除 · {reason}', {reason: esc(REASONS[d.excluded.reason] || '')})}</div>` : d.pending ? `<div class="cc-note">${t('待确认：像截图')}</div>` : '';
    $('inspector').innerHTML = `<div class="inspector-head"><button class="icon" data-nav="-1" aria-label="${t('上一张')}" title="${t('上一张（←）')}">${INSP_ICONS.prev}</button><button class="icon" data-nav="1" aria-label="${t('下一张')}" title="${t('下一张（→）')}">${INSP_ICONS.next}</button><div class="spacer"></div>${detailHeart(ident, !!d.fav)}<button class="icon" id="expand" title="${t('大图（Space）')}" aria-label="${t('大图')}">${INSP_ICONS.expand}</button><button class="icon" id="closeInspector" aria-label="${t('关闭详情')}" title="${t('关闭（Esc）')}">${INSP_ICONS.close}</button></div>
      <button class="preview" id="previewButton" title="${t('大图查看')}"><img src="/image?id=${encodeURIComponent(ident)}&size=preview" alt="${t('插画预览')}" draggable="false"></button>
      ${shouldAsk(d, ident) ? askHTML(d.question) : ''}
      <section class="inspector-section"><div class="chips-row"><button class="quiet small" id="likeOne">${t('更多像这张的')}</button></div></section>
      ${pc ? `<section class="inspector-section cc-card"><a class="cc-name" href="${charHash(pc.id)}" title="${t('在图库里只看 TA')}">${esc(pc.name)}${workSmall(pc)}</a>${original ? `<div class="cc-orig">${esc(original)}</div>` : ''}${path.length ? `<div class="cc-path">${path.map(esc).join(' · ')}</div>` : ''}${pendingNote}
        <div id="charStats" class="cc-stats"></div></section>`
        : `<section class="inspector-section cc-card"><div class="cc-name muted">${d.unknown ? t('已跳过') : t('还没认出是谁')}</div>${pendingNote}</section>`}
      <section class="inspector-section"><h3 class="section-title">${t('角色')}</h3>${identityChips(d)}${(d.tag_identities || []).length ? `<div class="tag-ids"><span>${t('标签')}</span>${d.tag_identities.map(x => `<a class="variant-chip" href="${charHash(x.id)}" title="${t('只当标签、不算角色的身份')}">${esc(x.name)}</a>`).join('')}</div>` : ''}</section>
      ${artistSection(d)}
      <section class="inspector-section"><h3 class="section-title">${t('我的标签')}</h3><div class="user-tags">${(d.user_tags || []).map(tag => `<span class="variant-chip"><a href="${hashFor(filterRoute({utag: tag}))}" data-utag-filter="${esc(tag)}"># ${esc(tag)}</a><button data-untag="${esc(tag)}" title="${t('去掉标签')}">×</button></span>`).join('')}<button class="variant-add" id="addTag">＋ ${t('标签')} <kbd>G</kbd></button></div></section>
      ${d.duplicates?.length ? `<section class="inspector-section"><h3 class="section-title">${t('重复副本 · 共 {n} 份', {n: d.duplicates.length + 1})}</h3><div class="dup-list">${d.duplicates.map(x => `<button class="dup-copy" data-copy="${esc(x.uuid)}" title="${t('查看这份副本')}"><img src="/image?id=${encodeURIComponent(x.uuid)}&size=${thumbSize(DUP_H)}" alt="" loading="lazy"><span><b>${x.pos}/${x.size}</b> ${esc((x.added_at || '').slice(0, 10))}<br><small>${x.names.length ? x.names.map(esc).join(' · ') : t('未识别')}</small></span></button>`).join('')}</div></section>` : ''}
      ${fileActions()}
      ${viewOpts.meta ? `<section class="inspector-section"><dl class="facts"><div><dt>${t('尺寸')}</dt><dd class="mono">${number(d.width)} × ${number(d.height)}</dd></div><div><dt>${t('加入')}</dt><dd class="mono">${esc((d.added_at || '').slice(0, 16).replace('T', ' '))}</dd></div><div><dt>${t('文件')}</dt><dd title="${esc(d.filename)}">${esc(d.filename)}</dd></div><div><dt>${t('文件形态')}</dt><dd>${d.src_kind === 'deriv' ? t('本地副本') : t('原图')}</dd></div></dl></section>
      <details class="inspector-section"><summary>${t('画面标签')}</summary><div class="tags">${d.tags.map(x => `<button class="tag" data-tag="${esc(x.value)}" title="${t('按此标签筛选图库 · WD {conf}', {conf: x.conf.toFixed(2)})}">${esc(x.value.replaceAll('_', ' '))}</button>`).join('') || `<p>${t('暂无')}</p>`}</div></details>` : ''}
      <div class="inspector-actions">${pageOf(state.route).skip && !d.memberships.length && !d.unknown ? `<button class="secondary large" id="skipOne">${t('跳过')} <kbd>S</kbd></button>` : ''}${d.unknown ? `<button class="secondary large" id="resumeOne">${t('恢复到未识别')}${viewOf(state.route).bar === 'skipped' ? ' <kbd>S</kbd>' : ''}</button>` : ''}${curate}</div>`;
    markDetailKeys();
    if (pc) loadCharStats(pc.id, ident, gen);
    $('inspector').querySelector('.preview img').addEventListener('error', e => { e.target.parentElement.innerHTML = `<div class="missing">${esc(unreadableText())}</div>`; }, {once: true});
    $('closeInspector').onclick = closeInspector;
    $('inspector').querySelector('.inspector-head [data-fav]').onclick = () => favoriteOne(ident);
    $('expand').onclick = $('previewButton').onclick = () => openViewer(indexOf(ident));
    for (const b of $('inspector').querySelectorAll('[data-nav]')) b.onclick = () => step(Number(b.dataset.nav));
    // 从详情栏点标签和从工具栏选一样：在图库页上叠加到现有的筛选上，在别的页面去图库只看它（#386 的 2-30）。
    // 链接在点的那一刻按当前的筛选算，详情栏开着时筛选可能已经变了。
    for (const b of $('inspector').querySelectorAll('[data-tag]')) b.onclick = () => go(filterRoute({tag: b.dataset.tag}));
    for (const a of $('inspector').querySelectorAll('[data-utag-filter]')) a.onclick = e => { e.preventDefault(); go(filterRoute({utag: a.dataset.utagFilter})); };
    for (const b of $('inspector').querySelectorAll('[data-copy]')) b.onclick = () => inspect(b.dataset.copy);
    for (const b of $('inspector').querySelectorAll('[data-curate]')) b.onclick = () => mutate('/api/curate', {ids: [ident], action: b.dataset.curate}, b.dataset.curate === 'exclude' || poolIs(state.route, 'excluded', 'pending'));
    $('skipOne')?.addEventListener('click', () => singleAction('skip'));
    $('addTag').onclick = () => singleAction('tag');
    $('likeOne').onclick = () => likeOne(ident);
    for (const b of $('inspector').querySelectorAll('[data-artist-verdict]')) b.onclick = () => {
      const v = b.dataset.artistVerdict || null;
      mutate('/api/artist/verdict', {ids: [ident], artist: b.dataset.artist, verdict: v, name: b.dataset.name}, !!v && leaves(state.route, `artist:${b.dataset.artist}`, v === 'yes'));
    };
    $('inspector').querySelector('[data-artist-find]')?.addEventListener('click', () => findArtists([ident], () => inspect(ident, true)));
    for (const b of $('inspector').querySelectorAll('[data-artist-more]')) b.onclick = () => findArtistWorks(b.dataset.artistMore, b.dataset.name, () => inspect(ident, true));
    for (const b of $('inspector').querySelectorAll('[data-untag]')) {
      b.onclick = () => mutate('/api/tag', {ids: [ident], tag: b.dataset.untag, add: false}, leaves(state.route, `utag:${b.dataset.untag}`, false));
    }
    $('resumeOne')?.addEventListener('click', () => mutate('/api/change', {ids: [ident], action: 'resume'}, poolIs(state.route, 'skipped')));
    bindIdentity($('inspector'), d, ident); bindAsk($('inspector'), d, ident);
    bindFileActions($('inspector'), ident);
    // 按住预览图拖动：把这张图拖到别的应用（#450）。
    wireDragOut($('previewButton'), () => [ident]);
  } catch (e) { if (gen === detailGen) { $('inspector').innerHTML = `<div class="loading-line">${esc(e.message)}<br><button class="quiet" id="closeInspector">${t('关闭')}</button></div>`; $('closeInspector').onclick = closeInspector; } }
}
function bindIdentity(root, d, ident) {
  const leaves = id => state.route.char === id;
  for (const b of root.querySelectorAll('[data-not]')) b.onclick = () => mutate('/api/change', {ids: [ident], action: 'rejected', entity: b.dataset.not}, leaves(b.dataset.not));
  for (const b of root.querySelectorAll('[data-swap]')) b.onclick = () => swapOne(d, ident, b.dataset.swap);
  for (const b of root.querySelectorAll('[data-unvariant]')) b.onclick = () => mutate('/api/change', {ids: [ident], action: 'remove_variant', entity: b.dataset.unvariant}, !!state.route.variant);
  for (const b of root.querySelectorAll('[data-addvariant]')) b.onclick = async () => {
    const epoch = state.readEpoch, mode = state.nsfw;
    const c = await fetchChar(b.dataset.addvariant);
    if (!c || epoch !== state.readEpoch || mode !== state.nsfw || (state.focused !== ident && state.viewerItem?.uuid !== ident)) return;
    openPicker({title: t('加造型'), subject: [ident], items: c.variants.map(v => ({id: v.id, name: v.name, hint: t('{n} 张', {n: number(v.count)}), counted: true})), placeholder: t('筛选造型'), sentence: e => t('给这张加上 {name}', {name: e.name}), pick: e => mutate('/api/change', {ids: [ident], action: 'add_variant', entity: e.id}, false)});
  };
  for (const b of root.querySelectorAll('[data-alt]')) b.onclick = e => pickAlternative(d, ident, Number(b.dataset.alt), e.altKey);
  for (const b of root.querySelectorAll('[data-also]')) b.onclick = () => addOne(d, ident);
}
// 给一张图追加角色（还没有角色时是设为）：详情栏、大图的「＋」和 C、A 是同一个面板，都能多选（#424 的 2-24）。
function addOne(d, ident) {
  openPicker({title: d.memberships.length ? t('这张也是…') : t('这张是…'), sentence: e => d.memberships.length ? t('给这张加上 {name}', {name: e.name}) : t('把这张设为 {name}', {name: e.name}), exclude: d.memberships.map(m => m.id), suggest: {ids: [ident]}, multi: true,
    pick: list => mutate('/api/change', {ids: [ident], action: 'confirmed', ...targets(list)}, pageOf(state.route).lead)});
}
// ---------- 主动提问：偶尔问一句「是 TA 吗？」 ----------
// 浏览时每隔几张图最多问一次，被略过三次后本次访问不再问；整理页（某个角色、未识别…）总是显示。回答发到
// /api/ask/answer，供学习使用。
const ask = {cool: 0, ignored: 0, off: false, pending: null};
function shouldAsk(d, ident) {
  if (!viewOpts.ask || !d.question) return false;
  if (ask.pending === ident) return true;
  if (ask.pending) { ask.ignored++; ask.pending = null; if (ask.ignored >= 3) ask.off = true; }
  if (!cleanup()) {
    if (ask.off) return false;
    if (ask.cool > 0) { ask.cool--; return false; }
    ask.cool = 3;
  }
  ask.pending = ident; return true;
}
// 导出给样式清单页调用（tools/styleguide/live.js，#352）：改名或改参数时，tools/check_styleguide.py 会报出来。
export function askHTML(q, dark = false) {
  const why = q.kind === 'audit' ? t('随机抽查') : q.kind === 'hint' ? t('和你确认过的 TA 很像') + (q.current_name ? ' · ' + t('现在算作 {name}', {name: esc(q.current_name)}) : '') : q.kind === 'ambiguous' ? t('和 {name} 分不太清', {name: esc(q.rival_name)}) : '';
  return `<div class="ask-bar${dark ? ' on-dark' : ''}"><div class="ask-q">${t('是 {name} 吗？', {name: `<b>${esc(q.name)}</b>${workSmall({id: q.entity, name: q.name})}`})}${why ? `<small>${why}</small>` : ''}</div>
    <div class="ask-btns"><button class="ask-b yes" data-ask="yes">${t('是')} <kbd>Y</kbd></button><button class="ask-b" data-ask="no">${t('不是')} <kbd>N</kbd></button>${q.rival ? `<button class="ask-b" data-ask="switch" title="${t('换成 {name}', {name: esc(q.rival_name)})}">${t('是 {name}', {name: esc(q.rival_name)})}</button>` : ''}</div></div>`;
}
function bindAsk(root, d, ident) {
  state.question = root.querySelector('.ask-bar') ? {q: d.question, ident} : null;
  for (const b of root.querySelectorAll('[data-ask]')) b.onclick = () => answerAsk(b.dataset.ask);
}
export function answerAsk(how) {
  const cur = state.question; if (!cur) return;
  const {q, ident} = cur; ask.pending = null; ask.ignored = 0;
  // 随机抽查：回答过的图离开列表，所以检查面板移到下一张。
  if (poolIs(state.route, 'audit') && state.viewer < 0 && state.focused === ident) { const i = indexOf(ident); state.focused = state.items[i + 1]?.uuid || state.items[i - 1]?.uuid || null; }
  const body = {ids: [ident], entity: q.entity, kind: q.kind, ...(how === 'yes' ? {yes: [ident]} : {no: [ident]}), ...(how === 'switch' ? {switch: q.rival} : {})};
  mutate('/api/ask/answer', body, how !== 'yes' && state.route.char === q.entity, {advance: poolIs(state.route, 'audit')});
}
// 一个操作所替换的身份：当前角色页的那个身份（如果适用），否则是第一个。
const primaryOf = d => d.memberships.find(m => m.id === state.route.char) || d.memberships[0];
function swapOne(d, ident, from) {
  const old = d.memberships.find(m => m.id === from);
  openPicker({title: t('换成…'), sentence: e => t('把这张从 {from} 换成 {name}', {from: old.name, name: e.name}), exclude: d.memberships.map(m => m.id), suggest: {ids: [ident]}, multi: true,
    pick: list => mutate('/api/reassign', {ids: [ident], old: from, new: newIds(list)}, state.route.char === from)});
}
export function pickAlternative(d, ident, i, also = false) {
  const alt = d.alternatives?.[i]; if (!alt) return;
  const from = primaryOf(d);
  if (from && !also) mutate('/api/reassign', {ids: [ident], old: from.id, new: alt.id}, state.route.char === from.id);
  else mutate('/api/change', {ids: [ident], action: 'confirmed', entity: alt.id}, pageOf(state.route).lead);
}
// 没有选中任何图（或大图盖住了选择栏）时，单图快捷键作用于当前图：大图 ＞ 详情栏 ＞ 光标（core/modes.js）。
export async function singleAction(kind) {
  const item = currentItem(); if (!item) return;
  const r = state.route;
  if (kind === 'remove') {
    if (r.char) mutate('/api/change', {ids: [item.uuid], action: 'rejected', entity: r.char}, true);
    else if (item.memberships.length === 1) mutate('/api/change', {ids: [item.uuid], action: 'rejected', entity: item.memberships[0].id}, false);
    else if (item.memberships.length) openPicker({title: t('这张不是…'), items: item.memberships, subject: [item.uuid], sentence: e => t('把 {name} 从这张移出', {name: e.name}), pick: e => mutate('/api/change', {ids: [item.uuid], action: 'rejected', entity: e.id}, false)});
  } else if (kind === 'tag') {
    const d = state.detail?.uuid === item.uuid ? state.detail : null;
    openTagPicker(() => ({ids: [item.uuid]}), 1, [{...item, user_tags: d?.user_tags || item.user_tags}], () => false);
  // 大图里跳过、排除之后到下一张，和随机抽查回答之后一样（用户 2026-11-14 定，#424 的 2-1）；改角色仍停在原图。
  } else if (kind === 'skip' && pageOf(r).skip) mutate('/api/change', {ids: [item.uuid], action: 'unknown'}, true, {advance: state.viewer >= 0});
  else if (kind === 'resume') mutate('/api/change', {ids: [item.uuid], action: 'resume'}, poolIs(r, 'skipped'));
  else if (kind === 'fav') favoriteOne(item.uuid);
  else if (kind === 'exclude' || kind === 'include') mutate('/api/curate', {ids: [item.uuid], action: kind}, true, {advance: kind === 'exclude' && state.viewer >= 0});
  else if (kind === 'swap' || kind === 'also') {
    const d = state.detail?.uuid === item.uuid ? state.detail : await api('/api/detail?id=' + encodeURIComponent(item.uuid));
    const from = primaryOf(d);
    if (kind === 'swap' && from) swapOne(d, item.uuid, from.id);
    else addOne(d, item.uuid);
  }
}
// 收藏写入成功、没有重读时（ui/grid.js 的 favorite）：详情栏和大图里那张也改掉。changed 是列表里改到的图，ids 是这次写入
// 点名的图（全选时为空）；这里改到的也加进 changed，好让心形跟着改。
export function noteFavorite(changed, ids, on) {
  for (const x of [state.detail, state.viewerItem]) if (x && (changed.has(x.uuid) || ids?.has(x.uuid))) { x.fav = on; changed.add(x.uuid); }
}
export const indexOf = uuid => state.items.findIndex(x => x?.uuid === uuid);
export async function step(delta) {
  const cur = shownUuid();
  if (kept.hold && kept.hold.uuid === cur && Math.abs(delta) === 1) {
    const target = indexOf(delta > 0 ? kept.hold.next : kept.hold.prev); kept.hold = null;
    if (target >= 0) { setCursor(target); ensureVisible(target); if (state.viewer >= 0) showViewer(target); else inspect(state.items[target].uuid); return; }
  }
  kept.hold = null;
  const at = cur ? indexOf(cur) : -1;
  const from = at >= 0 ? at : state.viewer >= 0 ? state.viewer : state.cursor;
  const n = Math.max(0, Math.min(state.total - 1, from + delta)); if (n === from || n < 0) return;
  if (!state.items[n]) await loadPage(Math.floor(n / PAGE) * PAGE);
  if (!state.items[n]) return;
  setCursor(n); ensureVisible(n);
  if (state.viewer >= 0) showViewer(n); else inspect(state.items[n].uuid);
}
// 把网格滚到第 i 张可见。主区不是网格的页面（首页从详情栏、大图翻页，按组整理看大图）不按网格的行去滚，交给页面登记的
// reveal 把那张图滚进视野（#424 的 2-11）。
export function ensureVisible(i) {
  if (viewOf(state.route).area !== 'grid') { reveal(state.items[i]?.uuid); return; }
  const v = $('viewport'), row = rowOf(i), top = rowTop(row), height = rowHeight(row);
  if (top < v.scrollTop) v.scrollTop = top; else if (top + height > v.scrollTop + v.clientHeight) v.scrollTop = top + height - v.clientHeight;
  syncCards();
}

// ---------- 全屏查看器 ----------
const view = {scale: 1, x: 0, y: 0};
// 大图是盖满页面的层：Esc 关它（焦点在层外的输入框里时让位给输入框失焦），关闭时焦点还给打开它的地方。
// 开着时焦点在大图上，它盖住的页面、选择栏和任务面板设成 inert，Tab 只在大图的按钮间走（#422）；提示条（撤销）不盖。
const viewerLayer = createLayer({kind: 'overlay', id: 'viewer', el: () => $('viewer'), yieldToInput: true, focusIn: 'self',
  inert: () => [document.querySelector('.app'), $('selectionBar'), $('taskPanel')], onDismiss: reason => closeViewer(reason)});
export function openViewer(i) { if (i < 0 || !state.items[i]) return; state.viewer = i; $('viewer').hidden = false; viewerLayer.open(); document.body.classList.add('viewing'); showViewer(i); }
// 详情栏开着时，关大图后它跟到大图里最后看的那张（core/modes.js 的 detailAfterViewer，#399）：光标已经在那张，
// 不跟的话快捷键改的是详情栏里的旧图，Space、方向键却从光标那张走。
// 面板页借列表看的大图（previewList）关掉时把列表还回去，光标和详情栏都不动，只把最后看的那张滚进视野。
export function closeViewer(reason) {
  viewerLayer.close(reason); if (state.viewer < 0) return;
  const follow = detailAfterViewer(), last = state.viewerItem?.uuid;
  kept.hold = null; state.viewerItem = null; $('viewer').hidden = true; document.body.classList.remove('viewing');
  if (endPreview()) { state.viewer = -1; reveal(last); markDetailKeys(); return; }
  setCursor(state.viewer); state.viewer = -1; ensureVisible(state.cursor);
  if (follow) inspect(follow, true); else markDetailKeys();   // 选择栏又不被盖住了：详情栏的键标跟着它
}
export async function showViewer(i, held = null) {
  if (i < 0 && !held) { closeViewer(); return; }
  if (!held && !state.items[i]) await loadPage(Math.floor(i / PAGE) * PAGE);
  const item = held || state.items[i]; if (!item) { closeViewer(); return; }
  const same = state.viewerItem?.uuid === item.uuid && !$('viewer').hidden;
  state.viewer = Math.max(0, i); setCursor(state.viewer); state.viewerItem = item;
  if (same) { $('viewerCount').textContent = indexOf(item.uuid) >= 0 ? `${number(indexOf(item.uuid) + 1)} / ${number(state.total)}` : t('已移出当前列表 · ← → 继续'); return refreshViewerBar(item, i); }
  const img = $('viewerImg'); img.style.transform = ''; img.hidden = false; $('viewerMissing').hidden = true; Object.assign(view, {scale: 1, x: 0, y: 0});
  img.onerror = () => { if (state.viewerItem?.uuid === item.uuid) { img.hidden = true; $('viewerMissing').textContent = unreadableText(); $('viewerMissing').hidden = false; } };
  img.src = `/image?id=${encodeURIComponent(item.uuid)}&size=preview`;
  const full = new Image(); full.onload = () => { if (state.viewerItem?.uuid === item.uuid) { img.src = full.src; $('viewerSize').textContent = t('本地副本 {w} × {h}', {w: full.naturalWidth, h: full.naturalHeight}); } }; full.src = `/image?id=${encodeURIComponent(item.uuid)}&size=full`;
  $('viewerSize').textContent = '';
  $('viewerCount').textContent = `${number(i + 1)} / ${number(state.total)}`;
  for (const d of [-2, -1, 1, 2]) { const x = state.items[i + d]; if (x) new Image().src = `/image?id=${encodeURIComponent(x.uuid)}&size=preview`; }
  return refreshViewerBar(item, i);
}
async function refreshViewerBar(item, i) {
  let d; try { d = await api('/api/detail?' + new URLSearchParams({id: item.uuid, ...MARKS})); } catch (e) { if (state.viewerItem?.uuid === item.uuid) readFailure($('viewerBar'), e); return; }
  if (!d || state.viewerItem?.uuid !== item.uuid) return;
  state.detail = d; $('viewerBar').innerHTML = (shouldAsk(d, item.uuid) ? askHTML(d.question, true) : '') + identityChips(d, true) + viewerActions(d);
  bindIdentity($('viewerBar'), d, item.uuid); bindAsk($('viewerBar'), d, item.uuid);
  for (const b of $('viewerBar').querySelectorAll('[data-vact]')) {
    const act = b.dataset.vact;
    b.onclick = () => act === 'info' ? toggleInfo() : act === 'like' ? likeOne(item.uuid) : singleAction(act);
  }
}
// 「更多像这张的」（#449）：详情栏预览图下面、大图底栏各一个，去相似结果（core/like.js；结果页的标题在 ui/like.js）。
// 在相似结果里再点，换成这一张作例图，比较方式不变。没有快捷键。
const likeOne = ident => go(likeRoute([ident], state.route.by));
// 大图底栏的「跳过 S」「排除 X」「信息 I」（#424 的 2-1、2-7）和「更多像这张的」（#449，没有键）：只在这一页按这个键做的就是它时才画。跳过和详情栏的
// 「跳过 S」一样，只在能跳过的页、图还没有角色也还没跳过时有（刚跳过、还停在原图时不再显示）；已排除的图 X 是恢复收录，不画；信息在有详情栏的页面才有。
// 「收藏 F」每页都有（#447）。
function viewerActions(d) {
  const r = state.route, b = [], btn = (act, key, label) => `<button class="alt" data-vact="${act}"><kbd>${key}</kbd>${label}</button>`;
  if (pageOf(r).skip && !d.memberships.length && !d.unknown) b.push(btn('skip', 'S', t('跳过')));
  if (!d.excluded) b.push(btn('exclude', 'X', t('排除')));
  b.push(viewerHeart(d.uuid, !!d.fav));
  if (keyScope(viewOf(r)) && !previewing()) b.push(btn('info', 'I', t('信息')));
  b.push(`<button class="alt" data-vact="like">${t('更多像这张的')}</button>`);
  return b.length ? `<div class="alts"><div class="alt-row">${b.join('')}</div></div>` : '';
}
function applyView() { $('viewerImg').style.transform = `translate(${view.x}px,${view.y}px) scale(${view.scale})`; $('viewerStage').classList.toggle('zoomed', view.scale > 1.01); }
export function zoomView(factor, cx, cy) {
  const stage = $('viewerStage').getBoundingClientRect(), px = cx - stage.left - stage.width / 2, py = cy - stage.top - stage.height / 2;
  const img = $('viewerImg'), limit = Math.max(2, (img.naturalWidth / img.clientWidth) * 2 || 2);
  const next = Math.max(1, Math.min(limit, view.scale * factor)), k = next / view.scale;
  view.x = px - (px - view.x) * k; view.y = py - (py - view.y) * k; view.scale = next;
  if (next === 1) { view.x = 0; view.y = 0; } applyView();
}
// 主区不是网格的页面把一张图滚进视野：页面在注册表里登记 reveal(uuid)（首页、按组整理）。
const reveal = uuid => { if (uuid) pageOf(state.route).reveal?.(uuid); };

// ---------- 面板页看大图（#424 的 2-2、2-34） ----------
// 按组整理的缩略图、首页「认一认」的图，点一下是「不算」或「不是 TA」，看大图用长按，或焦点在图上、鼠标停在图上时按
// Space（grid.js 的键盘处理调 peek）；身份整理的样图、画师抽查的图点一下就看大图（页面自己调 peekAt）。这样的图带
// data-peek（uuid），所在的区域用 wirePeek 登记，list(el) 返回它那一组的 uuid：大图里翻的就是这一组（grid.js 的 previewList）。
const LONG_PRESS = 450, peeks = [];
let hovered = null;
export function wirePeek(root, list) {
  peeks.push({root, list});
  let timer = 0, fired = false, start = [0, 0];
  const cancel = () => { clearTimeout(timer); timer = 0; };
  root.addEventListener('pointerdown', e => {
    fired = false; cancel();
    const el = e.target.closest('[data-peek]'); if (!el || e.button !== 0) return;
    start = [e.clientX, e.clientY];
    timer = setTimeout(() => { timer = 0; fired = peekAt(el); }, LONG_PRESS);
  });
  root.addEventListener('pointermove', e => { if (timer && Math.hypot(e.clientX - start[0], e.clientY - start[1]) > 8) cancel(); });
  for (const type of ['pointerup', 'pointercancel', 'pointerleave']) root.addEventListener(type, cancel);
  // 长按打开了大图：松手时的这次点击不再算「不算在这组里」。松手常落在大图上，这次点击到不了这里，标志留到下一次按下
  // 才清；所以只吞指针产生的点击（detail > 0），键盘 ↵ 按下按钮产生的点击（detail 为 0）照常。
  root.addEventListener('click', e => { if (fired && e.detail > 0) { e.preventDefault(); e.stopPropagation(); } fired = false; }, true);
  root.addEventListener('contextmenu', e => { if (fired || timer) e.preventDefault(); });
  root.addEventListener('mouseover', e => { hovered = e.target.closest('[data-peek]'); });
  root.addEventListener('mouseleave', () => { hovered = null; });
}
// Space：焦点所在的图，否则鼠标停着的那张（还在页面上、看得见）。打开了返回 true。
export function peek(target) {
  const el = target?.closest?.('[data-peek]') || (hovered?.isConnected && hovered.offsetParent ? hovered : null);
  return el ? peekAt(el) : false;
}
export function peekAt(el) {
  const p = peeks.find(x => x.root.contains(el)), ids = p?.list(el) || [], i = ids.indexOf(el.dataset.peek);
  if (i < 0) return false;
  previewList(ids, i); return true;
}

let viewGesture = 1;
let pan = null;

// 页面这一部分注册的处理函数。页面启动时调用一次。
export function wireDetail() {
  $('viewerStage').addEventListener('wheel', e => {
    e.preventDefault();
    if (e.ctrlKey || e.metaKey) zoomView(Math.exp(-e.deltaY / 200), e.clientX, e.clientY);
    else if (view.scale > 1) { view.x -= e.deltaX; view.y -= e.deltaY; applyView(); }
  }, {passive: false});
  $('viewerStage').addEventListener('gesturestart', e => { e.preventDefault(); viewGesture = 1; });
  $('viewerStage').addEventListener('gesturechange', e => { e.preventDefault(); zoomView(e.scale / viewGesture, e.clientX, e.clientY); viewGesture = e.scale; });
  $('viewerStage').addEventListener('dblclick', e => {
    const img = $('viewerImg'), actual = img.naturalWidth / img.clientWidth;
    if (view.scale > 1.01) { Object.assign(view, {scale: 1, x: 0, y: 0}); applyView(); } else zoomView(Math.max(2, actual), e.clientX, e.clientY);
  });
  // 没放大时按住大图拖动：把这张图拖到别的应用（#450）；放大后按住拖动是平移。
  wireDragOut($('viewerImg'), () => [state.viewerItem?.uuid].filter(Boolean), () => view.scale <= 1.01);
  $('viewerStage').addEventListener('pointerdown', e => { if (view.scale > 1) { pan = {x: e.clientX - view.x, y: e.clientY - view.y}; $('viewerStage').setPointerCapture(e.pointerId); } });
  $('viewerStage').addEventListener('pointermove', e => { if (pan) { view.x = e.clientX - pan.x; view.y = e.clientY - pan.y; applyView(); } });
  $('viewerStage').addEventListener('pointerup', () => { pan = null; });
  $('viewerStage').addEventListener('click', e => { if (e.target === $('viewerStage') && view.scale <= 1) closeViewer(); });
  $('viewerClose').onclick = () => closeViewer(); $('viewerPrev').onclick = () => step(-1); $('viewerNext').onclick = () => step(1);
}
