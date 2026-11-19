// 图库（#706）：网格视图的页头和说明，日历视图的占位，以及这几页空了时的去处。按天视图在 ui/home.js（C1 #707 重做），
// 网格本身在 ui/grid.js（C2 #708），日历由 C2 写进 ui/calendar.js 后替换这里的占位。角色页的页头还在 ui/shell.js 的 drawCharHeading，
// 由角色页那一步（G #712）搬进 ui/character.js。三种视图之间的切换画在页头上（ui/shell.js 按注册表的 view）。
import {t} from '../core/i18n.js';
import {favoritesOnly} from '../core/favorites.js';
import {implement, pageOf} from '../core/pages.js';
import {state} from '../core/state.js';
import {$, showArea} from './dom.js';
import {drawEmpty} from './empty.js';
import {conditionHelp, conditionTitle} from './filters.js';

// ---------- 日历视图：占位，C2（#708）换成 ui/calendar.js ----------
function renderCalendar() {
  showArea();
  drawEmpty($('calendarPage'), {icon: 'calendar', fact: t('日历视图还没做好。'), next: [{label: t('按天看'), href: '#/days'}]});
}

// 空了时的去处：「只看 R18」时给「显示全部」（#386 的 2-16）。
const showAll = () => state.nsfw === 'only' ? [{label: t('显示全部'), act: 'nsfw'}] : [];
// 收藏（只按收藏筛选的图库，#679）空着时说怎么收藏。隐藏或只看 R18 时空着不等于没有收藏，照图库的说法。
const libraryEmpty = pageOf({page: 'library'}).empty;

implement({
  // 图库的标题：只按收藏筛选时写「收藏」（#679）；只有一个条件时写这个条件（ui/filters.js，旧的单值参数和条件数组一样，#644）。
  library: {heading: (r, {n}) => [favoritesOnly(r) ? t('收藏') : conditionTitle(r) || t('图库'), t('{n} 张', {n})],
    help: r => favoritesOnly(r) ? [t('在详情面板或大图里点心形，或者按 F 收藏。')] : conditionHelp(r), helpHint: true,
    // 图库页不因计数变化重画。
    onMeta: () => {},
    emptyNext: showAll, emptyIcon: r => favoritesOnly(r) ? 'heart' : 'image',
    empty: (r, ctx = {}) => favoritesOnly(r) && !ctx.nsfw ? t('还没有收藏。在详情面板或大图里点心形，或者按 F 收藏。') : libraryEmpty(r, ctx)},
  calendar: {render: renderCalendar, heading: () => [t('图库'), '']},
  character: {emptyNext: showAll},
  like: {emptyNext: showAll},
});
