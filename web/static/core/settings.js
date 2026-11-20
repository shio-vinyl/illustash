// 设置页读取和更改的内容（#199）：启动开关、相似图片的折叠、备份和缩略图缓存、界面语言、名称的联网授权和模型
// API、图库及其来源、关联账号和更新检查。不碰 DOM。数据仍在 core/state.js（settings、fold、about、names、libraries、
// accounts、pixivWaiting、updates），直到拆分页面状态；core/ 之外没有地方写这些字段。onSettings(what, detail) 在每次
// 读取或更改后通知；what 为 data、libraries、accounts、pixiv 或 updates。
import {t, said} from './i18n.js';
import {session, api, post, shared, leaveSession} from './session.js';
import {state} from './state.js';
import {listeners} from './events.js';
import {mergeNames, setIndex, setNames} from './jobs.js';
const changed = listeners();
export const onSettings = changed.on;

// ---------- 设置页的数据 ----------
let loading = null;
// 一次读齐页面还没有的部分（refresh 时读全部）。每部分到达时就通知（onSettings('data', {key, error})），有用的先显示，
// 不用等慢的；为页面已经离开的图库发出的读取丢掉。
export function loadSettingsData(refresh = false) {
  if (loading) return loading;
  const token = session.token;
  const missing = [['settings', '/api/settings'], ['fold', '/api/fold'], ['index', '/api/index'], ['names', '/api/names']]
    .filter(([key]) => refresh || !state[key]);
  if (!missing.length) return;
  loading = Promise.all(missing.map(async ([key, path]) => {
    let error = null;
    try { const value = await shared(path); if (token !== session.token) return; state[key] = key === 'fold' ? value.kinds : value; }
    catch (e) { if (token !== session.token) return; error = e; }
    await changed.emit('data', {key, error});
  })).finally(() => { loading = null; });
  return loading;
}
// 「设置 › 角色资料」两张卡片的数据重新读一次（进这一节时，#714 前在 ui/settings.js 里）。
export const loadIndex = async () => { const token = session.token, value = await shared('/api/index'); if (token === session.token) setIndex(value); return value; };
export const loadNames = async () => { const token = session.token, value = await shared('/api/names'); if (token === session.token) setNames(value); return value; };
// 上次检查新角色时每个关注作品的情况（#561，GET /api/index/coverage）：本图库启用的数据包已经覆盖几个、自己补过几个、
// 还缺几个。/api/index 的回复冻结，所以另读一次；检查或添加角色之后（检查时间或最近一条历史变了）重新读。返回是否读了新的。
let coverage = null, coverageKey = '';
export const indexCoverage = () => coverage;
export async function loadCoverage(x = state.index, fresh = false) {
  const key = JSON.stringify([x?.check?.checked_at, x?.history?.[0]?.at]);
  if (!fresh && key === coverageKey) return false;
  coverageKey = key;
  try { coverage = await shared('/api/index/coverage'); } catch { coverage = null; }
  return true;
}
// /api/settings 的最新回应；任务面板读到新的也经这里换上。
export function setSettings(value) { state.settings = value; }
// 启动时的开关；本次启动仍以 --no-* 参数为准。
export async function saveStartup(body) { state.settings = await post('/api/startup', body); }
export async function saveFold(kinds) { state.fold = (await post('/api/fold', {kinds})).kinds; }
export async function backupNow() { const r = await post('/api/backup', {}); state.settings = null; return r; }
export async function clearCache() { const r = await post('/api/cache', {action: 'clear'}); state.settings = r; return r; }
// 全应用共用（preferences.json，#173）：每个窗口和图库都跟着它；之后页面用新语言重新载入。
export const saveLanguage = language => api('/api/preferences', {method: 'POST', body: JSON.stringify({language})});
// 桌面 App 里关窗转到后台以后，任务做完时退出还是留着（#451，desktop/src/background.rs 读它）。
export async function saveAfterTasks(value) {
  await api('/api/preferences', {method: 'POST', body: JSON.stringify({after_tasks: value})});
  if (state.settings?.desktop) state.settings.desktop.after_tasks = value;
}
export async function loadAbout() {
  const token = session.token, value = await shared('/api/about');
  if (token === session.token) state.about = value;
}

// ---------- 名称：联网授权（和 Danbooru 搜索共用）以及可选的模型 API ----------
export async function allowNames(allow) {
  mergeNames(await api('/api/names', {method: 'POST', body: JSON.stringify({action: 'network', allow})}));
}
// 一次 Danbooru 名称查询（只有 tag 和作品名）：它的结果，或失败的原因。这个图库没有授权时，后端以「显示名联网尚未授权」
// 拒绝；其他失败都是真正的网络或服务错误。
export const remoteNames = path => api(path).then(found => ({found}), e => ({found: null, unauthorized: said(e.said, '显示名联网尚未授权'), reason: e.message}));
export const remoteNote = r => r.unauthorized ? t('查 Danbooru 需要先允许这个图库联网查询名称（只发送文字）') : t('Danbooru 暂时连不上：{reason}', {reason: r.reason});
export async function saveModelApi(body) { mergeNames(await post('/api/names', {action: 'model_api', ...body})); }
export const testModelApi = () => post('/api/names', {action: 'model_api_test'});

// ---------- 图库（docs/LIBRARIES.md） ----------
export async function loadLibraries() {
  try { state.libraries = await api('/api/libraries'); } catch { state.libraries = null; }
  await changed.emit('libraries');
}
// 当前图库的 ID（library.json 的 id），给只属于一个图库的偏好做键（core/prefs.js 的 libraryPrefs，#523）。切换图库时
// 整页重新载入，所以每个页面只取一次；取不到时是空字符串，下次再取。
let libraryId = null;
export const currentLibraryId = () => libraryId ??= api('/api/libraries').then(x => x?.info?.id || '', () => '')
  .then(id => { if (!id) libraryId = null; return id; });
// POST /api/libraries/<action>，带着发起请求的那个页面的 token：确认框可能比它的会话活得久。
export const libraryRequest = (action, body = {}, token = session.token) =>
  api('/api/libraries/' + action, {method: 'POST', body: JSON.stringify(body), headers: {'X-CSRF-Token': token}});
// 跟踪进行中的导出，最多一小时。
export async function pollExport() {
  for (let i = 0; i < 2400 && state.libraries?.export?.status === 'running'; i++) { await new Promise(r => setTimeout(r, 1500)); await loadLibraries(); }
}
// 系统的文件夹选择框，可一次选多个；没有选择框的地方抛出「Unsupported…」。
export async function chooseFolders(multiple = true) {
  const r = await api('/api/libraries/choose', {method: 'POST', body: JSON.stringify({kind: 'folder', multiple})});
  return multiple ? r?.paths || [] : r?.path ? [r.path] : [];
}
// 打开另一个图库会让服务在它上面重启。`agreed` 是用户事先同意的事：'upgrade' 升级旧图库（#193），或 'rescue' 抢救它
// 损坏的 source.sqlite（#228）。之后这个页面属于它离开的那个图库：立即停止读写。后端拒绝时抛出拒绝的内容。
export async function openLibraryRequest(path, agreed = null) {
  await api('/api/libraries/open', {method: 'POST', body: JSON.stringify(agreed ? {path, [agreed]: true} : {path})});
  leaveSession();
}
// 服务是否在约 80 s 内回到了那个图库上。
export async function libraryOpened(path) {
  for (let i = 0; i < 120; i++) {
    await new Promise(r => setTimeout(r, 700));
    try {
      const s = await fetch('/api/settings').then(r => r.json());
      if (s.package?.path === path) return true;
    } catch { /* 服务正在重启 */ }
  }
  return false;
}

// ---------- 关联账号（docs/PROVENANCE.md 第 8 节）：所有图库共用；密钥存在钥匙串里 ----------
export async function loadAccounts() {
  try { state.accounts = await api('/api/accounts'); } catch { state.accounts = null; }
  await changed.emit('accounts');
}
export const accountRequest = (action, body = {}) => api('/api/accounts/' + action, {method: 'POST', body: JSON.stringify(body)});
// 在桌面 App 的窗口里登录 Pixiv：每 2 s 轮询一次，最多 10 分钟，同一时间只发一个请求；对已停止或被替换的等待的回应
// 不改变任何东西。结束时以 onSettings('pixiv', {bound}) 带上账号通知，或通知 {error}。
let pixivPoll = null;
export function stopPixivLogin() {
  if (pixivPoll) {
    clearTimeout(pixivPoll.timer);
    pixivPoll.controller.abort();
    pixivPoll = null;
  }
  state.pixivWaiting = false;
}
export function waitPixivLogin() {
  stopPixivLogin();
  const poll = {controller: new AbortController(), timer: null, until: Date.now() + 10 * 60e3};
  pixivPoll = poll;
  state.pixivWaiting = true; changed.emit('accounts');
  const tick = async () => {
    if (pixivPoll !== poll) return;
    if (Date.now() >= poll.until) { stopPixivLogin(); changed.emit('accounts'); return; }
    let r;
    try {
      r = await api('/api/accounts/pixiv/connect', {method: 'POST', body: '{}', signal: poll.controller.signal});
    } catch (error) {
      if (pixivPoll !== poll) return;
      stopPixivLogin(); changed.emit('pixiv', {error}); loadAccounts(); return;
    }
    if (pixivPoll !== poll) return;
    if (r.bound) { stopPixivLogin(); changed.emit('pixiv', {bound: r}); loadAccounts(); }
    else if (!r.login_open || Date.now() >= poll.until) { stopPixivLogin(); changed.emit('accounts'); }
    else poll.timer = setTimeout(tick, 2000);
  };
  poll.timer = setTimeout(tick, 2000);
}

// ---------- 更新检查（#191，native/terms/src/app_updates.rs） ----------
// 用户允许之前关闭。允许之后，后端在每次启动时检查，App 一直开着时每天检查一次；页面只询问是否该检查了。
// 「下载并重启」（#279）由桌面外壳做，状态在 state.updates.install（不在桌面 App 里时为 null）：这里转达 download、cancel、
// install，下载期间每秒读一次进度，下完或失败就停。
let updatesAt = 0, updatesPending = null, installTimer = 0;
const adoptUpdates = value => {
  state.updates = value; updatesAt = Date.now();
  clearTimeout(installTimer);
  if (value?.install?.stage === 'downloading') {
    installTimer = setTimeout(() => api('/api/updates').then(adoptUpdates).catch(() => {}).finally(() => changed.emit('updates')), 1000);
  }
  return value;
};
export async function updatesPost(body) {
  return adoptUpdates(await api('/api/updates', {method: 'POST', body: JSON.stringify(body)}));
}
// 手动检查：总会发出。
export const checkUpdates = () => updatesPost({action: 'check'});
// 由任务面板的定时调用：不时刷新缓存的状态，并运行到期的自动检查；完成时以 onSettings('updates') 通知，不管结果如何。
export function updatesTick() {
  if (updatesPending) return updatesPending;
  updatesPending = (async () => {
    if (!state.updates || Date.now() - updatesAt > 600000) adoptUpdates(await api('/api/updates'));
    if (state.updates.due) await updatesPost({action: 'check', auto: true});
  })().catch(() => {}).finally(() => { updatesPending = null; changed.emit('updates'); });
  return updatesPending;
}
