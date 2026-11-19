// 新图库的向导（桌面布局，#96、#199）：导览、清单，以及在需要时才问的问题。
import {WORK_NAMES} from '../core/catalog.js';
import {esc, number, shortcut} from '../core/format.js';
import {composing} from '../core/keys.js';
import {said, t, tb} from '../core/i18n.js';
import {PHOTOS_PRIVACY, SYNC_ACTIVE, photosHelp, photosRefused, pollSync, setIndex, setNames, startSync, syncLine} from '../core/jobs.js';
import {onPage, pageOf} from '../core/pages.js';
import {go} from '../core/route.js';
import {api, post, session} from '../core/session.js';
import {allowNames, updatesPost, updatesTick} from '../core/settings.js';
import {state} from '../core/state.js';
import {$, toast, redraw, setDrawn, drawnHTML, busy} from './dom.js';
import {drawUpdateButton, openRelease, selfUpdate, updateAct, updateActions, updateBar, updateStarted, updateStatus} from './settings.js';
import {taskList} from './tasks.js';
import {createLayer} from './overlay.js';
import {modelsNotice} from './models.js';
import {modelsWaiting, modelsArrived, onModels} from '../core/models.js';

// ---------- 新图库的首次引导（#96，native/terms/src/app_onboarding.rs） ----------
// 在真实界面上教：几处地方一次点亮一处，一份事情做完就自己打勾的清单，以及在需要时才问的问题（角色出现后问名字，
// 第一次同步完成后问作品）。向导进行到哪按图库保存；这里没有任何东西会在用户点击之前授予联网权限或启动任务。
const TOUR = ['welcome', 'tasks', 'home', 'characters', 'unmatched', 'end'];
const CHECKLIST = [['look', t('看看认出的角色'), '#/characters'], ['correct', t('纠正一张认错的图'), '', t('点开后按 C')], ['undo', t('撤销一次操作'), '', shortcut('Z')],
  ['group', t('确认一组没认出的图'), '#/groups'], ['follow', t('关注一部作品'), '#/characters/works']];
let guideChecked = false, guideLoadedAt = 0, guideWorksAt = 0, tourAt = -1, checklistOpen = false;
const guideOn = () => !!state.guide && state.guide.guide.status !== 'off';
const guideActive = () => ['new', 'active'].includes(state.guide?.guide?.status);
export async function loadGuide(works = false) {
  const token = session.token, value = await api('/api/onboarding' + (works ? '?works=1' : ''));
  if (token !== session.token) return null;
  state.guide = {...value, works: value.works || state.guide?.works};
  guideLoadedAt = Date.now();
  return value;
}
async function guideAction(body) {
  const token = session.token;
  try {
    const value = await api('/api/onboarding', {method: 'POST', body: JSON.stringify(body)});
    if (token === session.token) state.guide = {...value, works: value.works || state.guide?.works};
    return true;
  } catch (e) { toast(e.message); return false; }
}
// 打开时：新图库记下它的向导已经开始；导览本身等启动画面结束。
export async function guideOnOpen() {
  try { await loadGuide(); } catch (e) { return; }
  if (state.guide?.guide.status === 'new') await guideAction({action: 'step', step: 'welcome'});
}
const splashGone = () => new Promise(done => {
  const started = Date.now(), wait = () => (!document.getElementById('splash') || Date.now() - started > 8000) ? done() : setTimeout(wait, 150);
  wait();
});
export async function guideAfterBoot() {
  if (!guideOn()) return;
  await splashGone();
  if (guideActive()) showTour(Math.max(0, TOUR.indexOf(state.guide.guide.step)));
  drawGuide();
}
// 图库有了变化（一个同步步骤、一次改动）：重新读取向导，最多每几秒一次。
export async function guideRefresh(force = false) {
  if (!guideOn() || (!force && Date.now() - guideLoadedAt < 4000)) return;
  try { await loadGuide(); } catch (e) { return; }
  drawGuide();
}
export function drawGuide() {
  drawGuideNotice(); drawNudge(); drawChecklist();
  if (tourAt >= 0) placeTour(false);
}

// ----- 问题横条：缺识别或它的自检失败，或同步停下了（持续期间显示，#97） -----
function syncTrouble() {
  const s = state.sync, message = s.message ? `（${esc(tb(s.message))}）` : '', missing = said(s.message, '文件夹不存在或无法访问：{path}');
  if (s.status === 'interrupted') return t('同步没有做完{said}：已经加入和识别的图都保留了，再同步一次会接着处理剩下的。', {said: message});
  if (s.status !== 'error') return '';
  if (photosRefused(s)) return photosHelp();
  return t('同步出错了{said}：已经加入和识别的图都保留了。', {said: message}) + (missing
    ? t('检查来源文件夹是否还在、外接硬盘是否接上，或者在 {link} 里换一个文件夹，然后再同步一次。', {link: `<a class="link" href="#/settings/library">${t('设置 › 图库')}</a>`})
    : t('可以再同步一次；还不行的话，反馈时附上「设置 › 关于」里的诊断信息。'));
}
export function drawGuideNotice() {
  const el = $('guideNotice'), g = state.guide; if (!el) return;
  // 没带模型的安装（#168）：在模型就位之前，每个图库都显示，不管有没有向导。
  const models = modelsNotice();
  if (models || el.dataset.models) {
    el.dataset.models = models ? '1' : '';
    if (models) { el.hidden = false; redraw(el, models); return; }
  }
  const c = g?.capability, check = c?.check || {};
  if (guideOn() && c?.recognition && !modelsWaiting() && check.status === 'idle' && !guideChecked) { guideChecked = true; guideAction({action: 'check'}).then(() => setTimeout(() => guideRefresh(true), 1500)); }
  if (guideOn() && check.status === 'checking') setTimeout(() => guideRefresh(true), 1500);
  const problem = !guideOn() || modelsWaiting() ? '' : !c.recognition ? t('这个安装没有识别组件：图片会加入图库、可以手动整理，但不会自动认出角色。安装 Illustash 完整版后就能自动识别。')
    : check.status === 'failed' ? t('识别组件没能启动（{reason}）。图片仍会加入图库；可以重新检查，或重新安装完整版。', {reason: esc(tb(check.message || ''))}) : '';
  const trouble = guideOn() && !problem ? syncTrouble() : '';
  el.hidden = !problem && !trouble;
  redraw(el, problem ? `<span>${problem}</span>${c.recognition ? `<button class="quiet small" data-guide-check>${t('重新检查')}</button>` : ''}`
    : trouble ? `<span>${trouble}</span>${photosRefused(state.sync) ? `<button class="quiet small" data-guide-photos>${t('打开系统设置')}</button>`
      : `<button class="quiet small" data-guide-sync>${t('再同步一次')}</button>`}` : '');
}
// ----- 导览：一次点亮一个真实元素 -----
export function tourStop(id) {
  const g = state.guide, s = state.sync, r = g.results;
  const counting = SYNC_ACTIVE.includes(s.status) ? syncLine(s) : r.total ? t('已经有 {n} 张图，认出了 {chars} 个角色。', {n: number(r.total), chars: number(r.characters)}) : '';
  return {
    welcome: SYNC_ACTIVE.includes(s.status) || !r.total
      ? {title: t('图库建好了'), text: t('正在读取并识别你的图片。花一分钟认识一下界面？'), next: t('带我看看'), skip: t('自己逛逛')}
      : {title: t('认识一下界面'), text: t('花一分钟看看几个常用的地方？'), next: t('带我看看'), skip: t('不用了')},
    tasks: {target: () => $('taskButton'), title: t('进度在这里'), text: `${counting ? esc(counting) + '<br>' : ''}${t('同步和识别在后台进行，识别好的图会陆续出现在图库里。')}`},
    home: {route: 'days', target: () => document.querySelector('#homePage .home-j') || $('homePage'), title: t('图库'),
      text: document.querySelector('#homePage .home-j') ? t('新加入的图按日期排在这里，图上写着认出的角色。认错了就点开它按 C 换成正确的角色，{key} 随时撤销。', {key: shortcut('Z')}) : t('图片导入后会按日期排在这里，图上写着认出的角色。')},
    characters: {target: () => document.querySelector('#railEntries [data-entry="catalog"]'), title: t('资料库'), text: t('认出的角色按作品放在这里，点进去只看 TA 的图。')},
    unmatched: {target: () => document.querySelector('#railEntries [data-entry="tidy"]'), title: t('还没认出的图'), text: t('在「整理」里：长得像的会分成一组，一次确认一整组。')},
    end: {title: t('就这些'), text: t('右下角的「上手」清单会跟着你做过的事打勾，全做完就自己消失。以后想再看一遍，在「设置 › 关于」里。'), next: t('开始用吧')},
  }[id];
}
// 导览是盖满页面的层：Esc 等同"跳过"，由浮层管理器（ui/overlay.js）统一处理。开着时页面的单键快捷键不响应
// （ui/grid.js 的键盘处理看 #tour 是否显示），焦点留在导览的按钮上（#398）。
// 它盖住的页面和各个固定层设成 inert（#422）。
const tourLayer = createLayer({kind: 'overlay', id: 'tour', el: () => $('tour'), onDismiss: () => { closeTour(); guideAction({action: 'skip'}); },
  inert: () => [document.querySelector('.app'), $('selectionBar'), $('taskPanel'), $('viewer'), $('nudge'), $('checklist')]});
function showTour(at) {
  tourAt = at; drawNudge(); drawChecklist();
  const root = $('tour'); root.hidden = false; tourLayer.open(); requestAnimationFrame(() => root.classList.add('on'));
  placeTour(true);
}
function closeTour() {
  tourAt = -1; tourLayer.close(); const root = $('tour'); root.classList.remove('on');
  setTimeout(() => { if (tourAt < 0) root.hidden = true; }, 260);
  drawGuide();
}
async function placeTour(moved) {
  if (tourAt < 0 || !state.guide) return;
  const id = TOUR[tourAt], stop = tourStop(id), root = $('tour'), hole = root.querySelector('.tour-hole'), bubble = root.querySelector('.tour-bubble');
  if (moved && stop.route && state.route.page !== stop.route) { go({page: stop.route}); await new Promise(done => setTimeout(done, 450)); if (TOUR[tourAt] !== id) return; }
  // 停留在同一处时，只有文字跟着实时计数变；按钮绝不会在按下时被重建。
  if (!moved && bubble.dataset.stop === id) {
    const text = bubble.querySelector('.tour-text'); if (text && text.innerHTML !== stop.text) text.innerHTML = stop.text;
  }
  const html = `<b class="tour-title">${stop.title}</b><p class="tour-text">${stop.text}</p>
    <div class="tour-foot"><span class="tour-dots">${TOUR.map((_, i) => `<i class="${i === tourAt ? 'on' : ''}"></i>`).join('')}</span><div class="spacer"></div>
      ${stop.skip || tourAt === TOUR.length - 1 ? '' : `<button class="quiet small" data-tour="skip">${t('跳过')}</button>`}${stop.skip ? `<button class="quiet small" data-tour="skip">${stop.skip}</button>` : ''}
      ${tourAt && tourAt < TOUR.length - 1 ? `<button class="quiet small" data-tour="back">${t('上一步')}</button>` : ''}
      <button class="primary small" data-tour="${tourAt === TOUR.length - 1 ? 'done' : 'next'}">${stop.next || t('下一步')}</button></div>`;
  if (bubble.dataset.stop !== id || moved && drawnHTML(bubble) !== html) {
    if (moved && drawnHTML(bubble)) { bubble.classList.add('swap'); await new Promise(done => setTimeout(done, 120)); }
    setDrawn(bubble); redraw(bubble, html); bubble.dataset.stop = id; bubble.classList.remove('swap');
  }
  // 换了一步（按钮重建过）或刚打开：焦点放到主按钮上，Enter 就是「下一步」，Tab 在导览的按钮之间转（#398）。
  if (moved) bubble.querySelector('.primary')?.focus({preventScroll: true});
  const target = stop.target?.(), r = target?.getBoundingClientRect(), pad = 6, W = innerWidth, H = innerHeight, bw = bubble.offsetWidth, bh = bubble.offsetHeight;
  if (!r || !r.width) {
    Object.assign(hole.style, {left: W / 2 + 'px', top: H / 2 + 'px', width: '0px', height: '0px'}); hole.classList.add('none');
    Object.assign(bubble.style, {left: (W - bw) / 2 + 'px', top: (H - bh) / 2 + 'px'});
    return;
  }
  // 大的区域（首页）只点亮顶部，也就是最先几张图片所在的地方。
  const box = {left: r.left - pad, top: r.top - pad, width: r.width + pad * 2, height: Math.min(r.height, H * .45) + pad * 2};
  Object.assign(hole.style, {left: box.left + 'px', top: box.top + 'px', width: box.width + 'px', height: box.height + 'px'}); hole.classList.remove('none');
  const right = box.left + box.width + 14, below = box.top + box.height + 14;
  const [x, y] = right + bw < W - 12 ? [right, box.top] : below + bh < H - 12 ? [box.left, below] : [box.left, box.top - bh - 14];
  Object.assign(bubble.style, {left: Math.max(12, Math.min(x, W - bw - 12)) + 'px', top: Math.max(12, Math.min(y, H - bh - 12)) + 'px'});
}
// ----- 在需要时才问的问题，每个只问一次 -----
function nextNudge() {
  if (tourAt >= 0) return null;
  const g = state.guide, idle = !SYNC_ACTIVE.includes(state.sync.status);
  if (guideOn()) {
    const seen = g.guide.seen || [], net = state.names?.network;
    if (!seen.includes('names') && net && !net.granted_at && !net.this_run && g.results.characters > 0) return {key: 'names'};
    // 全应用共用、只问一次：不管怎么回答都会记下选择，所以别的图库不会再问（#191）。
    if (state.updates && state.updates.auto === null) return {key: 'updates'};
    if (!seen.includes('works') && idle && g.results.with_character > 0) {
      if (!g.works && Date.now() - guideWorksAt > 30000) { guideWorksAt = Date.now(); loadGuide(true).then(drawNudge).catch(() => {}); }
      const w = (g.works || []).find(x => x.watchable && !x.watched);
      if (w && !(state.index?.watch?.works || []).length) return {key: 'works', w};
    }
  }
  // 有新版本时，等到没有任务在运行再提示。用户点了「下载并重启」之后，进度和结果照常显示（#279）；点过「暂不」就收起。
  const u = state.updates, started = updateStarted(u);
  const going = ['downloading', 'installing'].includes(selfUpdate(u)?.stage);
  if (u?.newer && (going || !u.dismissed && (started || !taskList().some(task => task.active)))) return {key: 'update'};
  return null;
}
export function drawNudge() {
  const el = $('nudge'), n = nextNudge();
  if (guideOn() && !state.names && !el.dataset.names) { el.dataset.names = '1'; api('/api/names').then(v => { setNames(v); drawNudge(); }).catch(() => {}); }
  const u = state.updates;
  const html = !n ? '' : n.key === 'update'
    ? `<div><b>${t('有新版本 {version}', {version: esc(u.latest)})}</b><small>${updateStatus(u) || (selfUpdate(u)
        ? t('现在是 {current}。可以让 Illustash 下载这个版本，装好后自己重新打开；也可以在浏览器里打开下载页。', {current: esc(u.current)})
        : t('现在是 {current}。在浏览器里打开下载页，下载后覆盖安装即可。', {current: esc(u.current)}))}</small>${updateBar(u)}</div>
      <div class="nudge-acts">${['downloading', 'installing'].includes(selfUpdate(u)?.stage) ? '' : `<button class="quiet small" data-nudge="later">${t('暂不')}</button>`}
        ${updateActions(u, 'data-nudge')}</div>`
    : n.key === 'updates'
    ? `<div><b>${t('要自动检查新版本吗？')}</b><small>${t('启动时和之后每天向 GitHub 查一次有没有新版本，有就提示你；一次普通的 HTTPS 请求，不发送图库里的任何信息。可以在「设置 › 网络与隐私」里改。')}</small></div>
      <div class="nudge-acts"><button class="quiet small" data-nudge="later">${t('不用')}</button><button class="primary small" data-nudge="allow">${t('自动检查')}</button></div>`
    : n.key === 'names'
    ? `<div><b>${t('认出了 {n} 个角色。要联网查它们的中文名吗？', {n: number(state.guide.results.characters)})}</b><small>${t('只发送角色的 tag 和作品名，不发送图片。允许后对这个图库一直有效，可以在「设置 › 网络与隐私」里撤销。')}</small></div>
      <div class="nudge-acts"><button class="quiet small" data-nudge="later">${t('暂不')}</button><button class="primary small" data-nudge="allow">${t('允许')}</button></div>`
    : `<div><b>${t('你的图里《{work}》最多（{n} 张）', {work: esc(guideWorkName(n.w)), n: number(n.w.count)})}</b><small>${t('关注后可以从 Danbooru 补充这部作品里本地还不认识的角色。检查前会再问一次联网。')}</small></div>
      <div class="nudge-acts"><button class="quiet small" data-nudge="later">${t('暂不')}</button><button class="primary small" data-nudge="follow">${t('关注')}</button></div>`;
  if (drawnHTML(el) === html) return;
  el.dataset.key = n?.key || '';
  // 收起时不清空内容，留给淡出动画；记下空串，260 毫秒后还是空的才隐藏。
  if (!html) { setDrawn(el, ''); el.classList.remove('on'); setTimeout(() => { if (!drawnHTML(el)) el.hidden = true; }, 260); return; }
  redraw(el, html); el.hidden = false; requestAnimationFrame(() => el.classList.add('on'));
}
// ----- 清单：事情做完就自己打勾 -----
export function guideTick(item) {
  const g = state.guide; if (!guideOn() || g.guide.ticked.includes(item)) return;
  g.guide.ticked = [...g.guide.ticked, item]; drawChecklist(item);
  guideAction({action: 'tick', item}).then(() => drawChecklist());
}
// 每次成功的改动之后调用：它表明学会了清单里的哪一项。
export function guideAfterPost(path, body) {
  if (!guideOn()) return;
  if (path === '/api/undo') guideTick('undo');
  else if (path === '/api/change' && body?.action === 'confirmed' && pageOf(state.route).tab === 'groups') guideTick('group');
  else if (path === '/api/change' || path === '/api/reassign') guideTick('correct');
  else if (path === '/api/index' && body?.action === 'watch' && body.works?.length) guideTick('follow');
  if (Date.now() - guideLoadedAt > 4000) guideRefresh();
}
// 展开的清单是非模态浮层：Esc、点它和药丸之外的地方都收起（#398）。元素每次重画都换，所以传函数。
const checklistLayer = createLayer({kind: 'popover', id: 'checklist', el: () => $('checklist').querySelector('.cl-panel'), anchor: () => $('checklist').querySelector('.cl-pill'),
  onDismiss: reason => { checklistOpen = false; drawChecklist(); checklistLayer.close(reason); }});
export function drawChecklist(just = '') {
  const el = $('checklist'), g = state.guide;
  const ticked = g?.guide.ticked || [], all = ticked.length === CHECKLIST.length;
  const show = guideOn() && !g.guide.hidden && tourAt < 0 && (!all || just);
  el.hidden = !show; if (!show) return;
  const ring = `<svg class="cl-ring" viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="8"/><circle cx="10" cy="10" r="8" class="cl-done" style="stroke-dasharray:${(ticked.length / CHECKLIST.length * 50.3).toFixed(1)} 50.3"/></svg>`;
  el.classList.toggle('open', checklistOpen);
  el.innerHTML = `<button class="cl-pill" data-cl="toggle" aria-expanded="${checklistOpen}">${ring}<span>${all ? t('都会了') : t('上手')}</span><b>${ticked.length}/${CHECKLIST.length}</b></button>
    ${checklistOpen ? `<div class="cl-panel"><p class="cl-head">${t('做过的会自动打勾')}</p>${CHECKLIST.map(([k, label, href, hint]) => `<div class="cl-item${ticked.includes(k) ? ' done' : ''}${k === just ? ' just' : ''}"><span class="cl-check" aria-hidden="true">${ticked.includes(k) ? '✓' : ''}</span>${href && !ticked.includes(k) ? `<a class="link" href="${href}">${label}</a>` : `<span>${label}</span>`}${hint ? `<kbd>${esc(hint)}</kbd>` : ''}</div>`).join('')}
      <button class="link-button" data-cl="hide">${t('不再显示')}</button></div>` : ''}`;
  if (just) { el.classList.add('pulse'); setTimeout(() => el.classList.remove('pulse'), 900); }
  if (all && just) setTimeout(() => guideAction({action: 'hide'}).then(() => drawChecklist()), 2500);
}
// 图库已有的作品保留自己的名字；建议的 Danbooru tag 有内置中文名时显示中文名。
const guideWorkName = w => (!w.id && WORK_NAMES[w.tag]) || w.name;
// 关注一个建议的作品：它的 Danbooru tag 对应的本地作品（没有就新建），加进关注列表。
async function guideWatch(w) {
  try {
    const id = w.id || (await post('/api/works', {action: 'from_tag', tag: w.tag})).id;
    const x = await api('/api/index');
    setIndex(x.watch.works.includes(id) ? x : await post('/api/index', {action: 'watch', works: [...x.watch.works, id], min_posts: x.watch.min_posts}));
    toast(t('已关注「{work}」，可以在任务面板里检查它的新角色', {work: guideWorkName(w)}));
  } catch (e) { toast(e.message); }
  await loadGuide(true).catch(e => toast(e.message));
}
export async function replayGuide() {
  if (!await guideAction({action: 'reopen'})) return;
  if (onPage(state.route, 'settings')) go({page: 'days'});
  await new Promise(done => setTimeout(done, 300));
  showTour(0);
}

// 回答横条上的问题。按钮在回答期间禁用，回答完（不管成败）由 busy() 恢复（#429）。
async function answerNudge(key, act) {
  // 不是向导提示：回答的是更新选择或这个版本，由后端全应用保存。
  if (key === 'update' && act !== 'open' && act !== 'later') {
    await updateAct(act);
    setDrawn($('nudge')); drawNudge(); drawUpdateButton();
    return;
  }
  if (key === 'update' || key === 'updates') {
    if (key === 'update' && act === 'open') openRelease();
    try { await updatesPost(key === 'update' ? {action: 'dismiss'} : {action: 'auto', allow: act === 'allow'}); } catch (err) { toast(err.message); }
    if (key === 'updates' && act === 'allow') { toast(t('已打开自动检查更新')); updatesTick(); }
    drawGuide(); drawNudge();
    return;
  }
  if (act === 'allow') {
    try { await allowNames(true); toast(t('已允许这个图库联网查询名称')); }
    catch (err) { toast(err.message); }
  }
  if (act === 'follow') { const w = nextNudge()?.w; if (w) await guideWatch(w); }
  await guideAction({action: 'seen', key});
  drawGuide();
}

// 页面这一部分注册的处理函数。页面启动时调用一次。
export function wireGuide() {
  // 模型刚到：检查识别是否开始，和打开新图库时一样。
  onModels(async (before, start) => {
    if (modelsArrived(before, start) && guideOn()) { await guideAction({action: 'check'}); setTimeout(() => guideRefresh(true), 1500); }
    drawGuideNotice();
  });
  $('guideNotice').addEventListener('click', async e => {
    if (e.target.closest('[data-guide-check]')) { await guideAction({action: 'check'}); drawGuide(); setTimeout(() => guideRefresh(true), 1500); }
    if (e.target.closest('[data-guide-photos]')) location.href = PHOTOS_PRIVACY;
    if (e.target.closest('[data-guide-sync]')) { try { await startSync(); pollSync(); } catch (err) { toast(err.message); } drawGuide(); }
  });

  $('tour').addEventListener('click', async e => {
    const act = e.target.closest('[data-tour]')?.dataset.tour; if (!act) return;
    if (act === 'skip' || act === 'done') { closeTour(); await guideAction({action: act === 'done' ? 'done' : 'skip'}); drawGuide(); return; }
    tourAt = Math.max(0, Math.min(TOUR.length - 1, tourAt + (act === 'next' ? 1 : -1)));
    placeTour(true); guideAction({action: 'step', step: TOUR[tourAt]});
  });
  // Tab 只在导览的按钮之间转；点暗处不把焦点从按钮上拿走，免得 Tab 走到遮罩下面的侧栏（#398）。
  $('tour').addEventListener('keydown', e => {
    if (composing(e) || e.key !== 'Tab') return;
    const buttons = [...$('tour').querySelectorAll('.tour-bubble button')]; if (!buttons.length) return;
    const at = buttons.indexOf(document.activeElement), next = at < 0 ? 0 : (at + (e.shiftKey ? -1 : 1) + buttons.length) % buttons.length;
    e.preventDefault(); buttons[next].focus();
  });
  $('tour').addEventListener('mousedown', e => { if (!e.target.closest('button')) e.preventDefault(); });
  addEventListener('resize', () => { if (tourAt >= 0) requestAnimationFrame(() => placeTour(false)); });

  $('nudge').addEventListener('click', async e => {
    const act = e.target.closest('[data-nudge]')?.dataset.nudge, key = $('nudge').dataset.key; if (!act || !key) return;
    await busy([...$('nudge').querySelectorAll('button')], () => answerNudge(key, act));
  });

  $('checklist').addEventListener('click', async e => {
    const act = e.target.closest('[data-cl]')?.dataset.cl;
    if (act === 'toggle') { checklistOpen = !checklistOpen; if (checklistOpen) checklistLayer.open(); drawChecklist(); if (!checklistOpen) checklistLayer.close(); }
    else if (act === 'hide') { checklistOpen = false; checklistLayer.close('silent'); await guideAction({action: 'hide'}); drawChecklist(); }
    else if (e.target.closest('a[href]')) { checklistOpen = false; checklistLayer.close('silent'); drawChecklist(); }
  });
}
