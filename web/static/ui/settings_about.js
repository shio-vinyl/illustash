// 设置 › 关于（#714）：名字和版本、检查更新（文字和按钮在 ui/updates.js）、新手引导和诊断、致谢与第三方组件、诊断信息。
import {t, tb, I18N} from '../core/i18n.js';
import {state} from '../core/state.js';
import {diagnosticReport, diagnosticText, diagnosticLine, downloadDiagnostic, openDiagnosticLogs} from '../core/diagnostics.js';
import {readError} from '../core/session.js';
import {checkUpdates, loadAbout} from '../core/settings.js';
import {$, toast, error} from './dom.js';
import {attr, make, refill, rich, text, tpl} from './el.js';
import {drawNudge, replayGuide} from './guide.js';
import {openStatic} from './dialog.js';
import {groups, refreshSection} from './settings_group.js';
import {drawUpdateButton, updateAct, updateButtons, updateProgress, updateStarted, updatesHint, updatesTitle} from './updates.js';

const CREDITS = [
  ['WD SwinV2 Tagger v3', 'SmilingWolf', t('图片标签与角色识别'), 'https://huggingface.co/SmilingWolf/wd-swinv2-tagger-v3'],
  ['CCIP', 'DeepGHS', t('角色特征比对'), 'https://huggingface.co/deepghs/ccip'],
  ['Character Index', 'DeepGHS', t('角色索引数据'), 'https://huggingface.co/datasets/deepghs/character_index'],
  [t('WD 角色扩展'), 'SmilingWolf WD v3 · DeepGHS Character Index', t('WD 词表以外的较新角色'), 'https://github.com/shio-vinyl/illustash/blob/main/docs/THIRD_PARTY.md#模型与数据'],
  [t('CCIP 长尾包'), 'DeepGHS CCIP · Danbooru', t('图片较少的角色'), 'https://github.com/shio-vinyl/illustash/blob/main/docs/THIRD_PARTY.md#模型与数据'],
  [t('imgutils 检测与分级模型'), 'DeepGHS', t('人物检测、打码检测、内容分级'), 'https://github.com/deepghs/imgutils'],
  ['Anime Censorship Tagger', 'Maltox', t('打码类型识别'), 'https://huggingface.co/Maltox/anime-censorship-tagger-mnv3-384'],
  ['Danbooru', '', t('标签与角色资料'), 'https://danbooru.donmai.us'],
  [t('Bangumi 番组计划'), '', t('角色中文名称'), 'https://bgm.tv'],
  ['Pillow · NumPy · ONNX Runtime', '', t('图像处理与模型推理'), 'https://onnxruntime.ai']];
const HEAD = tpl`<div class="set-about"><img src="/favicon.svg" alt="" width="44" height="44"><div><h2 class="set-about-name" data-ref="name"></h2>
  <p class="set-about-version" data-ref="version"></p></div></div>`;

const version = a => `${a.version}${a.channel ? ' ' + tb(a.channel) : ''}`;
const environment = a => !a ? [] : [[t('应用'), a.name], [t('版本'), version(a)], [t('构建'), a.build || '—'], [t('数据库架构'), a.schema],
  [a.runtime ? t('运行时') : 'Python', a.runtime || a.python], [t('系统'), a.platform],
  ...Object.entries(a.libraries || {}).map(([k, v]) => [tb(k), tb(v) || t('未安装')])];
async function help(act) {
  const env = Object.fromEntries(environment(state.about));
  if (act === 'guide') return replayGuide();
  if (act === 'keys') return openStatic($('help'), {backdropClose: true, id: 'help'});
  if (act === 'copy') {
    try { await navigator.clipboard.writeText(diagnosticText({...await diagnosticReport(), environment: env})); toast(t('诊断信息已复制')); }
    catch { toast(t('无法访问剪贴板，请导出诊断信息')); }
  }
  if (act === 'export') { try { downloadDiagnostic({...await diagnosticReport(), environment: env}); } catch (e) { error(readError(e)); } }
  if (act === 'logs') { try { await openDiagnosticLogs(); } catch { toast(t('无法打开日志所在文件夹')); } }
}
// 检查、下载、取消以后，侧栏的更新入口和导览里的更新提示跟着变，不用等下一次通知。
async function update(act) {
  if (act !== 'check') await updateAct(act);
  else {
    try { const u = await checkUpdates(); toast(u.error ? tb(u.error) : u.newer ? t('有新版本 {version}', {version: u.latest}) : t('已是最新版本')); }
    catch (err) { toast(err.message); }
  }
  drawUpdateButton(); drawNudge();
  refreshSection();
}
// 许可说明里的链接。
const license = el => {
  el.className = 'set-group-foot';
  const a = make('a', 'row-link', 'BigScience Open RAIL-M');
  attr(a, 'href', 'https://bigscience.huggingface.co/blog/bigscience-openrail-m'); attr(a, 'target', '_blank'); attr(a, 'rel', 'noreferrer noopener');
  return {update: () => refill(el, I18N.language, () => [rich(t(
    '识别用的 CCIP 模型（随安装包附带，或第一次使用时下载）依据 {license} 许可提供给你，使用和再分发均受该许可第 5 条约束：不得用于其附件 A 列出的用途，再分发时须把这些限制写进分发协议并附上许可。许可原文和各模型的来源见应用资源目录里的 resources/licenses/models/。其余组件的许可条款以其项目主页为准。'),
    {license: a})])};
};

export function build(main, scope) {
  // 诊断记录（本机的，最近几条）：进这一节时读一次，留在这一节里；先发的读取后回来时不盖掉新的。
  let report = null, asked = 0;
  const view = groups(main, [
    {key: 'about', title: t('关于'), note: t('在本机识别、整理收藏的插画和里面的角色。'), parts: [
      {kind: 'note', text: t('正在读取…'), hidden: s => !!s.about},
      {kind: 'block', key: 'head', hidden: s => !s.about, mount: el => {
        const {el: head, refs} = HEAD(); el.appendChild(head);
        return {update: s => {
          text(refs.name, s.about.name);
          text(refs.version, t('版本 {v}', {v: version(s.about)}) + (s.about.build ? ' · ' + t('构建 {build}', {build: s.about.build}) : ''));
        }};
      }},
      {kind: 'action', key: 'updates', label: s => updatesTitle(s.updates), hint: s => updatesHint(s.updates), progress: s => updateProgress(s.updates),
       buttons: s => [...(s.updates?.newer || updateStarted(s.updates) ? updateButtons(s.updates) : []),
         ...(updateStarted(s.updates) ? [] : [{act: 'check', label: t('检查更新')}])],
       run: (s, act) => update(act)},
      {kind: 'action', key: 'help', label: t('帮助与诊断'), hidden: s => !s.about,
       buttons: s => [{act: 'guide', label: t('新手引导')}, {act: 'keys', label: t('键盘快捷键')}, {act: 'copy', label: t('复制诊断信息')},
         ...(s.about.can_open_logs ? [{act: 'logs', label: t('打开日志所在文件夹')}] : []), {act: 'export', label: t('导出诊断信息')}],
       run: (s, act) => help(act)},
      {kind: 'fold', key: 'credits', summary: t('致谢与第三方组件'), parts: [
        {kind: 'list', key: 'credits', label: '', items: CREDITS, itemKey: c => c[3] + c[0], itemText: c => c[0], itemHref: c => c[3],
         itemHint: c => [c[1], c[2]].filter(Boolean).join(' · ')},
        {kind: 'block', key: 'license', mount: license},
      ]},
      {kind: 'fold', key: 'diagnostics', summary: t('诊断信息'), hidden: s => !s.about, parts: [
        {kind: 'note', text: t('运行错误仅保存在本机，最多 128 条、保留 14 天；不会自动上传，不含图片、文件名、路径或账号凭据。')},
        {kind: 'list', key: 'records', label: () => report?.available ? t('最近的错误记录') : t('诊断记录暂不可读'),
         items: () => (report?.records || []).slice(0, 8), itemKey: r => r.at + diagnosticLine(r), itemText: r => diagnosticLine(r),
         itemHint: r => new Date(r.at * 1000).toLocaleString(I18N.locale)},
        {kind: 'list', key: 'environment', label: '', items: s => environment(s.about), itemKey: ([k]) => k, itemText: ([k]) => k, itemHint: ([, v]) => String(v ?? '')},
      ]},
    ]},
  ], {scope});
  // 进这一节：版本信息（读过就不再读）和诊断记录。离开以后回来的回复不画、不弹提示。
  function load() {
    const route = state.route, here = () => scope.alive && state.route === route, n = ++asked;
    if (!state.about) loadAbout().then(() => here() && refreshSection(), e => { if (here()) toast(e.message); });
    diagnosticReport().catch(() => ({available: false, records: []})).then(r => { if (here() && n === asked) { report = r; refreshSection(); } });
  }
  return {update: view.update, load};
}
