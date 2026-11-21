// 资料库的对话框和弹层（#199、#711）：编辑和新建作品、添加新角色、关注作品的选择器、只当标签，以及「全部作品」弹层和关注开关。
// 三个页签和作品页在 ui/catalog_page.js、ui/cast.js、ui/works_tab.js、ui/work_page.js、ui/artist_dir.js；它们读取和更改的内容在
// core/catalog.js。这里改了数据以后发 core/catalog.js 的 catalogChanged，资料库和作品页据此重画，这里不导入它们。
import {t} from '../core/i18n.js';
import {esc, number} from '../core/format.js';
import {composing, onTyped} from '../core/keys.js';
import {session, readError} from '../core/session.js';
import {state} from '../core/state.js';
import {pollIndex, indexAction} from '../core/jobs.js';
import {remoteNames, remoteNote} from '../core/settings.js';
import {catalogChanged, loadTagged, loadWorkTree, loadLocalLinks, castWorks, PSEUDO_NOTES, WORK_NAMES, workName, loadIndex, searchWorks, saveWatch, watchTarget,
  saveWork} from '../core/catalog.js';
import {$, toast, toastMore, error, namesAllowButton, searchIcon} from './dom.js';
import {chooseWork, openDialog} from './dialog.js';
import {attr, make, prop, tpl} from './el.js';
import {createLayer, openModal} from './overlay.js';
import {openPicker, toggleMascot} from './palette.js';
import {combo} from './combo.js';
import {charHash, go} from '../core/route.js';
import {fetchChar, refreshMeta} from '../core/meta.js';

// 名字是 Danbooru tag 的自有角色（只拿这些名字去问 Danbooru；没有就不发请求）。读到了就让资料库重画页头的按钮。
export async function refreshLocalLinks() {
  if (await loadLocalLinks()) catalogChanged.emit('links');
}
const avatar = (c, px) => `<i class="cast-av" style="width:${px}px;height:${px}px">${c?.cover ? `<img src="/image?id=${encodeURIComponent(c.cover)}&size=thumb" alt="" loading="lazy" decoding="async">` : ''}</i>`;

// ---------- 全部作品 ----------
// 资料库工具栏上的「全部作品 N」：点开一个可搜索的作品列表，点一部进它的作品页。非模态浮层，Esc、点外关闭由浮层管理器
// （ui/overlay.js）统一处理；列表随输入重画，所以层按 id 找元素。
const pop = {open: false, q: ''};
const popLayer = createLayer({kind: 'popover', id: 'cast-works', el: () => $('castPop'), anchor: () => $('castAll'), onDismiss: reason => setPop(false, reason)});
function setPop(on, reason) {
  pop.open = on;
  const box = $('castPopHost'), button = $('castAll');
  if (button) button.setAttribute('aria-expanded', String(on));
  if (box) {
    if (on) { box.innerHTML = castPop(castWorks()); popLayer.open(); $('castPopQ')?.focus(); }
    else box.replaceChildren();
  }
  if (!on) popLayer.close(reason);
}
// 工具栏上的按钮和弹层的位置，建一次。
export function allWorksButton() {
  const wrap = make('span', 'cat-allworks'), button = make('button', 'btn cat-allworks-button'), host = make('span');
  button.id = 'castAll'; button.type = 'button'; button.setAttribute('aria-expanded', 'false'); host.id = 'castPopHost';
  wrap.append(button, host);
  button.onclick = () => setPop(!pop.open);
  host.addEventListener('click', e => { const w = e.target.closest('[data-tab]'); if (w) { setPop(false, 'silent'); go({page: 'work', id: w.dataset.tab}); } });
  onTyped(host, e => {
    if (e.target.id !== 'castPopQ') return;
    pop.q = e.target.value; const pos = e.target.selectionStart;
    host.innerHTML = castPop(castWorks()); const i = $('castPopQ'); i?.focus(); i?.setSelectionRange(pos, pos);
  });
  return {el: wrap, update() { button.textContent = t('全部作品 {n}', {n: number(castWorks().filter(w => !w.pseudo).length)}); }};
}
// 导出给样式清单页调用（tools/styleguide/live.js，#352）：改名或改参数时，tools/check_styleguide.py 会报出来。
export function castPop(works) {
  const q = pop.q.trim().toLowerCase(), list = works.filter(w => !q || w.name.toLowerCase().includes(q));
  return `<div class="menu cast-pop" id="castPop"><div class="search-wrap">${searchIcon()}<input type="search" id="castPopQ" placeholder="${t('在 {n} 部作品里找', {n: number(works.filter(w => !w.pseudo).length)})}" value="${esc(pop.q)}" autocomplete="off"></div>
    <div class="cast-wcols">${list.map(w => `<button class="cast-wrow${w.pseudo ? ' pseudo' : ''}" data-tab="${esc(w.key)}"${w.pseudo ? ` title="${esc(PSEUDO_NOTES[w.key])}"` : ''}>${avatar(w.chars[0], 20)}<span>${esc(w.name)}</span><small>${number(w.images)}</small></button>`).join('') || `<p class="muted-note">${t('没有匹配的作品')}</p>`}</div></div>`;
}


// 只当标签（#293）：吉祥物和玩家替身（包括默认的那些）是标签，不是角色。在一处看到全部，把一个角色变成标签，或把它
// 变回来。
export async function openTagged() {
  const d = openDialog({title: t('只当标签的角色'), cls: 'tagged-dialog', backdropClose: true,
    body: `<p class="muted-note">${t('吉祥物、玩家化身（老师、博士、训练员这类）可以只当标签：不算角色，不出现在角色列表里，同框也不算多人图；图上仍挂着这个标签，可以按它筛选。')}</p>
    <div class="tagged-list" id="taggedList"></div>`,
    actions: `<button type="button" class="quiet" id="taggedAdd">${t('把一个角色设为只当标签…')}</button><div class="spacer"></div><button value="ok" class="primary">${t('完成')}</button>`});
  const list = () => {
    const rows = state.tagged || [];
    d.querySelector('#taggedList').innerHTML = rows.length ? rows.map(x => `<div class="tagged-row"><a href="${charHash(x.id)}" data-close>${esc(x.name)}</a><small>${t('{n} 张', {n: number(x.count)})}</small><div class="spacer"></div><button type="button" class="quiet small" data-untag="${esc(x.id)}">${t('改回角色')}</button></div>`).join('')
      : `<p class="muted-note">${t('现在没有只当标签的角色。')}</p>`;
  };
  const refresh = async () => { await loadTagged().catch(e => error(readError(e))); if (d.open) list(); catalogChanged.emit('tagged'); };
  d.addEventListener('click', async e => {
    const b = e.target.closest('[data-untag]'); if (!b) return;
    b.disabled = true;
    const x = state.tagged.find(r => r.id === b.dataset.untag);
    await toggleMascot({id: x.id, name: x.name, mascot: true}); await refresh();
  });
  d.querySelector('#taggedAdd').onclick = () => openPicker({title: t('把一个角色设为只当标签'), jump: true, exclude: (state.tagged || []).map(x => x.id),
    sentence: e => t('{name} 不再算角色，只当标签', {name: e.name}),
    pick: async e => { await toggleMascot({id: e.id, name: e.name, mascot: false}); await refresh(); if (!d.open) openModal(d, {backdropClose: true, id: 'tagged'}); }});
  list();
  refresh();
}

// ---------- 资料库共用的两个小部件 ----------
// 搜索框（三个页签各一个）：关掉拼写检查、自动更正和自动大写（#422）；`/` 聚焦 #toolbar 里的搜索框（ui/grid.js）。
const SEARCH = tpl`<label class="search"><svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="10" cy="10" r="6.5"/><path d="m15 15 5 5"/></svg><input type="search"
  data-ref="input" spellcheck="false" autocorrect="off" autocapitalize="off" autocomplete="off"></label>`;
export function searchField(label) {
  const {el, refs} = SEARCH();
  refs.input.placeholder = label; refs.input.setAttribute('aria-label', label);
  return {el, input: refs.input};
}
// 「关注新角色」开关（作品卡上、作品页横幅上）：点了经 setWatch 保存，开关的状态等索引回来再按它画。
const WATCH = tpl`<label><input type="checkbox" class="switch" data-ref="box"><span data-t>关注新角色</span></label>`;
export function watchToggle(s, className) {
  const {el, refs} = WATCH();
  let work = null, on = false;
  el.className = className;
  s.on(refs.box, 'change', () => { const want = refs.box.checked; prop(refs.box, 'checked', on); if (work) setWatch(work, want); });
  return {el, update(w, watched) {
    work = w; on = !!watched;
    prop(refs.box, 'checked', on);
    attr(el, 'title', w?.watchable ? t('定期检查 Danbooru 上这部作品还没收录的角色（只发送作品名）') : t('先在「编辑作品」里核对 Danbooru 作品 tag，才能关注'));
  }};
}

// ---------- 关注作品（#198） ----------
// 关注一个作品，意味着索引检查会去问 Danbooru（只发作品名）有哪些本地索引还没有的角色。关注开关需要一个用户核对过的
// Danbooru 作品 tag；没有时点开关先打开「编辑作品」去找一个（和 offerWatch 一样）。作品卡和作品页上的开关都走这里。
// w 可以是作品卡上的摘要：打开对话框前换成索引里的完整那份，免得保存时把上级作品这类没带上的字段清掉。
export async function setWatch(w, on) {
  if (on && !w.watchable) { await workDialog(state.index?.works?.find(x => x.id === w.id) || w, {watch: true}); return; }
  const list = (state.index?.watch.works || []).filter(id => id !== w.id);
  await saveWatchList(on ? [...list, w.id] : list);
}
async function saveWatchList(works) {
  try { await saveWatch(works); }
  catch (e) { toast(e.message); }
  catalogChanged.emit('watch');
}
// 来自 Danbooru 搜索：带这个 tag 的已有本地作品，或用户为它挑选或新建的一个。
async function adoptWatch(r) {
  try {
    const id = await watchTarget(r, (title, works) => chooseWork(title, works, '', t('新建作品')));
    if (id === null) return;
    watchDraft = '';
    if (!state.index.watch.works.includes(id)) await saveWatchList([...state.index.watch.works, id]); else catalogChanged.emit('watch');
  } catch (e) { toast(e.message); }
}
// 图片设为角色之后（#293）：它们属于同一个作品时，toast 提议关注这个作品的新角色，或说明它已在关注。没有核对过的
// Danbooru tag 的作品会打开它的对话框去找一个。
export async function offerWatch(ids) {
  const chars = await Promise.all(ids.map(id => state.charMap.get(id) || fetchChar(id).catch(() => null)));
  const works = [...new Set(chars.map(c => c?.kind === 'oc' ? null : c?.work_id).filter(Boolean))];
  if (works.length !== 1) return;
  let x = state.index; if (!x?.works) { try { x = await loadIndex(); } catch { return; } }
  const w = x?.works?.find(y => y.id === works[0]); if (!w) return;
  if (x.watch.works.includes(w.id)) { toastMore(' · ' + t('「{name}」已关注', {name: w.name})); return; }
  toastMore('', w.watchable ? t('关注「{name}」的新角色', {name: w.name}) : t('关注「{name}」…', {name: w.name}), async () => {
    if (!w.watchable) { const r = await workDialog(w, {watch: true}); if (r?.watch) toast(t('已关注「{name}」', {name: r.name})); return; }
    try { await saveWatch([...state.index.watch.works, w.id]); toast(t('已关注「{name}」', {name: w.name})); catalogChanged.emit('watch'); }
    catch (e) { toast(e.message); }
  });
}
export async function createWork() {
  const w = await workDialog(null);
  if (w) toast(w.watch ? t('已新建作品「{name}」并关注它的新角色', {name: w.name}) : t('已新建作品「{name}」', {name: w.name}));
}
// 新建作品和编辑作品共用一个对话框：一个填 tag（名字为空时也填名字）的 Danbooru 搜索、名字、用户核对过的 tag、可选的
// 上级作品，以及（有 tag 时）关注它的新角色（#293）。
// 导出给样式清单页调用（tools/styleguide/live.js，#352）：改名或改参数时，tools/check_styleguide.py 会报出来。
export function workDialog(w, {watch = false} = {}) {
  const works = state.index?.works || [], watched = new Set(state.index?.watch?.works || []);
  return new Promise(resolve => {
    const parents = works.filter(x => !w || x.id !== w.id);
    const body = `${watch && !w?.tag ? `<p class="dialog-lead">${t('关注要用 Danbooru 上的作品 tag。在下面找到这部作品，选中后保存。')}</p>` : ''}
      <label class="field"><span>${t('在 Danbooru 上找')}</span><div class="combo"><input id="wdFind" autocomplete="off" spellcheck="false" placeholder="${t('输入作品名，中文、日文、英文都行')}"><div class="combo-menu" id="wdFindMenu" hidden></div></div>
        <small>${t('选中后自动填好下面的 tag，名称空着时也一起填上。只发送你输入的文字。')}</small></label>
      <label class="field"><span>${t('名称')}</span><input id="wdName" name="title" value="${esc(w?.name || '')}" maxlength="120" required autocomplete="off">
        <small>${t('界面上显示的名字，和 Danbooru tag 无关。')}</small></label>
      <label class="field"><span>${t('Danbooru 作品 tag')}<em class="opt">${t('可不填')}</em></span><input id="wdTag" name="tag" value="${esc(w?.tag || '')}" placeholder="${t('例如 blue_archive')}" spellcheck="false" autocomplete="off">
        <small id="wdTagNote"></small></label>
      <label class="field"><span>${t('上级作品')}<em class="opt">${t('可不填')}</em></span><select id="wdParent" name="parent"><option value="">${t('无')}</option>${parents.map(x => `<option value="${esc(x.id)}" ${w?.parent_id === x.id ? 'selected' : ''}>${esc(x.name)}</option>`).join('')}</select>
        <small>${t('只在这部作品属于某个系列时填，比如「Fate/Grand Order」的上级是「Fate 系列」。在图库里按上级作品筛选时，下级作品的图也算在内。')}</small></label>
      <label class="check-row" id="wdWatchRow"><input type="checkbox" id="wdWatch"><span><b>${t('关注这部作品的新角色')}</b><small>${t('定期检查 Danbooru 上这部作品还没收录的角色（只发送作品名），检查前会确认联网')}</small></span></label>`;
    const d = openDialog({title: w ? t('编辑作品') : t('新建作品'), cls: 'work-dialog', body, backdropClose: false, focus: w && !watch ? '#wdName' : '#wdFind',
      actions: `<button type="button" class="quiet" data-close>${t('取消')}</button><button value="ok" class="primary">${w ? t('保存') : t('新建')}</button>`,
      onClose: async d => {
        const data = new FormData(d.querySelector('form')), ok = d.returnValue === 'ok', watch = d.querySelector('#wdWatch').checked;
        if (!ok) { resolve(null); return; }
        const name = String(data.get('title')).trim(), parent = data.get('parent'), tag = String(data.get('tag')).trim();
        try {
          const id = await saveWork(w?.id, {name, parent, tag});
          if (watch && tag && !state.index.watch.works.includes(id)) await saveWatch([...state.index.watch.works, id]);
          await loadWorkTree().catch(e => error(readError(e)));
          await refreshMeta().catch(e => error(readError(e)));
          catalogChanged.emit('watch');
          resolve({id, name, watch});
        } catch (err) { toast(err.message); resolve(null); }
      }});
    const q = sel => d.querySelector(sel), already = w && watched.has(w.id);
    let filled = '';  // 搜索填进来的名字，下一次挑选会替换它
    const tagNote = () => {
      const tag = q('#wdTag').value.trim(), owner = tag && works.find(x => x.tag === tag && x.id !== w?.id);
      q('#wdTagNote').innerHTML = owner ? `<span class="warn">${t('图库里的「{name}」已经用了这个 tag', {name: esc(owner.name)})}</span>`
        : w?.status === 'pending' && !tag ? t('链接还没确认：核对过就填上 tag；留空保存表示不关联。')
        : t('用上面的搜索填，或手动填你核对过的 tag，不会按名字自动关联。填了才能关注这部作品的新角色。');
      q('#wdWatchRow').hidden = !tag || already; if (!tag) q('#wdWatch').checked = false;
    };
    q('#wdTag').oninput = tagNote; tagNote();
    q('#wdFind').addEventListener('keydown', e => { if (!composing(e) && e.key === 'Enter') e.preventDefault(); });
    combo(q('#wdFind'), q('#wdFindMenu'), (text, show) => {
      if (!text) { show([]); return; }
      show([{label: 'Danbooru', items: [], note: t('正在查 Danbooru…')}]);
      remoteNames('/api/works?' + new URLSearchParams({q: text, remote: 1})).then(r => show([r.found
        ? {label: 'Danbooru', items: r.found.map(x => { const mine = works.find(y => y.tag === x.tag && y.id !== w?.id);
          return {tag: x.tag, label: workName(x.tag) || x.tag, detail: x.tag, extra: mine ? t('图库里已有：{name}', {name: mine.name}) : t('D 站 {n}', {n: number(x.posts)})}; }),
          note: r.found.length ? '' : t('没有找到这个作品。可以换个写法，比如英文或日文名')}
        : {label: 'Danbooru', items: [], note: remoteNote(r), allow: r.unauthorized}]));
    }, r => {
      q('#wdTag').value = r.tag;
      const name = q('#wdName');
      if (!name.value.trim() || name.value === filled) name.value = filled = r.label;
      tagNote(); q('#wdFind').value = '';
      if (!already && (!w || watch)) q('#wdWatch').checked = true;
    });
  });
}
// 上次索引检查在 Danbooru 上为关注的作品找到的角色；添加一个会下载约 20 张公开参考图。
export function openIndexAdd(tag = '') {
  const token = session.token, x = state.index, list = (x?.check?.missing || []).filter(r => !tag || r.copyright === tag);
  if (!list.length) return;
  const body = `<p class="muted-note">${t('这些角色在 Danbooru 上有、本地索引还没有。添加时每个角色下载约 20 张公开参考图（约 {mb} MB），算好特征后重新扫描图库。', {mb: x.mb_per_character})}</p>
    <div class="index-add-list"><table class="settings-table index-table"><tr><th><input type="checkbox" id="iaAll" aria-label="${t('全选')}" checked></th><th>${t('角色 tag')}</th><th>${t('作品')}</th><th>${t('D 站图数')}</th></tr>
      ${list.map(r => `<tr><td><input type="checkbox" data-pick="${esc(r.short_tag)}" ${indexSkip.has(r.short_tag) ? '' : 'checked'}></td><td><a class="link" href="https://danbooru.donmai.us/posts?tags=${encodeURIComponent(r.tag)}" target="_blank" rel="noreferrer noopener">${esc(r.tag)}</a></td><td>${esc(x.works.find(w => w.tag === r.copyright)?.name || workName(r.copyright))}</td><td>${number(r.post_count)}</td></tr>`).join('')}</table></div>`;
  const d = openDialog({title: t('添加新角色'), cls: 'index-add', body, backdropClose: false,
    actions: `<span class="muted-note" id="iaNote"></span><div class="spacer"></div><button value="cancel" class="quiet">${t('取消')}</button><button value="ok" class="primary" id="iaApply"></button>`,
    onClose: async d => {
      const tags = picks().filter(b => b.checked).map(b => b.dataset.pick), ok = d.returnValue === 'ok';
      if (!ok || !tags.length || token !== session.token) return;
      try { await indexAction({action: 'apply', tags, allow_network: true}); toast(t('开始添加 {n} 个角色，进度在任务面板里', {n: number(tags.length)})); pollIndex(); catalogChanged.emit('index'); }
      catch (e) { toast(e.message); }
    }});
  const q = s => d.querySelector(s), picks = () => [...d.querySelectorAll('[data-pick]')];
  const note = () => {
    const n = picks().filter(b => b.checked).length, mb = n * x.mb_per_character;
    q('#iaApply').textContent = t('添加 {n} 个（约 {mb} MB）', {n: number(n), mb: mb >= 10 ? Math.round(mb) : mb.toFixed(1)}); q('#iaApply').disabled = !n;
    q('#iaNote').textContent = mb > 500 ? t('下载量超过 500 MB，网络不方便时可以分几次添加') : '';
    indexSkip = new Set([...indexSkip, ...picks().filter(b => !b.checked).map(b => b.dataset.pick)].filter(t => !picks().some(b => b.checked && b.dataset.pick === t)));
  };
  for (const b of picks()) b.onchange = note;
  q('#iaAll').onchange = e => { for (const b of picks()) b.checked = e.target.checked; note(); };
  note();
}

// ---------- 关注作品：作品树下面的选择器 ----------
let indexSkip = new Set(), watchDraft = '';
// 作品选择器：先是你的图库已有的作品（即时，本地名字和别名），然后是按输入文字查到的 Danbooru copyright tag（只发出
// 文字）。挑一个就得到一个确切的 tag。
export function wireWatchAdd(x, add) {
  const token = session.token, input = $('watchAdd'), menu = $('watchMenu'), watched = new Set(x.watch.works);
  const here = () => token === session.token && input === $('watchAdd');
  const row = (r, i, on) => `<button class="menu-item watch-option${on ? ' on' : ''}" data-i="${i}" role="option" ${watched.has(r.id) || (r.id && !r.watchable) ? 'disabled' : ''}>
    <span class="watch-name">${esc(r.name || workName(r.tag))}</span><small>${esc(r.tag || t('D 站上还没有这个作品'))}</small>
    <em>${watched.has(r.id) ? t('已关注') : [r.count ? t('你有 {n} 张', {n: number(r.count)}) : '', r.posts ? t('D 站 {n}', {n: number(r.posts)}) : ''].filter(Boolean).join(' · ')}</em></button>`;
  // 每个部分都有标题；说明（可能带「允许查询…」按钮，所以是 HTML）放在最下面。
  const draw = (sections, index) => {
    let i = 0;
    const note = sections.map(sec => sec.note).find(Boolean) || '';
    return sections.filter(sec => sec.items.length).map(sec => `<div class="palette-label">${sec.label}</div>` + sec.items.map(r => { const at = i++; return row(r, at, at === index); }).join('')).join('')
      + (note ? `<p class="muted-note watch-note">${note}</p>` : '');
  };
  let remoteTimer = 0;
  const search = async (q, show) => {
    let local; try { local = await searchWorks(q); } catch (e) { if (here()) show([{label: '', items: [], note: esc(readError(e))}]); return; }
    if (!here()) return;
    if (!q) { show([{label: t('你的图库里还没关注的作品'), items: local.filter(r => !watched.has(r.id)).slice(0, 10)}]); return; }
    const mine = local.slice(0, 8);
    show([{label: t('你的图库里有'), items: mine, note: t('正在查 Danbooru…')}]);
    clearTimeout(remoteTimer);
    remoteTimer = setTimeout(async () => {
      const result = await remoteNames('/api/works?' + new URLSearchParams({q, remote: 1})), remote = result.found;
      if (!here()) return;
      const posts = new Map((remote || []).map(r => [r.tag, r.posts]));
      for (const r of mine) if (posts.has(r.tag)) r.posts = posts.get(r.tag);
      const known = new Set(mine.map(r => r.tag));
      const others = (remote || []).filter(r => !known.has(r.tag));
      show([{label: t('你的图库里有'), items: mine}, {label: 'Danbooru', items: others,
        note: remote === null ? (result.unauthorized ? esc(remoteNote(result)) + ' ' + namesAllowButton : esc(t('{reason}，可以稍后再试', {reason: remoteNote(result)}))) : !mine.length && !others.length ? t('没有找到这个作品。可以换个写法，比如英文或日文名') : ''}]);
    }, 450);
  };
  combo(input, menu, search, add, {id: 'watch-menu', delay: 150, draw, first: list => list.findIndex(r => !watched.has(r.id)),
    allowed: r => !watched.has(r.id) && !(r.id && !r.watchable), typed: value => { watchDraft = value; }});
  if (document.activeElement === input || watchDraft) { input.focus(); input.setSelectionRange(input.value.length, input.value.length); }
}
// 作品页签工具栏上的「关注 Danbooru 上的作品…」：关注列表变了就换一个新的输入框（选择器按当时的关注列表建），打了一半的字和焦点
// 留着（watchDraft）。
export function watchAddField() {
  const wrap = make('span', 'watch-add');
  let seen = null;
  return {el: wrap, update(x) {
    if (!x?.watch || x === seen) return;
    seen = x;
    const input = make('input'), menu = make('div', 'menu menu-drop watch-menu');
    Object.assign(input, {id: 'watchAdd', value: watchDraft, placeholder: t('关注 Danbooru 上的作品…'), spellcheck: false, autocomplete: 'off'});
    input.setAttribute('role', 'combobox'); input.setAttribute('aria-expanded', 'false'); input.setAttribute('aria-controls', 'watchMenu');
    Object.assign(menu, {id: 'watchMenu', hidden: true}); menu.setAttribute('role', 'listbox');
    wrap.replaceChildren(input, menu);
    wireWatchAdd(x, adoptWatch);
  }};
}
// 「以前关注的 Danbooru 作品还没对应到本地作品」里的「对应到作品…」。
export const adoptTag = tag => adoptWatch({tag, name: WORK_NAMES[tag]});
