// 角色页横幅下面的三组资料（#712）：最近加入的几张（点开详情）、收集曲线（最近 12 个月每月进了几张，ui/month_bars.js）、
// 常一起出现的角色（同框次数最多的几个；一个都没有时是同作品的角色），圆头像和名字，链到他们的角色页。
// 数据由 core/character.js 读好，这里只画；横幅本身在 ui/character.js。
import {coverUrl, imageUrl, monthLabel, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {charHash} from '../core/route.js';
import {inspect} from './detail.js';
import {attr, make, refill, show, text, tpl} from './el.js';
import {monthBars} from './month_bars.js';
import {setTint} from './tint.js';

const FACTS = tpl`<div class="char-facts">
  <section class="char-fact" data-ref="recentBox"><h2 class="char-fact-title" data-t>最近加入</h2><div class="char-recent" data-ref="recent"></div></section>
  <section class="char-fact" data-ref="curveBox"><h2 class="char-fact-title" data-t>收集曲线</h2><div class="char-curve" role="img" data-ref="curve"
    aria-label="最近 12 个月每月加入的张数" data-t-attr="aria-label"></div><p class="char-curve-axis text-meta"><span data-ref="from"></span><span data-ref="to"></span></p></section>
  <section class="char-fact" data-ref="peerBox"><h2 class="char-fact-title" data-ref="peerTitle"></h2><ul class="char-peers" data-ref="peers"></ul></section>
</div>`;

// 最近加入的一张：方图，读图之前是这张图自己的主色；点开详情。
function recentTile(item) {
  const b = make('button', 'char-recent-tile'), img = make('img');
  attr(b, 'type', 'button'); attr(b, 'aria-label', t('看详情'));
  attr(img, 'alt', ''); attr(img, 'loading', 'lazy'); attr(img, 'src', imageUrl(item.uuid, 'medium'));
  setTint(b, item.color || null);
  b.append(img); b.onclick = () => inspect(item.uuid);
  return b;
}
// 一个角色：圆头像和名字，链到角色页；悬停写同框几张（常一起出现）或一共几张（同作品）。
function personLink({c, count}, together) {
  const a = make('a', 'char-peer'), img = make('img', 'avatar');
  attr(a, 'href', charHash(c.id)); attr(a, 'title', together ? t('同框 {n} 张', {n: number(count)}) : t('{n} 张', {n: number(count)}));
  attr(img, 'alt', ''); attr(img, 'loading', 'lazy'); attr(img, 'src', coverUrl(c.cover));
  a.append(img, make('span', '', c.name));
  const li = make('li'); li.append(a);
  return li;
}

/** 三组资料的部件：{el, update(data), people(group), dispose()}。data 是 core/character.js 的 loadCharacter 的结果，null 时前两组藏起来
 * （切换 NSFW 后、新数据到之前）；group 是 peopleOf 的结果，每次画横幅都按当时的角色目录重算。 */
export function charFacts() {
  const {el, refs} = FACTS();
  const shown = {data: false, people: false};
  const showAll = () => show(el, shown.data || shown.people);
  function update(data) {
    const recent = data?.recent || [], months = data?.months || [], any = months.some(m => m.count > 0);
    show(refs.recentBox, recent.length > 0);
    refill(refs.recent, recent.map(x => x.uuid + ':' + (x.color || '')).join(','), () => recent.map(recentTile));
    show(refs.curveBox, any);
    monthBars(refs.curve, months, {min: 4, mark: m => m.count ? '' : 'is-zero'});
    text(refs.from, months.length ? monthLabel(months[0].month) : '');
    text(refs.to, months.length ? monthLabel(months[months.length - 1].month) : '');
    shown.data = recent.length > 0 || any; showAll();
  }
  function people({kind, people: list}) {
    const together = kind === 'together';
    text(refs.peerTitle, together ? t('常一起出现') : t('同作品'));
    show(refs.peerBox, list.length > 0);
    refill(refs.peers, kind + '|' + list.map(x => [x.c.id, x.c.cover, x.c.name, x.count].join(':')).join(','), () => list.map(x => personLink(x, together)));
    shown.people = list.length > 0; showAll();
  }
  update(null);
  return {el, update, people, dispose: () => el.remove()};
}
