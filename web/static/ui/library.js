// 图库（#706）：网格视图的页头和说明，以及这几页空了时的去处。按天视图在 ui/days.js（#707），
// 网格本身在 ui/grid.js（#708），日历在 ui/calendar.js（#708），角色页的页头和横幅在 ui/character.js（#712）。三种视图之间的
// 切换画在页头上（ui/shell.js 按注册表的 view）。
import {t} from '../core/i18n.js';
import {implement} from '../core/pages.js';
import {state} from '../core/state.js';
import {conditionHelp, conditionTitle} from './filters.js';

// 空了时的去处：「只看 NSFW」时给「显示全部」（#386 的 2-16）。空状态的文字和图标在注册表里（core/pages.js 的 library）。
const showAll = () => state.nsfw === 'only' ? [{label: t('显示全部'), act: 'nsfw'}] : [];

implement({
  // 图库的标题：只有一个条件时写这个条件（ui/filters.js，旧的单值参数和条件数组一样，#644）。收藏是画廊（#710）。
  // 还是旧写法 headingHTML：conditionTitle 返回带名字占位的 HTML（读到名字后由 ui/filters.js 换上），C2 把它改成节点时再迁到 head。
  library: {headingHTML: (r, {n}) => [conditionTitle(r) || t('图库'), t('{n} 张', {n})],
    help: r => conditionHelp(r), helpHint: true,
    // 图库页不因计数变化重画。
    onMeta: () => {}, emptyNext: showAll},
  character: {emptyNext: showAll},
  like: {emptyNext: showAll},
});
