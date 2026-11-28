// 页面的框架（桌面布局，#199、#706）：按地址切到对应页面、换页时的页面作用域、活页条的选中、每个页面的页头和工具栏。
// 页面在哪由 core/route.js 负责；计数的数据由 core/meta.js 负责；活页条在 ui/rail.js。各页的标题、说明、空状态的去处
// 由各页自己的模块登记（core/pages.js 的 implement），这里只画框架。一页怎么写见 docs/WEB_V2.md「页面样板」。
import {esc, isMac, number, short, shortcut} from '../core/format.js';
import {t} from '../core/i18n.js';
import {onTyped} from '../core/keys.js';
import {roleName} from '../core/labels.js';
import {fetchChar, onMeta, setChar} from '../core/meta.js';
import {prefs} from '../core/prefs.js';
import {pageOf, viewOf} from '../core/pages.js';
import {queues, railAt, viewRoute} from '../core/rail.js';
import {canGoBack, cleanup, defaultSort, go, hashFor, historyEntry, noteHistory, parseHash, reRoute, replaceRoute, setRoute, sortKey, stepHistory} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {$, error, helpTip, showArea, wireHelpTip} from './dom.js';
import {searchHTML} from './icons.js';
import {attr, cls, make, show, text, tpl} from './el.js';
import {closeInspector} from './detail.js';
import {closeViewer} from './viewer.js';
import {fixedLayout} from './grid_rows.js';
import {markSelectButton, placeFor, rememberPlace, reload, resetForRoute, selectAll, setNsfw, toggleFit, toggleSelecting} from './grid.js';
import {guideTick} from './guide.js';
import {filterBar, wireFilterBar} from './filters.js';
import {wireGalleries} from './gallery_actions.js';
import {saveButton, wireSaveButton} from './gallery_save.js';
import {enter} from './motion.js';
import {lockInert, unlockInert} from './inert.js';
import {railRoute, refresh as refreshRail} from './rail.js';
import {page as pageScope, currentPage} from './scope.js';

// 一次元数据读取在屏幕上改动什么：活页条（整理的数字和点；资料库钉住的角色和作品要用读到的角色目录，启动时它比钉住的列表晚到，
// 改名、张数变了也要跟着变），以及显示它们的标题或页面。
function metaShown({error: e, directory}) {
  if (e) { error(readError(e)); return; }
  refreshRail();
  // 角色还没读到时（state.char 为空）算图库页：这一刻不重画。
  const view = viewOf(state.route, state.char);
  if (view.onMeta) view.onMeta({directory}); else renderChrome();
}
let routeLoad = null;
function beginRouteLoad() {
  const status = $('routeLoading'), viewport = $('viewport');
  routeLoad?.finish(true);
  const lock = {}, loading = {
    freeze() { lockInert(viewport, lock); },
    thaw() { unlockInert(viewport, lock); },
    finish(continuing = false) {
      clearTimeout(timer);
      loading.thaw();
      if (routeLoad !== loading) return;
      routeLoad = null;
      if (!continuing) show(status, false);
    },
  };
  // 快切不闪提示，仍在读取时再切换则沿用已经显示的提示。提示浮在页面上，不占版面、不推动浏览区；
  // 正在显示的操作提示（撤销等）优先，同一位置不叠两个。
  const timer = currentPage() ? setTimeout(() => {
    if (!$('toast').hidden) return;
    text(status, t('正在读取…')); show(status, true);
  }, 300) : null;
  routeLoad = loading;
  return loading;
}
// 读角色失败后退回的地址：到达时页面一直停在那里，只登记历史，不重画。
let returning = null;
export async function applyRoute() {
  if (returning !== null) {
    const back = returning, here = parseHash(); returning = null;
    here.sort = here.sort || defaultSort(here); delete here.legacy;
    if (hashFor(here) === back) { noteHistory(); $('backButton').hidden = !canGoBack(); return; }
  }
  const loading = beginRouteLoad();
  let view;
  try { view = await switchRoute(loading); }
  finally {
    if (view?.drawn) void Promise.resolve(view.drawn).finally(() => loading.finish());
    else loading.finish();
  }
}
async function switchRoute(loading) {
  const route = parseHash(); route.sort = route.sort || defaultSort(route);
  const legacy = route.legacy; delete route.legacy;
  // 路由已写进历史记录、角色页还在读页头时，屏幕上仍是上一条实际显示过的页面。
  const previous = state.route;
  if (pageOf(previous).render) panelScroll.set(viewEntry ?? historyEntry(), $('viewport').scrollTop);
  const traversed = noteHistory(legacy ? hashFor(route) : undefined), entry = historyEntry();
  $('backButton').hidden = !canGoBack();
  const samePage = previous.page === route.page && previous.id === route.id && previous.char === route.char;
  // 换页（含同页换子视图和参数）先关大图，再记网格离开时的位置（#402）。
  closeViewer('silent');
  rememberPlace();
  let character = null;
  if (route.char) {
    // 角色读完才换页，等待时保留上一页。旧回复不能换走后续选到的页面。
    if (!samePage) loading.freeze();
    try { character = await fetchChar(route.char); }
    catch (e) {
      if (routeLoad === loading && state.route === previous) {
        error(readError(e));
        // 页面停在原来那页。刚点出的新历史退回去，免得后退时多按一次；前进、后退到的那条（或启动时的第一条）就地改回原地址。
        if (!traversed && canGoBack()) { returning = hashFor(previous); history.back(); }
        else { noteHistory(hashFor(previous)); viewEntry = entry; }
      }
      return;
    }
    if (routeLoad !== loading || state.route !== previous) return;
    character ||= {id: route.char, name: t('角色'), work: '', count: 0, unchecked: 0, variants: []};
  }
  loading.thaw();
  setRoute(route); resetForRoute();
  if (!samePage) closeInspector();
  const scope = !samePage || !currentPage() ? pageScope(route.page || 'library') : currentPage();
  const page = pageOf(route);
  railRoute(route);
  setChar(character);
  if (page.prepare && await page.prepare(route) === false) return;
  if (routeLoad !== loading || state.route !== route) return;
  viewEntry = entry;
  showArea(page.area);
  // 换了页面：内容页淡入（#706，和活页条的切换一起）；同一页换子视图不动。
  if (!samePage) enter($('viewport'), {from: 'fade'});
  if (page.guideTick) guideTick(page.guideTick);
  renderChrome();
  // 自己渲染的页面（按天、资料库、设置、按组整理……）换到时从顶部开始，不沿用上一页的滚动位置；后退、前进回到记过
  // 位置的那一条时，画完再滚回去（restore 让按天视图按上次读到的天数读，见 ui/days.js）。同一页换子视图、刷新时由页面自己
  // 决定留在哪里。网格页由 reload 定位（回到记过的地址时恢复，否则回顶部）。
  const saved = traversed ? panelScroll.get(entry) : undefined;
  if (page.render && !samePage) $('viewport').scrollTop = 0;
  // 网格页顶上跟着滚动的头部（角色页的横幅，#712）：进页和同一页又画一次时交给这一页（core/pages.js 的 gridHead）。
  viewOf(route, state.char).gridHead?.({samePage, scope});
  if (!page.render) { await reload(true, placeFor(route)); return; }   // 没有自己渲染入口的页面是网格页
  // 面板页的底栏：选择栏藏起来，张数清空（要写的页面读到数据以后自己写），提示写注册表的 hint。网格页的由 reload 写。
  $('selectionBar').hidden = true; $('resultCount').textContent = ''; $('hint').textContent = page.hint?.(route) || '';
  const drawn = page.render({samePage, restore: saved !== undefined, scope});
  if (saved === undefined) return {drawn};
  await drawn;
  if (state.route === route) $('viewport').scrollTop = saved;
}
// 自己渲染的页面在各条历史记录里停在哪里：{序号: scrollTop}。
const panelScroll = new Map();
let viewEntry = null;

// ---------- 页头：标题、副标题、本页的按钮，切换（图库的三种视图、整理的四个页签），右侧的控件组 ----------
// 各页用 head(r, ctx) 登记页头：返回 {title, sub?, alert?, actions?}，只收文字和节点（core/pages.js 的 implement 说明）：
// title、sub、alert 是纯文字，经 ui/el.js 的 text() 写进 .title-page、.page-sub；actions 是这一页自己的按钮节点。
// 旧的写法 headingHTML（返回 HTML 字符串）是存量，各页步骤改到哪页就迁到 head；drawHeading（整个页头自己画）也是。
const HEAD = tpl`<hgroup class="heading-main"><h1 class="title-page"><span data-ref="title"></span><button class="icon small" id="pageHelp" data-ref="help"
  title="这一页的说明" aria-label="这一页的说明" data-t-attr="title aria-label" aria-expanded="false" aria-controls="pageHelpPop" hidden>?</button></h1><p
  class="page-sub" data-ref="line"><span data-ref="sub"></span><strong data-ref="alert"></strong></p><div id="pageHelpPop" class="menu menu-drop glass" role="dialog"
  aria-label="这一页的说明" data-t-attr="aria-label" tabindex="-1" data-ref="pop" hidden></div></hgroup>`;
let head = null, actions = null, tucked = false;   // 建一次，之后只改变了的地方（模块载入时不建节点）
// 页头的标题这会儿藏不藏（角色页：横幅上已经写着名字，横幅的名字滚出视口以后才显示，#712）。由那一页调，离开时调回 false。
export function tuckHeading(on) { tucked = !!on; if (head) markTucked(); }
function markTucked() { cls(head.el, 'is-tucked', tucked); attr(head.el, 'aria-hidden', tucked ? 'true' : null); }
function drawHead(h, {title = '', sub = '', alert = '', actions: buttons = []} = {}, help) {
  head ||= HEAD(); actions ||= make('div', 'heading-actions');
  const {refs} = head;
  markTucked();
  text(refs.title, title);
  text(refs.sub, sub); text(refs.alert, alert && sub ? ' · ' + alert : alert);
  show(refs.line, !!(sub || alert));
  show(refs.help, help.length > 0);
  refs.pop.replaceChildren(...help.map(line => make('p', '', line)));
  actions.replaceChildren(...buttons);
  h.replaceChildren(head.el, ...(buttons.length ? [actions] : []));
}
// 图库三种视图之间、整理四个页签之间的分段切换。是链接（换地址），选中的那个 aria-current。图库的切换只在活页条落在「图库」时画
// （收藏、作品页虽然也是网格，活页条在画廊、资料库下，没有这个切换）。
function switches(r, view) {
  if (view.switches) return view.switches(r);   // 页面自己声明的分段切换（资料库的三个页签，#711）
  if (view.view && railAt(r).entry === 'library') {
    const label = {days: t('按天'), grid: t('网格'), calendar: t('日历')};
    return {label: t('视图'), list: ['days', 'grid', 'calendar'].map(v => ({label: label[v], href: hashFor(viewRoute(r, v)), on: view.view === v}))};
  }
  if (['unmatched', 'doubts', 'duplicates', 'excluded'].includes(view.tabs)) {
    return {label: t('整理'), list: queues(r).slice(0, 4).map(q => ({label: q.label, n: q.count, href: q.href, on: view.item === q.key}))};
  }
  return null;
}
let seg = null;   // 建一次，每次画页头时挪进去
function drawSwitches(r, view) {
  const sw = switches(r, view);
  if (!sw) { seg?.remove(); return null; }
  seg ||= make('nav', 'seg');
  attr(seg, 'aria-label', sw.label);
  seg.replaceChildren(...sw.list.map(x => {
    const a = make('a', '', x.label); a.href = x.href;
    if (x.on) a.setAttribute('aria-current', 'page');
    if (x.n) a.append(make('small', 'seg-count', number(x.n)));
    return a;
  }));
  return seg;
}
export function renderHeading() {
  const r = state.route, h = $('heading'), c = state.char, view = viewOf(r, c);
  if (view.drawHeading) { view.drawHeading(h, r, c); h.append(...[drawSwitches(r, view)].filter(Boolean)); return; }
  const n = state.totalReady ? number(state.total) : '…';
  // 撤销以后在等后台学习落定（#404）：计数还是撤销前的，落定后一次重读成最终值。
  const updating = text => state.learning ? [text, t('正在更新')].filter(Boolean).join(' · ') : text;
  // 标题下面只放数量、状态和必须一眼看到的警告；这一页是什么、怎么操作收进标题旁的「?」（#527）。网格页的操作提示
  // 就是注册表里的底栏提示（底栏默认隐藏），一个「·」分出一行，英文行首大写。
  const hints = view.helpHint ? (view.hint?.(r) || '').split(' · ').filter(Boolean).map(x => x.charAt(0).toLocaleUpperCase() + x.slice(1)) : [];
  const help = [...(view.help?.(r) || []), ...hints];
  if (view.head) {
    const x = view.head(r, {n});
    drawHead(h, {...x, sub: updating(x.sub || '')}, help);
  } else {
    const [title, sub, links = ''] = view.headingHTML?.(r, {n}) || ['', ''];
    const [helpButton, helpPop] = helpTip(help.map(esc));
    h.innerHTML = `<hgroup class="heading-main"><h1 class="title-page">${title ? `<span>${title}</span>` : ''}${helpButton}</h1><p class="page-sub">${updating(sub)}</p>${helpPop}</hgroup>${links}`;
  }
  const sw = drawSwitches(r, view); if (sw) h.append(sw);
  wireHelpTip();
}

// ---------- 工具栏 ----------
export function renderChrome() {
  renderHeading();
  const r = state.route, bar = $('toolbar'), view = viewOf(r), tabs = view.subTabs?.(r) || '';
  bar.hidden = view.toolbar === 'none';
  if (view.toolbar === 'none') { bar.replaceChildren(); return; }
  if (view.toolbar === 'tabs') { bar.innerHTML = tabs; return; }
  // 资料库（#711）：工具栏上的节点由页面自己建一次（ui/catalog_page.js 的 tools），这里只放进来。
  if (view.toolbar === 'index') { bar.replaceChildren(view.tools?.(r) || ''); return; }
  // 按天、日历（'filters'）：只有搜索框和筛选条（三种视图共用筛选，#703 第 2 问），没有排序、选择和显示方式。
  const filtersOnly = view.toolbar === 'filters';
  const tagPage = r.char && (state.char?.mascot || state.char?.group_tag);
  const SORT_LABELS = {suspicious: t('可疑优先'), newest: t('最新加入'), oldest: t('最早加入'), hinted: t('有线索优先'), similar: t('最相似'), color: t('按颜色')};
  // 排序的选项可以随地址变（画廊的色彩只按颜色排，#710）。
  const sorts = ((typeof view.sorts === 'function' ? view.sorts(r) : view.sorts) || []).filter(v => !(tagPage && v === 'suspicious')).map(v => [v, SORT_LABELS[v]]);
  const skins = r.char && !tagPage && !r.maybe ? (state.char?.variants || []).filter(v => v.count) : [];
  const variants = skins.length ? `<div class="chips-row">${[{id: '', name: t('全部'), count: state.char.count}, ...skins].map(v => `<button class="chip-filter${(r.variant || '') === v.id ? ' on' : ''}" data-variant="${esc(v.id)}">${esc(v.name)} <span>${number(v.count)}</span></button>`).join('')}</div>` : '';
  const maybeChip = r.char && !tagPage && (state.maybeCount || r.maybe || state.doubtCount || r.doubt) ? `<div class="chips-row">${state.maybeCount || r.maybe ? `<button class="chip-filter${r.maybe ? ' on' : ''}" id="maybeChip" title="${t('机器觉得像 TA，但算给了别人或没算的图')}">${t('可能也是 TA')} <span>${number(state.maybeCount)}</span></button>` : ''}${state.doubtCount || r.doubt ? `<button class="chip-filter${r.doubt ? ' on' : ''}" id="doubtChip" title="${t('单人图里，另一个角色的证据几乎一样强的图')}">${t('可能认错')} <span>${number(state.doubtCount)}</span></button>` : ''}</div>` : '';
  // 生效的筛选各是一个胶囊，点 × 只清它自己（#386 的 2-30）。角色页的角色胶囊在这里；其余条件是筛选条（ui/filters.js，#445），
  // 旧的单值参数（tag、utag、work、artist、day）也显示在那里。
  const pill = (id, label, title) => `<button class="quiet on" id="${id}" title="${title}">${label} ×</button>`;
  const name = esc(short(roleName(state.char)));
  const who = tagPage ? t('标签 · {name}', {name}) : t('角色 · {name}', {name});
  const filtersHTML = filtersOnly ? searchBox(r) + filterBar(r) + saveButton(r) : view.toolbar === 'library' ? searchBox(r) +
    (r.char ? pill('charPill', who, tagPage ? t('清除标签筛选') : t('清除角色筛选')) : '') +
    filterBar(r) + saveButton(r) + maybeChip + variants : tabs;
  bar.innerHTML = filtersOnly ? filtersHTML : filtersHTML +
    `<div class="page-actions"><select id="sort" aria-label="${t('排序')}">${sorts.map(([v, label]) => `<option value="${v}"${r.sort === v ? ' selected' : ''}>${label}</option>`).join('')}</select>` +
    `<button class="quiet" id="selectButton" title="${cleanup() ? t('选中全部符合条件的图片') : t('进入选择模式')}">${cleanup() ? `${t('全选')} <kbd>${shortcut('A')}</kbd>` : t('选择')}</button>` +
    `<div class="icon-seg" id="fitToggle" role="group" aria-label="${t('显示方式')}"${fixedLayout(r) ? ' hidden' : ''}><button data-fit="0" class="${state.fit ? '' : 'on'}" title="${t('方格：裁成正方形（R）')}" aria-label="${t('方格')}">${FIT_ICONS.square}</button><button data-fit="1" class="${state.fit ? 'on' : ''}" title="${t('原始比例：完整显示整张图（R）')}" aria-label="${t('原始比例')}">${FIT_ICONS.fit}</button></div></div>`;
  if (filtersOnly) { wireSearch(r); wireFilterBar(r); wireSaveButton(r); return; }
  // 改了排序就记成这一类页面的默认值，离开再回来还是它（#386 的 2-26）；记在哪个键由 core/route.js 的 sortKey 定。
  $('sort').onchange = e => { prefs.set(sortKey(r), e.target.value); replaceRoute({sort: e.target.value}); reload(true); };
  $('selectButton').onclick = () => cleanup() ? selectAll() : toggleSelecting();
  markSelectButton();
  for (const b of $('fitToggle').querySelectorAll('[data-fit]')) b.onclick = () => { if (Boolean(Number(b.dataset.fit)) !== state.fit) toggleFit(); };
  for (const b of bar.querySelectorAll('[data-variant]')) b.onclick = () => go({...r, variant: b.dataset.variant});
  $('maybeChip')?.addEventListener('click', () => go({...r, maybe: r.maybe ? '' : '1', variant: '', check: '', doubt: ''}));
  $('doubtChip')?.addEventListener('click', () => go({...r, doubt: r.doubt ? '' : '1', variant: '', check: '', maybe: ''}));
  if (view.toolbar === 'library') {
    wireSearch(r);
    wireFilterBar(r); wireSaveButton(r);
    $('charPill')?.addEventListener('click', () => go({...r, char: '', variant: '', maybe: '', check: '', doubt: '', sort: ''}));
  }
}
// 图库一类页面的搜索框（网格、角色页、画廊、按天、日历）。
const searchBox = r => `<div class="search-wrap">${searchHTML()}<input type="search" id="search" spellcheck="false" autocorrect="off" autocapitalize="off"`
  + ` placeholder="${t('搜索角色、别名或文件名')}" aria-label="${t('搜索图库')}" value="${esc(r.q)}"><kbd>/</kbd></div>`;
// 搜索词就地改进地址（不新增历史记录，换一个新的路由对象，旧回复认得出来），然后按新的筛选重画：网格页重读；自己渲染的页面（按天、日历）和筛选条上改了条件一样，
// 收到 render({samePage: true})（筛选变了只有这一个入口，docs/WEB_V2.md「页面样板」）。活页条的时间轴跟着筛选重读。
function wireSearch(r) {
  onTyped($('search'), e => {
    reRoute({q: e.target.value.trim()});
    const page = pageOf(state.route);
    if (page.render) page.render({samePage: true, restore: false, scope: currentPage()}); else reload(true);
    railRoute(state.route);
  }, 250);
}
const FIT_ICONS = {
  square: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="3" y="3" width="6" height="6" rx="1.5"/><rect x="11" y="3" width="6" height="6" rx="1.5"/><rect x="3" y="11" width="6" height="6" rx="1.5"/><rect x="11" y="11" width="6" height="6" rx="1.5"/></svg>',
  fit: '<svg viewBox="0 0 20 20" aria-hidden="true"><rect x="2.5" y="3.5" width="9" height="5.5" rx="1.5"/><rect x="13" y="3.5" width="4.5" height="5.5" rx="1.5"/><rect x="2.5" y="11" width="4.5" height="5.5" rx="1.5"/><rect x="8.5" y="11" width="9" height="5.5" rx="1.5"/></svg>',
};

// 页面这一部分注册的处理函数。页面启动时调用一次。
export function wireShell() {
  onMeta(metaShown);
  wireGalleries();
  // 后退（#526）：标题左边的「‹」（只在能后退时显示）、Alt+← 或 ⌘[（键盘处理在 ui/grid.js）、鼠标侧键，都走
  // core/route.js 的 stepHistory，只在这个图库的页面之间走。鼠标侧键在 mouseup 时拦下浏览器自己的后退，否则
  // 它会退出图库页面。
  $('backButton').onclick = () => stepHistory(-1);
  $('backButton').title = t('后退（{key}）', {key: isMac ? '⌘[' : 'Alt+←'});
  window.addEventListener('mouseup', e => { if (e.button === 3 || e.button === 4) { e.preventDefault(); stepHistory(e.button === 3 ? -1 : 1); } });
  $('empty').addEventListener('click', e => {
    const act = e.target.closest('[data-empty]')?.dataset.empty;
    if (act === 'nsfw') setNsfw('');
    else if (act) viewOf(state.route).emptyAct?.(act);
  });
  window.addEventListener('hashchange', applyRoute);
}

