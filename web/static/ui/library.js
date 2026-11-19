// 图库（#706）：网格视图的页头和说明，日历视图的占位，以及这几页空了时的去处。按天视图在 ui/home.js（C1 #707 重做），
// 网格本身在 ui/grid.js（C2 #708），日历由 C2 写进 ui/calendar.js 后替换这里的占位。角色页的页头还在 ui/shell.js 的 drawCharHeading，
// 由角色页那一步（G #712）搬进 ui/character.js。三种视图之间的切换画在页头上（ui/shell.js 按注册表的 view）。
import {t} from '../core/i18n.js';
import {favoritesOnly} from '../core/favorites.js';
import {implement} from '../core/pages.js';
import {viewRoute} from '../core/rail.js';
import {hashFor} from '../core/route.js';
import {state} from '../core/state.js';
import {$} from './dom.js';
import {drawEmpty} from './empty.js';
import {conditionHelp, conditionTitle} from './filters.js';

// ---------- 日历视图：占位，C2（#708）换成 ui/calendar.js ----------
// 主区已经由 applyRoute 切好（showArea），这里只画内容。「按天看」带着现在的筛选。
function renderCalendar() {
  drawEmpty($('calendarPage'), {icon: 'calendar', fact: t('日历视图还没做好。'), next: [{label: t('按天看'), href: hashFor(viewRoute(state.route, 'days'))}]});
}

// 空了时的去处：「只看 NSFW」时给「显示全部」（#386 的 2-16）。空状态的文字和图标在注册表里（core/pages.js 的 library）。
const showAll = () => state.nsfw === 'only' ? [{label: t('显示全部'), act: 'nsfw'}] : [];

implement({
  // 图库的标题：只按收藏筛选时写「收藏」（#679）；只有一个条件时写这个条件（ui/filters.js，旧的单值参数和条件数组一样，#644）。
  // 还是旧写法 headingHTML：conditionTitle 返回带名字占位的 HTML（读到名字后由 ui/filters.js 换上），C2 把它改成节点时再迁到 head。
  library: {headingHTML: (r, {n}) => [favoritesOnly(r) ? t('收藏') : conditionTitle(r) || t('图库'), t('{n} 张', {n})],
    help: r => favoritesOnly(r) ? [t('在详情面板或大图里点心形，或者按 F 收藏。')] : conditionHelp(r), helpHint: true,
    // 图库页不因计数变化重画。
    onMeta: () => {}, emptyNext: showAll},
  calendar: {render: renderCalendar, head: () => ({title: t('图库')})},
  character: {emptyNext: showAll},
  like: {emptyNext: showAll},
});
