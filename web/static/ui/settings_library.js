// 设置 › 图库（#714）：图库和来源（ui/settings_libraries.js）、同步、排除规则、重复与相似。
import {t, tb} from '../core/i18n.js';
import {number} from '../core/format.js';
import {state} from '../core/state.js';
import {REASONS, FOLD_OPTIONS} from '../core/labels.js';
import {refreshMeta} from '../core/meta.js';
import {SYNC_ACTIVE, syncLine, pollImports} from '../core/jobs.js';
import {loadLibraries, saveFold} from '../core/settings.js';
import {toast} from './dom.js';
import {groups, refreshSection, startupRow, taskRow, tiles} from './settings_group.js';
import {librariesGroup} from './settings_libraries.js';

// 排除规则的数字格：每个原因一格，链到被排除的图；最后一格是待确认。
const excluded = s => [
  ...Object.entries(REASONS).map(([k, label]) => ({key: k, href: `#/excluded?reason=${k}`, label, big: number(s.exclInfo?.reasons?.[k] || 0)})),
  {key: 'pending', href: '#/pending', label: t('待确认'), big: number(s.pendingCount || 0)}];
// 相似折叠的勾选项：几个勾选框共用一份「要保存的集合」，快速连点时每次改动都记进去，保存一个接一个地发、每次发最新的
// 集合，后点的不会把先点的冲掉。排着的保存都落定以后才清掉这份集合，回到照 state 显示。
let foldWant = null, foldChain = Promise.resolve(), foldQueued = 0;
const foldRow = ([kind, label, hint]) => ({kind: 'switch', key: 'fold-' + kind, label, hint,
  disabled: s => !s.fold, get: s => (foldWant ?? new Set(s.fold || [])).has(kind),
  set: on => {
    foldWant ??= new Set(state.fold || []);
    if (on) foldWant.add(kind); else foldWant.delete(kind);
    const want = foldWant;
    foldQueued++;
    foldChain = foldChain.then(async () => {
      try { await saveFold(FOLD_OPTIONS.map(o => o[0]).filter(k => want.has(k))); await refreshMeta(); toast(t('已更新，「重复」页现在有 {n} 组', {n: number(state.dupInfo.groups)})); }
      catch (e) { toast(e.message); }
      // 队列排空了才清：中间某一次先回来时，后面还排着的保存仍要拿到完整的集合。
      if (--foldQueued === 0 && foldWant === want) foldWant = null;
    });
    return foldChain;
  }});
// 按 App 分的计数只有照片来源才有（X 或 Pixiv 保存的图片）；文件夹图库没有。
const apps = s => Object.entries((s.syncHistory || []).find(r => r.sources)?.sources || {}).sort((a, b) => b[1].photos - a[1].photos);
const hasPhotos = s => (s.settings?.package?.sources || s.libraries?.info?.sources || []).some(x => x.kind === 'photos');

export function build(main, scope) {
  const view = groups(main, [
    librariesGroup(),
    {key: 'sync', title: t('同步'), scope: 'library', note: t('读取来源里的图片，新增的导入并识别；来源里删掉的图会从图库移除，人工整理记录保留。'), parts: [
      taskRow('syncStatus', s => s.sync?.status === 'off' ? t('这次启动没有开同步') : syncLine(s.sync),
        s => SYNC_ACTIVE.includes(s.sync?.status) ? 'busy' : ['error', 'needs_recognition'].includes(s.sync?.status) ? 'warn' : s.sync?.status === 'off' ? 'off' : ''),
      startupRow('sync', t('启动时同步'), t('检查来源里新增和删除的图')),
      {kind: 'fold', key: 'sourcesApps', hidden: s => !hasPhotos(s) || !apps(s).length,
       summary: s => t('iCloud 照片按存图的 App 统计（{n} 个 App）', {n: number(apps(s).length)}), parts: [
         {kind: 'note', text: t('iCloud 照片里只导入 X 和 Pixiv 存下的图。')},
         {kind: 'list', key: 'apps', label: 'App', items: apps, itemKey: ([k]) => k, itemText: ([, v]) => tb(v.name),
          itemHint: ([, v]) => t('照片里 {photos} 张，已导入 {collected} 张', {photos: number(v.photos), collected: number(v.collected || 0)})},
       ]},
    ]},
    {key: 'exclude', title: t('排除规则'), scope: 'library', note: t('照片、截图、文字图这类不是插画的图不算进图库。你的判断优先于自动规则。'), parts: [
      {kind: 'block', key: 'tiles', mount: tiles(excluded)},
    ]},
    {key: 'fold', title: t('重复与相似'), scope: 'library', note: t('勾选的关系会合并显示：图库里只留分辨率最高的一张，整组在「重复」页查看。'),
     more: t('改动组里任一张会同步到整组。组里角色不一致时不合并，留在「重复」页等你处理。字节完全相同的副本总是合并。'), parts: [
      startupRow('similar', t('启动时检测相似图片'), t('只比较新导入的图')),
      ...FOLD_OPTIONS.map(foldRow),
    ]},
  ], {scope});
  return {update: view.update, load};
}
// 进这一节读一次图库列表；读到导出还在进行（比如导出途中刷新了页面），core/settings.js 采用时接上进度轮询。
const load = () => loadLibraries();
// 页面启动时调用一次：读图库列表（顶栏的图库名要用），接着看有没有在跑的导入。
export function wireLibrary() { loadLibraries(); pollImports(); }
