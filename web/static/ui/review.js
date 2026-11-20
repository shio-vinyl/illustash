// 「整理」各页（桌面布局，#199）：未识别图片的相似分组、整个图库里的相似图片、相似的角色对，以及画师抽查。
import {edits, exclusive} from '../core/edits.js';
import {esc, number, stamp} from '../core/format.js';
import {t} from '../core/i18n.js';
import {SIMILAR_ACTIVE, similarLine} from '../core/jobs.js';
import {RELATION_KINDS, RELATION_NAMES} from '../core/labels.js';
import {refreshMeta, setIdentityCount} from '../core/meta.js';
import {implement, onPage} from '../core/pages.js';
import {prefs} from '../core/prefs.js';
import {charHash, go, hashFor} from '../core/route.js';
import {api, post, readError} from '../core/session.js';
import {state} from '../core/state.js';
import {closeInspector} from './detail.js';
import {peekAt, wirePeek} from './viewer.js';
import {$, error, readFailure, reconcileSections, redraw, showArea, toast, written} from './dom.js';
import {filters, mutate} from './grid.js';
import {listIds} from '../core/assets.js';
import {markOc, openPicker, openRelate, targets} from './palette.js';
import {renderHeading} from './shell.js';
import {refresh as refreshRail} from './rail.js';

// ---------- 未匹配图片的相似分组 ----------
// 'look'：中心接近；'trait'：离得更远，但有相同的独特 WD 特征（较弱的线索）。
const nearText = g => g.near_kind === 'trait' ? t('外观相似：第 {n} 组', {n: g.near}) : t('可能和第 {n} 组是同一人', {n: g.near});
// 未识别，按组整理：CCIP 即使叫不出名字（45%），也能可靠地把同一个人的图放在一起（99%）。
// 大多数代表图意见一致的组整组问「是 TA 吗？」；其余的组给出「设为…」/「原创角色」/「跳过」。
// 点一张图，把它排除在这组的回答之外。回答过的组原地离开页面（不会因重新加载而跳动）。
export const groups = {list: [], shown: 40, off: new Map(), revision: 0};
// 各组的先后：一键就能确认的组排在前面（大的在前），然后是还要起名字的组。组页空了时的「下一组」也按它走。
const groupOrder = clusters => [...clusters].sort((a, b) => !!b.lead - !!a.lead || b.size - a.size || a.cid - b.cid);
export async function renderGroups(keep = false) {
  if (!onPage(state.route, 'groups')) return;
  showArea();
  closeInspector(); $('selectionBar').hidden = true; $('resultCount').textContent = ''; $('hint').textContent = t('点一下图片：不算在这组里 · 长按或 Space 看大图 · 组头的按钮对整组生效');
  const scroll = $('viewport').scrollTop, revision = ++groups.revision, nsfw = state.nsfw;
  let result; try { result = await api('/api/clusters?nsfw=' + state.nsfw); } catch (e) { if (onPage(state.route, 'groups') && revision === groups.revision && nsfw === state.nsfw) readFailure($('groupsPage'), e); return; }
  const {clusters, learner} = result;
  if (!onPage(state.route, 'groups') || revision !== groups.revision || nsfw !== state.nsfw) return;
  groups.list = groupOrder(clusters); groups.learner = learner;
  if (!keep) { groups.shown = 40; groups.off = new Map(); }
  drawGroups();
  if (keep) $('viewport').scrollTop = scroll;
}
export function drawGroups() {
  const list = groups.list, busy = ['learning', 'grouping', 'waiting'].includes(groups.learner?.status), images = list.reduce((s, g) => s + g.size, 0);
  const rest = Math.max(0, (state.pools.unmatched || 0) - images), named = list.filter(g => g.lead).length;
  if (!$('groupsPage').querySelector('.gsecs')) $('groupsPage').innerHTML = '<div class="groups-controls"></div><div class="gsecs"></div><div class="groups-more"></div>';
  $('groupsPage').querySelector('.groups-controls').innerHTML = `<div class="groups-bar"><span class="muted-note">${t('{n} 组 · 共 {images} 张', {n: number(list.length), images: number(images)})}${named ? ' · ' + t('{n} 组能直接确认是谁', {n: number(named)}) : ''}${list[0] ? ' · ' + t('分组于 {time}', {time: esc(stamp(list[0].built_at))}) : ''}${busy ? ' · ' + esc(groups.learner.status === 'waiting' ? groups.learner.message || '' : t('正在学习…')) : ''}</span><div class="spacer"></div>${rest ? `<a class="quiet link" href="#/unmatched">${t('没成组的 {n} 张', {n: number(rest)})} →</a>` : ''}<button class="quiet" id="regroup" title="${t('用当前的未识别图重新分组')}">${t('重新分组')}</button></div>`;
  reconcileSections($('groupsPage').querySelector('.gsecs'), list.slice(0, groups.shown).map(g => [String(g.cid), groupSection(g)]), t('没有成组的未识别图。'));
  $('groupsPage').querySelector('.groups-more').innerHTML = `${list.length > groups.shown ? `<div class="gsec-more"><button class="quiet" id="moreGroups">${t('再显示 {n} 组', {n: number(Math.min(40, list.length - groups.shown))})}</button></div>` : ''}`;
  $('regroup').onclick = async () => { try { await post('/api/regroup', {}); toast(t('正在重新分组…')); setTimeout(() => renderGroups(), 3000); } catch (e) { toast(e.message); } };
  $('moreGroups')?.addEventListener('click', () => { groups.shown += 40; drawGroups(); });
}
function groupSection(g) {
  const off = groups.off.get(g.cid) || new Set(), n = g.members.length - off.size, max = 16;
  // 「是 X 吗？」的组同样能整组标为原创角色或跳过（#386 的 2-41）；原创角色设为共用的「原创角色（未细分）」，和组页的
  // 「整组标为原创角色」一样，不新建独立的（#694）。
  const ocSkip = `<button class="quiet" data-gact="oc" title="${t('把这组图都设为「原创角色（未细分）」')}">${t('原创角色')}</button><button class="quiet" data-gact="skip">${t('跳过')}</button>`;
  const head = g.lead
    ? `<span class="gsec-q">${t('是 {name} 吗？', {name: `<a href="${charHash(g.lead.id)}">${esc(g.lead.name)}</a>`})}<small>${t('{n} 张的线索都指向 TA', {n: number(g.lead.count)})}</small></span>
       <button class="primary" data-gact="yes"${n ? '' : ' disabled'}>${off.size ? t('其余 {n} 张是', {n: number(n)}) : t('都是')} ✓</button><button class="secondary" data-gact="no">${t('都不是')} ✗</button><button class="quiet" data-gact="assign">${t('是别人…')}</button>${ocSkip}`
    : `<span class="gsec-q muted">${t('还不知道是谁')}</span><button class="primary" data-gact="assign"${n ? '' : ' disabled'}>${t('设为…')}</button>${ocSkip}`;
  const thumbs = g.members.slice(0, max).map(u => `<button class="gsec-img${off.has(u) ? ' off' : ''}" data-gimg="${esc(u)}" data-peek="${esc(u)}" title="${off.has(u) ? t('点一下：算回这组') : t('点一下：不算在这组里')} · ${t('长按或 Space 看大图')}"><img src="/image?id=${encodeURIComponent(u)}&size=thumb" loading="lazy" alt="" draggable="false"><i aria-hidden="true">✕</i></button>`).join('');
  return `<section class="gsec" data-cid="${g.cid}"><div class="gsec-head"><b>${t('第 {id} 组', {id: g.cid})}</b><span class="muted-note">${t('{n} 张', {n: number(g.size)})}${g.traits?.length ? ' · ' + g.traits.slice(0, 2).map(esc).join(' · ') : ''}</span>${head}<a class="quiet link gsec-open" href="#/g/${g.cid}">${t('逐张看')} →</a></div>
    <div class="gsec-imgs">${thumbs}${g.members.length > max ? `<a class="gsec-rest" href="#/g/${g.cid}">+${number(g.members.length - max)}</a>` : ''}</div>${g.near ? `<div class="group-near">${nearText(g)}</div>` : ''}</section>`;
}
// ---------- 整个图库里的相似图片（native/terms/src/research_compute/similar.rs） ----------
// 标过「看过了」的组（#427）不在列表里，「看过的 N 组」（#/similar?seen=1）只列它们；组里进了新图，后端就不再算它看过。
// 检查进行中每 3 秒重读一次。重读、撤销、学习落定之后（keep）只就地改状态行、工具条和进度条，组卡片按组号比对，
// 内容没变的留在原处，封面不重新加载（#601）。组列表先显示 40 组，「再显示」接着往下，和按组整理一样；换页签、
// 切换看过的组、切换 NSFW 时回到前 40 组。
const SIMILAR_PAGE = 40;
export const similar = {groups: [], others: 0, state: {}, kind: '', seen: false, shown: SIMILAR_PAGE, revision: 0, timer: 0};
export async function renderSimilar(keep = false) {
  if (!onPage(state.route, 'similar')) return;
  clearTimeout(similar.timer);
  if (!keep) { showArea(); closeInspector(); $('selectionBar').hidden = true; $('resultCount').textContent = ''; $('hint').textContent = t('点一组进去看'); }
  const kind = state.route.kind || '', seen = state.route.seen === '1', revision = ++similar.revision;
  const fresh = () => onPage(state.route, 'similar') && revision === similar.revision;
  let result; try { result = await api('/api/similar?' + new URLSearchParams({kind, nsfw: state.nsfw, seen: '1'})); } catch (e) { if (fresh()) readFailure($('groupsPage'), e); return; }
  if (!fresh()) return;
  const groups = result.groups.filter(g => !!g.seen === seen);
  Object.assign(similar, {groups, others: result.groups.length - groups.length, state: result.state, kind, seen});
  if (!keep) similar.shown = SIMILAR_PAGE;
  drawSimilar();
  if (SIMILAR_ACTIVE.includes(result.state.status)) similar.timer = setTimeout(() => renderSimilar(true), 3000);
}
const similarCard = g => `<a class="group-card" href="#/s/${g.gid}"><div class="group-covers">${g.covers.map(u => `<img src="/image?id=${encodeURIComponent(u)}&size=thumb" loading="lazy" alt="">`).join('')}</div>
      <div class="group-meta"><b>${t('相似组 {id}', {id: g.gid})}</b><span>${t('{n} 张', {n: number(g.size)})}</span></div>
      <div class="group-traits">${Object.entries(g.kinds).sort((a, b) => b[1] - a[1]).map(([k, n]) => `<span>${RELATION_NAMES[k] || k}${n > 1 ? ' ×' + n : ''}</span>`).join('')}</div></a>`;
export function drawSimilar() {
  const {groups: list, others, state: s, kind, seen, shown} = similar, box = $('groupsPage');
  if (!box.querySelector('.similar-list')) {
    box.innerHTML = `<div class="similar-status"><span class="local-dot"></span><span class="muted-note"></span></div><div class="groups-bar"></div>
      <div class="progress" hidden><span></span></div><div class="group-grid similar-list"></div><div class="similar-more"></div>`;
  }
  const busy = SIMILAR_ACTIVE.includes(s.status), line = similarLine(s);
  const count = {n: number(list.length), images: number(list.reduce((n, g) => n + g.size, 0))};
  const status = box.querySelector('.similar-status');
  status.firstElementChild.classList.toggle('busy', busy);
  status.lastElementChild.textContent = (seen ? t('看过的 {n} 组 · 共 {images} 张', count) : t('{n} 组 · 共 {images} 张', count)) + (line ? ' · ' + line : '');
  const otherText = seen ? t('没看过的 {n} 组', {n: number(others)}) : t('看过的 {n} 组', {n: number(others)});
  const other = others ? `<a class="quiet link" href="${hashFor({page: 'similar', kind, seen: seen ? '' : '1'})}">${otherText} →</a>` : '';
  redraw(box.querySelector('.groups-bar'), `<div class="segmented">${RELATION_KINDS.map(([k, label]) => `<button class="${k === kind ? 'on' : ''}" data-kind="${k}">${label}</button>`).join('')}</div>
      <div class="spacer"></div>${other}
      <button class="quiet" id="similarScan" ${busy || s.status === 'off' ? 'disabled' : ''} title="${t('比较新加入的图和还没比较过的候选')}">${t('重新检查')}</button>`);
  const progress = box.querySelector('.progress');
  progress.hidden = !(busy && s.total);
  if (busy && s.total) progress.firstElementChild.style.width = Math.round((s.done || 0) / s.total * 100) + '%';
  const empty = seen ? t('没有看过的组。') : others ? t('相似的组都看过了。') : busy ? t('还在检查，找到的组会陆续出现。') : t('没有找到相似的图。');
  reconcileSections(box.querySelector('.similar-list'), list.slice(0, shown).map(g => [String(g.gid), similarCard(g)]), empty);
  const more = box.querySelector('.similar-more');
  redraw(more, list.length > shown ? `<div class="gsec-more"><button class="quiet">${t('再显示 {n} 组', {n: number(Math.min(SIMILAR_PAGE, list.length - shown))})}</button></div>` : '');
  more.onclick = e => { if (e.target.closest('button')) { similar.shown += SIMILAR_PAGE; drawSimilar(); } };
  for (const b of box.querySelectorAll('[data-kind]')) b.onclick = () => go({page: 'similar', kind: b.dataset.kind, seen: state.route.seen});
  $('similarScan').onclick = async () => { try { await post('/api/similar', {}); setTimeout(() => renderSimilar(true), 800); } catch (e) { toast(e.message); } };
}

// ---------- 相似组 N（#/s/<id>，网格页）的「看过了」（#427） ----------
// 进入前读一次这组算不算看过、看得到几张。读不到就不显示按钮，不挡住这一页。
async function readSeen(route) {
  const nsfw = state.nsfw;
  let result; try { result = await api('/api/similar?' + new URLSearchParams({nsfw, seen: '1'})); } catch { result = null; }
  if (state.route !== route) return false;
  const g = result?.groups.find(x => String(x.gid) === route.id);
  state.sgroupSeen = g ? {seen: !!g.seen, size: g.size, nsfw} : null;
}
function sgroupHeading(r, {n}) {
  const g = state.sgroupSeen;
  const relate = state.total >= 2 && state.total <= 12 ? `<button class="quiet" id="sgroupRelate">${t('判断关系')}</button>` : '';
  const seen = !g ? '' : g.seen
    ? `<button class="quiet" id="sgroupSeen" title="${t('回到「相似的图」列表')}">${t('标为没看过')}</button>`
    : `<button class="quiet" id="sgroupSeen" title="${t('从「相似的图」列表里隐去；组里进了新图会再出现')}">${t('看过了')}</button>`;
  return [t('相似组 {id}', {id: esc(r.id)}), t('{n} 张', {n}), relate || seen ? `<span class="heading-links">${relate}${seen}</span>` : ''];
}
// 标为看过以后回到列表，这组不在了；撤销在提示条上。张数和读的时候对不上（组刚变过）后端拒绝，这里重读。
async function toggleSeen() {
  const route = state.route;
  if (state.sgroupSeen && state.sgroupSeen.nsfw !== state.nsfw) await readSeen(route);
  const g = state.sgroupSeen; if (!g || state.route !== route) return;
  await exclusive(async () => {
    let r; try { r = await post('/api/similar/seen', {gid: Number(route.id), size: g.size, nsfw: g.nsfw, seen: !g.seen}); }
    catch (e) { toast(e.message); if (e.status === 409 && await readSeen(route) !== false) renderHeading(); return; }
    written(r.undo);
    if (!g.seen) go({page: 'similar'});
    else if (state.route === route) { state.sgroupSeen = {...g, seen: false}; renderHeading(); }
  });
}

// ---------- 身份复核：相似的角色对（/api/identity/queue，native/terms/src/http_characters.rs） ----------
// 一次点击决定一对；每个决定都是一条可撤销的关系，原始记录从不移动。
const IDENTITY_PAGE = 12;
const identity = {items: [], total: 0, offset: 0, busy: new Set()};
function identitySuggestion(x) {
  const h = x.hint; if (!h) return '';
  if (h.kind === 'duplicate_of') return 'same';
  if (h.kind === 'form_of') return h.subject && h.subject === x.b.tag ? 'b_of_a' : h.subject && h.subject === x.a.tag ? 'a_of_b' : '';
  return '';
}
function identityEvidence(x) {
  const h = x.hint, name = tag => tag === x.a.tag ? x.a.name : tag === x.b.tag ? x.b.name : tag.replaceAll('_', ' ');
  const parts = [];
  if (h?.kind === 'duplicate_of') parts.push(t('{source}：同一个 tag 的别名', {source: esc(h.source || t('外部资料'))}));
  else if (h?.kind === 'form_of') parts.push(t('{source}：「{subject}」是「{object}」的一个造型', {source: esc(h.source || t('外部资料')), subject: esc(name(h.subject)), object: esc(name(h.object))}));
  if (x.distance !== null && x.distance !== undefined) parts.push(t('参考图外观距离 {d}', {d: x.distance.toFixed(3)}) + (x.distance <= 0.01 ? t('（几乎分不开）') : ''));
  return parts.join(' · ') || t('参考图外观很接近');
}
function identitySide(s, label) {
  return `<div class="pair-side"><div class="pair-name"><span class="pair-label">${label}</span><a href="${charHash(s.id)}" title="${esc(s.tag || s.name)}">${esc(s.name)}</a><small>${t('{n} 张', {n: number(s.count)})}</small></div>
    <div class="pair-thumbs">${s.samples.map(u => `<img src="/image?id=${encodeURIComponent(u)}&size=thumb" loading="lazy" alt="" data-peek="${esc(u)}" title="${t('看大图')}">`).join('') || `<span class="muted-note">${t('没有可显示的图')}</span>`}</div></div>`;
}
// 一对角色的键：判断、撤销以后按它比对，只换变了的卡片（#452）。
const pairKey = x => `${x.a.id}|${x.b.id}`;
function identityCard(x) {
  const s = identitySuggestion(x), btn = (act, text, title) => `<button class="${s === act ? 'primary' : 'quiet'}" data-act="${act}" title="${esc(title)}">${text}</button>`;
  return `<article class="pair-card" data-pair="${esc(pairKey(x))}">
    <div class="pair-body">${identitySide(x.a, 'A')}${identitySide(x.b, 'B')}</div>
    <p class="pair-evidence">${identityEvidence(x)}</p>
    <div class="pair-actions">
      ${btn('same', t('记为同一角色（不搬图）'), t('把两个角色合在一起显示，图和人工结论留在原处'))}
      ${btn('b_of_a', t('B 是 A 的一个造型'), t('B 挂到 A 下面作为造型（服装、异格、变身等）'))}
      ${btn('a_of_b', t('A 是 B 的一个造型'), t('A 挂到 B 下面作为造型'))}
      ${btn('different', t('不同角色'), t('记为不同角色，以后不再问这一对'))}
      <span class="spacer"></span>${btn('skip', t('暂不判断'), t('先放一边，队列里不再出现'))}
    </div></article>`;
}
// how：'more' 在后面再接一页；'keep' 判断或撤销之后，按现在显示的对数从头重读、留在原来的滚动位置，判过的那对原地
// 移走、撤销的那对回到原处，点过「再显示一些」的人不被带回前 12 对（#386 的 2-36）；不给就是从头显示前 12 对。
// 卡片按角色对比对，没变的留在原处（#452）。
async function renderIdentity(how = '') {
  if (!onPage(state.route, 'identity')) return;
  showArea();
  closeInspector(); $('selectionBar').hidden = true; $('hint').textContent = t('点角色名看全部图 · 高亮的是外部资料建议的做法');
  const keep = how === 'keep', scroll = $('viewport').scrollTop, limit = keep ? Math.max(IDENTITY_PAGE, identity.items.length) : IDENTITY_PAGE;
  const offset = how === 'more' ? identity.items.length : 0;
  let page;
  try { page = await api('/api/identity/queue?' + new URLSearchParams({limit, offset, nsfw: state.nsfw})); }
  catch (e) { $('groupsPage').innerHTML = `<p class="empty">${esc(readError(e))}</p>`; return; }
  if (!onPage(state.route, 'identity')) return;
  if (how !== 'more') identity.items = [];
  identity.items.push(...page.items); identity.total = page.total; setIdentityCount(page.total); refreshRail('tidy');
  $('resultCount').textContent = t('{n} 对待判断', {n: number(page.total)});
  renderHeading();
  const box = $('groupsPage');
  if (!box.querySelector('.pair-list')) {
    box.innerHTML = `<div class="pair-list"></div><div class="pair-more" hidden><button class="quiet" id="identityMore">${t('再显示一些')}</button></div>`;
    $('identityMore').addEventListener('click', () => renderIdentity('more'));
  }
  const list = box.querySelector('.pair-list');
  reconcileSections(list, identity.items.map(x => [pairKey(x), identityCard(x)]), t('没有需要判断的角色了。'));
  box.querySelector('.pair-more').hidden = identity.items.length >= identity.total;
  for (const b of list.querySelectorAll('[data-act]')) b.onclick = () => decideIdentity(b.closest('.pair-card').dataset.pair, b.dataset.act);
  if (keep) $('viewport').scrollTop = scroll;
}
async function decideIdentity(key, act) {
  const x = identity.items.find(y => pairKey(y) === key); if (!x || identity.busy.has(key) || edits.busy) return;
  const card = [...$('groupsPage').querySelectorAll('.pair-card')].find(el => el.dataset.pair === key);
  identity.busy.add(key); card?.classList.add('working');
  // 重复的角色显示在图多的那一边下面。
  const [small, big] = x.a.count <= x.b.count ? [x.a, x.b] : [x.b, x.a];
  const body = act === 'skip' ? null : {status: 'accepted', reason: '身份整理',
    ...{same: {a: small.id, b: big.id, kind: 'duplicate_of'}, b_of_a: {a: x.b.id, b: x.a.id, kind: 'form_of'},
        a_of_b: {a: x.a.id, b: x.b.id, kind: 'form_of'}, different: {a: x.a.id, b: x.b.id, kind: 'different_from'}}[act]};
  try {
    const r = act === 'skip' ? await post('/api/identity/skip', {a: x.a.id, b: x.b.id}) : await post('/api/identity/decision', body);
    written(r.undo, act === 'skip' ? t('已暂不判断') : {same: t('已把 {a} 和 {b} 记为同一角色', {a: small.name, b: big.name}), b_of_a: t('{a} 已作为 {b} 的一个造型', {a: x.b.name, b: x.a.name}),
      a_of_b: t('{a} 已作为 {b} 的一个造型', {a: x.a.name, b: x.b.name}), different: t('已记下 {a} 和 {b} 是不同角色', {a: x.a.name, b: x.b.name})}[act]);
    card?.classList.add('leaving');
    await new Promise(done => setTimeout(done, 170));
    await renderIdentity('keep');
    card?.classList.remove('working', 'leaving');   // 卡片按角色对比对，这一对还在队列里时留下的是同一张卡片
    refreshMeta().catch(e => error(readError(e)));
  } catch (e) { toast(e.message); card?.classList.remove('working'); }
  finally { identity.busy.delete(key); }
}

// ---------- 画师抽查：找回画师的验收（docs/PROVENANCE.md 第 8 节） ----------
// 一份固定的随机样本（服务端按 id 的哈希排序），用和详情面板一样的「是 TA」/「不是」回答，所以每个回答都进撤销历史，
// 同时也修正图库。
const AUDIT_METHODS = {pixel_hash: [t('像素哈希'), t('和 Danbooru 上的帖子是同一个文件，画师取自帖子的画师标签')],
  gallery_match: [t('本地比对'), t('和 TA 公开作品的小图逐张比对一致')], ocr: [t('签名识别'), t('图上的签名或水印')]};
const auditMethod = a => a.methods?.includes('pixel_hash') ? 'pixel_hash' : a.methods?.[0] || '';
async function renderArtistAudit() {
  if (!onPage(state.route, 'artist-audit')) return;
  showArea();
  closeInspector(); $('selectionBar').hidden = true; $('resultCount').textContent = '';
  $('hint').textContent = t('点原帖或画师主页对照 · 看不出来就先留着');
  const n = Number(prefs.get('artistAuditN', 50)) || 50, scroll = $('viewport').scrollTop;
  let d;
  try { d = await api('/api/artist-audit?n=' + n); } catch (e) { $('groupsPage').innerHTML = `<p class="muted-note">${esc(readError(e))}</p>`; return; }
  if (!onPage(state.route, 'artist-audit')) return;
  const pairs = d.items.flatMap(it => it.artists.map(a => ({...a, method: auditMethod(a)})));
  const tally = list => { const yes = list.filter(a => a.confirmed).length, no = list.filter(a => a.rejected).length; return {yes, no, done: yes + no, all: list.length}; };
  const tl = tally(pairs), acc = tl.done ? tl.yes / tl.done : null;
  const byMethod = Object.keys(AUDIT_METHODS).map(m => [m, tally(pairs.filter(a => a.method === m))]).filter(([, x]) => x.all);
  const link = (url, label) => `<a class="link" href="${esc(url)}" target="_blank" rel="noreferrer noopener">${esc(label)}</a>`;
  const artistRow = (it, a) => {
    const m = auditMethod(a), [mName, mHint] = AUDIT_METHODS[m] || [m || t('其他'), ''];
    const data = `data-uuid="${esc(it.uuid)}" data-artist="${esc(a.id)}" data-name="${esc(a.name)}"`;
    const judged = a.confirmed ? `<span class="aa-verdict yes">✓ ${t('对')}</span><button class="aa-b" data-aa="" ${data}>${t('改')}</button>`
      : a.rejected ? `<span class="aa-verdict no">✗ ${t('不对')}</span><button class="aa-b" data-aa="" ${data}>${t('改')}</button>`
      : `<button class="aa-b yes" data-aa="yes" ${data}>${t('对')}</button><button class="aa-b" data-aa="no" ${data}>${t('不对')}</button>`;
    return `<div class="aa-artist"><div class="aa-top"><a class="artist-name" href="${hashFor({page: 'library', artist: a.id})}" title="${t('在图库里只看 TA 的图')}">${esc(a.name)}</a><small>${t('库里 {n} 张', {n: number(a.count)})}</small></div>
      ${a.homes.length ? `<div class="aa-links">${t('主页')} ${a.homes.map(h => link(h.url, h.label)).join(' · ')}</div>` : ''}
      <div class="aa-how" title="${esc(mHint)}">${esc(mName)}</div>
      <div class="aa-btns">${judged}</div></div>`;
  };
  const card = (it, i) => {
    const state_ = it.artists.some(a => a.rejected) ? ' no' : it.artists.length && it.artists.every(a => a.confirmed) ? ' yes' : '';
    return `<div class="aa-card${state_}"><a class="aa-img" href="/image?id=${encodeURIComponent(it.uuid)}&size=full" target="_blank" data-peek="${esc(it.uuid)}" title="${t('看大图')}"><img src="/image?id=${encodeURIComponent(it.uuid)}&size=medium" alt="" loading="lazy"><span class="aa-no">${i + 1}</span></a>
      <div class="aa-body">${it.artists.map(a => artistRow(it, a)).join('')}${it.posts?.length ? `<div class="aa-links">${t('原帖')} ${it.posts.map(p => link(p.url, p.site)).join(' · ')}</div>` : ''}</div></div>`;
  };
  // 判断以后按图比对，只换判断过的那张卡片（#452）；统计和说明每次重写。
  const box = $('groupsPage');
  if (!box.querySelector('.aa-grid')) {
    box.innerHTML = `<div class="aa-summary"></div><p class="muted-note"></p><div class="aa-grid"></div><p class="aa-more" hidden><button class="quiet" id="aaMore">${t('再加 10 张')}</button></p>`;
    $('aaMore').addEventListener('click', () => { prefs.set('artistAuditN', (Number(prefs.get('artistAuditN', 50)) || 50) + 10); renderArtistAudit(); });
  }
  box.querySelector('.aa-summary').innerHTML = `<div class="prov-stat"><b>${acc === null ? '—' : (acc * 100).toFixed(1) + '%'}</b><span>${t('准确率')}<small>${t('目标 ≥ 95%')}</small></span></div>
      <div class="prov-stat"><b>${number(tl.done)}<em> / ${number(tl.all)}</em></b><span>${t('已判断')}<small>${t('对 {yes} · 不对 {no}', {yes: number(tl.yes), no: number(tl.no)})}</small></span></div>
      ${byMethod.map(([m, x]) => `<div class="prov-stat"><b>${x.done ? (x.yes / x.done * 100).toFixed(0) + '%' : '—'}</b><span>${esc(AUDIT_METHODS[m][0])}<small>${t('{done} / {all} 已判', {done: number(x.done), all: number(x.all)})}</small></span></div>`).join('')}`;
  box.querySelector('.muted-note').innerHTML = t('从 {pool} 张有画师的图里按固定的随机顺序取的前 {n} 张。判断会写进图库，判为「不对」的画师不再显示在这张图上。', {pool: number(d.pool), n: number(d.items.length)});
  const grid = box.querySelector('.aa-grid');
  if (d.items.length) reconcileSections(grid, d.items.map((it, i) => [it.uuid, card(it, i)])); else grid.innerHTML = '';
  box.querySelector('.aa-more').hidden = d.items.length >= d.pool;
  $('viewport').scrollTop = scroll;
  for (const b of grid.querySelectorAll('[data-aa]')) {
    b.disabled = false;   // 留下来的卡片上可能还是点下去时禁用的那个按钮（写入失败时）
    b.onclick = async () => {
      const ran = await exclusive(async () => {
        b.disabled = true;
        try { written((await post('/api/artist/verdict', {ids: [b.dataset.uuid], artist: b.dataset.artist, verdict: b.dataset.aa || null, name: b.dataset.name})).undo); }
        catch (e) { toast(e.message); }
        return true;
      });
      if (ran) renderArtistAudit();
    };
  }
}

// 页面这一部分注册的处理函数。页面启动时调用一次。
// 面板页看大图时翻哪一组（ui/detail.js 的 wirePeek）：按组整理是这一组的全部图（不只是露出来的 16 张），身份整理是
// 这一边的样图，画师抽查是这一批。
function peekList(el) {
  const sec = el.closest('.gsec'), side = el.closest('.pair-thumbs');
  if (sec) return groups.list.find(x => x.cid === Number(sec.dataset.cid))?.members || [];
  if (side) return [...side.querySelectorAll('[data-peek]')].map(x => x.dataset.peek);
  return [...$('groupsPage').querySelectorAll('.aa-img[data-peek]')].map(x => x.dataset.peek);
}
// 大图关掉时把最后看的那张缩略图滚进视野（按组整理）。
function revealThumb(uuid) { [...$('groupsPage').querySelectorAll('[data-peek]')].find(x => x.dataset.peek === uuid)?.scrollIntoView({block: 'nearest'}); }
export function wireReview() {
  wirePeek($('groupsPage'), peekList);
  $('groupsPage').addEventListener('click', async e => {
    // 身份整理的样图、画师抽查的图：点一下看大图（#424 的 2-34）。画师抽查的图按住 ⌘／Ctrl、Shift 点还是在新窗口开原图。
    const look = e.target.closest('.pair-thumbs [data-peek], .aa-img[data-peek]');
    if (look && !e.metaKey && !e.ctrlKey && !e.shiftKey) { e.preventDefault(); peekAt(look); return; }
    const sec = e.target.closest('.gsec'); if (!sec) return;
    const g = groups.list.find(x => x.cid === Number(sec.dataset.cid)); if (!g) return;
    const img = e.target.closest('[data-gimg]');
    if (img) {
      const off = groups.off.get(g.cid) || new Set(), u = img.dataset.gimg;
      off.has(u) ? off.delete(u) : off.add(u); groups.off.set(g.cid, off);
      drawGroups(); return;
    }
    const act = e.target.closest('[data-gact]')?.dataset.gact; if (!act) return;
    const off = groups.off.get(g.cid) || new Set(), ids = g.members.filter(u => !off.has(u)), n = ids.length;
    const done = async ok => { if (!ok) return; groups.list = groups.list.filter(x => x !== g); groups.off.delete(g.cid); drawGroups(); };
    if (act === 'yes') done(await mutate('/api/change', {ids, action: 'confirmed', entity: g.lead.id}, false));
    else if (act === 'no') { if (await mutate('/api/change', {ids, action: 'rejected', entity: g.lead.id}, false)) renderGroups(true); }
    else if (act === 'skip') done(await mutate('/api/change', {ids, action: 'unknown'}, false));
    else if (act === 'oc') done(await markOc({ids}, false));
    // 面板顶上写「第 N 组 · M 张」，和缩略图一起（#424 的 2-3）；能多选，和选择栏的「设为…」一样（2-24）。
    else if (act === 'assign') openPicker({title: t('这 {n} 张是…', {n: number(n)}), subjectText: t('第 {id} 组', {id: g.cid}) + ' · ' + t('{n} 张', {n: number(n)}),
      sentence: x => t('把这 {n} 张设为 {name}', {n: number(n), name: x.name}), suggest: {ids}, exclude: g.lead ? [g.lead.id] : [], multi: true,
      pick: async list => done(await mutate('/api/change', {ids, action: 'confirmed', ...targets(list)}, false))});
  });
}

// 复核面板页登记：渲染入口，以及撤销、切换 NSFW 之后怎么重画。
implement({
  // 在这页开的大图里改了图（'edit' 时大图还开着）：重读分组，免得组里还留着已经设了角色、跳过或排除的图。组头按钮的写入
  // 不走这里，它们自己把答过的组从页面上拿掉。
  groups: {render: () => renderGroups(), reveal: revealThumb,
    refresh: why => { if (why === 'undo' || why === 'learned' || why === 'edit' && state.viewer >= 0) return renderGroups(true); else if (why === 'nsfw') renderGroups(); }},
  // 撤销之后重画（#386 的 2-41）：撤销的可能就是「看过了」。撤销、学习落定后按组比对、留在已显示的组数（#601）。
  similar: {render: () => renderSimilar(), refresh: why => { if (why === 'undo' || why === 'learned') return renderSimilar(true); if (why === 'nsfw') return renderSimilar(); }},
  sgroup: {prepare: readSeen, headingHTML: sgroupHeading, wireHeading: () => {
    $('sgroupSeen')?.addEventListener('click', toggleSeen);
    $('sgroupRelate')?.addEventListener('click', async () => openRelate(await listIds(filters(), 12)));
  },
    // 改动、撤销、同步之后组里看得到的张数可能变了，重读一次，免得点「看过了」时张数对不上。
    refresh: async why => { if (why !== 'view' && why !== 'ask') { const route = state.route; if (await readSeen(route) !== false) renderHeading(); } }},
  identity: {render: () => renderIdentity(), refresh: why => { if (why === 'undo') renderIdentity('keep'); else if (why === 'nsfw') renderIdentity(); }},
  // 画师抽查不按 NSFW 筛选（固定的一批随机图）：切换 NSFW 只重画标题里的提醒（#386 的 2-35）。
  'artist-audit': {render: () => renderArtistAudit(), refresh: why => { if (why === 'undo') renderArtistAudit(); else if (why === 'nsfw') renderHeading(); }},
  // 一组图的页面（#/g/<id>，网格页）要先读分组：标题里相近的一组和这组的特征，以及空了时「下一组」的先后。
  group: {prepare: async route => {
    let result; try { result = await api('/api/clusters?nsfw=' + state.nsfw); } catch (e) { if (state.route === route) error(readError(e)); return false; }
    if (state.route !== route) return false;  // 分组加载期间已经导航走了：这个回应不属于任何页面
    const g = result.clusters.find(x => String(x.cid) === route.id);
    state.groupNear = g?.near ? nearText(g) : null; state.groupNearId = g?.near || null; state.groupTraits = g?.traits || [];
    state.groupOrder = groupOrder(result.clusters).map(x => String(x.cid));
  },
  // 一组整理完了（#386 的 2-12）：去下一组，或回到按组整理。
  emptyNext: r => [...(state.groupOrder.some(id => id !== r.id) ? [{label: t('下一组') + ' →', act: 'next-group'}] : []), {label: t('回到按组整理'), href: '#/groups'}],
  emptyAct: act => { if (act === 'next-group') nextGroup(); }},
});
// 「下一组」：重读分组，取原来排在这组后面、还在的第一组；后面都没了就取最前面那组；一组都没了回到按组整理。
async function nextGroup() {
  const at = state.route.id, order = state.groupOrder, from = order.indexOf(at);
  let result; try { result = await api('/api/clusters?nsfw=' + state.nsfw); } catch (e) { error(readError(e)); return; }
  const left = new Set(result.clusters.map(x => String(x.cid))); left.delete(at);
  const pick = order.slice(from + 1).find(id => left.has(id)) || groupOrder(result.clusters).map(x => String(x.cid)).find(id => id !== at);
  location.hash = pick ? '#/g/' + encodeURIComponent(pick) : '#/groups';
}
