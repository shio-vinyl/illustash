// 任务面板（桌面布局，#199）：所有后台任务的进度、确认和手动运行集中在一处。任务本身、它们的轮询和文字在
// core/jobs.js。
import {diagnosticLine} from '../core/diagnostics.js';
import {t, tb} from '../core/i18n.js';
import {esc, home, megabytes, number, stamp, localTime} from '../core/format.js';
import {state} from '../core/state.js';
import {api, post, shared, readError} from '../core/session.js';
import {SYNC_ACTIVE, INDEX_ACTIVE, NAMES_ACTIVE, SIMILAR_ACTIVE, PROV_ACTIVE, RUN_STATUS, onJob, syncLine, syncBar, syncDoneText, indexLine, indexEndText, resumeIndex, discardIndex,
  namesLine, taskSimilar, pollSync, pollIndex, pollNames, loadProvenance, startSync, checkIndex, refreshNames, provenanceAction, PHOTOS_PRIVACY, photosHelp, photosRefused,
  importActive, importSummary, cancelImport, pollImports} from '../core/jobs.js';
import {loadLibraries, setSettings, updatesTick} from '../core/settings.js';
import {$, toast, error, fold, wireFolds, redraw, setDrawn, busy} from './dom.js';
import {confirmAction} from './dialog.js';
import {createLayer} from './overlay.js';
import {showPop} from './pop.js';
import {openIndexAdd} from './catalog.js';
import {modelsLine, modelsBar, modelsActions} from './models.js';
import {cancelExport, exportActive, exportLine, loadFiles, onFiles, revealExport, revealLabel} from '../core/files.js';
import {loadModels, onModels} from '../core/models.js';
import {packTask} from './packs.js';
import {loadPack, onPack} from '../core/packs.js';
// 「开始」先显示有哪些内容会离开本机；授权只覆盖这一次运行。
let provenanceArmed = false;
// 打开面板；带 armProvenance 时停在一次找回画师运行的授权上。
export function openTasks({armProvenance = false} = {}) {
  if (armProvenance) provenanceArmed = true;
  toggleTasks(true);
}
// 文件能打开、但不是 App 能读的图片的那些：它们在哪里，和图库页上一样。
export function syncDamaged(d) {
  if (!d?.count) return '';
  const rows = d.paths.map(p => `<div class="setting-line lib-folder"><span class="lib-path" title="${esc(p)}"><bdi>${esc(home(p))}</bdi></span></div>`).join('');
  const more = d.count > d.paths.length ? `<p class="muted-note">${t('另有 {n} 张未列出。', {n: number(d.count - d.paths.length)})}</p>` : '';
  return fold('task-sync-damaged', t('{n} 张图片已损坏或格式不支持', {n: number(d.count)}),
    `<p class="muted-note">${t('这些文件读得到，但不是能打开的图片：已经加入图库，图上显示「无法打开」，不参与识别。修好或换成正常的图片后，下次同步会重新识别。')}</p>${rows}${more}`);
}
// 由 worker 的数字画的进度条（docs/BACKEND_TASKS.md）：这一步的条目数，以及相对本次运行上限的流量。
function provenanceBars(s) {
  const p = s.progress; if (!p || !p.total) return '';
  const bar = (label, done, total, detail) => `<div class="prov-bar-row"><span class="prov-bar-label">${label}</span><span class="prov-bar" role="progressbar" aria-valuemin="0" aria-valuemax="${total}" aria-valuenow="${done}"><span style="width:${Math.min(100, done / total * 100).toFixed(1)}%"></span></span><span class="prov-bar-num">${detail}</span></div>`;
  // 这一步有两层时显示两层（本地比对：全部画师，然后是当前画师的作品），然后是流量。
  const sub = p.sub_total ? bar(esc(tb(p.sub_label) || t('当前')), p.sub_done || 0, p.sub_total, `${number(p.sub_done || 0)} / ${number(p.sub_total)}`) : '';
  return `<div class="prov-bars">${bar(esc(tb(p.label) || t('进度')), p.done || 0, p.total, `${number(p.done || 0)} / ${number(p.total)}${p.found != null ? ' · ' + t('对上 {n}', {n: number(p.found)}) : ''}`)}${sub}${p.mb != null && s.budget_mb ? bar(t('流量'), p.mb, s.budget_mb, `${number(Math.round(p.mb))} / ${number(s.budget_mb)} MB`) : ''}</div>`;
}

// ---------- 任务面板（#199）：所有后台任务的进度、确认和手动运行集中在一处 ----------
// 每个任务保留自己的接口和轮询；这里只是把它们汇到一起。有任务在运行时，它的轮询每 2 s 自己重新排上；否则由一个慢的
// 定时调用（面板打开时变快）发现在别处启动的任务。
export const taskPanel = {open: false, timer: 0, settingsAt: 0};
export function taskList() {
  const s = state.sync, ix = state.index?.state || {}, nm = state.names?.state || {}, sim = state.settings?.tasks?.similar || {status: 'off'};
  const pv = state.provenance, ps = pv?.state || {}, ex = state.libraries?.export || {};
  const folder = !(state.settings?.package?.sources || state.libraries?.info?.sources || []).some(x => x.kind === 'photos');
  const bar = (done, total) => total ? `<div class="progress"><span style="width:${Math.min(100, Math.round((done || 0) / total * 100))}%"></span></div>` : '';
  const list = [
    {key: 'sync', name: folder ? t('文件夹同步') : t('照片同步'), busy: t('同步中'), active: SYNC_ACTIVE.includes(s.status), off: s.status === 'off', line: s.status === 'off' ? t('这次启动没有开同步') : syncLine(s),
      warn: ['error', 'needs_recognition', 'interrupted'].includes(s.status), body: syncBar(s, bar) + (SYNC_ACTIVE.includes(s.status) ? '' : syncDamaged(state.syncDamaged)),
      action: `<button class="quiet small" data-task="sync" ${SYNC_ACTIVE.includes(s.status) || s.status === 'off' ? 'disabled' : ''}>${t('立即同步')}</button>`,
      log: state.syncHistory?.length ? [t('同步记录'), `<table class="settings-table"><tr><th>${t('时间')}</th><th>${t('新增')}</th><th>${t('移除', {context: 'column'})}</th><th>${t('导入')}</th><th>${t('失败')}</th></tr>${state.syncHistory.slice(0, 8).map(r => `<tr><td>${esc(stamp(r.started_at))}</td><td>${number(r.found)}</td><td>${number(r.removed)}</td><td>${number(r.imported)}</td><td>${number(r.failed)}</td></tr>`).join('')}</table>`] : null},
    {key: 'index', name: t('检查新角色'), busy: t('检查新角色中'), active: INDEX_ACTIVE.includes(ix.status), line: state.index ? indexLine(ix, state.index.check) : '…', warn: ix.status === 'error',
      body: INDEX_ACTIVE.includes(ix.status) || ix.status === 'paused' ? bar(ix.done, ix.total) : '',
      action: `<button class="quiet small" data-task="index" ${INDEX_ACTIVE.includes(ix.status) || ix.status === 'off' || ix.status === 'paused' || !state.index?.watch?.works.length ? 'disabled' : ''} title="${ix.status === 'paused' ? t('先继续或放弃暂停的添加') : state.index?.watch?.works.length ? t('查关注作品在 Danbooru 上有没有新角色（只发送作品名）') : t('先在资料库里关注作品')}">${t('立即检查')}</button>`},
    {key: 'names', name: t('角色名称'), busy: t('角色名称中'), active: NAMES_ACTIVE.includes(nm.status), off: nm.status === 'off', line: namesLine(nm) + (nm.error ? t('：{reason}', {reason: tb(nm.error)}) : ''), warn: nm.status === 'error', body: NAMES_ACTIVE.includes(nm.status) ? bar(nm.done, nm.total) : '',
      action: `<button class="quiet small" data-task="names" ${NAMES_ACTIVE.includes(nm.status) || nm.status === 'off' || !state.names ? 'disabled' : ''} title="${t('从 Bangumi 重新获取全部角色的中文名（只发送 tag 与作品名）')}">${t('重新获取')}</button>`},
    {key: 'similar', name: t('相似图片'), busy: t('相似图片中'), active: SIMILAR_ACTIVE.includes(sim.status), off: sim.status === 'off', line: taskSimilar(sim), warn: sim.status === 'error', body: SIMILAR_ACTIVE.includes(sim.status) ? bar(sim.done, sim.total) : '',
      action: `<button class="quiet small" data-task="similar" ${SIMILAR_ACTIVE.includes(sim.status) || sim.status === 'off' ? 'disabled' : ''}>${t('重新检查')}</button>`},
  ];
  if (pv?.available) list.push({key: 'provenance', name: t('找回画师'), busy: t('找回画师中'), active: PROV_ACTIVE.includes(ps.status), warn: ps.status === 'failed',
    line: ps.status === 'stopping' ? t('正在停止…') : PROV_ACTIVE.includes(ps.status) ? t('正在查找 · 第 {step}/{steps} 步', {step: (ps.index || 0) + 1, steps: ps.total || 1}) + (ps.text ? ' · ' + tb(ps.text) : '')
      : ps.status === 'failed' ? t('上次出错：{reason}', {reason: tb(ps.reason || '')}) : pv.runs?.[0] ? t('上次 {time} · 新找到 {n}', {time: localTime(pv.runs[0].started_at), n: number(pv.runs[0].found || 0)}) : t('还没有查过'),
    body: PROV_ACTIVE.includes(ps.status) ? provenanceBars(ps) + (ps.side ? `<p class="task-side">${t('同时进行')} · ${esc(tb(ps.side.name || ps.side.step))}${ps.side.text ? ' · ' + esc(tb(ps.side.text)) : ''}</p>${provenanceBars({progress: ps.side.progress})}` : '') : '',
    action: PROV_ACTIVE.includes(ps.status) ? `<button class="quiet small" data-prov="stop" ${ps.status === 'stopping' ? 'disabled' : ''}>${t('停止')}</button>` : provenanceArmed ? '' : `<button class="quiet small" data-prov="arm">${t('开始…')}</button>`,
    log: pv.runs?.length ? [t('运行记录'), `<table class="settings-table"><tr><th>${t('开始')}</th><th>${t('方式')}</th><th>${t('新找到')}</th><th>${t('流量')}</th><th>${t('结果')}</th></tr>${pv.runs.slice(0, 8).map(r => `<tr><td>${esc(localTime(r.started_at))}</td><td>${r.kind === 'new' ? t('同步后') : r.kind === 'full' ? t('手动') : t('命令行')}</td><td>${number(r.found || 0)}</td><td>${(r.traffic_mb || 0).toFixed(1)} MB</td><td title="${esc(tb(r.reason || ''))}">${RUN_STATUS[r.status] || esc(r.status)}</td></tr>`).join('')}</table>`] : null});
  // 没带模型的安装（#168）：在模型就位之前，以及下载或导入进行期间。
  const m = state.models;
  if (m?.on_demand && (m.status !== 'ready' || m.job?.status === 'running')) list.push({key: 'models', name: t('识别模型'), busy: m.job?.kind === 'import' ? t('导入识别模型中') : t('下载识别模型中'),
    active: m.job?.status === 'running', warn: m.job?.status === 'failed', line: modelsLine(m), body: modelsBar(m), action: modelsActions(m, true)});
  // 角色数据包（#550、#559）：每个包一项，下载或导入、启用、撤回共用，运行中和结束后都列出来。
  const packs = [packTask()].filter(Boolean);
  list.push(...packs);
  if (['running', 'done', 'error'].includes(ex.status)) list.push({key: 'export', name: t('导出图库'), busy: t('导出图库中'), active: ex.status === 'running', warn: ex.status === 'error',
    line: ex.status === 'running' ? t('正在导出…') : ex.status === 'done' ? t('已导出 {mb} MB', {mb: megabytes(ex.bytes)}) : t('导出失败：{error}', {error: tb(ex.error || '')}), body: '', action: ''});
  // 选中的图导出到文件夹（#450）：运行中可以取消，结束后可以打开那个文件夹。
  const fx = state.files?.export;
  if (['running', 'done', 'error', 'cancelled'].includes(fx?.status)) {
    const action = exportActive() ? `<button class="quiet small" data-task="export-cancel">${t('取消导出')}</button>`
      : fx.status !== 'error' ? `<button class="quiet small" data-task="export-reveal">${revealLabel()}</button>` : '';
    list.push({key: 'files', name: t('导出图片'), busy: t('导出图片中'), active: exportActive(), warn: fx.status === 'error',
      line: exportLine(fx), body: exportActive() ? bar(fx.done, fx.total) : '', action});
  }
  // 导入（图库归档、识别结果、人工结论共用一个槽；#386 的 2-37）：和导出一样，运行中、结束后都在这里有一行，运行时可以取消。
  const im = state.importTask;
  if (['running', 'stopping', 'done', 'error', 'cancelled'].includes(im?.status)) list.push({key: 'import',
    name: im.kind === 'recognition' ? t('导入识别结果') : im.kind === 'human_conclusions' ? t('导入人工结论') : t('导入图库'), busy: t('导入中'),
    active: importActive(), warn: im.status === 'error', line: importSummary(im), body: '',
    action: im.status === 'running' ? `<button class="quiet small" data-task="import-cancel">${t('取消导入')}</button>` : ''});
  const diagnoses = {sync: s, index: ix, names: nm, similar: sim, provenance: ps, models: m?.job, export: ex, import: im, ...Object.fromEntries(packs.map(p => [p.key, p]))};
  for (const task of list) {
    const note = diagnosticLine(diagnoses[task.key]?.diagnostic);
    if (note && task.warn) task.body += `<p class="task-side">${esc(note)}</p>`;
  }
  return list;
}
// 等用户处理的事：一次授权、要添加的角色、需要动手的同步。
function taskAsks() {
  const asks = [], p = state.provenance, missing = state.index?.check?.missing || [], s = state.sync;
  if (provenanceArmed && p?.available) asks.push(`<div class="task-ask prov-confirm"><b>${t('找回画师这次要访问外网，确认后才会发出：')}</b>
    <ul><li>${t('发给 Danbooru：图片的像素哈希和感知哈希（IQDB 签名，都是一串文本，不是图片）、画师名和账号')}</li>
    <li>${state.accounts?.pixiv?.bound ? t('用你绑定的 Pixiv 登录状态读取你的关注列表（只读）') : t('没有绑定 Pixiv：只看公开作品，不含 NSFW')}</li>
    <li>${t('本次最多 {mb} MB；遇到拒绝、限流或验证码就停，不绕过', {mb: number(p.budgets?.full || 1024)})}</li></ul>
    <p class="muted-note">${t('授权只对这一次有效，下次开始会再问。查过的图和画师不会重查。')}</p>
    <div class="prov-confirm-btns"><button class="primary" data-prov="start" data-network="1">${t('授权并开始')}</button><button class="quiet" data-prov="start">${t('只跑本地（签名识别）')}</button><button class="quiet" data-prov="cancel">${t('取消')}</button></div></div>`);
  if (photosRefused(s)) asks.push(`<div class="task-ask"><span>${photosHelp()}</span><button class="primary small" data-task="photos-privacy">${t('打开系统设置')}</button></div>`);
  const ix = state.index?.state || {};
  if (ix.status === 'paused') asks.push(`<div class="task-ask"><span>${t('添加角色已暂停：已下载 {done}/{total} 个角色的参考图。{reason}', {done: number(ix.done || 0), total: number(ix.total || 0), reason: esc(tb(ix.message || ''))})}</span><button class="primary small" data-task="index-resume">${t('继续下载…')}</button><button class="quiet small" data-task="index-discard">${t('放弃')}</button></div>`);
  else if (missing.length && !INDEX_ACTIVE.includes(ix.status)) asks.push(`<div class="task-ask"><span>${t('上次检查发现 {n} 个还没收录的角色', {n: `<b>${number(missing.length)}</b>`})}</span><button class="primary small" data-task="index-add">${t('查看并添加…')}</button></div>`);
  return asks;
}
// 自动运行的开关在设置的哪几节（#386 的 2-37）：同步和相似图片在「图库」，检查新角色和角色名称在「角色资料」，
// 找回画师在「画师」。
const footLink = (section, label) => `<a class="link" href="#/settings/${section}">${label}</a>`;
// renderTasks 和 taskPanel 也 导出给样式清单页调用（tools/styleguide/live.js，#352）：改名或改参数时，tools/check_styleguide.py 会报出来。
export function renderTasks() {
  const list = taskList(), active = list.filter(t => t.active), warn = list.some(t => t.warn), asks = taskAsks();
  const button = $('taskButton'), badge = $('taskBadge');
  button.classList.toggle('busy', !!active.length);
  $('taskLabel').textContent = active.length > 1 ? t('{n} 项进行中', {n: active.length}) : active.length ? active[0].busy : t('任务');
  button.title = active.length ? active.map(task => t('{name}：{line}', {name: task.name, line: task.line})).join('\n') : asks.length ? t('有事等你确认') : warn ? t('有任务出错了') : t('任务：同步、检查新角色、找回画师…');
  badge.hidden = !asks.length && !warn; badge.className = 'task-badge' + (warn && !asks.length ? ' warn' : '');
  if (!taskPanel.open) return;
  const order = [...active, ...list.filter(t => !t.active)];
  const html = `<div class="task-top"><h2>${t('任务')}</h2><button class="icon" data-task="close" aria-label="${t('关闭')}">×</button></div>
    ${asks.length ? `<h6>${t('等你确认')}</h6><div class="task-asks">${asks.join('')}</div>` : ''}
    <h6>${active.length ? t('进行中与其他任务') : t('全部任务')}</h6><div class="view-group task-group">${order.map(task => `<div class="task-item${task.active ? ' on' : ''}">
      <div class="task-main"><span class="local-dot${task.active ? ' busy' : task.warn ? ' warn' : task.off ? ' off' : ''}"></span><span class="task-text"><b>${task.name}</b><small>${esc(task.line)}</small></span>${task.action}</div>
      ${task.body ? `<div class="task-body">${task.body}</div>` : ''}${task.log ? fold('task-' + task.key, task.log[0], task.log[1]) : ''}</div>`).join('')}</div>
    <p class="task-foot">${t('各任务是否自动运行，在设置的{library}、{characters}、{artists}里改。',
      {library: footLink('library', t('图库')), characters: footLink('characters', t('角色资料')), artists: footLink('artists', t('画师'))})}</p>`;
  const pop = $('taskPanel');
  // 任务运行期间会轮询：只在有变化时重画，这样点击绝不会因为按钮被替换而丢失。
  // 焦点在面板里时，重画后放回同一个按钮（按 data-task／data-prov 找），找不到就放在面板上，不掉到页面上（#422）。
  const had = pop.contains(document.activeElement) ? document.activeElement : null;
  const key = had && ['task', 'prov', 'pack'].map(k => had.dataset?.[k] ? `[data-${k}="${had.dataset[k]}"]` : '').find(Boolean);
  if (!redraw(pop, html)) return;
  wireFolds(pop);
  if (had && !pop.contains(document.activeElement)) (key && pop.querySelector(key) || pop).focus({preventScroll: true});
}
export async function taskTick() {
  clearTimeout(taskPanel.timer);
  const jobs = [pollSync(), pollIndex(), pollNames(), loadProvenance()];
  // 模型下载时有自己的轮询；这里发现在另一个窗口里启动的下载。
  if (taskPanel.open || !state.models) jobs.push(loadModels());
  // 角色资料包同样：运行中有自己的轮询，这里发现别处开始的下载或任务。
  if (taskPanel.open || !state.pack) jobs.push(loadPack());
  // 相似任务的状态随 /api/settings 一起来，它还要测量缓存：只在有人看时才读。
  if (taskPanel.open && Date.now() - taskPanel.settingsAt > (SIMILAR_ACTIVE.includes(state.settings?.tasks?.similar?.status) ? 2500 : 15000)) { taskPanel.settingsAt = Date.now(); jobs.push(shared('/api/settings').then(s => { setSettings(s); }).catch(e => error(readError(e)))); }
  // 面板开着时发现在别的窗口里启动的导入；运行中的导入有自己的轮询。
  if (taskPanel.open && !importActive()) jobs.push(pollImports());
  // 用图动作的状态（#450）：第一次读来知道能不能拖出；面板开着时发现在别的窗口里开始的导出，运行中的导出有自己的轮询。
  if (!state.files || (taskPanel.open && !exportActive())) jobs.push(loadFiles());
  // 面板打开后读一次图库列表（同步任务的名字和导出的进度）。
  if (taskPanel.open && !state.libraries) jobs.push(loadLibraries());
  await Promise.allSettled(jobs);
  renderTasks();
  updatesTick();
  taskPanel.timer = setTimeout(taskTick, taskPanel.open ? 3000 : 30000);
}
// 任务面板是非模态浮层：Esc、点外关闭由浮层管理器（ui/overlay.js）统一处理；它上面再开对话框时，对话框先关。
// 它在页面的最后，Tab 从「任务」按钮走不过去：打开时焦点移到面板上，Esc 或 × 关闭后回到打开它的按钮（#422）。
const taskLayer = createLayer({kind: 'popover', id: 'tasks', el: () => $('taskPanel'), anchor: () => $('taskButton'), focusIn: 'self', onDismiss: reason => toggleTasks(false, reason)});
export function toggleTasks(open = !taskPanel.open, reason) {
  taskPanel.open = open; showPop($('taskPanel'), open);
  $('taskButton').setAttribute('aria-expanded', open); $('taskButton').classList.toggle('on', open);
  if (open) { taskLayer.open(); setDrawn($('taskPanel')); renderTasks(); taskTick(); }
  else { provenanceArmed = false; taskLayer.close(reason); }
}
// 面板的处理函数。页面启动时调用一次。
export function wireTasks() {
  // 模型下载（#168）有自己的轮询：按钮和面板跟着每次读取变。
  onModels(() => renderTasks());
  onFiles(() => renderTasks());
  onPack(() => renderTasks());
  onJob(async (job, change) => {
    if (job === 'sync' && change.ended && state.sync.status === 'done' && state.sync.imported) toast(syncDoneText(state.sync));
    if (job === 'index' && change.ended && indexEndText(state.index.state)) toast(indexEndText(state.index.state));
    renderTasks();
  });
  $('taskPanel').addEventListener('click', async e => {
    const b = e.target.closest('[data-prov]'); if (!b) return;
    // 「开始」先显示有哪些内容会离开本机；授权只覆盖这一次运行。
    if (b.dataset.prov === 'arm' || b.dataset.prov === 'cancel') { provenanceArmed = b.dataset.prov === 'arm'; renderTasks(); return; }
    const body = {action: b.dataset.prov};
    if (b.dataset.prov === 'start') { body.allow_network = b.dataset.network === '1'; provenanceArmed = false; }
    await busy(b, () => provenanceAction(body).catch(err => toast(err.message)));
  });
  $('taskButton').onclick = e => { e.stopPropagation(); toggleTasks(); };
  $('taskPanel').addEventListener('click', async e => {
    e.stopPropagation();
    const b = e.target.closest('[data-task]'); if (!b) { if (e.target.closest('a[href]')) toggleTasks(false); return; }
    const act = b.dataset.task;
    if (act === 'close') { toggleTasks(false); return; }
    if (act === 'index-add') { openIndexAdd(); return; }
    if (act === 'photos-privacy') { location.href = PHOTOS_PRIVACY; return; }
    // 返回 false：在确认框里取消了，不用重新读取。
    const started = await busy(b, async () => {
      try {
        if (act === 'sync') await startSync();
        else if (act === 'index') return await checkIndex(confirmAction);
        else if (act === 'index-resume') { if (!await resumeIndex(confirmAction)) return false; pollIndex(); }
        else if (act === 'index-discard') return await discardIndex(confirmAction);
        else if (act === 'import-cancel') await cancelImport();
        else if (act === 'export-cancel') await cancelExport();
        else if (act === 'export-reveal') await revealExport();
        else if (act === 'names') { await refreshNames(); toast(t('已开始重新获取角色名称')); }
        else if (act === 'similar') { await post('/api/similar', {}); setSettings(await api('/api/settings')); }
      } catch (err) { toast(err.message); }
      return true;
    });
    if (started) taskTick();
  });
}
