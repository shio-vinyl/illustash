// 对角色的操作（桌面布局，#199）：命令面板（换成 / 也是 / 设为角色 / 跳到角色）、原创角色、编辑角色资料、合并，以及
// 选中图片之间的关系。
import {forgetLocalLinks, workName} from '../core/catalog.js';
import {readProfile} from '../core/character.js';
import {exclusive} from '../core/edits.js';
import {esc, number} from '../core/format.js';
import {t, tb} from '../core/i18n.js';
import {composing, onTyped} from '../core/keys.js';
import {roleName, workLabel} from '../core/labels.js';
import {fetchChar, refreshMeta} from '../core/meta.js';
import {poolIs} from '../core/pages.js';
import {libraryPrefs, prefs} from '../core/prefs.js';
import {charRoute, go, replaceRoute} from '../core/route.js';
import {api, post, readError} from '../core/session.js';
import {currentLibraryId, remoteNames, remoteNote} from '../core/settings.js';
import {state} from '../core/state.js';
import {refreshLocalLinks} from './catalog.js';
import {$, charOption, namesAllowButton, toast, written} from './dom.js';
import {closeModal, openDialog, openStatic} from './dialog.js';
import {combo, listNav} from './combo.js';
import {mutate, reload} from './grid.js';
import {profileDecisions} from './profile_decisions.js';
import {renderChrome, renderHeading} from './shell.js';

// 命令面板搜索的防抖。
let pickerTimer = 0;
// ---------- command palette (换成 / 也是 / 设为角色 / 跳到角色) ----------
let palette = null;
// 命令面板结果的键盘和鼠标（ui/combo.js 的 listNav），高亮的是第几项在它的 index 里。wirePalette 之前（只有测试会这样）
// 用一个只记位置的替身。
let pickerNav = {index: 0, mark() {}};
// 多选面板（设为角色 / 也是）：⇧+点击、⇧+↵ 或行上的 ＋ 收集角色；↵ 一次全部应用。
export const targets = list => list.length > 1 ? {entities: list.map(e => e.id)} : {entity: list[0].id};
// 「换成」接受多个角色；只有一个时照以前的方式发送。
export const newIds = list => list.length > 1 ? list.map(e => e.id) : list[0].id;
const stageKey = e => e.id || (e.ocShared ? 'oc:shared' : e.remote ? 'tag:' + e.tag : 'new:' + e.create);
// 最近使用的标签（tagPicks）和角色（picks）按图库分开存（#523）：标签和角色都属于图库，角色 ID 换了图库甚至指向
// 别的角色。各记最近 8 个。
const recentIn = async () => libraryPrefs(await currentLibraryId());
const remember = (store, key, item, same) => store.set(key, [item, ...store.get(key, []).filter(x => !same(x))].slice(0, 8));
const rememberChar = async entity => remember(await recentIn(), 'picks', {id: entity.id, name: entity.name}, x => x.id === entity.id);
export async function openPicker(opts) {
  const p = palette = {...opts, list: [], index: 0, generation: 0, staged: [], recent: libraryPrefs('')};
  renderStaged(); renderSubject(opts);
  $('pickerSearch').value = ''; $('pickerSearch').placeholder = opts.placeholder || t('输入角色 tag 或别名…');
  openStatic($('picker'), {backdropClose: true, id: 'picker', onDismiss: () => { if (palette === p) palette = null; }}); $('pickerSearch').focus();
  p.recent = await recentIn();
  if (palette === p) await paletteSearch();
}
async function paletteSearch() {
  const p = palette; if (!p) return;
  const gen = ++p.generation, q = $('pickerSearch').value.trim();
  let sections = [];
  if (p.tagMode) {
    const low = q.toLowerCase(), mine = state.userTags.filter(x => !q || x.tag.toLowerCase().includes(low));
    if (p.present.size && !q) sections.push({label: t('去掉已有标签'), items: [...p.present.keys()].map(tag => ({tag, name: tag, remove: true, hint: t('所选图片中 {n} 张有', {n: number(p.present.get(tag))}), counted: true}))});
    const recent = p.recent.get('tagPicks', []).filter(tag => !q || tag.toLowerCase().includes(low));
    if (recent.length) sections.push({label: t('最近使用'), items: recent.map(tag => ({tag, name: tag, hint: t('我的标签')}))});
    const rest = mine.filter(x => !recent.includes(x.tag));
    if (rest.length) sections.push({label: t('我的标签'), items: rest.map(x => ({tag: x.tag, name: x.tag, hint: t('{n} 张', {n: number(x.count)}), counted: true}))});
    if (q) {
      let wd; try { wd = await api('/api/general-tags?q=' + encodeURIComponent(q)); } catch (e) { if (gen === p.generation && palette === p) renderPalette([{label: t('本地标签'), items: [], note: readError(e)}], q); return; }
      if (gen !== p.generation || palette !== p) return;   // 换了一次搜索，或面板已经关了
      const known = new Set(state.userTags.map(x => x.tag));
      if (wd.length) sections.push({label: t('采纳 WD 通用标签'), items: wd.filter(tag => !known.has(tag)).slice(0, 8).map(tag => ({tag, name: tag, hint: t('机器词表，采纳后成为你的标签')}))});
      if (!known.has(q) && !wd.includes(q)) sections.push({label: '', items: [{tag: q, name: q, create: q, hint: t('新建标签')}]});
    }
  } else if (p.items) sections = [{label: '', items: p.items.filter(e => !q || roleName(e).toLowerCase().includes(q.toLowerCase()))}];
  else if (q) {
    let list; try { list = await api('/api/entities?q=' + encodeURIComponent(q)); } catch (e) { if (gen === p.generation && palette === p) renderPalette([{label: t('本地角色'), items: [], note: readError(e)}], q); return; }
    if (gen !== p.generation || palette !== p) return;   // 换了一次搜索，或面板已经关了
    const items = list.filter(e => !(p.exclude || []).includes(e.id)).map(e => ({...e, hint: e.matched ? t('别名：{name}', {name: e.matched}) : '', count: e.count}));
    sections = [{label: '', items}];
    if (!p.jump) {
      // 还不在图库里：Danbooru 角色（只用输入的文字）稍后就到；只在本地新建的一项留在最后。
      // 原创角色不按名字新建，只给共用的「原创角色（未细分）」（ocShared，选中时才建；#694）。它已在本地结果里时不重复。
      const shared = list.some(e => e.label_only && e.kind === 'oc') ? []
        : [{ocShared: true, name: t('原创角色（未细分）'), hint: t('画师的原创角色，不属于任何作品')}];
      const create = list.some(e => e.name.toLowerCase() === q.toLowerCase()) ? [] : [{create: q, name: q, hint: t('只在本地新建，不关联 Danbooru')}, ...shared];
      const remote = {label: t('Danbooru 角色 · 选中即新建并关联'), items: [], note: t('正在查 Danbooru…')};
      sections.push(remote, {label: '', items: create});
      clearTimeout(p.remoteTimer);
      p.remoteTimer = setTimeout(async () => {
        const result = await remoteNames('/api/entities?' + new URLSearchParams({q, remote: 1}));
        if (gen !== p.generation || palette !== p) return;
        remote.items = (result.found || []).map(r => ({...r, remote: true, typed: q, hint: t('D 站 {n} 张', {n: number(r.posts)}), counted: true}));
        remote.note = result.found ? '' : result.unauthorized ? t('{reason}，也可以先只在本地新建', {reason: remoteNote(result)}) : t('{reason}，可以稍后再试，或先只在本地新建', {reason: remoteNote(result)});
        remote.allow = result.unauthorized;
        // 没动过：最佳匹配获得高亮（第一个本地结果，否则是第一个 Danbooru 结果）。
        renderPalette(sections, q, p.touched && pickerNav.index > 0);
      }, 380);
    }
  } else {
    if (p.suggest) {
      let sug; try { sug = await post('/api/suggest', p.suggest); } catch (e) { if (gen === p.generation && palette === p) renderPalette([{label: t('本地候选'), items: [], note: readError(e)}], q); return; }
      if (gen !== p.generation || palette !== p) return;   // 换了一次搜索，或面板已经关了
      if (sug.length) sections.push({label: t('也可能是'), items: sug.filter(e => !(p.exclude || []).includes(e.id)).map(e => ({...e, hint: e.count > 1 ? t('{n} 张的机器候选', {n: number(e.count)}) : t('机器候选'), counted: e.count > 1}))});
    }
    const seen = new Set(sections.flatMap(s => s.items.map(e => e.id)));
    const recent = p.recent.get('picks', []).filter(e => !seen.has(e.id) && !(p.exclude || []).includes(e.id));
    if (recent.length) {
      // 最近记录只存 ID 和原名。先取当前目录，目录外的零张角色按 ID 读取现名与限定词。
      const items = await Promise.all(recent.map(e => state.charMap.get(e.id) || fetchChar(e.id).catch(() => e)));
      if (gen !== p.generation || palette !== p) return;
      sections.push({label: t('最近使用'), items: items.filter(Boolean)});
      recent.forEach(e => seen.add(e.id));
    }
    const top = state.chars.filter(e => !seen.has(e.id) && !(p.exclude || []).includes(e.id)).slice(0, 8);
    if (top.length) sections.push({label: t('常用'), items: top});
  }
  if (palette === p) renderPalette(sections, q);
}
function renderPalette(sections, q, keep = false) {
  if (!palette) return;   // 面板在退场中（Esc 时已经作废）：迟到的结果不写
  const p = palette, current = keep ? p.list[pickerNav.index] : null;
  p.list = sections.flatMap(s => s.items);
  pickerNav.index = current ? Math.max(0, p.list.indexOf(current)) : 0;
  // 只在本地新建和共用原创角色是兜底：只要有别的匹配，它们绝不是默认项。
  const fallback = e => e.create || e.ocShared;
  if (!current && fallback(p.list[pickerNav.index] || {})) pickerNav.index = Math.max(0, p.list.findIndex(e => !fallback(e)));
  let n = 0;
  $('pickerResults').innerHTML = sections.filter(s => s.items.length || s.note).map(s => (s.label ? `<div class="palette-label">${s.label}</div>` : '') + (s.note && !s.items.length ? `<p class="palette-note">${esc(s.note)}${s.allow ? ' ' + namesAllowButton : ''}</p>` : '') + s.items.map(e => {
    const i = n++, char = e.id && state.charMap.get(e.id), cover = char?.cover;
    const work = p.tagMode || e.create || e.remove ? '' : e.remote ? e.work_name || '' : e.id ? workLabel(e) : '';   // 标签、新建、移除这几种不是角色
    const staged = p.multi && p.staged.some(x => stageKey(x) === stageKey(e));
    const add = p.multi ? `<span class="palette-add${staged ? ' on' : ''}" data-add="${i}" title="${staged ? t('已加入，再点一次取消') : t('再加一个角色（⇧+点击）')}">${staged ? '✓' : '＋'}</span>` : '';
    return `<button class="palette-item${staged ? ' staged' : ''}" data-i="${i}" role="option">${cover ? `<img src="/image?id=${encodeURIComponent(cover)}" alt="" loading="lazy">` : e.remote ? `<img class="remote" src="/api/entity-preview?tag=${encodeURIComponent(e.tag)}" alt="" loading="lazy" title="${t('Danbooru 上评分最高的一张全年龄图')}">` : `<span class="palette-glyph${e.remote ? ' remote' : ''}">${e.remote ? 'D' : e.create ? '＋' : e.remove ? '−' : e.tag ? '#' : '◌'}</span>`}<span class="palette-text"><span class="palette-name">${e.create ? t('新建「{name}」', {name: esc(e.name)}) : charOption(e, work)}</span><small>${esc(e.hint || '')}${char && !e.counted ? ' ' + t('{n} 张', {n: number(char.count)}) : ''}</small></span>${i < 9 && !q ? `<kbd>${i + 1}</kbd>` : ''}${add}</button>`;
  }).join('')).join('') || `<p class="palette-empty">${q ? t('没有匹配的角色') : t('输入角色 tag 或别名')}</p>`;
  for (const img of $('pickerResults').querySelectorAll('img')) img.addEventListener('error', () => img.replaceWith(Object.assign(document.createElement('span'), img.classList.contains('remote') ? {className: 'palette-glyph remote', textContent: 'D'} : {className: 'palette-glyph', textContent: '◌'})), {once: true});
  highlight();
}
function highlight() {
  const p = palette; if (!p) return;
  pickerNav.mark();
  const e = p.list[pickerNav.index], chosen = p.multi ? finalList(e, p.touched) : e ? [e] : [];
  const names = chosen.map(roleName).join(t('、'));
  const sentence = chosen.length && p.sentence ? p.sentence({...chosen[0], name: names}) : names;
  $('pickerFoot').innerHTML = chosen.length ? `<kbd>↵</kbd> ${esc(sentence)}${p.multi ? `<span class="palette-tip"><kbd>⇧↵</kbd> ${p.staged.length ? t('继续加') : t('多选')}</span>` : ''}` : `<span>${esc(p.title || '')}</span>`;
}
// 暂存的角色加上高亮的那个（除非它已经暂存了）。
// 刚暂存完时，↵ 只应用暂存的角色；用户移动、输入或点击之后，高亮的那个才重新算进去。
function finalList(e, include) {
  const p = palette, list = [...p.staged];
  if (e && (include || !list.length) && !e.remove && !list.some(x => stageKey(x) === stageKey(e))) list.push(e);
  return list;
}
// 面板顶上的作用对象（#424 的 2-3）：要改的是哪几张图（最多画 8 张缩略图）和面板的标题（如「把这 10 张设为…」，按组整理是
// 「第 2 组 · 10 张」），认名字时不用关掉面板去看图。作用于图片的面板才有：调用处给了 subject（uuid 列表；全选时为空，
// 只写张数），或给了 suggest（按这些图推荐候选）；跳到角色、合并角色没有。
const SUBJECT_MAX = 8;
function renderSubject(opts) {
  const box = $('pickerSubject'), ids = opts.subject || opts.suggest?.ids || (opts.suggest ? [] : null);
  box.hidden = !ids;
  if (!ids) { box.innerHTML = ''; return; }
  const more = ids.length - SUBJECT_MAX, text = opts.subjectText || opts.title || '';
  box.innerHTML = ids.slice(0, SUBJECT_MAX).map(u => `<img src="/image?id=${encodeURIComponent(u)}&size=thumb" alt="" loading="lazy" draggable="false">`).join('')
    + `<span class="palette-label">${more > 0 ? `+${number(more)} · ` : ''}${esc(text)}</span>`;
}
function renderStaged() {
  const p = palette, box = $('pickerStaged');
  box.hidden = !p?.staged.length;
  box.innerHTML = (p?.staged || []).map((e, i) => {
    const label = e.create ? t('新建「{name}」', {name: e.name}) : roleName(e);
    return `<button class="staged-chip" data-unstage="${i}" title="${t('移除')}">${esc(label)} ×</button>`;
  }).join('');
}
// 导出给样式清单页调用（tools/styleguide/live.js，#352）：改名或改参数时，tools/check_styleguide.py 会报出来。
export function stage(e) {
  const p = palette, k = stageKey(e), at = p.staged.findIndex(x => stageKey(x) === k);
  if (at >= 0) p.staged.splice(at, 1); else p.staged.push(e);
  p.touched = false; renderStaged(); $('pickerSearch').value = ''; $('pickerSearch').focus(); paletteSearch();
}
async function resolveEntity(e) {
  const p = palette;
  const entity = e.remote ? await post('/api/entities', {tag: e.tag, typed: e.typed})
    : e.ocShared ? await post('/api/oc/shared', {kind: 'oc'})
    : e.create ? await post('/api/entities', {name: e.create}) : e;
  if (p && !p.items) remember(p.recent, 'picks', {id: entity.id, name: entity.name}, x => x.id === entity.id);
  return entity;
}
async function choose(i, more = false, explicit = false) {
  const p = palette, e = p?.list[i];
  if (p?.multi) {
    if (more) { if (e) stage(e); return; }
    const list = finalList(e, explicit || p.touched); if (!list.length) return;
    const entities = [];
    // 一个个建：每个回来都核对面板还是这一次的，关掉以后剩下的不再发（不多建角色）
    try {
      for (const x of list) { entities.push(await resolveEntity(x)); if (palette !== p) return; }
    } catch (err) { if (palette === p) toast(err.message); return; }
    palette = null; closeModal($('picker')); p.pick(entities); return;
  }
  if (!e) return;
  if (p.tagMode) {
    if (!e.remove) remember(p.recent, 'tagPicks', e.tag, tag => tag === e.tag);
    palette = null; closeModal($('picker')); p.pick(e); return;
  }
  let entity; try { entity = await resolveEntity(e); } catch (err) { if (palette === p) toast(err.message); return; }
  if (palette !== p) return;
  palette = null; closeModal($('picker')); p.pick(entity);
}
export function mergeCharacter(c) {
  openPicker({title: t('把 {name} 合并到…', {name: roleName(c)}), placeholder: t('输入同一个人的另一个角色'), jump: true, exclude: [c.id],
    sentence: e => t('把 {name} 并入 {target}：图、人工结论、造型和别名都过去', {name: roleName(c), target: e.name}),
    pick: async e => {
      try { const r = await post('/api/merge', {source: c.id, target: e.id}); written(r.undo); await refreshMeta(); go(charRoute(e.id)); }
      catch (err) { toast(err.message); }
    }});
}
// ---------- 原创角色 ----------
// 原创角色（kind='oc'）：画师自己的角色，不属于任何作品。
// 界面上的「标为原创角色」（选中图的工具条、按组整理页的组头、组页标题的「整组标为原创角色」、角色选择器）一律设为
// 共用的「原创角色（未细分）」：只当标签，不学习、不出候选。按长相聚出的一组常混着几位画师各自的 OC，不再整组新建
// 独立的原创角色（用户 2026-11-19 定，#694；以后怎么细分见 #695）。后端仍能建独立原创角色，已有的照常显示。返回写入是否成功。
export async function markOc(body, leaving) {
  let entity; try { entity = await post('/api/oc/shared', {kind: 'oc'}); } catch (err) { toast(err.message); return false; }
  await rememberChar(entity);
  const ok = await mutate('/api/change', {...body, action: 'confirmed', entity: entity.id}, leaving);
  if (ok && poolIs(state.route, 'cluster') && !state.total) go(charRoute(entity.id));
  return !!ok;
}
// 拆分共用的 OC 标记：按需计算相似分组，每组一次点击归到它自己的 OC（可撤销）。
// 界面入口（角色页的「按相似细分」）随 #694 隐藏，只剩样式清单调用；留给 #695 重新设计细分时参考。
export async function splitOc(c) {
  const d = $('ocSplitDialog'), body = $('ocSplitBody');
  $('ocSplitTitle').textContent = t('按相似细分 · {name}', {name: c.name});
  body.innerHTML = `<p class="muted-note">${t('正在比较长相…')}</p>`;
  openStatic(d, {backdropClose: true, id: 'oc-split'});
  let r; try { r = await api('/api/oc/groups?' + new URLSearchParams({entity: c.id})); } catch (e) { body.innerHTML = `<p class="muted-note">${esc(readError(e))}</p>`; return; }
  const grouped = r.groups.reduce((n, g) => n + g.length, 0);
  body.innerHTML = `<p class="muted-note">${t('{n} 张里有 {groups} 组长得像（共 {grouped} 张）；其余的不像任何一张，留在这里。', {n: number(r.total), groups: number(r.groups.length), grouped: number(grouped)})}</p>` +
    (r.groups.map((g, i) => `<section class="oc-split-group" data-g="${i}"><div class="oc-split-thumbs">${g.slice(0, 8).map(u => `<img src="/image?id=${encodeURIComponent(u)}" alt="" loading="lazy">`).join('')}${g.length > 8 ? `<span>+${number(g.length - 8)}</span>` : ''}</div><div class="oc-split-meta"><span>${t('{n} 张', {n: number(g.length)})}</span><button class="quiet" data-split="${i}">${t('设为新的原创角色')}</button></div></section>`).join('') || `<p class="palette-empty">${t('没有长得像、能成组的图。')}</p>`);
  for (const b of body.querySelectorAll('[data-split]')) b.onclick = async () => {
    const ids = r.groups[Number(b.dataset.split)]; b.disabled = true;
    let entity; try { entity = await post('/api/entities', {kind: 'oc'}); } catch (e) { toast(e.message); b.disabled = false; return; }
    if (await mutate('/api/reassign', {ids, old: c.id, new: entity.id}, true)) b.closest('.oc-split-group').remove();
    else b.disabled = false;
  };
}
// ---------- 角色资料（编辑资料） ----------
// 先是本地结果（即时），然后只用输入的文字查 Danbooru。
export function twoStage(local, remote, extra = () => []) {
  return (q, show) => {
    local(q).then(mine => {
      if (!q) { show([{label: '', items: mine}, ...extra(q)]); return; }
      show([{label: '', items: mine}, {label: 'Danbooru', items: [], note: t('正在查 Danbooru…')}, ...extra(q)]);
      remote(q).then(result => {
        // 图库里已有的 Danbooru 条目只显示一次，显示为本地的那条。
        const found = result.found, fresh = found && found.filter(r => !mine.some(m => m.detail && m.detail === r.detail));
        show([{label: '', items: mine}, {label: 'Danbooru', items: fresh || [], allow: result.unauthorized,
          note: found === null ? remoteNote(result) : fresh.length ? '' : found.length ? '' : t('没有找到，可以换英文或日文写法')}, ...extra(q)]);
      });
    }).catch(e => show([{label: t('本地资料'), items: [], note: readError(e)}]));
  };
}
export async function editProfile(c) {
  let p; try { p = await readProfile(c.id); } catch (e) { toast(e.message); return; }
  const draft = {aliases: [...p.aliases], work: p.work_id ? {id: p.work_id, label: p.work_name} : null, tag: p.tag};
  const autoName = p.tag && !p.user_name ? p.name : '';
  const body = `${p.label_only ? '' : `<label class="field"><span>${t('Danbooru 角色')}</span><div class="combo"><input id="pfTag" autocomplete="off" spellcheck="false" placeholder="${t('搜索角色名或 tag，如 karen blue、カレン')}"><div class="combo-menu" id="pfTagMenu" hidden></div></div>
      <small id="pfTagNote"></small></label>`}
    <label class="field"><span>${t('显示名')}</span><input id="pfName" maxlength="120" value="${esc(p.user_name)}" placeholder="${esc(autoName ? t('留空：用自动名称「{name}」', {name: autoName}) : t('角色名称'))}">
      <small>${p.tag ? t('关联了 Danbooru 时可以留空，中文名会自动补全') : t('没有关联 Danbooru 时需要一个名称')}</small></label>
    <div class="field"><span>${t('别名')}</span><div class="alias-box" id="pfAliases"></div><input id="pfAlias" maxlength="120" placeholder="${t('输入别名后按回车')}">
      <small>${p.other_aliases ? t('另有 {n} 个来自 Danbooru 等来源的别名，会自动更新', {n: number(p.other_aliases)}) : ''}</small></div>
    <label class="field"><span>${t('所属作品')}</span>${p.oc ? `<small>${t('原创角色不属于任何作品')}</small>` : `<div class="combo"><input id="pfWork" autocomplete="off" placeholder="${t('搜索作品，找不到可以新建')}"><div class="combo-menu" id="pfWorkMenu" hidden></div></div><small id="pfWorkNote"></small>`}</label>
    <div id="pfDecisions" hidden></div>`;
  // 填了内容（和打开时不一样，或输入框里还有没按回车、没选中的字）时 Esc 先确认（#422 的 2-20）。
  const typed = () => !!q('#pfAlias').value.trim() || !!q('#pfTag') && q('#pfTag').value !== (draft.tag ? draft.tag.replaceAll('_', ' ') : '')
    || !!q('#pfWork') && q('#pfWork').value !== (draft.work?.label || '');
  const d = openDialog({title: t('编辑资料'), body, backdropClose: false, focus: !p.label_only && !p.tag ? '#pfTag' : '#pfName', dirty: () => Object.keys(changes()).length > 1 || typed(),
    actions: `<button type="button" class="quiet" data-close>${t('取消')}</button><button class="primary" id="pfSave">${t('保存')}</button>`});
  const close = () => closeModal(d);
  const q = sel => d.querySelector(sel);
  const aliases = () => { q('#pfAliases').innerHTML = draft.aliases.map((a, i) => `<button type="button" class="staged-chip" data-a="${i}" title="${t('移除')}">${esc(a)} ×</button>`).join(''); };
  q('#pfAliases').onclick = e => { const b = e.target.closest('[data-a]'); if (b) { draft.aliases.splice(Number(b.dataset.a), 1); aliases(); } };
  q('#pfAlias').onkeydown = e => { if (composing(e)) return; const v = e.target.value.trim(); if (e.key === 'Enter') { e.preventDefault(); if (v && !draft.aliases.includes(v)) draft.aliases.push(v); e.target.value = ''; aliases(); } };
  aliases();
  profileDecisions(q('#pfDecisions'), p, async () => {
    await refreshMeta();
    const {char, variant} = state.route;
    if (char !== p.id) return;
    if (variant && !state.char?.variants?.some(v => v.id === variant)) replaceRoute({variant: ''});
    renderChrome();
    await reload();
  });
  const tagNote = () => { if (!q('#pfTagNote')) return; q('#pfTag').value = draft.tag ? draft.tag.replaceAll('_', ' ') : ''; q('#pfTagNote').innerHTML = draft.tag ? `${t('已关联 {tag}', {tag: `<code>${esc(draft.tag)}</code>`})} · <button type="button" class="link-button" id="pfUnlink">${t('取消关联')}</button>` : t('补上中文名、别名和所属作品'); q('#pfUnlink')?.addEventListener('click', () => { draft.tag = ''; tagNote(); }); };
  const workNote = () => { if (!q('#pfWork')) return; q('#pfWork').value = draft.work?.label || ''; q('#pfWorkNote').innerHTML = draft.work ? `${draft.work.create ? t('保存时新建这个作品') + ' · ' : draft.work.tag ? t('保存时从 Danbooru 建立这个作品') + ' · ' : ''}<button type="button" class="link-button" id="pfNoWork">${t('不归属任何作品')}</button>` : (draft.tag && draft.tag !== p.tag && !p.work_id ? (draft.tagWork ? t('留空：按 Danbooru 自动设置（{work}）', {work: draft.tagWork}) : t('留空：按 Danbooru 自动设置')) : t('未归类')); q('#pfNoWork')?.addEventListener('click', () => { draft.work = null; workNote(); }); };
  tagNote(); workNote();
  if (q('#pfTag')) combo(q('#pfTag'), q('#pfTagMenu'), (text, show) => {
    if (!text) { show([]); return; }
    show([{label: 'Danbooru', items: [], note: t('正在查 Danbooru…')}]);
    remoteNames('/api/entities?' + new URLSearchParams({q: text, remote: 1})).then(({found, ...result}) => show([found
      ? {label: 'Danbooru', items: found.map(r => ({...r, label: r.name, detail: r.tag, extra: [r.work_name, t('D 站 {n}', {n: number(r.posts)})].filter(Boolean).join(' · ')})), note: found.length ? '' : t('没有找到，或已经在图库里（那就用「合并到另一个角色」）')}
      : {label: 'Danbooru', items: [], note: remoteNote(result), allow: result.unauthorized}]));
  }, r => { draft.tag = r.tag; draft.tagWork = r.work_name; tagNote(); workNote(); });
  if (q('#pfWork')) combo(q('#pfWork'), q('#pfWorkMenu'), twoStage(
    text => api('/api/works?' + new URLSearchParams({q: text})).then(ws => ws.slice(0, 8).map(w => ({id: w.id, label: w.name, detail: w.tag || '', extra: w.count ? t('你有 {n} 张', {n: number(w.count)}) : ''}))),
    text => remoteNames('/api/works?' + new URLSearchParams({q: text, remote: 1})).then(r => r.found ? {found: r.found.map(w => ({tag: w.tag, label: workName(w.tag) || w.tag, detail: w.tag, extra: t('D 站 {n}', {n: number(w.posts)})}))} : r),
    text => text ? [{label: '', items: [{create: text, label: t('新建作品「{name}」', {name: text}), detail: t('只在本地')}]}] : []),
    r => { draft.work = r; workNote(); });
  // 要保存的改动：只有 entity 一项就是没改。
  function changes() {
    const body = {entity: p.id};
    const name = q('#pfName').value.trim(); if (name !== p.user_name) body.name = name;
    if (JSON.stringify(draft.aliases) !== JSON.stringify(p.aliases)) body.aliases = draft.aliases;
    if (q('#pfTag') && draft.tag !== p.tag) body.tag = draft.tag;
    // 新作品和它所属的资料在同一个请求（和事务）里建好。新建项和 Danbooru 项没有 id，一律算改动；只有选的是
    // 已有作品、id 和原来相同才算没改（#464）。
    if (q('#pfWork') && (draft.work ? !draft.work.id || draft.work.id !== p.work_id : !!p.work_id)) {
      body.work = !draft.work ? '' : draft.work.create ? {create: draft.work.create} : draft.work.tag ? {tag: draft.work.tag} : draft.work.id;
    }
    return body;
  }
  q('form').onsubmit = async e => {
    e.preventDefault();
    if (q('#pfSave').disabled) return;  // 同一时间只保存一次：双击不会多出第二个作品或资料
    const body = changes();
    if (Object.keys(body).length === 1) { close(); return; }
    q('#pfSave').disabled = true;
    try {
      const r = await post('/api/character/profile', body);
      close(); written(r.undo);
      forgetLocalLinks(); await refreshMeta(); renderHeading();
    } catch (err) { q('#pfSave').disabled = false; toast(err.message); }
  };
}
export async function openLocalLinks() {
  const items = state.localLinks || [];
  const links = items.filter(x => x.kind === 'link'), merges = items.filter(x => x.kind === 'merge');
  const body = `${links.length ? `<p class="muted-note">${t('关联后补上中文名、别名和所属作品，原来的名字留作别名。名字和 tag 完全一致的已勾选；标「可能是」的是按名字搜到的最接近的一个，确认后再勾选。')}</p>
      <div class="link-list">${links.map(x => `<label class="link-row"><input type="checkbox" data-link="${esc(x.id)}"${x.guess ? '' : ' checked'}><span><b>${esc(x.name)}${x.guess ? ` <i class="guess">${t('可能是')}</i>` : ''}</b><small>${esc(x.tag)}${x.work_name ? ' · ' + esc(x.work_name) : ''} · ${t('D 站 {n} 张', {n: number(x.posts)})}</small></span><em>${t('{n} 张', {n: number(x.count)})}</em></label>`).join('')}</div>` : ''}
    ${merges.length ? `<p class="muted-note">${t('下面的自建角色和图库里已有的角色像是同一个，合并后图和人工结论都会过去：')}</p>
      <div class="link-list">${merges.map(x => `<div class="link-row"><span><b>${esc(x.name)}${x.guess ? ` <i class="guess">${t('可能是')}</i>` : ''}</b><small>${t('{n} 张', {n: number(x.count)})} → ${esc(x.target.name)} · ${esc(x.tag)}</small></span><button type="button" class="quiet" data-merge="${esc(x.id)}">${t('合并到「{name}」', {name: esc(x.target.name)})}</button></div>`).join('')}</div>` : ''}
`;
  const d = openDialog({title: t('关联自建角色'), body, backdropClose: false,
    actions: `<button type="button" class="quiet" data-close>${t('关闭')}</button>${links.length ? '<button class="primary" id="llApply"></button>' : ''}`});
  const close = () => closeModal(d);
  const count = () => { const n = d.querySelectorAll('[data-link]:checked').length; if ($('llApply')) { $('llApply').textContent = t('关联选中的 {n} 个', {n: number(n)}); $('llApply').disabled = !n; } };
  d.addEventListener('change', count);
  count();
  for (const b of d.querySelectorAll('[data-merge]')) b.onclick = async () => {
    const x = merges.find(m => m.id === b.dataset.merge); b.disabled = true;
    try { const r = await post('/api/merge', {source: x.id, target: x.target.id}); written(r.undo); b.closest('.link-row').remove(); await refreshMeta(); refreshLocalLinks(); }
    catch (e) { b.disabled = false; toast(e.message); }
  };
  d.querySelector('form').onsubmit = async e => {
    e.preventDefault();
    const chosen = links.filter(x => d.querySelector(`[data-link="${CSS.escape(x.id)}"]`)?.checked).map(x => ({id: x.id, tag: x.tag}));
    if (!chosen.length) return;
    $('llApply').disabled = true; $('llApply').textContent = t('正在关联…');
    try { const r = await post('/api/local-links', {items: chosen}); close(); written(r.undo); await refreshMeta(); refreshLocalLinks(); }
    catch (err) { count(); toast(err.message); }
  };
}
export function toggleOc(c) {
  return editCharacterFlag('/api/kind', {entity: c.id, kind: c.kind === 'oc' ? '' : 'oc'});
}
export function toggleMascot(c) {
  return editCharacterFlag('/api/character/mascot', {entity: c.id, mascot: !c.mascot});
}
function editCharacterFlag(path, body) {
  return exclusive(async () => {
    try {
      const {undo} = await post(path, body);
      if (undo) written(undo);
      await refreshMeta(); await reload();
    } catch (err) { toast(err.message); }
  });
}
// ---------- 选中图片之间的关系（relate，native/terms/src/research_compute/relations.rs） ----------
// 每次打开一个序号，关掉时作废：迟到的回复（成功或失败）只写给它自己那一次，不盖掉下一组选图的结论（#715）。
let relateSeq = 0;
export async function openRelate(ids) {
  const d = $('relateDialog'), seq = ++relateSeq, current = () => seq === relateSeq;
  $('relateTitle').textContent = t('{n} 张图的关系', {n: number(ids.length)});
  $('relateBody').innerHTML = `<p class="muted-note">${t('正在逐对比较…')}</p>`;
  openStatic(d, {backdropClose: true, id: 'relate', onDismiss: () => { if (current()) relateSeq++; }});
  let r; try { r = await post('/api/relate', {ids}); } catch (e) { if (current()) $('relateBody').innerHTML = `<p class="muted-note">${esc(readError(e))}</p>`; return; }
  if (!current()) return;
  const no = u => ids.indexOf(u) + 1;
  const figure = (u, boxes) => `<figure class="rel-fig"><div class="rel-img"><img src="/image?id=${encodeURIComponent(u)}&size=medium" alt="">${boxes.map(b =>
    `<i class="${b.match ? 'match' : 'diff'}" style="left:${(b.x * 100).toFixed(2)}%;top:${(b.y * 100).toFixed(2)}%;width:${(b.w * 100).toFixed(2)}%;height:${(b.h * 100).toFixed(2)}%"></i>`).join('')}<span class="rel-no">${no(u)}</span></div>
    <figcaption>${r.sizes[u] ? `${number(r.sizes[u].width)} × ${number(r.sizes[u].height)}` : ''}</figcaption></figure>`;
  const shown = r.pairs.filter(p => p.kind !== 'unrelated'), rest = r.pairs.length - shown.length;
  const summary = [...r.same.map(g => t('第 {list} 张是同一张图', {list: g.map(no).join(t('、'))})), ...(r.missing.length ? [t('{n} 张读不到原图（可能还在 iCloud），没有比较', {n: number(r.missing.length)})] : [])];
  $('relateBody').innerHTML = `${summary.length ? `<p class="rel-summary">${summary.map(esc).join(t('；'))}</p>` : ''}
    ${shown.map(p => `<section class="rel-pair"><div class="rel-head"><span class="rel-kind k-${p.kind}">${esc(tb(p.label))}</span><span>${esc(tb(p.note || ''))}</span></div>
      <div class="rel-figs">${figure(p.a, p.boxes_a)}${figure(p.b, p.boxes_b)}</div></section>`).join('')}
    ${rest ? `<p class="muted-note">${shown.length ? t('另外 {n} 对是不同的图', {n: number(rest)}) : t('{n} 对是不同的图', {n: number(rest)})}</p>` : ''}
    <p class="dialog-note">${t('红框：两张不一样的地方；蓝色虚线：小图在大图里的位置。结果不保存。')}</p>`;
}
export function jumpToCharacter() { openPicker({title: t('跳到角色'), jump: true, sentence: e => t('打开 {name}', {name: e.name}), pick: e => go(charRoute(e.id))}); }

// 页面这一部分注册的处理函数。页面启动时调用一次。
export function wirePalette() {
  // 0.2.0 及以前不分图库存的最近使用：不知道是哪个图库的，丢掉（#523）。
  prefs.drop('tagPicks'); prefs.drop('picks');
  onTyped($('pickerSearch'), () => { if (palette) palette.touched = true; clearTimeout(pickerTimer); pickerTimer = setTimeout(paletteSearch, 140); });
  // ↑ ↓ 循环、↵ 选（⇧↵ 加入多选）、点一项（⇧+点击或行上的 ＋ 加入多选），没输入时数字键直接选前 9 项；Esc 关对话框。
  pickerNav = listNav($('pickerSearch'), $('pickerResults'), {shown: () => !!palette, hover: true, count: () => palette.list.length,
    pick: (i, e) => e.type === 'click' ? choose(i, e.shiftKey || !!e.target.closest('[data-add]'), true) : choose(i, e.shiftKey),
    moved: (i, how) => { if (!palette) return; if (how === 'key') palette.touched = true; highlight(); },
    more: e => { if (/^[1-9]$/.test(e.key) && !$('pickerSearch').value) { e.preventDefault(); choose(Number(e.key) - 1, false, true); } }});
  $('pickerStaged').addEventListener('click', e => { const b = e.target.closest('[data-unstage]'); if (b && palette) stage(palette.staged[Number(b.dataset.unstage)]); });
  $('picker').addEventListener('close', () => { palette = null; });
  $('ocSplitClose').onclick = () => closeModal($('ocSplitDialog'));
  $('relateClose').onclick = () => { relateSeq++; closeModal($('relateDialog')); };

  $('jumpButton').onclick = jumpToCharacter;
}
