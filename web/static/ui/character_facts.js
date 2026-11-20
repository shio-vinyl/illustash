// 角色页横幅下面的三组资料（#712）：最近加入的几张（点开详情）、收集曲线（最近 12 个月每月进了几张）、同作品的角色
// （圆头像和名字，链到他们的角色页）。数据由 core/character.js 读好，这里只画；横幅本身在 ui/character.js。
import {t} from '../core/i18n.js';
import {imageUrl, monthText, number} from '../core/format.js';
import {charHash} from '../core/route.js';
import {inspect} from './detail.js';
import {attr, css, make, refill, show, text, tpl} from './el.js';

const FACTS = tpl`<div class="char-facts">
  <section class="char-fact" data-ref="recentBox"><h2 class="char-fact-title" data-ref="recentTitle"></h2><div class="char-recent" data-ref="recent"></div></section>
  <section class="char-fact" data-ref="curveBox"><h2 class="char-fact-title" data-ref="curveTitle"></h2><div class="char-curve" role="img" data-ref="curve"></div>
    <p class="char-curve-axis"><span data-ref="from"></span><span data-ref="to"></span></p></section>
  <section class="char-fact" data-ref="peerBox"><h2 class="char-fact-title" data-ref="peerTitle"></h2><ul class="char-peers" data-ref="peers"></ul></section>
</div>`;

// 月份「2026-03」写成本地的年月（core/format.js 的 monthText）。
const monthLabel = m => { const [y, mo] = String(m).split('-').map(Number); return monthText(y, mo); };

function recentTile(item) {
  const b = make('button', 'char-recent-tile');
  attr(b, 'type', 'button'); attr(b, 'aria-label', t('看详情'));
  const img = make('img'); attr(img, 'alt', ''); attr(img, 'loading', 'lazy'); attr(img, 'src', imageUrl(item.uuid, 'medium'));
  b.append(img); b.onclick = () => inspect(item.uuid);
  return b;
}
function peerLink(c) {
  const a = make('a', 'char-peer');
  attr(a, 'href', charHash(c.id)); attr(a, 'title', t('{n} 张', {n: number(c.count)}));
  const img = make('img', 'avatar'); attr(img, 'alt', ''); attr(img, 'loading', 'lazy');
  if (c.cover) attr(img, 'src', imageUrl(c.cover));
  a.append(img, make('span', '', c.name));
  const li = make('li'); li.append(a);
  return li;
}

/** 三组资料的部件：{el, update(data)}。data 是 core/character.js 的 loadCharacter 的结果；没读到之前是 null（三组都藏）。 */
export function charFacts() {
  const {el, refs} = FACTS();
  text(refs.recentTitle, t('最近加入')); text(refs.curveTitle, t('收集曲线')); text(refs.peerTitle, t('同作品'));
  function update(data) {
    const recent = data?.recent || [], months = data?.months || [], peers = data?.peers || [];
    show(refs.recentBox, recent.length > 0);
    refill(refs.recent, recent.map(x => x.uuid).join(','), () => recent.map(recentTile));
    const peak = Math.max(0, ...months.map(m => m.count));
    show(refs.curveBox, peak > 0);
    refill(refs.curve, months.map(m => m.month + ':' + m.count).join(','), () => months.map(m => {
      const bar = make('span', m.count ? 'char-bar' : 'char-bar is-zero');
      css(bar, 'height', `${peak ? Math.max(4, Math.round(m.count / peak * 100)) : 4}%`);
      attr(bar, 'title', `${monthLabel(m.month)} · ${t('{n} 张', {n: number(m.count)})}`);
      return bar;
    }));
    attr(refs.curve, 'aria-label', t('最近 12 个月每月加入的张数'));
    text(refs.from, months.length ? monthLabel(months[0].month) : '');
    text(refs.to, months.length ? monthLabel(months[months.length - 1].month) : '');
    show(refs.peerBox, peers.length > 0);
    refill(refs.peers, peers.map(c => c.id + ':' + c.cover + ':' + c.name).join(','), () => peers.map(peerLink));
    show(el, recent.length > 0 || peak > 0 || peers.length > 0);
  }
  update(null);
  return {el, update};
}
