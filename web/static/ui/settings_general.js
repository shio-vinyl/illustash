// 设置 › 通用（#714）：显示（外观、图片网格、NSFW、两种默认排序）、界面语言、关闭窗口以后。
import {t, I18N} from '../core/i18n.js';
import {prefs} from '../core/prefs.js';
import {saveAfterTasks, saveLanguage} from '../core/settings.js';
import {toast} from './dom.js';
import {setFit, setNsfw} from './grid.js';
import {groups, refreshSection} from './settings_group.js';

const opts = list => list.map(([value, label]) => ({value, label}));
// 浏览器里的偏好：存进 prefs 以后立刻重画，分段控件跟着变。
const pref = (key, fallback) => ({get: () => prefs.get(key, fallback), set: v => { prefs.set(key, v); refreshSection(); }});
// 整个页面用新语言重画；先保存设置，这样每个窗口和图库都跟着变。
async function setLanguage(language) {
  if (language === I18N.preference) return;
  try { await saveLanguage(language); location.reload(); } catch (e) { toast(e.message); }
}

export function build(main, scope) {
  return groups(main, [
    {key: 'display', title: t('显示'), scope: 'browser', foot: t('缩略图大小、图上显示什么、详情面板的内容，在顶栏的「视图选项」里调。'), parts: [
      {kind: 'select', seg: true, key: 'theme', label: t('外观'), options: opts([['', t('跟随系统')], ['light', t('浅色')], ['dark', t('深色')]]),
       get: () => prefs.get('theme', ''), set: v => { prefs.set('theme', v); window.applyTheme?.(); refreshSection(); }},
      {kind: 'select', seg: true, key: 'fit', label: t('图片网格'), options: opts([['0', t('方形裁切')], ['1', t('原比例')]]),
       get: s => s.fit ? '1' : '0', set: v => { setFit(v === '1'); refreshSection(); }},
      {kind: 'select', seg: true, key: 'nsfw', label: t('NSFW 内容'), options: opts([['', t('显示')], ['hide', t('隐藏')], ['only', t('只看 NSFW')]]),
       get: s => s.nsfw, set: v => { setNsfw(v); refreshSection(); }},
      {kind: 'select', seg: true, key: 'sortLibrary', label: t('图库默认排序'), options: opts([['newest', t('最新加入')], ['oldest', t('最早加入')]]),
       ...pref('sortLibrary', 'newest')},
      {kind: 'select', seg: true, key: 'sortCharacter', label: t('按角色筛选时的默认排序'),
       options: opts([['suspicious', t('可疑优先')], ['newest', t('最新加入')], ['oldest', t('最早加入')]]), ...pref('sortCharacter', 'suspicious')},
    ]},
    // 全应用共用（preferences.json），所以桌面外壳和每个图库、每个浏览器都共用它（#173）。可选项是 web/static/locales
    // 里的表（#182），每个用它自己的语言命名。
    {key: 'language', title: t('语言'), scope: 'shared', parts: [
      {kind: 'select', key: 'language', label: t('界面语言'), hint: t('切换后页面会重新载入'),
       options: () => [{value: 'system', label: t('跟随系统')}, ...I18N.languages.map(([value, label]) => ({value, label}))],
       get: () => I18N.preference, set: setLanguage},
    ]},
    // 只在桌面 App 里有（GET /api/settings 的 desktop，#451）：关窗时有任务在跑就在后台做完，这里定做完以后怎样。
    {key: 'window', title: t('关闭窗口'), scope: 'shared', hidden: s => !s.settings?.desktop, parts: [
      {kind: 'select', seg: true, key: 'afterTasks', label: t('任务做完以后'), hint: t('有任务在跑时关窗，窗口隐藏，任务在后台继续'),
       options: opts([['quit', t('退出')], ['stay', t('留在后台')]]), get: s => s.settings?.desktop?.after_tasks,
       set: v => saveAfterTasks(v).then(refreshSection, e => toast(e.message))},
    ]},
  ], {scope});
}
