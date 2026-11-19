// 首页（桌面布局，#199）：按加入日期分组的最新图片、同步条，以及「认一认」卡片。
import {edits} from '../core/edits.js';
import {dayLabel, esc, number, thumbSize} from '../core/format.js';
import {I18N, t} from '../core/i18n.js';
import {nameWithWork} from '../core/labels.js';
import {implement, onPage} from '../core/pages.js';
import {charHash, hashFor} from '../core/route.js';
import {api, post, readError} from '../core/session.js';
import {state} from '../core/state.js';
import {INSP_ICONS, indexOf, inspect, openViewer, wirePeek} from './detail.js';
import {$, reconcileSections, showArea, toast, workSmall} from './dom.js';
import {mutate, showList, viewOpts} from './grid.js';
import {MARKS} from '../core/favorites.js';
import {homeHeart} from './favorites.js';

// ---------- 首页：按加入日期分组的最新图片 ----------
const weekday = day => { const [y, m, d] = day.split('-').map(Number); return new Intl.DateTimeFormat(I18N.locale, {weekday: 'short'}).format(new Date(y, m - 1, d)); };
// 首页的状态：它的各天、下一个要读的、「认一认」卡片的状态，以及盯着列表末尾的东西。
export const home = {days: [], next: '', gen: 0, loading: false, ask: null, askSkip: [], askOff: new Set(), observer: null, resize: null, width: 0};
async function renderHome(keep = false) {
  showArea();
  $('selectionBar').hidden = true; $('resultCount').textContent = ''; $('hint').textContent = t('点图片看详情 · 点角色名只看 TA');
  home.loading = false; home.observer?.disconnect(); dayLoading.clear();
  const gen = ++home.gen, scroll = keep ? $('viewport').scrollTop : 0;
  if (!keep) { home.days = []; home.next = ''; $('viewport').scrollTop = 0; $('homePage').innerHTML = `<div class="loading-line">${t('读取中…')}</div>`; }
  try {
    const r = await api('/api/home?' + new URLSearchParams({nsfw: state.nsfw, days: Math.min(20, Math.max(5, home.days.length)), ...MARKS}));
    if (gen !== home.gen || !onPage(state.route, 'days')) return;
    home.days = r.days; home.next = r.next;
    if (!keep) { home.askSkip = []; home.askOff = new Set(); home.ask = await nextAsk(); if (gen !== home.gen || !onPage(state.route, 'days')) return; }
    drawHome();
    if (keep) $('viewport').scrollTop = scroll;
  } catch (e) { if (gen === home.gen && onPage(state.route, 'days')) $('homePage').innerHTML = `<p class="empty">${esc(readError(e))}</p>`; }
}
export async function moreHome() {
  if (home.loading || !home.next || !onPage(state.route, 'days')) return;
  home.loading = true; const gen = home.gen; let appended = false;
  $('homeSentinel').textContent = t('正在读取更早的图…');
  try {
    const r = await api('/api/home?' + new URLSearchParams({nsfw: state.nsfw, before: home.next, ...MARKS}));
    if (gen !== home.gen || !onPage(state.route, 'days')) return;
    const known = new Set(home.days.map(g => g.day)), days = r.days.filter(g => !known.has(g.day) && known.add(g.day));
    home.days.push(...days); home.next = r.next;
    // 已有的几天和哨兵内容不变，留在原处；新的几天接在后面。
    reconcileSections($('homePage'), homeSections());
    for (const day of days) for (const el of $('homePage').querySelectorAll(`.home-day[data-day="${day.day}"] .home-strip`)) syncStrip(el);
    markHomeFocus();
    showList([...state.items, ...days.flatMap(g => g.items)]);
    if (!home.next) home.observer?.disconnect();
    appended = true;
  } catch (e) { if (gen === home.gen && onPage(state.route, 'days')) {
    $('homeSentinel').innerHTML = `<button class="home-retry" data-home-retry>${t('读取失败，重试')}</button>`; toast(e.message);
  } }
  finally {
    if (gen === home.gen) {
      home.loading = false;
      // 每次追加后重新观察，这样仍在边距内的哨兵会再次加载。
      if (appended && home.next && onPage(state.route, 'days') && $('homeSentinel')) { home.observer?.unobserve($('homeSentinel')); home.observer?.observe($('homeSentinel')); }
    }
  }
}
export const ratio = x => x.width && x.height ? Math.min(2.4, Math.max(.4, x.width / x.height)) : .75;
// rowH 是这一行的名义高度（CSS 像素），按它选缩略图档位（#452）。
function homeTile(x, style, rowH) {
  const names = x.memberships.map(m => m.name), ids = x.memberships.map(m => m.id).join(' ');
  const badge = viewOpts.marks && x.dup ? `<span class="home-badge" title="${t('共 {n} 份副本', {n: x.dup.size})}">×${x.dup.size}</span>` : '';
  return `<button class="home-j" data-id="${esc(x.uuid)}" data-chars="${esc(ids)}" style="${style}"${names.length ? ` title="${esc(x.memberships.map(nameWithWork).join(t('、')))}"` : ''}><img src="/image?id=${encodeURIComponent(x.uuid)}&size=${thumbSize(rowH)}" alt="" loading="lazy" decoding="async" draggable="false">${badge}${homeHeart(x)}<span class="home-ov">${names.length ? names.map(esc).join(' · ') : x.unknown ? t('已跳过') : t('未识别')}</span></button>`;
}
// 整行拉伸到满宽（比目标高度略矮）；需要长到目标高度 ROW_GROW 倍以上的行（图片少的一天、一天的最后一行）保持目标
// 高度，右侧留空。
const ROW_GROW = 1.15;
export function homeRows(items, width, target) {
  const G = 6, rows = []; let row = [], sum = 0;
  for (const x of items) { row.push(x); sum += ratio(x); if (sum * target + G * (row.length - 1) >= width) { rows.push(row); row = []; sum = 0; } }
  if (row.length) rows.push(row);
  return rows.map(r => { const s = r.reduce((a, x) => a + ratio(x), 0), fit = (width - G * (r.length - 1)) / s, full = fit <= target * ROW_GROW, h = full ? fit : target;
    return `<div class="home-row" style="height:${Math.round(h)}px">${r.map(x => homeTile(x, full ? `flex:${ratio(x)} 1 0` : `flex:none;width:${Math.round(ratio(x) * h)}px`, target)).join('')}</div>`; }).join('');
}
function heroTile(hero, w, h) {
  const top = hero.memberships[0];
  return `<button class="home-j home-hero" data-id="${esc(hero.uuid)}" data-chars="${esc(hero.memberships.map(m => m.id).join(' '))}" style="width:${w}px;height:${h}px"><img src="/image?id=${encodeURIComponent(hero.uuid)}&size=${thumbSize(h)}" alt="" loading="lazy" decoding="async" draggable="false">${homeHeart(hero)}${top ? `<span class="home-cap"><b>${esc(top.name)}${workSmall(top)}</b>${top.tag && top.tag !== top.name ? `<small>${esc(top.tag.replaceAll('_', ' '))}</small>` : ''}</span>` : ''}</button>`;
}
// 宽度放不下的一天横向滚动：先是大图，然后是按最短优先填充的两行。
function stripRows(items, rowH) {
  const rows = [[], []], w = [0, 0];
  for (const x of items) { const i = w[0] <= w[1] ? 0 : 1; rows[i].push(x); w[i] += ratio(x) * rowH + 6; }
  return rows.map(r => `<div class="home-row strip-row" style="height:${rowH}px">${r.map(x => homeTile(x, `flex:none;width:${Math.round(ratio(x) * rowH)}px`, rowH)).join('')}</div>`).join('');
}
function homeSizes() {
  const i = viewOpts.homeSize in [0, 1, 2] ? viewOpts.homeSize : 1;
  return {target: [150, 190, 240][i], heroW: [200, 240, 280][i]};
}
function homeDay(g, width) {
  const {target, heroW} = homeSizes();
  const hero = viewOpts.hero && g.items.length >= 4 && width > heroW * 2.4 ? g.items.find(x => x.uuid === g.hero) || g.items[0] : null;
  const rest = hero ? g.items.filter(x => x !== hero) : g.items;
  const rowH = hero ? (Math.round(heroW / .72) - 6) / 2 : target;
  // 两行条带的自然宽度；溢出时（或还有更多图片在等）这一天横向滚动。
  const natural = (hero ? heroW + 6 : 0) + rest.reduce((a, x) => a + ratio(x) * rowH + 6, 0) / 2;
  const strip = g.count > g.items.length || natural > width * 1.5;
  let body;
  if (strip) {
    const heroH = Math.round(rowH * 2 + 6);
    body = `<div class="home-strip-wrap"><button class="strip-nav prev" data-strip="-1" aria-label="${t('向左')}" hidden>${INSP_ICONS.prev}</button>
      <div class="home-strip" data-day="${g.day}">${hero ? heroTile(hero, heroW, heroH) : ''}<div class="strip-rows">${stripRows(rest, Math.round(rowH))}</div>
        ${g.count > g.items.length ? `<a class="strip-more" href="${hashFor({page: 'library', day: g.day})}" style="height:${heroH}px">${t('{n} 张没显示', {n: `<b>${number(g.count - g.items.length)}</b>`})}<small>${t('在图库里看全部')}</small></a>` : ''}</div>
      <button class="strip-nav next" data-strip="1" aria-label="${t('向右')}">${INSP_ICONS.next}</button></div>`;
  } else {
    // 大图旁边的两行拉伸到它的高度和侧边的宽度；第二行会大半空着时（图片少的一天），这一天改为不带大图排版（#293）。
    const heroH = Math.round(heroW / .72), sideW = width - heroW - 6, side = [[], []], fill = [0, 0];
    let r = 0, used = 0;
    if (hero) for (const x of rest) { if (r > 1) break; side[r].push(x); fill[r] += ratio(x) * rowH + 6; used++; if (fill[r] - 6 >= sideW) r++; }
    body = hero && fill[1] - 6 >= sideW * .75
      ? `<div class="home-top">${heroTile(hero, heroW, heroH)}<div class="home-side">${side.map(rw => `<div class="home-row">${rw.map(x => homeTile(x, `flex:${ratio(x)} 1 0`, rowH)).join('')}</div>`).join('')}</div></div>${homeRows(rest.slice(used), width, target)}`
      : homeRows(g.items, width, target);
  }
  const more = g.people - g.characters.length;
  const chips = g.characters.map(c => { const cover = state.charMap.get(c.id)?.cover;
    return `<a class="home-cc" href="${charHash(c.id)}" data-char="${esc(c.id)}" title="${esc(nameWithWork(c))} · ${t('在图库里只看 TA')}">${cover ? `<img src="/image?id=${encodeURIComponent(cover)}&size=thumb" alt="" loading="lazy">` : '<i></i>'}${esc(c.name)}<small>${number(c.count)}</small></a>`; }).join('');
  return `<section class="home-day" data-day="${g.day}"><div class="home-dh"><b>${esc(dayLabel(g.day))}</b><span>${weekday(g.day)} · ${t('{n} 张', {n: number(g.count)})}${viewOpts.people && g.people ? ' · ' + t('{n} 位角色', {n: number(g.people)}) : ''}</span><a class="home-all" href="${hashFor({page: 'library', day: g.day})}">${t('在图库里看')} ›</a></div>
    ${chips ? `<div class="home-chars">${chips}${more > 0 ? `<span class="home-more">${t('另外 {n} 位', {n: number(more)})}</span>` : ''}</div>` : ''}${body}</section>`;
}
// 横向滚动：只有那个方向还有内容时才显示箭头；快到尽头时加载那一天的下一批图片。
function syncStrip(el) {
  const wrap = el.parentElement, max = el.scrollWidth - el.clientWidth - 2;
  wrap.querySelector('.prev').hidden = el.scrollLeft <= 2; wrap.querySelector('.next').hidden = el.scrollLeft >= max;
  if (el.scrollLeft + el.clientWidth > el.scrollWidth - 600) moreOfDay(el.dataset.day);
}
export const dayLoading = new Set();
export async function moreOfDay(day) {
  const g = home.days.find(x => x.day === day); if (!g || g.items.length >= g.count || dayLoading.has(day)) return;
  dayLoading.add(day); const gen = home.gen;
  try {
    const r = await api('/api/assets?' + new URLSearchParams({day, nsfw: state.nsfw, order: 'newest', offset: g.items.length, limit: 30, count: 0}));
    if (gen !== home.gen || !onPage(state.route, 'days')) return;
    const known = new Set(g.items.map(x => x.uuid)); g.items.push(...r.items.filter(x => !known.has(x.uuid)));
    if (!r.items.length) g.count = g.items.length;
    const el = $('homePage').querySelector(`.home-strip[data-day="${day}"]`), left = el?.scrollLeft || 0;
    const sec = el?.closest('.home-day'); if (!sec) return;
    const tmp = document.createElement('div'); tmp.innerHTML = homeDay(g, home.width); sec.replaceWith(tmp.firstElementChild); markHomeFocus();
    const fresh = $('homePage').querySelector(`.home-strip[data-day="${day}"]`); if (fresh) { fresh.scrollLeft = left; syncStrip(fresh); }
    showList(home.days.flatMap(x => x.items));
  } catch (e) { if (gen === home.gen && onPage(state.route, 'days')) toast(e.message); } finally { if (gen === home.gen) dayLoading.delete(day); }
}
// 首页的各块：每天一块（第一天后面跟「认一认」）和末尾的哨兵。重画时只换内容变了的块（#452）：写入之后没变的那几天
// 连同图条的横向位置都留着。哪张图开着详情不写进块里，由 markHomeFocus 标。
function homeSections() {
  const ask = askCardHTML();
  return [...home.days.flatMap((g, i) => [[g.day, homeDay(g, home.width)], ...(i === 0 && ask ? [['ask', ask]] : [])]),
    ['more', `<div class="home-more-line" id="homeSentinel">${sentinelText()}</div>`]];
}
const sentinelText = () => home.next ? t('正在读取更早的图…') : t('已经到底了');
function drawHome() {
  if (!onPage(state.route, 'days')) return;
  const page = $('homePage'); home.width = page.clientWidth;
  if (!home.days.length) { page.innerHTML = `<p class="empty">${t('没有符合当前 R18 设置的图片。')}</p>`; showList([]); return; }
  reconcileSections(page, homeSections());
  $('homeSentinel').textContent = sentinelText();   // 留下来的哨兵可能还写着「读取失败，重试」
  showList(home.days.flatMap(g => g.items));
  for (const el of page.querySelectorAll('.home-strip')) syncStrip(el);
  markHomeFocus();
  home.observer?.disconnect();
  if (home.next) { home.observer = new IntersectionObserver(es => { if (es.some(e => e.isIntersecting)) moreHome(); }, {root: $('viewport'), rootMargin: '600px'}); home.observer.observe($('homeSentinel')); }
}
// 详情栏、大图翻页时把那张图滚进视野：横向的图条和整页都滚（#424 的 2-11，ui/detail.js 的 ensureVisible）。同一张图既是
// 主图又在图条里时滚图条里那个。
function revealHome(uuid) {
  const all = [...$('homePage').querySelectorAll('.home-j')].filter(x => x.dataset.id === uuid), b = all.find(x => x.closest('.home-strip')) || all[0];
  b?.scrollIntoView({block: 'nearest', inline: 'nearest', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth'});
}
export function markHomeFocus() { if (onPage(state.route, 'days')) for (const b of $('homePage').querySelectorAll('.home-j')) b.classList.toggle('on', b.dataset.id === state.focused); }
// 首页「认一认」：一个角色，几张机器识别出的图；点掉错的，然后确认其余的。整批都错时直接点「都不是」；全部点掉以后
// 主按钮本身就是「都不是」，常驻的那个就不再画（#675）。
async function nextAsk() {
  if (!viewOpts.ask) return null;
  try { return (await api('/api/ask?' + new URLSearchParams({nsfw: state.nsfw, skip: home.askSkip.join(',')}))).batch; } catch { return null; }
}
export function askCardHTML() {
  const b = home.ask; if (!b || !viewOpts.ask || !b.items.length) return '';
  const off = home.askOff.size, all = off === b.items.length, why = b.kind === 'hint' ? t('和你确认过的 TA 很像') : b.kind === 'seed' ? '' : t('随机抽查');
  return `<section class="ask-card" id="askCard"><div class="ask-head"><b>${t('认一认')}</b><span>${t('这些是 {name} 吗？', {name: `<a href="${charHash(b.entity)}">${esc(b.name)}</a>${workSmall({id: b.entity, name: b.name})}`})}</span><small>${why}</small></div>
    <div class="ask-row">${b.items.map(x => `<button class="ask-img${home.askOff.has(x.uuid) ? ' off' : ''}" data-askimg="${esc(x.uuid)}" data-peek="${esc(x.uuid)}" title="${home.askOff.has(x.uuid) ? t('点一下：改回「是」') : t('点一下：不是 TA')} · ${t('长按或 Space 看大图')}" style="aspect-ratio:${ratio(x)}"><img src="/image?id=${encodeURIComponent(x.uuid)}&size=thumb" alt="" loading="lazy" draggable="false"><i aria-hidden="true">✕</i></button>`).join('')}</div>
    <div class="ask-foot"><span>${off ? t('{n} 张不是 TA', {n: off}) : t('点掉不是 TA 的')}</span><button class="secondary" data-askcard="later">${t('以后再说')}</button>${all ? ''
      : `<button class="secondary" data-askcard="none" title="${t('{n} 张都记成不是 TA', {n: b.items.length})}">${t('都不是')}</button>`}<button class="primary" data-askcard="ok">${
      all ? t('都不是') : off ? t('其余 {n} 张是', {n: b.items.length - off}) : t('都对')}</button></div></section>`;
}
function redrawAskCard() { const el = $('askCard'), html = askCardHTML(); if (!el) return drawHome(); if (!html) { el.remove(); return; } const t = document.createElement('div'); t.innerHTML = html; el.replaceWith(t.firstElementChild); }
async function submitAskCard(how) {
  const b = home.ask; if (!b || edits.busy) return;
  // 「都不是」（#675）：不管点掉了哪几张，整批都记成不是 TA，和全部点掉再确认是同一个请求。
  const ids = b.items.map(x => x.uuid), off = how === 'none' ? new Set(ids) : home.askOff;
  const no = ids.filter(u => off.has(u)), yes = ids.filter(u => !off.has(u));
  home.askSkip.push(b.entity); home.askOff = new Set();
  if (how === 'later') {
    home.ask = null; redrawAskCard();
    post('/api/ask/answer', {entity: b.entity, kind: b.kind, later: ids}).catch(e => toast(e.message));
    return;
  }
  home.ask = await nextAsk();   // 页面重画之前下一个角色已经准备好
  await mutate('/api/ask/answer', {entity: b.entity, kind: b.kind, yes, no}, false);
}

// 页面这一部分注册的处理函数。页面启动时调用一次。
export function wireHome() {
  $('homePage').addEventListener('click', e => {
    if (e.target.closest('[data-home-retry]')) { moreHome(); return; }
    const ai = e.target.closest('[data-askimg]');
    if (ai) { const u = ai.dataset.askimg; home.askOff.has(u) ? home.askOff.delete(u) : home.askOff.add(u); redrawAskCard(); return; }
    const ac = e.target.closest('[data-askcard]'); if (ac) { submitAskCard(ac.dataset.askcard); return; }
    const n = e.target.closest('[data-strip]');
    if (n) { const el = n.parentElement.querySelector('.home-strip'); el.scrollBy({left: Number(n.dataset.strip) * el.clientWidth * .8, behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth'}); return; }
    const b = e.target.closest('.home-j'); if (b) inspect(b.dataset.id);
  });
  // 「认一认」的图点一下是「不是 TA」，长按或 Space 看大图，大图里翻的是这一批（#424 的 2-34）。
  wirePeek($('homePage'), () => home.ask?.items.map(x => x.uuid) || []);
  $('homePage').addEventListener('scroll', e => { if (e.target.classList?.contains('home-strip')) syncStrip(e.target); }, true);
  $('homePage').addEventListener('dblclick', e => { const b = e.target.closest('.home-j'); if (b) openViewer(indexOf(b.dataset.id)); });
  $('homePage').addEventListener('mouseover', e => {
    const c = e.target.closest('.home-cc'), day = c?.closest('.home-day');
    for (const d of $('homePage').querySelectorAll('.home-day.focus')) if (d !== day) d.classList.remove('focus');
    if (!c) return;
    day.classList.add('focus'); for (const b of day.querySelectorAll('.home-j')) b.classList.toggle('hit', b.dataset.chars.split(' ').includes(c.dataset.char));
  });
  $('homePage').addEventListener('mouseleave', () => { for (const d of $('homePage').querySelectorAll('.home-day.focus')) d.classList.remove('focus'); });
  new ResizeObserver(() => { if (!onPage(state.route, 'days') || !home.days.length || Math.abs($('homePage').clientWidth - home.width) < 8) return; clearTimeout(home.resize); home.resize = setTimeout(drawHome, 120); }).observe($('homePage'));
}

// 首页登记：渲染入口，以及数据变了之后怎么重画（撤销、同步到新图、改动之后保持滚动；切换 R18 重新读；视图选项只重排）。
// 后退回到首页（restore）时按上次读到的天数重读、不换「认一认」，好让 applyRoute 滚回原来的位置（#526）。
implement({days: {render: ({restore} = {}) => renderHome(!!restore && home.days.length > 0), reveal: revealHome,
  heading: () => [t('图库'), state.recentCount ? t('最近一次同步新加入 {n} 张', {n: number(state.recentCount)}) : ''], help: () => [t('按加入日期分组。')],
  refresh: why => { if (why === 'view') drawHome(); else renderHome(why !== 'nsfw' && why !== 'ask'); }}});
