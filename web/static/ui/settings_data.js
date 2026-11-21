// 设置 › 数据（#714）：备份与存储（整库备份、人工结论快照、数据库检查、缩略图缓存、存储位置）和识别模型（文字和按钮的
// 数据在 ui/models.js）。三样东西分开写（#502）：整库备份（library.sqlite 的完整副本）、人工结论快照（ui/human.js 画进
// #humanBlock，这一步没有重写）、图库导出（在「图库」那一节）。
import {t} from '../core/i18n.js';
import {bytes, home, localStamp, number} from '../core/format.js';
import {state} from '../core/state.js';
import {checkNow, pollIntegrity} from '../core/integrity.js';
import {loadHuman} from '../core/human.js';
import {saveAcceleration} from '../core/models.js';
import {backupNow, clearCache, reloadSettings} from '../core/settings.js';
import {toast} from './dom.js';
import {renderHumanBlock} from './human.js';
import {accelerationFell, accelerationHint, characterBaseButtons, characterBaseText, mirrorNote, modelsButtons, modelsDeleteButtons, modelsFileItems,
  modelsLine, modelsManual, modelsProgress, modelsResultItems, runModels} from './models.js';
import {groups, refreshSection} from './settings_group.js';

// 整库备份在图库文件夹里的两处（#517，说法同 #492 的损坏提示）：backups 里是「立即备份」留下的，设置里的份数和清单只算
// 这些；library-backups 里是升级图库格式前自动留下的。
export const backupFolder = (library, name) => library + (library.includes('\\') ? '\\' : '/') + name;
const info = s => s.settings;
function integrityHint(g) {
  if (!g) return '';
  const when = file => g.checked?.[file] ? localStamp(g.checked[file]) : t('还没有检查过');
  return g.running ? t('正在检查 {file}，{percent}%', {file: g.running.database, percent: Math.floor(g.running.done * 100)})
    : t('上次检查：library.sqlite {library}，source.sqlite {source}', {library: when('library.sqlite'), source: when('source.sqlite')});
}
const m = s => s.models;
// 整库备份：备份完 /api/settings 要重读（份数、最近一次、缩略图都在里面），新发一次，不用备份前就在途的那份。备份成功、
// 重读失败时分别说。
async function backup() {
  let r;
  try { r = await backupNow(); } catch (e) { toast(e.message); return; }
  toast(t('备份完成：{name}', {name: r.path.split('/').pop()}));
  try { await reloadSettings(); } catch { /* 下面照 state.settings 说明 */ }
  if (!state.settings) { toast(t('备份完成，但没能读回备份份数：请重新打开设置页')); refreshSection(); }
}

export function build(main, scope) {
  const view = groups(main, [
    {key: 'backup', title: t('备份与存储'),
     note: t('这里有两种备份。整库备份是应用数据库（人工整理记录所在的库）的完整副本，只能整份换回；人工结论快照只含人工整理的结论，可以导回这个或别的图库，只补上缺少的部分。要连识别结果和来源一起带到别处，用「设置 › 图库」里的「导出当前图库」。'),
     parts: [
       {kind: 'sub', label: t('整库备份')},
       {kind: 'action', key: 'backupNow', label: s => info(s) ? t('共 {n} 份备份', {n: number(info(s).backup_count)}) : '…',
        hint: s => info(s)?.backups?.[0] ? t('最近一次 {time}', {time: localStamp(info(s).backups[0].time)}) : '',
        buttons: s => [{act: 'backup', label: t('立即备份'), disabled: !info(s)}],
        run: backup},
       {kind: 'note', text: t('上面的份数只算「立即备份」留下的，在图库文件夹的 backups 里。升级图库格式前还会自动另存一份，放在 library-backups 里，内容停在那次升级之前，换回后打开时会再升级一次。换回整库备份前先退出 app；备份之后的改动都会丢掉。')},
       {kind: 'fold', key: 'backupFolders', summary: t('备份放在哪里'), hidden: s => !info(s), parts: [
         {kind: 'list', key: 'folders', label: '', itemKey: x => x.name, itemText: x => x.label, itemHint: x => home(backupFolder(info(state).package.path, x.name)),
          items: [{label: t('立即备份'), name: 'backups'}, {label: t('升级前的备份'), name: 'library-backups'}]},
       ]},
       {kind: 'fold', key: 'backupList', summary: t('最近的备份'), hidden: s => !info(s)?.backups?.length, parts: [
         {kind: 'list', key: 'backups', label: '', items: s => info(s).backups.slice(0, 5), itemKey: b => b.name, itemText: b => b.name, itemHint: b => bytes(b.bytes)},
       ]},
       {kind: 'sub', label: t('人工结论快照')},
       {kind: 'block', key: 'human', mount: el => { el.id = 'humanBlock'; return {update: () => renderHumanBlock()}; }},
       {kind: 'sub', label: t('检查与存储')},
       {kind: 'note', text: t('每次打开图库后会在后台检查识别结果数据库，整理记录数据库每 7 天检查一次；上次没有正常关闭时，打开前先检查两个库。')},
       {kind: 'action', key: 'integrity', label: t('数据库检查'), hint: s => integrityHint(s.integrity),
        warn: s => s.integrity?.error && !s.integrity.running ? t('上次检查没有完成：{error}', {error: s.integrity.error}) : '',
        buttons: s => [{act: 'check', label: t('立即检查'), disabled: !s.integrity || !!s.integrity.running || !!s.integrity.damaged}],
        run: async () => { try { await checkNow(); } catch (e) { toast(e.message); } }},
       {kind: 'action', key: 'thumbnails', label: t('缩略图缓存'),
        hint: s => info(s) ? t('{n} 个文件，{size}；清除后浏览时会重新生成', {n: number(info(s).thumbnails.files), size: bytes(info(s).thumbnails.bytes)}) : '',
        buttons: s => [{act: 'clear', label: t('清除'), disabled: !info(s)?.thumbnails.files}],
        run: async () => { try { const r = await clearCache(); toast(t('已清除 {n} 个缩略图', {n: number(r.removed)})); } catch (e) { toast(e.message); } refreshSection(); }},
       {kind: 'fold', key: 'dataPaths', summary: t('存储位置'), hidden: s => !info(s), parts: [
         {kind: 'list', key: 'paths', label: '', itemKey: x => x.key, itemText: x => x.label,
          items: s => [['source', t('源数据库（识别结果，可以重建）')], ['library', t('应用数据库（人工整理，不要删）')], ['thumbnails', t('缩略图缓存（可以删）')]]
            .map(([key, label]) => ({key, label, ...info(s)[key]})),
          itemHint: d => `${home(d.path)} · ${bytes(d.bytes)}`},
       ]},
     ]},
    {key: 'models', title: t('识别模型'), scope: 'shared', hidden: s => !m(s),
     note: s => m(s).on_demand ? t('图片识别要用 WD 和 CCIP 两个模型，安装包里没有带，第一次使用时下载。每个文件都按应用里写好的大小和 SHA-256 核对，对不上就不用。') : t('这个安装自带识别模型。'),
     parts: [
       {kind: 'action', key: 'modelsStatus', label: s => modelsLine(m(s)), link: s => modelsManual(m(s)), progress: s => modelsProgress(m(s)),
        buttons: s => modelsButtons(m(s)), run: (s, act) => runModels(act)},
       {kind: 'list', key: 'modelsResults', label: '', hidden: s => !modelsResultItems(m(s)).length, items: s => modelsResultItems(m(s)),
        itemKey: x => x.key, itemText: x => x.text, itemHint: x => x.hint || ''},
       {kind: 'action', key: 'modelsDelete', label: '', hidden: s => !modelsDeleteButtons(m(s)).length, buttons: s => modelsDeleteButtons(m(s)), run: (s, act) => runModels(act)},
       {kind: 'action', key: 'characterBase', label: s => characterBaseText(s.characterBase), hidden: s => !characterBaseText(s.characterBase),
        buttons: s => characterBaseButtons(s.characterBase), run: (s, act) => runModels(act)},
       // 「使用 GPU 加速识别」（#270）：只在这个构建带加速器的地方出现。全应用共用（preferences.json）；worker 启动时读取。
       {kind: 'switch', key: 'acceleration', label: t('使用 GPU 加速识别'), hidden: s => !m(s).acceleration?.available,
        hint: s => accelerationHint(m(s)), warn: s => accelerationFell(m(s)), get: s => !!m(s).acceleration?.on,
        set: async on => { try { await saveAcceleration(on); toast(t('设置已保存')); } catch (err) { toast(err.message); } refreshSection(); }},
       {kind: 'field', key: 'mirror', label: t('Hugging Face 镜像'), hint: s => mirrorNote(m(s)), wide: true, plain: true, hidden: s => !m(s).on_demand,
        get: s => m(s).mirror, set: v => runModels('mirror', v)},
       {kind: 'action', key: 'mirrorReset', label: '', hidden: s => !m(s).on_demand || m(s).mirror === m(s).mirror_default,
        buttons: () => [{act: 'mirror-reset', label: t('恢复默认')}], run: (s, act) => runModels(act)},
       {kind: 'fold', key: 'modelFiles', summary: t('模型文件'), parts: [
         {kind: 'list', key: 'files', label: '', items: s => modelsFileItems(m(s)), itemKey: x => x.key, itemText: x => x.text, itemHint: x => x.hint,
          itemTitle: x => x.title},
       ]},
     ]},
  ], {scope});
  return {update: view.update, load};
}
// 进这一节：人工结论快照读一次（数据库检查、模型状态由各自的模块在启动时读）。
const load = () => { loadHuman(); if (!state.integrity) pollIntegrity(); };
