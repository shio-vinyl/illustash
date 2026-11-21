// 每月张数的柱子（详情卡片的角色卡 #709、角色页的收集曲线 #712）：一个月一根 <i>，高度按这几个月里最多的那个月算百分比，
// 悬停写「月份 · N 张」。数据是 /api/character/stats 的 months（core/detail.js 的 readCharStats）。月份和张数都没变时不动 DOM。
import {monthLabel, number} from '../core/format.js';
import {t} from '../core/i18n.js';
import {attr, css, make, refill} from './el.js';

/** 把 months（[{month, count}]）画进 node。min 是最矮的柱子占几成（百分比）；mark(m, i) 返回这一根的类（最后一个月、空月……）。 */
export function monthBars(node, months, {min = 8, mark = () => ''} = {}) {
  const peak = Math.max(1, ...months.map(m => m.count));
  refill(node, JSON.stringify(months), () => months.map((m, i) => {
    const bar = make('i', mark(m, i));
    css(bar, 'height', `${Math.max(min, Math.round(m.count / peak * 100))}%`);
    attr(bar, 'title', monthLabel(m.month) + ' · ' + t('{n} 张', {n: number(m.count)}));
    return bar;
  }));
}
