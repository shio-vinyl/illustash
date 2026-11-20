// 角色页（#712）：网格上面跟着滚动的横幅（这个角色最近的一张宽图往下渐变成图的颜色、圆头像、名字、作品和 Danbooru、
// 张数和未检查、标为已检查和更多菜单），横幅下面的三组资料（ui/character_facts.js），以及页头里的小标题（横幅滚出去以后才显示）。
// 角色页没有自己的 render，主区是网格（core/pages.js 的 character 视图）：横幅放进 C2 留的网格头部（ui/grid.js 的 gridHead），
// 在页头每次重画时（drawHeading）确保建好、只更新。数据在 core/character.js。工具栏（造型、可能也是 TA、可能认错、排序、选择）
// 照旧由 ui/shell.js 画。
import {loadCharacter} from '../core/character.js';
import {imageUrl, localDate, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {nameTip, workLabel} from '../core/labels.js';
import {implement} from '../core/pages.js';
import {hashFor} from '../core/route.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {charFacts} from './character_facts.js';
import {$, error} from './dom.js';
import {attr, cls, show, text, tpl} from './el.js';
import {gridHead, sweep} from './grid.js';
import {createLayer} from './overlay.js';
import {editProfile, mergeCharacter, toggleMascot, toggleOc} from './palette.js';
import {currentPage} from './scope.js';
import {setTint} from './tint.js';

const HERO = tpl`<section class="char-hero" aria-labelledby="charHeroName">
  <div class="char-hero-art" aria-hidden="true"><img alt="" decoding="async" data-ref="art"></div>
  <div class="char-hero-bar">
    <a class="char-hero-back" data-ref="back"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M12 4.5 6.5 10l5.5 5.5"/></svg><span data-ref="backLabel"></span></a>
    <div class="char-hero-actions">
      <button class="quiet" type="button" id="sweepButton" data-ref="sweep"></button>
      <div class="tag-control more-control"><button class="quiet" type="button" id="moreButton" data-ref="more">⋯</button>
        <div id="morePopover" class="menu menu-drop more-menu" data-ref="menu" hidden>
          <button class="menu-item" type="button" id="ocButton" data-ref="oc"></button>
          <button class="menu-item" type="button" id="mascotButton" data-ref="mascot"></button>
          <button class="menu-item" type="button" id="profileButton" data-ref="profile"></button>
          <button class="menu-item" type="button" id="mergeButton" data-ref="merge"></button>
        </div></div>
    </div>
  </div>
  <div class="char-hero-main">
    <img class="avatar is-large char-hero-avatar" alt="" data-ref="avatar">
    <div class="char-hero-id">
      <h1 class="char-hero-name" id="charHeroName" data-ref="name"></h1>
      <p class="char-hero-meta"><span class="oc-badge" data-ref="badge"></span><span data-ref="work"></span><code class="char-hero-tag" data-ref="tag"></code><button
        class="link-button" type="button" id="profileNudge" data-ref="nudge"></button></p>
    </div>
    <dl class="char-hero-stats">
      <div><dt data-ref="countLabel"></dt><dd data-ref="count"></dd></div>
      <div data-ref="recentBox"><dt data-ref="recentLabel"></dt><dd data-ref="recent"></dd></div>
      <div data-ref="checkBox"><dt data-ref="checkLabel"></dt><dd><a class="unchecked-link" data-ref="check"></a></dd></div>
    </dl>
  </div>
  <div data-ref="facts"></div>
  <p class="char-hero-foot" data-ref="foot"></p>
</section>`;

// 更多菜单是非模态浮层：Esc、点外关闭由浮层管理器（ui/overlay.js）统一处理。横幅只建一次，菜单元素不换；层按 id 找元素。
// 打开时焦点进第一项，↑ ↓ 在各项之间移动焦点（menu，#422）。
const moreMenu = createLayer({kind: 'popover', id: 'more-menu', el: () => $('morePopover'), anchor: () => $('moreButton'), menu: true, focusIn: 'first',
  onDismiss: reason => setMenu(false, reason)});
function setMenu(open, reason) {
  const el = $('morePopover'); if (el) el.hidden = !open;
  if (open) moreMenu.open(); else moreMenu.close(reason);
}

// 横幅部件：{el, update(r, c), out, dispose}。out 是横幅是否已经滚出视口（页头据此显示小标题）。
function charHero(scope, onOut) {
  const {el, refs} = HERO(), facts = charFacts();
  refs.facts.replaceWith(facts.el);
  let c = null, loaded = '';
  text(refs.backLabel, t('资料库')); attr(refs.back, 'href', '#/characters');
  attr(refs.more, 'title', t('更多操作')); attr(refs.more, 'aria-label', t('更多操作'));
  text(refs.profile, t('编辑资料…')); attr(refs.profile, 'title', t('名称、别名、所属作品、Danbooru 关联'));
  text(refs.merge, t('合并到另一个角色…')); attr(refs.merge, 'title', t('把这个角色的图和人工结论并到另一个角色'));
  text(refs.nudge, t('关联 Danbooru…')); attr(refs.nudge, 'title', t('补上中文名、别名和所属作品'));
  text(refs.countLabel, t('张数')); text(refs.recentLabel, t('最近 30 天')); text(refs.checkLabel, t('未检查'));
  text(refs.sweep, t('标为已检查')); attr(refs.sweep, 'title', t('把剩下的机器识别都记为已检查；以后新识别的图会重新计入未检查'));
  scope.on(refs.sweep, 'click', sweep);
  scope.on(refs.more, 'click', () => setMenu($('morePopover').hidden));
  scope.on(refs.merge, 'click', () => { setMenu(false); mergeCharacter(c); });
  scope.on(refs.profile, 'click', () => { setMenu(false); editProfile(c); });
  scope.on(refs.nudge, 'click', () => editProfile(c));
  scope.on(refs.oc, 'click', () => { setMenu(false); toggleOc(c); });
  scope.on(refs.mascot, 'click', () => { setMenu(false); toggleMascot(c); });
  // 横幅上按下不开始网格的拖动选择（ui/grid.js 在 #grid 上听 pointerdown）。
  scope.on(el, 'pointerdown', e => e.stopPropagation());
  // 横幅的名字有一部分滚出视口：页头显示小标题。
  const seen = scope.observe(new IntersectionObserver(([entry]) => onOut(entry.intersectionRatio < 1), {root: $('viewport'), threshold: [0, 1]}));
  seen.observe(refs.name);

  // 横幅的图、最近加入、曲线和同作品：按角色和数据版本读（core/character.js 缓存），回来时还在这个角色上才画。
  // 这些数据不随排序、造型、筛选变，所以守卫看作用域和角色，不看路由对象。
  async function load(char) {
    let data;
    try { data = await loadCharacter(char, {signal: scope.signal}); } catch (e) { if (scope.alive && e.name !== 'AbortError') error(readError(e)); return; }
    if (!scope.alive || state.char?.id !== char.id) return;
    const art = data.banner;
    attr(refs.art, 'src', art ? imageUrl(art.uuid, 'medium') : null);
    show(refs.art, !!art);
    setTint(el, art?.color || char.color || null);
    text(refs.recent, data.recent30 === null ? '' : number(data.recent30));
    show(refs.recentBox, data.recent30 !== null);
    facts.update(data);
  }

  function update(r, char) {
    c = char;
    const tagOnly = !!c.mascot;   // 只当标签：没有要复核的，所以没有检查、清扫、OC 这些控件
    text(refs.name, c.name); attr(refs.name, 'title', nameTip(c));
    attr(refs.avatar, 'src', c.cover ? imageUrl(c.cover) : null);
    show(refs.badge, tagOnly); text(refs.badge, t('只当标签'));
    attr(refs.badge, 'title', (c.mascot_origin === 'default' ? t('老师、博士、训练员这类没有固定形象的玩家化身和吉祥物默认只当标签。') : '') +
      t('不算角色：不出现在角色列表里，也不算多人图；图上仍挂着这个标签'));
    const work = tagOnly ? '' : workLabel(c);
    text(refs.work, work); show(refs.work, !!work);
    text(refs.tag, c.tag && c.tag !== c.name ? c.tag : ''); show(refs.tag, !!c.tag && c.tag !== c.name);
    show(refs.nudge, !c.tag && c.kind !== 'oc' && !c.label_only);
    text(refs.count, number(c.count));
    // 未检查：点它只看还没检查过的机器识别，再点一次回到全部。
    show(refs.checkBox, !tagOnly && !!(c.unchecked || r.check));
    text(refs.check, number(c.unchecked) + (r.check ? ' ×' : ''));
    cls(refs.check, 'on', !!r.check);
    attr(refs.check, 'href', hashFor({...r, check: r.check ? '' : '1', maybe: ''}));
    attr(refs.check, 'title', r.check ? t('显示全部') : t('只看机器识别、还没检查过的图'));
    show(refs.sweep, !tagOnly); attr(refs.sweep, 'disabled', !c.unchecked);
    show(refs.oc, !c.label_only && !tagOnly);
    text(refs.oc, c.kind === 'oc' ? t('取消原创角色') : t('标为原创角色')); attr(refs.oc, 'title', t('画师自己的原创角色，不属于任何作品'));
    show(refs.mascot, !c.label_only);
    text(refs.mascot, c.mascot ? t('算作角色') : t('不算角色，只当标签'));
    attr(refs.mascot, 'title', c.mascot ? t('恢复成角色：出现在角色列表里，同框就算多人图') : t('像佩洛洛这样的吉祥物、玩家化身：不算角色，不算多人图；图上仍挂着这个标签，可以按它筛选'));
    const checked = !tagOnly && c.reviewed_at ? t('上次检查 {date}', {date: localDate(c.reviewed_at)}) : '';   // 后端存的是 UTC（#386 的 2-15）
    text(refs.foot, [t('全部 {n} 张', {n: number(c.count)}), checked].filter(Boolean).join(' · '));
    const key = [c.id, state.nsfw, state.readEpoch].join('|');
    if (key !== loaded) { loaded = key; load(c); }
  }
  return {el, update, scope};
}

// 页头里的小标题：横幅在视野里时藏着（名字已经在横幅上），滚出去以后显示名字和张数。
const TITLE = tpl`<hgroup class="heading-main char-title"><h1 class="title-page" data-ref="name"></h1><p class="page-sub" data-ref="sub"></p></hgroup>`;
let hero = null, title = null, out = false;
function drawTitle(h, c) {
  title ||= TITLE();
  text(title.refs.name, c.name);
  text(title.refs.sub, t('{n} 张', {n: number(c.count)}));
  cls(title.el, 'is-out', out);
  attr(title.el, 'aria-hidden', !out);
  if (h.firstChild !== title.el || h.childNodes.length !== 1) h.replaceChildren(title.el);
}
// 每次画页头时调（ui/shell.js 的 renderHeading）：这一页的横幅没建就建一个（换角色是换页，作用域是新的），然后只更新。
function drawHeading(h, r, c) {
  const page = currentPage(), head = gridHead();
  if (!hero || !hero.scope.alive || hero.page !== page) {
    const scope = page.child('character-hero');
    hero = {...charHero(scope, o => { out = o; if (title) { cls(title.el, 'is-out', o); attr(title.el, 'aria-hidden', !o); } }), page};
    out = false;
    head.replaceChildren(hero.el); cls(head, 'is-hero', true); show(head, true);
    // 离开这一页：网格头部清空、藏起来，别的网格页没有横幅。
    const el = hero.el;
    scope.add(() => { if (head.firstChild === el) { head.replaceChildren(); cls(head, 'is-hero', false); show(head, false); } setMenu(false); });
  }
  hero.update(r, c);
  drawTitle(h, c);
}

implement({character: {drawHeading}});
