// 角色页（#712）：网格上面跟着滚动的横幅（这个角色最近的一张宽图往下渐变成图的颜色、圆头像、名字、作品和 Danbooru、
// 张数和未检查、标为已检查和「⋯」菜单），横幅下面的三组资料（ui/character_facts.js）。页头只有一个小标题（名字和张数），
// 横幅上的名字整个看得见时藏着（ui/shell.js 的 tuckHeading）。
// 角色页没有自己的 render，主区是网格（core/pages.js 的 character 视图）：横幅放进 C2 留的网格头部（ui/grid.js 的 gridHead），
// 由框架的 gridHead 钩子在进页和同一页又画一次时建好、更新；元数据重读（写入、撤销以后）时横幅自己在作用域上听 onMeta。
// 数据在 core/character.js。工具栏（造型、可能也是 TA、可能认错、排序、选择）照旧由 ui/shell.js 画。
import {loadCharacter, peopleOf} from '../core/character.js';
import {coverUrl, imageUrl, localDate, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {nameTip, roleName, workLabel} from '../core/labels.js';
import {onMeta} from '../core/meta.js';
import {implement, narrowed} from '../core/pages.js';
import {go} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {backLink} from './back_link.js';
import {charFacts} from './character_facts.js';
import {$, error} from './dom.js';
import {attr, show, text, tpl} from './el.js';
import {gridHead, sweep} from './grid.js';
import {moreMenu} from './more_menu.js';
import {pageView} from './page_view.js';
import {editProfile, mergeCharacter, toggleMascot, toggleOc} from './palette.js';
import {tuckHeading} from './shell.js';
import {setTint} from './tint.js';

// 没有来路（打开图库后的第一页就是这个角色）时，返回去资料库。
const BACK = {label: t('资料库'), href: '#/characters'};

const HERO = tpl`<section class="char-hero" aria-labelledby="charHeroName">
  <div class="char-hero-art" aria-hidden="true"><img alt="" decoding="async" data-ref="art"></div>
  <div class="char-hero-bar"><span data-ref="back"></span><div class="char-hero-actions"><button class="btn glass" type="button" data-ref="sweep"
    title="把剩下的机器识别都记为已检查；以后新识别的图会重新计入未检查" data-t-attr="title" data-t>标为已检查</button><span data-ref="more"></span></div></div>
  <div class="char-hero-main">
    <img class="avatar char-hero-avatar" alt="" data-ref="avatar">
    <div class="char-hero-id">
      <h1 class="title-hero char-hero-name" id="charHeroName" data-ref="name"></h1>
      <p class="char-hero-meta"><span class="oc-badge" data-ref="badge" data-t>只当标签</span><span data-ref="work"></span><code class="char-hero-tag"
        data-ref="tag"></code><button class="link-button" type="button" data-ref="nudge" title="补上中文名、别名和所属作品" data-t-attr="title" data-t>关联 Danbooru…</button></p>
    </div>
    <dl class="char-hero-stats">
      <div><dt class="text-meta" data-t>张数</dt><dd data-ref="count"></dd></div>
      <div data-ref="recentBox"><dt class="text-meta" data-t>最近 30 天</dt><dd data-ref="recent"></dd></div>
      <div data-ref="checkBox"><dt class="text-meta" data-t>未检查</dt><dd><button class="char-hero-check" type="button" data-ref="check"></button></dd></div>
    </dl>
  </div>
  <div data-ref="facts"></div>
  <p class="char-hero-foot text-meta" data-ref="foot"></p>
</section>`;

// 横幅部件：{el, update(r, c), scope, dispose()}。建在页面作用域的子作用域上，放进网格头部；作用域释放时清空、藏起头部，
// 页头的小标题恢复常显。
function charHero(scope) {
  const {el, refs} = HERO(), facts = charFacts(), back = backLink(scope, BACK);
  let c = null, route = null, data = null, key = '', nsfw = null, seq = 0;
  const more = scope.add(moreMenu({id: 'char-menu', menuId: 'charMenu', button: 'btn glass', items: () => menuItems(c)}));
  refs.facts.replaceWith(facts.el); refs.back.replaceWith(back.el); refs.more.replaceWith(more.el);
  scope.on(refs.sweep, 'click', sweep);
  scope.on(refs.nudge, 'click', () => editProfile(c));
  // 未检查：按下只看还没检查过的机器识别，再按一次回到全部（新增一条历史，返回仍回到进这一页之前的地方，core/route.js）。
  scope.on(refs.check, 'click', () => go({...route, check: route.check ? '' : '1', maybe: ''}));
  // 元数据重读以后（写入、撤销、改资料）角色的数字和名字变了：照新的画。换页途中（state.char 还没读到）不画。
  scope.add(onMeta(() => { if (state.char?.id === c?.id) update(state.route, state.char); }));
  // 横幅上的名字整个看得见时，页头的小标题藏着；滚出去一部分就显示。
  const seen = scope.observe(new IntersectionObserver(([entry]) => tuckHeading(entry.intersectionRatio >= 1), {root: $('viewport'), threshold: [0, 1]}));
  seen.observe(refs.name);
  tuckHeading(true);
  const head = gridHead();
  head.replaceChildren(el); show(head, true);
  scope.add(() => { if (head.firstElementChild === el) { head.replaceChildren(); show(head, false); } tuckHeading(false); });

  // 横幅的图、最近加入、曲线和常一起出现：按角色、NSFW 和数据版本读（core/character.js 缓存）。同一页上几次读取不分先后回来，
  // 只认最后发出的那次（seq，页面样板第 5 条），成功和失败都这样判断。角色在这个作用域里不变，不用再比角色。
  async function load() {
    const mine = ++seq;
    let got;
    try { got = await loadCharacter(c, {signal: scope.signal}); } catch (e) {
      if (scope.alive && mine === seq && e.name !== 'AbortError') error(readError(e));
      return;
    }
    if (!scope.alive || mine !== seq) return;
    data = got;
    const art = data.banner;
    attr(refs.art, 'src', art ? imageUrl(art.uuid, 'preview') : null);
    show(refs.art, !!art);
    setTint(el, art?.color || c.color || null);
    text(refs.recent, data.recent30 === null ? '' : number(data.recent30));
    show(refs.recentBox, data.recent30 !== null);
    facts.update(data);
    facts.people(peopleOf(c, data.together, state.chars, state.charMap));
  }

  function update(r, char) {
    route = r; c = char;
    const tagOnly = c.mascot || c.group_tag;   // 只当标签：没有角色归属需要复核
    text(refs.name, roleName(c)); attr(refs.name, 'title', nameTip(c));
    attr(refs.avatar, 'src', coverUrl(c.cover));
    show(refs.badge, tagOnly);
    attr(refs.badge, 'title', (c.mascot_origin === 'default' ? t('老师、博士、训练员这类没有固定形象的玩家化身和吉祥物默认只当标签。') : '') +
      t('不算角色：不出现在角色列表里，也不算多人图；图上仍挂着这个标签'));
    const work = tagOnly ? '' : workLabel(c), tag = c.tag && c.tag !== c.name ? c.tag : '';
    text(refs.work, work); show(refs.work, !!work);
    text(refs.tag, tag); show(refs.tag, !!tag);
    show(refs.nudge, !c.tag && c.kind !== 'oc' && !c.label_only);
    text(refs.count, number(c.count));
    show(refs.checkBox, !tagOnly && !!(c.unchecked || r.check));
    text(refs.check, number(c.unchecked));
    attr(refs.check, 'aria-pressed', r.check ? 'true' : 'false');
    attr(refs.check, 'title', r.check ? t('显示全部') : t('只看机器识别、还没检查过的图'));
    show(refs.sweep, !tagOnly); attr(refs.sweep, 'disabled', !c.unchecked);
    // 「全部 N 张」只在下面的网格就是全部的时候写；有筛选（未检查、可能也是 TA、可能认错、造型、搜索、条件）时工具栏的芯片已经写明。
    const plain = !narrowed(r) && !r.maybe && !r.variant;
    const checked = !tagOnly && c.reviewed_at ? t('上次检查 {date}', {date: localDate(c.reviewed_at)}) : '';   // 后端存的是 UTC（#386 的 2-15）
    text(refs.foot, [plain ? t('全部 {n} 张', {n: number(c.count)}) : '', checked].filter(Boolean).join(' · '));
    back.update();
    // 常一起出现的名字和头像按当时的角色目录取：元数据比横幅的数据晚到时也跟着换。
    if (data) facts.people(peopleOf(c, data.together, state.chars, state.charMap));
    const next = [c.id, state.nsfw, state.readEpoch].join('|');
    if (next === key) return;
    // NSFW 变了：上一种设置下的图先藏起来，新数据到了再显示（读失败也不留着）。
    if (nsfw !== null && nsfw !== state.nsfw) { data = null; show(refs.art, false); facts.update(null); facts.people({kind: 'work', people: []}); }
    key = next; nsfw = state.nsfw;
    load();
  }
  return {el, update, scope, dispose: () => scope.dispose()};
}

// 「⋯」菜单的项：原创角色、只当标签、编辑资料、合并。每一步都进撤销栈（ui/palette.js）。
function menuItems(c) {
  if (!c) return [];
  return [
    !c.group_tag && !c.label_only && !c.mascot && [c.kind === 'oc' ? t('取消原创角色') : t('标为原创角色'), () => toggleOc(c), t('画师自己的原创角色，不属于任何作品')],
    !c.group_tag && !c.label_only && [c.mascot ? t('算作角色') : t('不算角色，只当标签'), () => toggleMascot(c),
      c.mascot ? t('恢复成角色：出现在角色列表里，同框就算多人图') : t('像佩洛洛这样的吉祥物、玩家化身：不算角色，不算多人图；图上仍挂着这个标签，可以按它筛选')],
    [t('编辑资料…'), () => editProfile(c), t('名称、别名、所属作品、Danbooru 关联')],
    !c.group_tag && [t('合并到另一个角色…'), () => mergeCharacter(c), t('把这个角色的图和人工结论并到另一个角色')],
  ];
}

// 横幅建在页面作用域的子作用域上：pageView 按横幅自己的作用域判断死活。
const hero = pageView(scope => charHero(scope.child('character-hero')));
implement({character: {
  // 页头：小标题（名字和张数），横幅上的名字看得见时藏着。
  head: () => ({title: roleName(state.char), sub: state.char ? t('{n} 张', {n: number(state.char.count)}) : ''}),
  // 刚进这一页（或横幅的作用域已经释放）才建；同一页又画一次（换造型、只看未检查……）只按新地址更新。
  gridHead: ({samePage, scope}) => {
    if (!state.char) return;
    hero.enter({samePage, scope}).update(state.route, state.char);
  },
}});
