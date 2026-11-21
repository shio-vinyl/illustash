// 选择栏（选了图以后底部那一条；#708 审核从 ui/grid.js 拆出）：按这一页登记的种类（core/pages.js 的 bar）画按钮，按下后
// 按 data-act 分派。写入、重读、标签面板这些动作在 ui/grid.js；这里导入它，ui/grid.js 不反过来导入这里：载入时用 setBar()
// 把画栏、分派和抽查的回答登记过去（ui/grid.js 的 renderSelection、键盘经它调用）。app.js 载入这个文件。
//
// 加一个按钮：barButtons 里加 <button data-act="名字">，runBarAction 里加同名的分支（画廊的两个在 ui/gallery_bar.js，#710）。
import {edits} from '../core/edits.js';
import {exportActive} from '../core/files.js';
import {number, short} from '../core/format.js';
import {wantsAdd} from '../core/favorites.js';
import {t} from '../core/i18n.js';
import {canLike, likeRoute} from '../core/like.js';
import {viewOf} from '../core/pages.js';
import {go} from '../core/route.js';
import {gridFilters as filters, listIds} from '../core/assets.js';
import {state} from '../core/state.js';
import {findArtists} from './artists.js';
import {$, toast} from './dom.js';
import {openExport} from './files.js';
import {favoriteSelection, isSelected, markDetailKeys, mutate, openTagPicker, quitSelecting, reload, selCount, selectionIds, selectionPayload, setArmed, setBar} from './grid.js';
import {make} from './el.js';
import {galleryAction, galleryButtons} from './gallery_bar.js';
import {showPop} from './pop.js';
import {markOc, newIds, openPicker, openRelate, targets} from './palette.js';

// 一个按钮：act 是分派用的名字（runBarAction 按它分支），label 是文字，primary 是主按钮，key 是键标，tip 是悬停提示，off 有值时
// 按钮不能点、提示写原因。要「再点一次」确认的按钮（needsArm）在 state.armed 是它的 act 时带 armed 样子，文字由调用处按 armed 写。
const button = (act, label, {primary = false, key = '', tip = '', off = ''} = {}) => ({act, label, primary, key, tip, off});
// 「再点一次」时的文字，平时的文字。
const armed = (act, again, normal) => state.armed === act ? again : normal;

function render() {
  const n = selCount(), bar = $('selectionBar'), r = state.route, kind = viewOf(r).bar;   // 选择栏的种类由页面登记
  // 靠 translateX(-50%) 居中，退场只淡出（#715）
  showPop(bar, n > 0, {to: 'fade'}); markDetailKeys(); if (!n) { setArmed(null); return; }
  const clear = make('button', 'icon', '×'); clear.id = 'clearSel'; clear.title = t('取消选择（Esc）'); clear.onclick = quitSelecting;
  const count = make('span', 'sel-count', state.sel.all ? t('已选全部 {n} 张', {n: number(n)}) : t('已选 {n} 张', {n: number(n)}));
  const nodes = [clear, count, make('div', 'bar-divider')];
  for (const b of barButtons(kind, n)) {
    const el = make('button', `${b.primary ? 'primary' : 'quiet'}${state.armed === b.act ? ' armed' : ''}`, b.key ? `${b.label} ` : b.label);
    el.dataset.act = b.act;
    if (b.key) el.append(make('kbd', '', b.key));
    if (b.off || b.tip) el.title = b.off || b.tip;
    if (b.off) el.disabled = true;
    el.onclick = () => runBarAction(b.act);
    nodes.push(el);
  }
  // 超过 500 张要再点一次（抽查页是只答前 500 张）；重复页上改哪一份都会同步到整组副本：这些放在动手的地方，不写在副本页常驻的
  // 说明里（#528）。
  const note = n > 500 ? kind === 'audit' ? t('一次最多回答 500 张，这次答前 500 张') : t('超过 500 张需再点一次确认')
    : viewOf(r).card?.dup ? t('改动会同步到同组的全部副本') : '';
  if (note) nodes.push(make('span', 'bar-note', note));
  bar.replaceChildren(...nodes);
}
// 选择栏的按钮：先是这一页（注册表的 bar）自己的几个，后面是各页都有的。点了以后由 runBarAction 按 act 分发。
// 默认选择栏的末尾是画廊的「放进画廊…」「从画廊拿出」（ui/gallery_bar.js，#710）。
function barButtons(kind, n) {
  const r = state.route, out = [], N = {n: number(n)};
  const oc = button('oc', t('原创角色'), {tip: t('把画师自己的原创角色统一标为「原创角色（未细分）」')});
  if (r.char && r.maybe) {
    const name = short(state.char?.name || t('该角色'));
    out.push(button('isme', armed('isme', t('再点一次：{n} 张设为 {name}', {...N, name}), t('是 {name}', {name})), {primary: true}),
      button('remove', armed('remove', t('再点一次：{n} 张不是', N), t('不是 TA')), {key: '⌫'}));
  } else if (r.char) {
    const name = short(state.char?.name || t('该角色'));
    out.push(button('remove', armed('remove', t('再点一次：移出 {n} 张', N), t('不是 {name}', {name})), {primary: true, key: '⌫'}),
      button('swap', t('换成…'), {key: 'C'}), button('also', t('也是…'), {key: 'A'}));
    if (state.char?.variants?.length) out.push(button('variant', t('造型'), {key: 'T'}));
  } else if (kind === 'audit') {
    // 随机抽查：每张图问的是它自己的角色（深色角标）；可以一次回答整个选择。
    out.push(button('audit_yes', t('都是'), {primary: true, key: 'Y', tip: t('选中的图都是角标上的角色')}),
      button('audit_no', t('都不是'), {key: 'N', tip: t('选中的图都不是角标上的角色')}));
  } else if (kind === 'unmatched') {
    out.push(button('assign', t('设为角色…'), {primary: true, key: 'C'}), oc, button('skip', armed('skip', t('再点一次：跳过 {n} 张', N), t('跳过')), {key: 'S'}));
  } else if (kind === 'excluded') {
    out.push(button('include', armed('include', t('再点一次：恢复 {n} 张', N), t('恢复收录')), {primary: true, key: 'X'}));
  } else if (kind === 'pending') {
    out.push(button('exclude', armed('exclude', t('再点一次：排除 {n} 张', N), t('排除')), {primary: true, key: 'X'}),
      button('include', armed('include', t('再点一次：保留 {n} 张', N), t('保留'))));
  } else if (kind === 'skipped') {
    out.push(button('resume', armed('resume', t('再点一次：恢复 {n} 张', N), t('恢复到未识别')), {primary: true, key: 'S'}), button('assign', t('设为角色…'), {key: 'C'}), oc);
  } else {
    // C＝换成、A＝追加（#399）：选中的图都还没有角色时只有「设为角色」，C、A 都是它。
    const chars = selectedChars(), why = !chars ? t('全选时请进入角色页换成') : !chars.common.length ? t('所选图片没有共同的角色') : '';
    if (chars && !chars.all.size) out.push(button('assign', t('设为角色…'), {primary: true, key: 'C'}));
    else out.push(button('replace', t('换成…'), {primary: true, key: 'C', off: why}), button('assign', t('加角色…'), {key: 'A'}));
    out.push(button('notlib', t('不是…'), {key: '⌫', off: state.sel.all ? t('全选时请进入角色页移出') : ''}));
  }
  if (!['excluded', 'pending'].includes(kind)) {
    out.push(button('exclude', armed('exclude', t('再点一次：排除 {n} 张', N), t('排除')), {key: 'X', tip: t('把截图、照片等不是插画的图移到「已排除」')}));
  }
  out.push(favButton(n), button('tag', t('标签'), {key: 'G'}),
    button('findartist', t('查找画师'), {tip: t('用像素哈希和 IQDB 查这些图的画师（只发哈希文本，不发图片；一次最多 500 张）')}),
    button('export', t('导出…'), exportActive() ? {off: t('正在导出，完成后再开始下一次')} : {tip: t('把原图复制到一个文件夹')}));
  if (!state.sel.all && canLike(n)) out.push(button('like', n === 1 ? t('更多像这张的') : t('更多像这些的')));
  if (!state.sel.all && n >= 2 && n <= 12) out.push(button('relate', t('判断关系'), {tip: t('两两比较，判断是同一张图、裁剪、差分、局部编辑，还是不同的图')}));
  if (kind === 'default') out.push(...galleryButtons(r).map(g => button(g.act, g.label, {tip: g.tip})));
  return out;
}

// 选择栏的按钮：选中的都已收藏时是「取消收藏」。全选时不知道，写「收藏」，点下去再定。
function favButton(n) {
  const add = state.sel.all || wantsAdd(state.items.filter(x => x && isSelected(x.uuid)), state.sel.ids.size), N = {n: number(n)};
  return button('fav', add ? armed('fav', t('再点一次：收藏 {n} 张', N), t('收藏', {context: 'verb'})) : armed('fav', t('再点一次：取消收藏 {n} 张', N), t('取消收藏')), {key: 'F'});
}
// 选中的图上的角色（只看已读到的图；全选时为 null）：all 是出现过的，common 是每张都有的，C 换掉的就是它（#399）。
function selectedChars() {
  if (state.sel.all) return null;
  const all = new Map(); let common = null;
  for (const x of state.items) {
    if (!x || !state.sel.ids.has(x.uuid)) continue;
    const ids = new Set((x.memberships || []).map(m => m.id));
    for (const m of x.memberships || []) all.set(m.id, m);
    common = common ? new Set([...common].filter(id => ids.has(id))) : ids;
  }
  return {all, common: [...common || []].map(id => all.get(id))};
}
// 把选中的图从 from 换成别的角色：角色页换掉本页的角色，默认选择栏换掉选中的图共同的角色。exclude 是候选里不列的角色
// （图上本来就有的）。
// fixed：点下按钮时定下的 {n, sel, payload}（#715）。「换掉哪个角色？」先选一个再开这一步，中间选区变了也照旧写那一次选的图。
function swapSelected(from, exclude = [from.id], fixed = fixedSelection()) {
  const {n, sel, payload} = fixed;
  openPicker({title: t('把 {n} 张换成…', {n: number(n)}), sentence: e => t('把 {n} 张从 {from} 换成 {name}', {n: number(n), from: from.name, name: e.name}),
    exclude, suggest: sel, multi: true,
    pick: list => mutate('/api/reassign', {...payload, old: from.id, new: newIds(list)}, state.route.char === from.id)});
}
// 选区在点下按钮的这一刻定下来：命令面板开着、请求在途、分两步选时选区再变，写进去的仍是这一次选的图（#715）。
function fixedSelection() {
  const n = selCount(), sel = selectionPayload();
  return {n, sel, payload: {...sel, confirm: n > 500}};
}
function needsArm(key) {
  if (selCount() <= 500 || state.armed === key) return false;
  setArmed(key); render(); setTimeout(() => { if (state.armed === key) { setArmed(null); render(); } }, 4000); return true;
}
async function answerAudit(yes) {
  const sel = state.sel;
  // 后端一次最多收 500 张的回答：多选了的只答前 500 张，选择栏上事先写明（#386 的 2-41）。
  const ids = (sel.all ? (await listIds(filters(), 500)).filter(u => !sel.exclude.has(u)) : [...sel.ids]).slice(0, 500);
  if (ids.length) mutate('/api/audit/answer', {ids, ...(yes ? {yes: ids} : {no: ids})}, false, {advance: true});
}
function runBarAction(act) {
  const r = state.route, fixed = fixedSelection(), {n, sel} = fixed; if (!n || edits.busy) return;
  const payload = () => fixed.payload;
  if (galleryAction(act, n, payload)) return;
  if (act === 'audit_yes' || act === 'audit_no') { answerAudit(act === 'audit_yes'); return; }
  if (act === 'export') { openExport(sel, n); return; }
  if (act === 'like') { if (!state.sel.all) go(likeRoute([...state.sel.ids])); return; }
  if (act === 'findartist') {
    const sel = state.sel;
    (sel.all ? listIds(filters(), 500).then(ids => ids.filter(u => !sel.exclude.has(u))) : Promise.resolve([...sel.ids].slice(0, 500)))
      .then(ids => ids.length && findArtists(ids, () => reload())).catch(err => toast(err.message));
    return;
  }
  if (act === 'remove') { if (!needsArm('remove')) mutate('/api/change', {...payload(), action: 'rejected', entity: r.char}, true); }
  else if (act === 'skip') { if (!needsArm('skip')) mutate('/api/change', {...payload(), action: 'unknown'}, true); }
  else if (act === 'isme') { if (!needsArm('isme')) mutate('/api/change', {...payload(), action: 'confirmed', entity: r.char}, true); }
  else if (act === 'exclude' || act === 'include') { if (!needsArm(act)) mutate('/api/curate', {...payload(), action: act}, true); }
  else if (act === 'resume') { if (!needsArm('resume')) mutate('/api/change', {...payload(), action: 'resume'}, true); }
  else if (act === 'swap') swapSelected({id: r.char, name: state.char?.name}, [r.char], fixed);
  else if (act === 'replace') {
    const chars = selectedChars();
    if (!chars) toast(t('全选时请进入角色页换成'));
    else if (!chars.all.size) runBarAction('assign');
    else if (!chars.common.length) toast(t('所选图片没有共同的角色'));
    else if (chars.common.length === 1) swapSelected(chars.common[0], chars.common.map(m => m.id), fixed);
    // 第二个面板等这一个的 close 事件之后再开：在那之前浮层管理器还把它算作开着，同一个对话框不会再打开。
    else openPicker({title: t('换掉哪个角色？'), items: chars.common, subject: selectionIds(), sentence: e => t('把 {n} 张的 {name} 换掉', {n: number(n), name: e.name}),
      pick: e => $('picker').addEventListener('close', () => swapSelected(e, chars.common.map(m => m.id), fixed), {once: true})});
  }
  else if (act === 'also') openPicker({title: t('这 {n} 张也是…', {n: number(n)}), sentence: e => t('给 {n} 张加上 {name}', {n: number(n), name: e.name}),
    exclude: [r.char], suggest: sel, multi: true,
    pick: list => mutate('/api/change', {...payload(), action: 'confirmed', ...targets(list)}, false)});
  else if (act === 'assign') {
    // 默认选择栏（图库、重复、相似组）里选中的图已有角色时，按钮是「加角色…」，原有的角色保留；否则是给还没有角色的图设角色（#423）。
    // 默认选择栏的页设了角色图还在本页，不淡出（#386 的 2-22：相似组是整理型的页，原来按浏览页判断，卡片闪一下）；
    // 未识别、已跳过这类页设了角色图就离开。
    const stays = viewOf(r).bar === 'default';
    const chars = stays ? selectedChars() : {all: new Map()}, adding = !chars || chars.all.size > 0;
    openPicker({title: adding ? t('给 {n} 张加角色', {n: number(n)}) : t('把 {n} 张设为…', {n: number(n)}),
      sentence: e => adding ? t('给 {n} 张加上 {name}', {n: number(n), name: e.name}) : t('把 {n} 张设为 {name}', {n: number(n), name: e.name}), suggest: sel, multi: true,
      pick: list => mutate('/api/change', {...payload(), action: 'confirmed', ...targets(list)}, !stays)});
  }
  else if (act === 'oc') markOc(payload(), true);
  else if (act === 'relate') openRelate([...state.sel.ids]);
  else if (act === 'fav') { if (!needsArm('fav')) favoriteSelection(payload()); }
  else if (act === 'variant') openVariantMenu(payload);
  else if (act === 'tag') openTagPicker(payload, n, state.sel.all ? null : state.items.filter(x => x && isSelected(x.uuid)));
  else if (act === 'notlib') {
    if (state.sel.all) { toast(t('全选时请进入角色页移出')); return; }   // 按钮这时是禁用的，⌫ 说一样的话
    const present = new Map(); for (const id of state.sel.ids) { const item = state.items.find(x => x?.uuid === id); for (const m of item?.memberships || []) present.set(m.id, m); }
    if (!present.size) { toast(t('所选图片还没有角色')); return; }
    openPicker({title: t('这些图不是…'), items: [...present.values()], subject: selectionIds(), sentence: e => t('把 {name} 从 {n} 张中移出', {name: e.name, n: number(n)}),
      pick: e => mutate('/api/change', {...payload(), action: 'rejected', entity: e.id}, false)});
  }
}

function openVariantMenu(payload) {
  const r = state.route, n = selCount(), items = [];
  if (r.variant) { const v = state.char.variants.find(v => v.id === r.variant); if (v) items.push({id: v.id, name: v.name, remove: true, hint: t('从所选图片去掉')}); }
  for (const v of state.char.variants) if (v.id !== r.variant) items.push({id: v.id, name: v.name, hint: t('{n} 张', {n: number(v.count)}), counted: true});
  openPicker({title: t('造型'), items, subject: selectionIds(), placeholder: t('筛选造型'),
    sentence: e => e.remove ? t('从 {n} 张去掉 {name}', {n: number(n), name: e.name}) : t('给 {n} 张加上 {name}', {n: number(n), name: e.name}),
    pick: e => mutate('/api/change', {...payload(), action: e.remove ? 'remove_variant' : 'add_variant', entity: e.id}, !!e.remove)});
}

setBar({render, run: runBarAction, answerAudit});
