// 资料库页（#711，地址 #/characters、/works、/artists）：三个页签用同一个页头（大标题「资料库」+ 只写数量的副标题，右侧是
// 角色｜作品｜画师的分段切换，由框架画在页头里，标题高度不随页签变）、同样的宽度，内容前最多一行工具（#toolbar）：
// - 角色（ui/cast.js）：搜索、全部作品、只当标签、书架｜列表；
// - 作品（ui/works_tab.js）：搜索、全部｜已关注｜待确认、关注 Danbooru 上的作品、新建作品；
// - 画师（ui/artist_dir.js）：搜索、排序。
// 照 docs/WEB_V2.md「页面样板」：进页面时在页面作用域上建一次三个页签的部件，切页签（同一页）只显隐、更新；数据在 core/catalog.js。
// 对话框改了数据会发 catalogChanged（ui/catalog.js），这里据此更新。
import {catalogChanged, forgetActivity, loadActivity, loadIndex, loadTagged, loadWorkTree, searchCharacters, workKey} from '../core/catalog.js';
import {number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {onTyped} from '../core/keys.js';
import {refreshMeta} from '../core/meta.js';
import {implement} from '../core/pages.js';
import {prefs} from '../core/prefs.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {artistsView, dirHeading} from './artist_dir.js';
import {cast, castView} from './cast.js';
import {allWorksButton, openTagged, refreshLocalLinks} from './catalog.js';
import {$, error, readFailure} from './dom.js';
import {attr, make, show, text, tpl, value} from './el.js';
import {openLocalLinks} from './palette.js';
import {currentPage} from './scope.js';
import {renderHeading} from './shell.js';
import {launch} from './work_page.js';
import {worksView} from './works_tab.js';

const PAGE = tpl`<div class="cat-page"><div data-ref="cast"></div><div data-ref="works"></div><div data-ref="artists"></div></div>`;
const CAST_TOOLS = tpl`<div class="cat-tools"><label class="search"><svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="10" cy="10" r="6.5"/><path d="m15 15 5 5"/></svg><input
  type="search" data-ref="q" spellcheck="false" autocomplete="off" placeholder="搜索角色或作品" aria-label="搜索角色" data-t-attr="placeholder aria-label"></label><span
  data-ref="all"></span><span class="spacer"></span><button type="button" class="btn" data-ref="tagged" title="不算角色、只当标签的吉祥物和玩家化身" data-t-attr="title"><span
  data-t>只当标签</span><span class="chip-count" data-ref="taggedN"></span></button><span class="seg" role="group" aria-label="显示方式" data-t-attr="aria-label"><button
  type="button" data-ref="shelf" data-mode="shelf" data-t>书架</button><button type="button" data-ref="list" data-mode="list" data-t>列表</button></span></div>`;
const TABS = [['', () => t('角色')], ['works', () => t('作品')], ['artists', () => t('画师')]];

// 资料库这一页的部件：三个页签各一份，进页面时建一次。
function indexView(s) {
  const {el, refs} = PAGE(), tools = CAST_TOOLS(), all = allWorksButton();
  tools.refs.all.replaceWith(all.el);
  $('charIndex').replaceChildren(el);
  const castTab = castView(s), works = worksView(s, {open: (key, card) => launch(key, card)}), artists = artistsView(s, {onDraw: renderHeading});
  refs.cast.append(castTab.el); refs.works.append(works.el); refs.artists.append(artists.el);
  let asked = 0;

  // ---------- 角色页签的工具 ----------
  onTyped(tools.refs.q, () => search(), 200);
  s.on(tools.refs.tagged, 'click', openTagged);
  for (const b of [tools.refs.shelf, tools.refs.list]) s.on(b, 'click', () => { cast.mode = b.dataset.mode; prefs.set('castMode', cast.mode); drawCast(); });
  async function search() {
    const q = tools.refs.q.value.trim(), ask = ++asked, route = state.route;
    if (!q) { cast.list = null; drawCast(); return; }
    let list; try { list = await searchCharacters(q); } catch (e) { if (ask === asked && s.alive) readFailure(refs.cast, e); return; }
    if (ask !== asked || !s.alive || state.route !== route) return;
    cast.list = list; drawCast();
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
  }
  async function enterCast(route) {
    cast.list = null; value(tools.refs.q, '');
    if (state.chars.some(c => c.summary)) {
      try { await refreshMeta(); } catch (e) { error(e.message); return; }
      if (!s.alive || state.route !== route) return;
    }
    drawCast();
    loadTagged().then(() => { if (s.alive) drawCast(); }, e => error(readError(e)));
    await loadWorkTree().catch(e => error(readError(e)));
    if (!s.alive) return;
    drawCast();
    readActivity();
    refreshLocalLinks();
  }
  async function readActivity() {
    let fresh; try { fresh = await loadActivity(); } catch (e) { error(readError(e)); return; }
    if (fresh && s.alive) drawCast();
  }
  async function enterWorks(route) {
    works.update();
    try { await Promise.all([loadIndex(), loadWorkTree()]); } catch (e) { if (s.alive && state.route === route) readFailure(refs.works, e); return; }
    if (!s.alive || state.route !== route) return;
    renderHeading(); works.update();
  }
  function open(route, opts) {
    showTab();
    const tab = section();
    if (tab === 'artists') return artists.enter(opts);
    if (tab === 'works') return enterWorks(route);
    return enterCast(route);
  }
  // 对话框改了数据：关注、作品、只当标签、自建角色的关联。
  s.add(catalogChanged.on(async why => {
    if (why === 'links') { renderHeading(); return; }
    if (why === 'watch' || why === 'index') { try { await loadIndex(); } catch { /* 读不到就留着上一次的 */ } }
    if (why === 'tagged' || why === 'watch' || why === 'index') { if (!s.alive) return; renderHeading(); works.update(); drawCast(); }
  }));
  function tools_(r) { return r.section === 'artists' ? artists.tools : r.section === 'works' ? works.tools : tools.el; }
  return {scope: s, open, tools: tools_, drawCast, works, artists, castTab};
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
  // 计数到达后只重画角色目录（搜索时不动搜索结果）。
  onMeta: ({directory}) => { if (directory && view?.scope.alive && !state.route.section) { view.drawCast(); } },
  refresh: async why => {
    const v = view; if (!v?.scope.alive) return;
    if (state.route.section === 'artists') { v.artists.refresh(why); return; }
    if (why === 'nsfw') { forgetActivity(); await loadWorkTree().catch(() => {}); }
    if (why === 'undo' && state.route.section === 'works') { await loadIndex().catch(e => error(readError(e))); }
    if (why === 'undo') await loadTagged().catch(e => error(readError(e)));
    if (!v.scope.alive) return;
    renderHeading();
    if (state.route.section === 'works') v.works.update(); else { v.drawCast(); if (why === 'nsfw') loadActivity().then(fresh => { if (fresh && v.scope.alive) v.drawCast(); }, () => {}); }
  }}});
