// 整理（#706）：未识别、重复、已排除三个页签各自的标题、说明、子页签和空了时的去处，以及随机抽查、身份整理、画师抽查的标题。
// 三个页签之间的分段切换画在页头上（ui/shell.js）；各页的主区在 ui/groups.js、ui/similar.js、ui/identity.js、ui/artist_audit.js 和网格里。
import {esc, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {REASONS} from '../core/labels.js';
import {implement, viewOf} from '../core/pages.js';
import {state} from '../core/state.js';
import {make} from './el.js';
import {gridFilters as filters} from '../core/assets.js';
import {markOc} from './palette.js';

// 同一个页签下的子页签（未识别：按组整理、全部、已跳过；重复：全部、角色不一致、相似的图；已排除：全部、各原因、待确认）。
// 张数和组数写在子页签上，标题下面不再写一遍（#426）。
function subTabs(r) {
  // 字符串：网格页的工具栏把它和别的控件拼在一起（ui/shell.js renderChrome）。文字都是译文和数字，标题经 esc。
  const tab = (href, label, n, on, title = '') => `<a class="chip" href="${href}"${on ? ' aria-current="page"' : ''}${title ? ` title="${esc(title)}"` : ''}>` +
    `${esc(label)}${n ? ` <span class="chip-count">${n}</span>` : ''}</a>`;
  const p = state.pools, row = tabs => `<div class="chips-row">${tabs.join('')}</div>`;
  // 页签组和本页对应的页签由页面登记（core/pages.js 的 tabs、tab）。
  const at = viewOf(r).tab, all = at === 'all';
  switch (viewOf(r).tabs) {
    case 'unmatched': return row([
      // 未识别的总数写在按组整理上：二级层「未识别」落在这里，组里的和没成组的合起来就是它。
      tab('#/groups', t('按组整理'), p.unmatched ? number(p.unmatched) : '', at === 'groups', t('按长相分组的未识别图')),
      tab('#/unmatched', t('全部未识别'), '', all),
      tab('#/skipped', t('已跳过'), p.skipped ? number(p.skipped) : '', at === 'skipped')]);
    case 'duplicates': return row([
      tab('#/duplicates', t('全部'), t('{n} 组', {n: number(state.dupInfo.groups)}), all && !r.dup),
      tab('#/duplicates?dup=mixed', t('角色不一致'), t('{n} 组', {n: number(state.dupInfo.mixed)}), all && r.dup === 'mixed', t('几份副本标的角色不同')),
      tab('#/similar', t('相似的图'), '', at === 'similar', t('同一幅画的不同文件'))]);
    case 'excluded': return row([
      tab('#/excluded', t('全部'), number(state.exclInfo.count), all && !r.reason),
      ...Object.entries(REASONS).filter(([k]) => state.exclInfo.reasons?.[k]).map(([k, label]) => tab('#/excluded?reason=' + k, label, number(state.exclInfo.reasons[k]), all && r.reason === k)),
      ...(state.pendingCount || at === 'pending' ? [tab('#/pending', t('待确认'), number(state.pendingCount), at === 'pending', t('像截图，但有人物'))] : [])]);
  }
  return '';
}
const tidy = () => ({title: t('整理')});
// 第 N 组的页头按钮：整组标为原创角色；和它长得像的另一组。
function groupActions() {
  const out = [];
  if (state.total) {
    const b = make('button', 'quiet', t('整组标为原创角色'));
    b.title = t('把这组图都设为「原创角色（未细分）」');
    b.onclick = () => markOc({filter: filters(), exclude: [], confirm: state.total > 500}, false);
    out.push(b);
  }
  if (state.groupNear) { const a = make('a', 'quiet link', state.groupNear + ' →'); a.href = '#/g/' + encodeURIComponent(state.groupNearId); out.push(a); }
  return out;
}

// 页头（head：只收文字和节点，ui/shell.js 画）、子页签、「?」里的说明（#527：help 是这一页的说明，一行一句；helpHint 表示再接上
// 注册表里的操作提示）、空了时的去处。
implement({
  unmatched: {head: tidy, subTabs, helpHint: true, emptyNext: () => [{label: t('去图库'), href: '#/library'}]},
  skipped: {head: tidy, subTabs, help: () => [t('选中的图可以恢复到未识别。')], helpHint: true, emptyNext: () => [{label: t('回到按组整理'), href: '#/groups'}]},
  groups: {head: tidy, subTabs,
    help: () => [t('按长相分组的未识别图，可以整组确认。'), t('点一下缩略图，把它排除在这组之外。'), t('长按缩略图，或鼠标停在上面按 Space，看大图。'), t('组头的按钮对整组生效。')]},
  // 一组整理完了，标题不写「0 张」，由空状态给去处（#386 的 2-12）。
  group: {subTabs, helpHint: true,
    head: (r, {n}) => ({title: t('第 {id} 组', {id: r.id}), actions: groupActions(),
      sub: [state.totalReady && !state.total ? '' : t('{n} 张长得像的未识别图', {n}), ...(state.groupTraits || [])].filter(Boolean).join(' · ')})},
  duplicates: {head: tidy, subTabs, emptyNext: () => [{label: t('相似的图'), href: '#/similar'}], help: () => [t('同色角标是同一组。'), t('组里的角色不一致时不折叠，对齐以后折叠成一张。')]},
  similar: {head: tidy, subTabs, help: () => [t('同一幅画的不同文件。'), t('点一组进去看。')]},
  sgroup: {subTabs, emptyNext: () => [{label: t('回到相似的图'), href: '#/similar'}], help: () => [t('选中两张以上可以判断它们的关系。')], helpHint: true},
  excluded: {head: tidy, subTabs, helpHint: true, emptyNext: () => [{label: t('去图库'), href: '#/library'}]},
  pending: {head: tidy, subTabs, help: () => [t('像截图，但有人物。')], helpHint: true, emptyNext: () => [{label: t('回到已排除'), href: '#/excluded'}]},
  audit: {head: (r, {n}) => ({title: t('随机抽查'), sub: t('还剩 {left} / {n} 张', {left: number(state.pools.audit || 0), n}),
      alert: state.nsfw ? t('NSFW 筛选开着：抽查要看全部才准，按 H 切换') : ''}), emptyNext: () => [{label: t('去图库'), href: '#/library'}],
    help: () => [t('鼠标移到角色 tag 上，点 ✓ 或 ✗ 回答。'), t('选中多张后按 Y（是）或 N（不是）。'), t('打开大图也可以回答。'), t('看不清的可以跳过，换一张。')]},
  identity: {head: () => ({title: t('身份整理'), sub: t('{n} 对长得像的角色', {n: number(state.identityCount || 0)})}),
    help: () => [t('按涉及的图片数排序。'), t('高亮的按钮是外部资料建议的做法。')]},
  'artist-audit': {head: () => ({title: t('画师抽查'), alert: state.nsfw ? t('这页不按 NSFW 筛选：NSFW 的图也在里面') : ''}),
    help: () => [t('一组固定的随机图，逐张核对「找回画师」找到的画师。'), t('每次打开都是同一批。')]},
});
