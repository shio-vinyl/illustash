// 资料库页（#711，地址 #/characters、/works、/artists）：三个页签用同一个页头（大标题「资料库」+ 只写数量的副标题，右侧是
// 角色｜作品｜画师的分段切换，由框架画在页头里，标题高度不随页签变）、同样的宽度，内容前最多一行工具（#toolbar）：
// - 角色（ui/cast.js）：搜索、全部作品、只当标签、书架｜列表；
// - 作品（ui/works_tab.js）：搜索、全部｜已关注｜待确认、关注 Danbooru 上的作品、新建作品；
// - 画师（ui/artist_dir.js）：搜索、排序。
// 照 docs/WEB_V2.md「页面样板」：进页面时在页面作用域上建一次三个页签的部件，切页签（同一页）只显隐、更新；数据在 core/catalog.js。
// 对话框改了数据会发 catalogChanged（ui/catalog.js），这里据此更新。
import {catalogChanged, forgetActivity, loadActivity, loadTagged, loadWorkTree, searchCharacters, workKey} from '../core/catalog.js';
import {loadIndex} from '../core/jobs.js';
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {onTyped} from '../core/keys.js';
import {canShowChars, refreshMeta} from '../core/meta.js';
import {implement} from '../core/pages.js';
import {prefs} from '../core/prefs.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {artistsView, dirHeading} from './artist_dir.js';
import {cast, castView} from './cast.js';
import {allWorksButton, openTagged, refreshLocalLinks, searchField} from './catalog.js';
import {$} from './dom.js';
import {attr, make, show, text, tpl, value} from './el.js';
import {openLocalLinks} from './palette.js';
import {currentPage} from './scope.js';
import {renderHeading} from './shell.js';
import {launch} from './work_page.js';
import {worksView} from './works_tab.js';

const PAGE = tpl`<div class="cat-page"><p class="cat-error" role="alert" data-ref="error"></p><div data-ref="cast"></div><div data-ref="works"></div><div
  data-ref="artists"></div></div>`;
const CAST_TOOLS = tpl`<div class="cat-tools"><span data-ref="q"></span><span data-ref="all"></span><span class="spacer"></span><button type="button" class="btn" data-ref="tagged"
  title="不算角色、只当标签的吉祥物和玩家化身" data-t-attr="title"><span data-t>只当标签</span><span class="chip-count" data-ref="taggedN"></span></button><span class="seg"
  role="group" aria-label="显示方式" data-t-attr="aria-label"><button type="button" data-ref="shelf" data-mode="shelf" data-t>书架</button><button type="button" data-ref="list"
  data-mode="list" data-t>列表</button></span></div>`;
const TABS = [['', () => t('角色')], ['works', () => t('作品')], ['artists', () => t('画师')]];

// 资料库这一页的部件：三个页签各一份，进页面时建一次。读失败时错误写在页签上面的一行里（部件留着），下一次读成功就清掉。
function indexView(s) {
  const {el, refs} = PAGE(), tools = CAST_TOOLS(), all = allWorksButton(), q = searchField(t('搜索角色或作品'));
  tools.refs.all.replaceWith(all.el); tools.refs.q.replaceWith(q.el);
  $('charIndex').replaceChildren(el);
  const castTab = castView(s), works = worksView(s, {open: (key, card) => launch(key, card)}), artists = artistsView(s, {onDraw: renderHeading});
  refs.cast.append(castTab.el); refs.works.append(works.el); refs.artists.append(artists.el);
  let asked = 0;
  // 还在这一页、这一个地址（同一页换了页签或筛选，路由对象就换了）。
  const here = route => s.alive && state.route === route;
  const fail = (route, e) => { if (here(route)) { text(refs.error, readError(e)); show(refs.error, true); } };
  const ok = route => { if (here(route)) show(refs.error, false); };

  // ---------- 角色页签的工具 ----------
  onTyped(q.input, () => search(), 200);
  s.on(tools.refs.tagged, 'click', openTagged);
  for (const b of [tools.refs.shelf, tools.refs.list]) s.on(b, 'click', () => { cast.mode = b.dataset.mode; prefs.set('castMode', cast.mode); drawCast(); });
  async function search() {
    const word = q.input.value.trim(), ask = ++asked, route = state.route;
    if (!word) { cast.list = null; drawCast(); return; }
    let list; try { list = await searchCharacters(word); } catch (e) { if (ask === asked) fail(route, e); return; }
    if (ask !== asked || !here(route)) return;
    ok(route); cast.list = list; drawCast();
  }
  function drawCast() {
    attr(tools.refs.shelf, 'aria-pressed', String(cast.mode === 'shelf')); attr(tools.refs.list, 'aria-pressed', String(cast.mode === 'list'));
    text(tools.refs.taggedN, state.tagged?.length ? number(state.tagged.length) : '');
    all.update();
    castTab.update();
  }

  // ---------- 进页签、刷新 ----------
  const section = () => state.route.section || '';
  function showTab() {
    const tab = section();
    show(refs.cast, tab === ''); show(refs.works, tab === 'works'); show(refs.artists, tab === 'artists');
    show(refs.error, false);
  }
  async function enterCast(route) {
    cast.list = null; value(q.input, '');
    // 摘要已有首屏用的名字、数量和封面。变体的张数等完整数据回来再补。
    if (state.chars.some(c => c.summary)) {
      if (canShowChars()) drawCast();
      try { await refreshMeta(); } catch (e) { fail(route, e); return; }
      if (!here(route)) return;
    }
    drawCast();
    loadTagged().then(() => { if (here(route)) drawCast(); }, e => fail(route, e));
    try { await loadWorkTree(); } catch (e) { fail(route, e); return; }
    if (!here(route)) return;
    ok(route); drawCast();
    readActivity(route);
    refreshLocalLinks();
  }
  async function readActivity(route) {
    let fresh; try { fresh = await loadActivity(); } catch (e) { fail(route, e); return; }
    if (fresh && here(route)) drawCast();
  }
  async function enterWorks(route) {
    works.update();
    try { await Promise.all([loadIndex(), loadWorkTree()]); } catch (e) { fail(route, e); return; }
    if (!here(route)) return;
    ok(route); renderHeading(); works.update();
  }
  function open(route, opts) {
    showTab();
    const tab = section();
    if (tab === 'artists') return artists.start(opts);
    if (tab === 'works') return enterWorks(route);
    return enterCast(route);
  }
  // 数据变了（refresh）：NSFW 换了要重读作品样图和最近动态，正在搜角色时按新设置重搜；撤销后重读作品和「只当标签」。
  async function refresh(why) {
    const route = state.route;
    if (section() === 'artists') { artists.reload(why); return; }
    try {
      if (why === 'nsfw') { forgetActivity(); await loadWorkTree(); }
      if (why === 'undo') await Promise.all([loadTagged(), section() === 'works' ? loadIndex() : null]);
    } catch (e) { fail(route, e); return; }
    if (!here(route)) return;
    renderHeading();
    if (section() === 'works') { works.update(); return; }
    if ((why === 'nsfw' || why === 'undo') && q.input.value.trim()) search(); else drawCast();
    if (why === 'nsfw') readActivity(route);
  }
  // 对话框改了数据：关注、作品、只当标签、自建角色的关联。保存关注、作品时 core/catalog.js 已经换上新的索引，这里只重画。
  s.add(catalogChanged.on(why => {
    if (!s.alive) return;
    renderHeading();
    if (why !== 'links') { works.update(); drawCast(); }
  }));
  function toolsFor(r) { return r.section === 'artists' ? artists.tools : r.section === 'works' ? works.tools : tools.el; }
  return {scope: s, open, refresh, tools: toolsFor, drawCast, works};
}

let view = null;
// 页面作用域换了（离开过资料库）就重建；同一页切页签沿用。工具栏先于 render 画，所以两处都从这里取。
function ensure() {
  const s = currentPage();
  if (!view || view.scope !== s || !s?.alive) view = indexView(s);
  return view;
}

// 页头：三个页签各写自己的数量；角色页签另有「N 个自建角色能对上 Danbooru」的按钮。
function head(r) {
  const sub = r.section === 'artists' ? dirHeading() : r.section === 'works'
    ? t('{n} 部作品', {n: number(state.index?.works?.length || 0)}) + (state.index?.watch?.works.length ? ' · ' + t('关注 {n} 部', {n: number(state.index.watch.works.length)}) : '')
    : t('{n} 位角色 · {works} 部作品', {n: number(state.chars.length), works: number(new Set(state.chars.map(workKey).filter(k => k !== 'oc' && k !== 'other')).size)});
  const actions = [];
  if (!r.section && state.localLinks?.length) {
    const b = make('button', 'btn', t('{n} 个自建角色能对上 Danbooru', {n: number(state.localLinks.length)}));
    b.type = 'button'; b.title = t('补上中文名、别名和作品，和已有角色重复的可以合并'); b.onclick = openLocalLinks;
    actions.push(b);
  }
  return {title: t('资料库'), sub, actions};
}

implement({characters: {head,
  // 页头里的分段切换（框架画，ui/shell.js）。
  switches: r => ({label: t('资料库'), list: TABS.map(([v, label]) => ({label: label(), href: '#/characters' + (v ? '/' + v : ''), on: (r.section || '') === v}))}),
  tools: r => ensure().tools(r),
  render: opts => ensure().open(state.route, opts),
  // 计数（角色目录）到达后：角色页签重画（搜索时不动搜索结果），作品页签的头像也取角色的封面。
  onMeta: ({directory}) => { if (!directory || !view?.scope.alive) return; if (state.route.section === 'works') view.works.update(); else if (!state.route.section) view.drawCast(); },
  refresh: why => view?.scope.alive ? view.refresh(why) : undefined,
}});
