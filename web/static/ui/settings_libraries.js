// 设置 › 图库的第一组「图库」（docs/LIBRARIES.md）：当前图库和它的来源、这台电脑上的其他图库、新建图库的表单、导入导出。
// 打开一个图库会让服务在它上面重启。新图库和当前图库的来源按出处分组（#107）：每个文件夹都写明它的图片从哪来，不从文件夹
// 名猜测；文件夹包含它的子文件夹，不能互相嵌套。
import {t, tb, said} from '../core/i18n.js';
import {home, isMac, megabytes, number} from '../core/format.js';
import {session} from '../core/session.js';
import {state} from '../core/state.js';
import {importLine, importActive, importStarted, cancelImport, pollSync} from '../core/jobs.js';
import {libraryRequest, pollExport, chooseFolders, openLibraryRequest, libraryOpened, loadLibraries} from '../core/settings.js';
import {$, toast} from './dom.js';
import {confirmAction} from './dialog.js';
import {changeFolder, folderResult} from './folder-move.js';
import {keyed} from './keyed.js';
import {rows} from './rows.js';
import {devMode, refreshSection} from './settings_group.js';

const ORIGINS = [['pixiv', 'Pixiv'], ['x', 'X'], ['other', t('其他')]];
// 后端列出它知道的出处（#111）；后备值覆盖没有图库信息的旧布局。
const origins = () => { const list = state.libraries?.info?.origins; return list?.length ? list.map(o => [o.id, tb(o.label)]) : ORIGINS; };
// 新图库表单里输入的内容存在这里，卡片在后台刷新时不会丢失；有意跨节保留（换节再回来，填了一半的表单还在）。
// created：已发布但没有登记到最近图库里的图库（#97），提供它而不是再新建一个。gen 是这一份表单的编号：取消、重开都换一个，
// 换以前发出的请求（新建、选文件夹）回来时不再动表单、不自动打开。
export const libForm = {open: false, name: '', folders: {}, photos: false, created: null, gen: 0};
// 选文件或发请求之后还在不在原来那一页（离开设置页、换节时路由对象会换）。
const still = route => state.route === route;
// #41：可分享的导出不包含个人输入；照实说明，而不要暗示这是一次完整的搬家。
const EXPORT_LEAVES_OUT = t('不含图片、画师出处缓存和导入过的 X 关注列表；在新电脑上要回查画师，需要重新导入 X 归档。');

// 系统的文件夹选择框（可一次选多个）。没有选择框的地方，用输入的路径代替。
async function pickFolders(multiple = true) {
  try { return await chooseFolders(multiple); }
  catch (err) {
    if (!/Unsupported/.test(err.message)) { toast(err.message); return []; }
    const typed = prompt(t('这台电脑上没有文件夹选择窗口，请输入文件夹路径：'));
    return typed?.trim() ? [typed.trim()] : [];
  }
}
async function libraryAction(action, body = {}, token = session.token) {
  try { return await libraryRequest(action, body, token); } catch (err) { toast(err.message); return null; }
}
export async function startImport(path) {
  const r = await libraryAction('import', {path});
  if (r?.task) importStarted(r.task);
}
// 旧格式的图库要用户同意升级后才能打开（#193）：后端这样说明，页面用后端自己的解释询问，然后带上 upgrade 再请求一次。
// 新页面会说明备份在哪。检查发现 source.sqlite 损坏的图库以同样的方式提供抢救（#228）；抢救在图库打开之前运行，大图库
// 上可能要几分钟。
export async function openLibrary(path, name, agreed = null) {
  try { await openLibraryRequest(path, agreed); }
  catch (err) {
    if (!agreed && said(err.said, '这个图库需要升级到新格式才能打开。升级后旧版本的 Illustash 打不开它；升级前会自动备份，升级没有完成时图库保持原样。')) {
      if (await confirmAction(err.message, t('「{name}」需要升级', {name}), t('升级并打开'))) return openLibrary(path, name, 'upgrade');
      return;
    }
    if (!agreed && said(err.said,
        '这个图库的识别结果数据库（source.sqlite）已损坏，为保护数据已停止写入；人工整理记录在 library.sqlite 里，没有受影响。可以点「抢救重建」：还读得出的识别结果和每张图的 ID 都会保留，读不出的部分重新扫描来源文件夹、重新识别；损坏的文件移到图库的 backups 文件夹里留存。抢救前请确认来源文件夹都能访问。')) {
      if (await confirmAction(err.message, t('「{name}」的识别结果数据库已损坏', {name}), t('抢救重建'))) {
        toast(t('正在抢救「{name}」的识别结果数据库，可能要几分钟…', {name}));
        return openLibrary(path, name, 'rescue');
      }
      return;
    }
    toast(err.message); return;
  }
  // 这个页面的状态属于它离开的那个图库：在下面重新载入之前不再读写。
  toast(t('正在打开「{name}」…', {name}));
  if (await libraryOpened(path)) { location.hash = '#/days'; location.reload(); return; }
  $('errorBanner').textContent = t('切换图库超时：请检查启动终端，然后刷新页面。'); $('errorBanner').hidden = false;
}

// ---------- 来源 ----------
const info = () => state.libraries?.info;
const editable = () => !!info() && !info().test;
const removingSources = new Set();
export async function removeSource(id) {
  if (removingSources.has(id)) return;
  removingSources.add(id);
  const token = session.token, route = state.route;
  try {
    const ask = await libraryAction('sources/remove', {source: id}, token); if (!ask) return;
    if (token !== session.token || route !== state.route) return;
    if (!await confirmAction(t('这个来源的 {n} 张图会从图库移除。人工整理记录会保留，重新添加同一个文件夹可以恢复。确定移除吗？', {n: number(ask.count)}), t('移除来源'))) return;
    const r = await libraryAction('sources/remove', {source: id, confirm: 1}, token);
    await loadLibraries();
    if (r) { toast(t('已移除来源（{n} 张图）', {n: number(r.removed)}) + (r.sync ? t('，正在同步') : '')); if (r.sync) pollSync(); }
  } finally { removingSources.delete(id); }
}
const removeButton = () => {
  const last = info()?.sources.length <= 1;
  return {act: 'remove', label: t('移除'), disabled: last, title: last ? t('图库至少要保留一个来源') : ''};
};
const changeOf = s => state.libraries?.folder_changes?.find(c => c.source === s.id);
// 一个文件夹来源两行：路径和位置操作，下面是图片来自哪里（#293）。
function folderRows(first, scope) {
  const own = scope.child('folder'), box = document.createElement('div');
  let src = first;
  const panel = rows(box, [
    {kind: 'action', key: 'folder', label: () => home(src.path), hint: () => folderResult(changeOf(src)) || (editable() ? '' : origins().find(o => o[0] === src.preset)?.[1] || t('其他')),
     buttons: () => !editable() ? [] : [
       ...(changeOf(src) ? [{act: 'undo', label: t('撤销')}] : []),
       {act: 'move', label: t('这个文件夹搬到了…')}, {act: 'recognize', label: t('认回原图…')}, removeButton()],
     run: (s, act) => act === 'remove' ? removeSource(src.id)
       : changeFolder(act === 'move' ? {folderMove: src.id} : act === 'recognize' ? {folderRecognize: src.id} : {folderUndo: src.id, token: changeOf(src).token},
         pickFolders, libraryAction)},
    {kind: 'select', seg: true, key: 'origin', label: t('图片来自'), hidden: () => !editable(),
     options: () => origins().map(([value, label]) => ({value, label})), get: () => src.preset,
     set: async preset => { if (await libraryAction('preset', {source: src.id, preset})) { toast(t('出处已更新')); await loadLibraries(); } }},
  ], {scope: own});
  panel.update(state);   // 建好就画一次，进这一节就能点
  return {el: box, update: next => { src = next; panel.update(state); }, dispose: () => own.dispose()};
}
const folders = () => {
  const list = origins(), originOf = s => list.some(o => o[0] === s.preset) ? s.preset : 'other';
  return (info()?.sources || []).filter(s => s.kind === 'folder').sort((a, b) => list.findIndex(o => o[0] === originOf(a)) - list.findIndex(o => o[0] === originOf(b)));
};
const photos = () => info()?.sources.find(s => s.kind === 'photos');

// ---------- 新建图库 ----------
async function create() {
  const sources = Object.entries(libForm.folders).flatMap(([preset, paths]) => paths.map(path => ({kind: 'folder', path, preset})));
  if (libForm.photos) sources.push({kind: 'photos'});
  if (!sources.length) { toast(t('请至少添加一个文件夹')); return; }
  const gen = libForm.gen, route = state.route;
  const lib = await libraryAction('new', {name: libForm.name, sources});
  if (!lib) return;
  // 建好了但没能登记到最近图库（listed 为 false）：路径只在这个回复里，不管还在不在这一节，都留进这一份表单（没取消、
  // 没重开的话），回来时照样显示原路径、原因和「打开这个图库」，再点创建不会另建一份。取消或重开过的，提示一句路径。
  if (lib.listed === false) {
    if (libForm.gen === gen) { libForm.created = lib; refreshSection(); }
    else toast(t('保存在 {path}，但没能加入最近图库列表：{reason}', {path: home(lib.path), reason: tb(lib.error)}));
    return;
  }
  // 自动打开只在原来的操作还有效时：等回复期间点了取消、重开了表单或者离开了这一节，图库照样建好（在图库列表里）。
  if (libForm.gen !== gen || !still(route)) { loadLibraries(); return; }
  libForm.open = false; libForm.gen++; openLibrary(lib.path, lib.name);
}
async function pick(origin) {
  // 选择框开着的时候离开了这一节（路由换了）、取消或重开了表单：这次选的作废，不写回表单。
  const gen = libForm.gen, route = state.route, picked = await pickFolders();
  if (libForm.gen !== gen || !libForm.open || !still(route)) return;
  const column = libForm.folders[origin] ||= [];
  for (const p of picked) if (!Object.values(libForm.folders).some(c => c.includes(p))) column.push(p);
  refreshSection();
}
const closeForm = () => { libForm.open = false; libForm.gen++; refreshSection(); };
const picked = () => origins().flatMap(([id, label]) => (libForm.folders[id] || []).map((path, i) => ({id, label, path, i})));

async function manage(act) {
  if (act === 'new') { Object.assign(libForm, {open: true, name: '', folders: {}, photos: false, created: null, gen: libForm.gen + 1}); refreshSection(); return; }
  if (act === 'library' || act === 'zip') {
    const route = state.route, r = await libraryAction('choose', {kind: act}); if (!r?.path || !still(route)) return;
    if (act === 'library') openLibrary(r.path, r.path.split('/').pop().replace(/\.pilib$/, ''));
    else await startImport(r.path);
    return;
  }
  if (act === 'export' && await libraryAction('export')) { await loadLibraries(); pollExport(); }
}
function exportText(ex = state.libraries?.export || {}) {
  if (ex.status === 'running') return t('正在导出到 {path}…', {path: home(ex.path)});
  if (ex.status === 'done') return t('已导出：{path}（{mb} MB）。', {path: home(ex.path), mb: megabytes(ex.bytes)}) + EXPORT_LEAVES_OUT;
  if (ex.status === 'error') return t('导出失败：{reason}', {reason: tb(ex.error)});
  return '';
}

// 「图库」这一组的配置（给 ui/settings_library.js 的 groups()）。
export const librariesGroup = () => ({key: 'libraries', title: t('图库'), note: t('每个图库各自保存识别结果和人工整理记录，图片本身留在原处、不复制。'),
  foot: s => t('导出的 zip 包含识别结果、人工整理记录和来源配置，不含模型连接设置或联网授权，可以换电脑或发给别人。导入后需重新设置模型接口，并自行确认名称查询的联网授权；旧版导出包也会清除这些设置。个人备份请保留完整图库目录。')
    + (s.libraries?.export?.status === 'done' ? '' : EXPORT_LEAVES_OUT),
  parts: [
    {kind: 'note', text: t('正在读取…'), hidden: s => !!s.libraries},
    {kind: 'note', text: t('当前是旧的数据布局（不是图库包）。'), hidden: s => !s.libraries || !!info()},
    {kind: 'info', key: 'current', label: () => info()?.name || '', hint: () => home(info()?.path || ''), value: t('当前图库'), hidden: () => !info()},
    {kind: 'sub', label: t('来源'), hidden: () => !info()},
    {kind: 'block', key: 'folders', hidden: () => !info(), mount: (el, scope) => {
      const list = keyed(el, {key: s => s.id, create: s => folderRows(s, scope), scope});
      return {update: () => list.set(folders())};
    }},
    {kind: 'action', key: 'photos', label: t('iCloud 照片'), hint: t('本机「照片」图库，按存图的 App 区分 X 和 Pixiv'), hidden: () => !photos(),
     buttons: () => editable() ? [removeButton()] : [], run: () => removeSource(photos().id)},
    {kind: 'info', key: 'noSources', label: t('还没有来源'), hidden: () => !info() || folders().length || photos()},
    {kind: 'action', key: 'addSource', label: t('添加文件夹'), hint: t('子文件夹会一起读取'), hidden: () => !editable(),
     buttons: () => [...origins().map(([id, label]) => ({act: id, label, title: t('图片来自 {origin} 的文件夹，子文件夹会一起读取', {origin: label})})),
       ...(!photos() && isMac ? [{act: 'photos', label: t('添加 iCloud 照片')}] : [])],
     run: async (s, act) => {
       const route = state.route, body = act === 'photos' ? {kind: 'photos'} : {kind: 'folder', preset: act, paths: await pickFolders()};
       if ((body.paths && !body.paths.length) || !still(route)) return;
       const r = await libraryAction('sources/add', body);
       await loadLibraries();
       if (r) { toast((act === 'photos' ? t('已添加 iCloud 照片') : t('已添加 {n} 个文件夹', {n: r.added?.length || 1})) + (r.sync ? t('，正在同步') : '')); if (r.sync) pollSync(); }
     }},
    {kind: 'sub', label: t('图库管理')},
    {kind: 'list', key: 'others', label: t('这台电脑上的其他图库'), hidden: s => !(s.libraries?.libraries || []).some(l => l.path !== s.libraries.current),
     items: s => (s.libraries?.libraries || []).filter(l => l.path !== s.libraries.current), itemKey: l => l.path,
     // 测试图库是由开发者工具生成的，所以它的标记只在开发模式下显示。
     itemText: l => l.name + (devMode && l.test ? ` · ${l.test}` : ''), itemHint: l => home(l.path),
     actions: [{label: t('打开'), run: l => openLibrary(l.path, l.name)},
       {label: t('移出列表'), title: () => t('只从列表里移除，不删除图库'), run: async l => { if (await libraryAction('forget', {path: l.path})) loadLibraries(); }}]},
    {kind: 'action', key: 'manage', label: t('新建、打开或导入图库'), hint: () => exportText(),
     buttons: s => [{act: 'new', label: t('新建图库')}, {act: 'library', label: t('打开其他图库…')},
       {act: 'zip', label: t('导入…'), disabled: importActive()},
       ...(info() ? [{act: 'export', label: t('导出当前图库'), disabled: s.libraries?.export?.status === 'running'}] : [])],
     run: (s, act) => manage(act)},
    // #88：导入作为后台任务运行（core/jobs.js）：状态行、运行期间的取消按钮，以及它是怎样结束的。
    {kind: 'action', key: 'import', label: s => importLine(s.importTask) || '', hidden: s => !importLine(s.importTask),
     buttons: s => {
       const job = s.importTask, r = job?.result || {};
       return job?.status === 'running' ? [{act: 'cancel', label: t('取消导入')}]
         : job?.status === 'done' && r.listed === false ? [{act: 'open', label: t('打开这个图库'), primary: true}] : [];
     },
     run: (s, act) => act === 'cancel' ? cancelImport().catch(err => toast(err.message)) : openLibrary(s.importTask.result.path, s.importTask.result.name)},
    // 新建图库的表单。
    {kind: 'sub', label: t('新建图库'), hidden: () => !libForm.open},
    {kind: 'note', text: t('按图片来自哪里分别添加文件夹，每类可以有多个。子文件夹会一起读取；图片保留在原位置，不复制。选 X 或 Pixiv 的文件夹才能用以图搜图、画师回查。'), hidden: () => !libForm.open},
    {kind: 'list', key: 'formFolders', label: t('要读取的文件夹'), hidden: () => !libForm.open || !picked().length,
     items: picked, itemKey: x => x.id + '\n' + x.path, itemText: x => home(x.path), itemHint: x => x.label,
     actions: [{label: t('不添加'), run: x => { libForm.folders[x.id]?.splice(x.i, 1); refreshSection(); }}]},
    {kind: 'action', key: 'formPick', label: t('添加文件夹'), hidden: () => !libForm.open,
     buttons: () => origins().map(([id, label]) => ({act: id, label})), run: (s, act) => pick(act)},
    {kind: 'switch', key: 'formPhotos', label: t('同时读取 iCloud 照片'), hint: t('本机「照片」图库，按存图的 App 区分 X 和 Pixiv'), hidden: () => !libForm.open || !isMac,
     get: () => libForm.photos, set: on => { libForm.photos = on; }},
    {kind: 'field', key: 'formName', label: t('名字'), hint: s => t('保存在 {path}', {path: home(s.libraries?.home || '')}), placeholder: t('新图库'),
     hidden: () => !libForm.open, get: () => libForm.name, set: v => { libForm.name = v; }},
    {kind: 'action', key: 'formCreate', label: '', hidden: () => !libForm.open || !!libForm.created,
     buttons: () => [{act: 'create', label: t('创建并打开'), primary: true}, {act: 'cancel', label: t('取消')}],
     run: (s, act) => act === 'create' ? create() : closeForm()},
    {kind: 'action', key: 'formCreated', label: t('图库已经建好'), hidden: () => !libForm.open || !libForm.created,
     hint: () => libForm.created ? t('保存在 {path}，但没能加入最近图库列表：{reason}', {path: home(libForm.created.path), reason: tb(libForm.created.error)})
       + ' ' + t('打开它会再试一次加入列表；再点创建会另建一个带编号的图库。') : '',
     buttons: () => [{act: 'open', label: t('打开这个图库'), primary: true}, {act: 'again', label: t('仍然新建一个')}],
     run: (s, act) => act === 'open' ? openLibrary(libForm.created.path, libForm.created.name) : (libForm.created = null, refreshSection())},
  ]});
