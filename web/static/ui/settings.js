// 设置页的外壳（#199，0.4 第 I 步 #714 改成配置表）：页面登记、七节导航、节的切换和这一页的作用域，以及读取或更改了
// 设置数据之后重画当前这一节。每节的分组和行写在 ui/settings_<节>.js（配置表，行在 ui/rows.js、分组在
// ui/settings_group.js）；它们读取和更改的内容在 core/settings.js（任务自己的状态在 core/jobs.js）。
// 每节导出 build(main, scope) → {update(state), load?(fresh)}：进这一节时建一次并调 load 读这一节的数据（节自己的状态，
// 如诊断记录、读取失败，留在这个实例里，随节的作用域释放），之后数据变了只 update。
import {t} from '../core/i18n.js';
import {implement, onPage} from '../core/pages.js';
import {state} from '../core/state.js';
import {onModels} from '../core/models.js';
import {onPack, loadPack} from '../core/packs.js';
import {onIntegrity} from '../core/integrity.js';
import {onJob} from '../core/jobs.js';
import {onSettings, loadSettingsData, updatesTick} from '../core/settings.js';
import {readError} from '../core/session.js';
import {$, toast, error} from './dom.js';
import {attr, make} from './el.js';
import {closeInspector} from './detail.js';
import {undo} from './grid.js';
import {drawGuide, drawNudge} from './guide.js';
import {onHumanClick, wireHuman} from './human.js';
import {pageView} from './page_view.js';
import {onRefresh} from './settings_group.js';
import {drawUpdateButton, selfUpdate, setTasksBusy, updateAct} from './updates.js';
import {taskList} from './tasks.js';
import * as general from './settings_general.js';
import * as library from './settings_library.js';
import * as characters from './settings_characters.js';
import * as artists from './settings_artists.js';
import * as network from './settings_network.js';
import {setNamesNetwork} from './settings_network.js';
import * as data from './settings_data.js';
import * as about from './settings_about.js';

// 用户带来的每个问题一节：看起来怎样、图片从哪来、图里是谁、谁画的、什么会离开本机、数据存在哪（#/settings/<id>）。
const SECTIONS = [
  ['general', t('通用'), t('语言、外观、图片网格与默认排序'), general],
  ['library', t('图库'), t('当前图库、来源、同步与排除'), library],
  ['characters', t('角色资料'), t('角色资料包、新角色检查、角色名称与翻译'), characters],
  ['artists', t('画师'), t('找回画师与绑定的账号'), artists],
  ['network', t('网络与隐私'), t('所有会访问网络的功能，以及它们发送什么'), network],
  ['data', t('数据'), t('备份、缓存、识别模型与存储位置'), data],
  ['about', t('关于'), t('版本、致谢与诊断信息'), about]];
export const settingsSection = () => SECTIONS.find(s => s[0] === state.route.section) || SECTIONS[0];

// ---------- 页面 ----------
// 这一页的部件（导航和当前一节），建在页面作用域上；换节时只换节的那一块。
function buildPage(scope) {
  const page = make('div', 'set-page'), nav = make('nav', 'set-nav'), main = make('div', 'set-main');
  attr(nav, 'aria-label', t('设置'));
  const links = SECTIONS.map(([id, title]) => {
    const a = make('a', '', title); attr(a, 'href', `#/settings/${id}`); nav.append(a);
    return [id, a];
  });
  page.append(nav, main);
  $('settingsPage').replaceChildren(page);
  // 数据变了就更新当前这一节；订阅挂在页面作用域上，离开设置页时一起取消。
  for (const on of [onSettings, onPack, onModels, onIntegrity, onJob]) scope.add(on(refreshSettings));
  return {scope, main, links, id: null, section: null, part: null};
}
const view = pageView(buildPage);
function showSection(v, id) {
  const mod = SECTIONS.find(s => s[0] === id)[3];
  for (const [key, a] of v.links) attr(a, 'aria-current', key === id ? 'page' : null);
  if (v.id === id) return false;
  v.section?.dispose();
  v.main.replaceChildren();
  v.id = id;
  v.section = v.scope.child('settings-' + id);
  v.part = mod.build(v.main, v.section);
  $('viewport').scrollTop = 0;
  return true;
}
// 当前这一节按最新的状态更新（值没变的地方不动 DOM，焦点和打了一半的字都不丢）。
export function refreshSettings() {
  const v = view.get();
  if (!v || !onPage(state.route, 'settings')) return;
  v.part?.update(state);
}
onRefresh(refreshSettings);
function render(o) {
  closeInspector();
  const v = view.enter(o), samePage = !!o.samePage;
  const [id] = settingsSection();
  const entered = showSection(v, id);
  // 进这一页时整页重读；同一页里换节只补还没有的。
  loadSettingsData(!samePage);
  if (id === 'network' || id === 'about') updatesTick();
  if (entered || !samePage) v.part.load?.(!samePage);
  refreshSettings();
}

// 名称联网授权的按钮：搜索结果下面的「允许查询…」（允许后把输入的内容再搜一次）。点到了返回 true。
function onNamesClick(e) {
  const allow = e.target.closest('[data-names-allow]'); if (!allow) return false;
  e.preventDefault();
  const input = allow.closest('dialog, .combo, .watch-add, #picker')?.querySelector('input');
  setNamesNetwork(true).then(ok => { if (ok) { drawGuide(); input?.focus(); input?.dispatchEvent(new Event('input', {bubbles: true})); } });
  return true;
}
// 导览里更新提示的按钮（data-updates）。
function onUpdatesClick(e) {
  const act = e.target.closest('[data-updates]'); if (!act) return false;
  act.disabled = true;
  updateAct(act.dataset.updates).then(redrawUpdates);
  return true;
}
export function redrawUpdates() { drawNudge(); drawUpdateButton(); refreshSettings(); }
async function updateButtonClick() {
  const stage = selfUpdate()?.stage;
  if (stage === 'downloading' || stage === 'installing') { location.hash = '#/settings/about'; return; }
  await updateAct(stage === 'ready' ? 'restart' : selfUpdate() ? 'install' : 'open');
  redrawUpdates();
}

// 设置页的处理函数，以及读取或更改它们的数据之后要做的事。页面启动时调用一次。
let catalogChecked;
export function wireSettings() {
  // 名称联网授权和检查更新的按钮不只在设置页里（搜索结果下面的「允许查询…」、导览里的更新提示），所以在 document 上
  // 用一个监听接住（#432 ③-9，原来是三个）。
  document.addEventListener('click', e => { onNamesClick(e) || onUpdatesClick(e); });
  onSettings((what, detail) => {
    // 设置页数据的读取失败只在设置页还开着时报：离开以后才回来的失败不弹到别的页面上。
    if (what === 'data' && detail.error && view.get() && onPage(state.route, 'settings')) error(readError(detail.error));
    if (what === 'libraries') {
      const cur = state.libraries?.info;
      $('brandLib').textContent = cur?.test ? t('测试·') + (cur.name.split(' · ').pop()) : '';
      $('brand').title = cur ? `Illustash · ${cur.name}` : 'Illustash';
    }
    if (what === 'pixiv') toast(detail.error ? detail.error.message : t('已绑定 Pixiv：{name}', {name: detail.bound.name || detail.bound.user_id}));
    if (what === 'updates') {
      drawNudge(); drawUpdateButton();
      // 检查更新时后端顺带读了包目录（#602）：检查时间变了就重读一次角色资料包（读到过才读）。
      if (state.updates?.checked_at !== catalogChecked) { catalogChecked = state.updates?.checked_at; if (state.pack) loadPack().catch(() => {}); }
    }
  });
  // 人工结论快照（ui/human.js 画进 #humanBlock）的按钮。
  $('settingsPage').addEventListener('click', onHumanClick);
  wireHuman({undo});
  $('updateButton').onclick = updateButtonClick;
  setTasksBusy(() => taskList().some(task => task.active));
  library.wireLibrary();
}

// 设置页登记：进这一页时建导航和当前一节，换节时只换那一块；设置的副标题是这一节的内容清单。
implement({settings: {render, head: () => ({title: t('设置'), sub: settingsSection()[2]}), onMeta: refreshSettings}});
