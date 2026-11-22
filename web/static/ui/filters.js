// 筛选条（#445）：图库、角色页和画廊页工具栏里的条件胶囊和「＋ 条件」选择器。条件怎么读写在 core/filters.js；
// 这里只画胶囊、读名字、开选择器。胶囊的名字是链接：点一下在「含」和「不含」之间切换；× 去掉。图库页的标题和「?」
// 也从这里读（#644）：旧的单值参数和条件数组两种写法，标题、说明和名字都一样。
import {ownConditionOf} from '../core/galleries.js';
import {loadWorkTree} from '../core/catalog.js';
import {dayLabel, esc, number} from '../core/format.js';
import {I18N, t} from '../core/i18n.js';
import {nameWithWork} from '../core/labels.js';
import {addCondition, conditions, count, datesLabel, dropCondition, flipCondition, kindOf, LIMIT, setDates, sole, SOURCES, validDates, valueOf, yearDates} from '../core/filters.js';
import {go, hashFor} from '../core/route.js';
import {api} from '../core/session.js';
import {onTyped} from '../core/keys.js';
import {state} from '../core/state.js';
import {listNav} from './combo.js';
import {openDialog} from './dialog.js';
import {$, charOption, toast} from './dom.js';
import {createLayer} from './overlay.js';
import {popOpen, showPop} from './pop.js';

// ---------- 条件的文字 ----------
// 角色、作品、画师、画面标签的名字第一次用到时去读；读到后只换胶囊和标题上的那段文字（整条工具栏重画会打断正在输入的
// 搜索框）。names 里 null 是正在读，'' 是读不到。图库标题和胶囊共用这一份，同一位画师只读一次（#644）。
const names = new Map(), fetched = new Map();
const SOURCE_NAMES = {pixiv: 'Pixiv', x: 'X', other: t('其他出处')};
const tagText = key => key.replaceAll('_', ' ');
// 一个条件的文字；还没读到名字时先用 id。
export function conditionLabel(c) {
  const kind = kindOf(c), value = valueOf(c);
  if (kind === 'fav') return t('收藏');
  if (kind === 'source') return SOURCE_NAMES[value] || value;
  if (kind === 'utag') return '# ' + value;
  if (kind === 'char') { const found = state.charMap.get(value); if (found) return found.name; }
  if (!names.has(c)) lookUp(c);
  return names.get(c) || (kind === 'tag' ? tagText(value) : value);
}
// 标题上的名字：还在读时写「…」，不先露出 id。
function pendingLabel(c) { const label = conditionLabel(c); return names.get(c) === null ? '…' : label; }
// 角色条件的角色：图库里的角色，或单独读到的（只当标签的角色不在 state.chars 里）。
const charOf = c => state.charMap.get(valueOf(c)) || fetched.get(c);
// 读一个条件的名字，读到后换掉胶囊和标题上的文字。读不到就一直用 id（作品或画师删了，后端会在读图时说明）；画师没有名字
// 时写「未知画师」。
async function lookUp(c) {
  names.set(c, null);
  const kind = kindOf(c), value = valueOf(c);
  let name = '';
  try {
    if (kind === 'tag' && I18N.language === 'zh-CN') name = (await api('/api/general-tags?' + new URLSearchParams({q: value, format: 'menu'}))).find(x => x.tag === value)?.zh || '';
    else if (kind === 'work') { if (!state.workTree) await loadWorkTree(); name = state.workTree.find(w => w.id === value)?.name || ''; }
    else if (kind === 'artist') name = (await api('/api/artist?' + new URLSearchParams({id: value}))).name || t('未知画师');
    else if (kind === 'char') { const found = (await api('/api/characters?' + new URLSearchParams({id: value})))[0]; if (found) fetched.set(c, found); name = found?.name || ''; }
  } catch { name = ''; }
  names.set(c, name);
  for (const a of document.querySelectorAll('[data-cond]')) if (a.dataset.cond === c) { a.textContent = pillText(c, a.dataset.neg === '1'); a.title = pillTitle(c, a.dataset.neg === '1'); }
  for (const a of document.querySelectorAll('[data-cond-name]')) if (a.dataset.condName === c) a.textContent = conditionLabel(c);
}
const pillText = (c, negated) => negated ? t('不含 {name}', {name: conditionLabel(c)}) : conditionLabel(c);
// 胶囊的悬停提示：角色胶囊只写名字，悬停时写「名字（作品）」（#294、#678），后面是点一下做什么。
function pillTitle(c, negated) {
  const hint = negated ? t('点一下改回包含') : t('点一下改成不含'), char = kindOf(c) === 'char' && charOf(c);
  return char ? nameWithWork(char) + ' · ' + hint : hint;
}

// ---------- 图库的标题 ----------
// 只有一个条件（或旧写法的某一天）时，标题写这个条件；从哪条路进来（旧的单值参数、筛选条上加的条件数组）都一样（#644）。
// 名字放在 data-cond-name 里，读到后由 lookUp 换上。收藏由图库页自己判断，这里不出标题。没有这样的条件时返回 ''。
const named = c => `<span data-cond-name="${esc(c)}">${esc(pendingLabel(c))}</span>`;
export function conditionTitle(r) {
  const x = conditions(r), c = sole(r);
  if (!c) return !x.has.length && !x.not.length && !x.from && !x.to && x.day ? esc(t('{day} 加入', {day: dayLabel(x.day)})) : '';
  const kind = kindOf(c);
  if (kind === 'artist') return t('画师 · {name}', {name: named(c)});
  if (kind === 'work') return named(c);
  if (kind === 'utag') return esc(conditionLabel(c));
  return '';
}
// 图库「?」里跟条件有关的说明：含某部作品、某位画师时各写一句，不论有几个条件。
export function conditionHelp(r) {
  const kinds = new Set(conditions(r).has.map(kindOf)), lines = [];
  if (kinds.has('work')) lines.push(t('包含所有子作品的图。'));
  if (kinds.has('artist')) lines.push(t('画师是按画作比对找到的。'));
  return lines;
}

// ---------- 筛选条 ----------
// 工具栏里搜索框后面的一段：每个条件一枚胶囊，日期一枚，最后是「＋ 条件」和它的选择器。
export function filterBar(r) {
  const x = conditions(r);
  const pill = (c, negated) => `<span class="variant-chip">` +
    `<a href="${esc(hashFor(flipCondition(r, c)))}" data-cond="${esc(c)}" data-neg="${negated ? 1 : 0}" title="${esc(pillTitle(c, negated))}">${esc(pillText(c, negated))}</a>` +
    `<button data-drop="${esc(c)}" title="${t('去掉条件')}" aria-label="${t('去掉条件')}">×</button></span>`;
  const when = x.from || x.to ? datesLabel(x.from, x.to) : x.day ? t('{day} 加入', {day: dayLabel(x.day)}) : '';
  const dates = when ? `<span class="variant-chip"><span>${esc(when)}</span><button data-drop-dates title="${t('去掉日期')}" aria-label="${t('去掉日期')}">×</button></span>` : '';
  const full = count(r) >= LIMIT;
  // 画廊页上定义这个画廊的那个条件（手选画廊的 gallery:<id>、收藏的 fav，#710）不画：它就是这一页，去掉它就不是这个画廊了。
  const own = ownConditionOf(r);
  return [...x.has.filter(c => c !== own).map(c => pill(c, false)), ...x.not.map(c => pill(c, true))].join('') + dates +
    `<div class="tag-control"><button id="condButton" class="quiet"${full ? ` disabled title="${t('最多 {n} 个条件', {n: LIMIT})}"` : ''}>＋ ${t('条件')}</button>` +
    `<div id="condPopover" class="menu menu-drop" hidden>` +
    `<input id="condSearch" type="search" spellcheck="false" autocorrect="off" autocapitalize="off" placeholder="${t('标签、角色、作品、画师、出处或年份')}" aria-label="${t('搜索条件')}">` +
    `<div id="condOptions"></div></div></div>`;
}
// 画好以后接上事件；r 是画的时候的路由。
export function wireFilterBar(r) {
  const bar = $('toolbar');
  resultQuery = null; options = [];
  for (const b of bar.querySelectorAll('[data-drop]')) b.onclick = () => go(dropCondition(r, b.dataset.drop));
  bar.querySelector('[data-drop-dates]')?.addEventListener('click', () => go(setDates(r)));
  $('condButton').onclick = () => { const open = !popOpen($('condPopover')); setMenu(open); if (open) { userTags = null; $('condSearch').focus(); loadOptions(); } };
  onTyped($('condSearch'), e => loadOptions(e.target), 120);
  // 焦点留在搜索框里：↑ ↓ 在选项之间循环，↵ 或点击加上这个条件，Esc 关菜单、焦点回到按钮（同标签菜单，#432）。
  nav = listNav($('condSearch'), $('condOptions'), {shown: () => popOpen($('condPopover')), count: () => options.length,
    pick: i => { if ($('condSearch').value.trim() === resultQuery) choose(options[i]); }});
}
// 选择器是非模态浮层，Esc、点外关闭由浮层管理器处理；元素随工具栏重画换掉，所以按 id 找。
const layer = createLayer({kind: 'popover', id: 'condition-menu', el: () => $('condPopover'), anchor: () => $('condButton'), focusIn: 'first', onDismiss: reason => setMenu(false, reason)});
function setMenu(open, reason) {
  showPop($('condPopover'), open);
  if (open) layer.open(); else layer.close(reason);
}
let nav = null, options = [], userTags = null, resultQuery = null;

// ---------- 选择器 ----------
// 选项：{label, detail, section, cond} 或 {label, section, dates: true}（开起止日期对话框）、{label, year}。角色选项另带
// html：名字和作品小字，和「跳到角色」同一种写法（dom.js 的 charOption）。
const fold = s => s.toLocaleLowerCase(I18N.locale).normalize('NFKC');
const SECTION_NAMES = () => ({best: t('最佳匹配'), source: t('出处'), date: t('时间'), mine: t('我的标签'), tag: t('画面标签'), char: t('角色'), work: t('作品'), artist: t('画师'), other: t('其他')});
async function candidates(q) {
  const r = state.route, x = conditions(r), taken = new Set([...x.has, ...x.not]), low = fold(q), hit = s => !q || fold(s).includes(low);
  const free = o => !o.cond || !taken.has(o.cond), out = [];
  // 同档沿用原来各组的先后。先排序，再给每组限数，避免同组里的模糊匹配把名字对上的项挡在限额外。
  const rank = o => {
    if (!q) return 2;
    const name = fold(o.matchName ?? o.label);
    return name === low ? 0 : name.startsWith(low) ? 1 : 2;
  };
  const now = new Date().getFullYear();
  userTags ||= api('/api/user-tags?limit=1000').catch(() => state.userTags);
  const [mine, tags, artists] = await Promise.all([userTags,
    q ? api('/api/general-tags?' + new URLSearchParams({q, format: 'menu'})).catch(() => []) : [],
    q ? api('/api/artist-directory?' + new URLSearchParams({q, limit: 6})).then(d => d.items).catch(() => []) : []]);
  for (const s of SOURCES) if (hit(SOURCE_NAMES[s]) || hit(s)) out.push({section: 'source', label: SOURCE_NAMES[s], cond: `source:${s}`});
  const years = /^\d{4}$/.test(q) ? [Number(q)] : q ? [] : [now, now - 1, now - 2];
  for (const y of years) out.push({section: 'date', label: t('{year} 年', {year: y}), year: y});
  if (!q || hit(t('起止日期…')) || hit(t('日期'))) out.push({section: 'date', label: t('起止日期…'), dates: true});
  if (hit(t('收藏')) || hit('fav')) out.push({section: 'other', label: t('收藏'), cond: 'fav'});
  out.push(...(mine || []).filter(m => hit(m.tag)).map(m => ({
    section: 'mine', label: '# ' + m.tag, matchName: m.tag, detail: number(m.count), cond: `utag:${m.tag}`,
  })));
  if (q) {
    // 中文页面上画面标签显示中文名（#446），后面小字是原来的标签。
    const zh = x => I18N.language === 'zh-CN' && x.zh;
    out.push(...tags.map(x => ({section: 'tag', label: zh(x) || tagText(x.tag), detail: zh(x) ? tagText(x.tag) : '', cond: `tag:${x.tag}`})));
    const chars = state.chars.filter(c => hit(c.name) || (c.tag && hit(c.tag)));
    out.push(...chars.map(c => ({section: 'char', label: c.name, html: charOption(c), detail: number(c.count), cond: `char:${c.id}`})));
    if (!state.workTree) await loadWorkTree().catch(() => {});
    out.push(...(state.workTree || []).filter(w => hit(w.name)).map(w => ({section: 'work', label: w.name, detail: w.qualifier || '', cond: `work:${w.id}`})));
    out.push(...artists.map(a => ({section: 'artist', label: a.name, detail: number(a.count), cond: `artist:${a.id}`})));
  }
  const limit = {mine: q ? 12 : 6, tag: 12, char: 8, work: 6};
  const counts = {};
  return out.filter(free).sort((a, b) => rank(a) - rank(b)).filter(o => {
    counts[o.section] = (counts[o.section] || 0) + 1;
    return counts[o.section] <= (limit[o.section] ?? Infinity);
  }).map(o => rank(o) < 2 ? {...o, section: 'best'} : o);
}
async function loadOptions(input = $('condSearch')) {
  if (!input || input !== $('condSearch')) return;
  const q = input.value.trim();
  let found; let failure;
  try { found = await candidates(q); } catch (e) { failure = e; }
  if (input !== $('condSearch') || q !== input.value.trim()) return;
  resultQuery = q;
  if (failure) { $('condOptions').textContent = failure.message; options = []; return; }
  options = found;
  const sections = SECTION_NAMES();
  let last = '', html = '';
  found.forEach((o, i) => {
    if (o.section !== last) { html += `<div class="palette-label">${esc(sections[o.section])}</div>`; last = o.section; }
    html += `<button class="menu-item" data-i="${i}">${o.html || esc(o.label)}${o.detail ? ` <small>${esc(o.detail)}</small>` : ''}</button>`;
  });
  $('condOptions').innerHTML = html || `<p>${t('没有匹配的条件')}</p>`;
  nav?.mark(0);
}
function choose(o) {
  if (!o) return;
  setMenu(false);
  const r = state.route;
  if (o.cond) go(addCondition(r, o.cond));
  else if (o.year) go(setDates(r, ...yearDates(o.year)));
  else if (o.dates) askDates(r);
}
// 起止日期：两格日期，可以只填一格。表单，点背景不关。
function askDates(r) {
  const x = conditions(r);
  openDialog({title: t('加入日期'), backdropClose: false, focus: '[name="from"]',
    body: `<label class="field"><span>${t('从')}</span><input type="date" name="from" value="${esc(x.from)}"></label>` +
      `<label class="field"><span>${t('到')}</span><input type="date" name="to" value="${esc(x.to)}"></label>`,
    actions: `<button value="cancel" class="quiet">${t('取消')}</button><button value="ok" class="primary">${t('确认')}</button>`,
    onClose: d => {
      if (d.returnValue !== 'ok' || state.route !== r) return;
      const from = d.querySelector('[name="from"]').value, to = d.querySelector('[name="to"]').value;
      if (!validDates(from, to)) { toast(t('开始日期不能晚于结束日期，两格至少填一格。')); return; }
      go(setDates(r, from, to));
    }});
}