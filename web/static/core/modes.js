// 图片交互的模式（#344）：浏览、选择、详情、大图由 core/state.js 里的几个字段组成。这里把它们读成明确的模式，
// 并把「当前图取哪个来源」「一个快捷键在这一页、这个模式下做什么」各写成一处，键盘处理和单图动作都从这里取。
// 只读 state、不写，不碰 DOM，可以在 Node 里单独测试（tests/modes.test.cjs）。
//
// 四个模式和叠加规则（docs/WEB_UI_MAP.md 第 6 节）：
//   浏览  默认：没有选中、不在选择模式、没有详情栏、没有大图。
//   选择  选中了至少一张（sel 非空），或浏览页进入了选择模式（selecting）还没选。选择栏只在选中数 > 0 时出现。
//   详情  详情栏打开（focused 有值）。可以和「选择」同时在：都在时，单图快捷键先作用于选中的图（选择栏）。
//   大图  大图打开（viewer >= 0）。盖住网格、选择栏和详情栏：快捷键只作用于大图里那张，被盖住的选择栏不响应
//        （选中保留，关掉大图后照旧；在大图里改了那张图之后，和其他写入一样清空选择）。
// 当前图的来源优先级：大图 ＞ 详情栏 ＞ 键盘光标。
import {isMac} from './format.js';
import {t} from './i18n.js';
import {state} from './state.js';

// 选中了几张：全选用「过滤条件 + 排除集」表示，张数由总数减去排除的。
export const selectionSize = (s = state) => s.sel.all ? Math.max(0, s.total - s.sel.exclude.size) : s.sel.ids.size;

// 现在叠着哪几个模式。浏览是其余三个都不在。
export function modes(s = state) {
  const viewer = s.viewer >= 0, detail = !!s.focused, select = s.selecting || selectionSize(s) > 0;
  return {viewer, detail, select, browse: !viewer && !detail && !select};
}
// 最上面的一个模式：键盘先交给它。
export function topMode(s = state) {
  const m = modes(s);
  return m.viewer ? 'viewer' : m.detail ? 'detail' : m.select ? 'select' : 'browse';
}

// 快捷键作用于几张选中的图：大图打开时选择栏被盖住，算 0，快捷键改为作用于大图里那张。
export const keySelection = (s = state) => s.viewer >= 0 ? 0 : selectionSize(s);

// 大图或详情栏里那张图的 uuid（大图优先）；两者都没有时为空。
export const shownUuid = (s = state) => s.viewer >= 0 ? s.viewerItem?.uuid : s.focused;

// 当前图从哪来：大图或详情栏里那张（{from, uuid}），否则键盘光标（{from: 'cursor', index}），都没有为 null。
// 大图刚打开、图还没读到时（viewerItem 为空）退到光标。
export function currentSource(s = state) {
  const uuid = shownUuid(s);
  if (uuid) return {from: s.viewer >= 0 ? 'viewer' : 'focused', uuid};
  return s.cursor >= 0 ? {from: 'cursor', index: s.cursor} : null;
}

// Space 打开大图时打开哪一张（序号）：光标所在的，否则详情栏里那张，否则第一张。和「当前图」的优先级不同：
// 这时还没有大图，光标总是跟着最近一次点的或移到的那张。
export function viewerTarget(s = state) {
  if (s.cursor >= 0) return s.cursor;
  return s.focused ? s.items.findIndex(x => x?.uuid === s.focused) : 0;
}

// 关掉大图时详情栏换到哪一张（#399）：详情栏开着、大图里最后看的不是它时，换成大图那张，这样「刚才看的那张」和
// 「快捷键作用的那张」（详情栏 ＞ 光标）、Space 再打开的那张（光标）是同一张；不用换时为 null。
export const detailAfterViewer = (s = state) => s.focused && s.viewerItem?.uuid && s.viewerItem.uuid !== s.focused ? s.viewerItem.uuid : null;

// 单图／选择快捷键在这一页做什么。key：'remove'（⌫、Delete）、'c'、'a'、't'、'g'、'x'、's'、'f'。
// ctx：view 页面注册表里本页的条目（core/pages.js 的 viewOf），char 是否在角色页，maybe 是否在角色页的「可能也是 TA」，
// variants 本页的角色有没有造型，n 作用于几张选中的图（keySelection），item 当前图（没有选中时用，决定 ⌫ 有没有
// 可移出的角色）。
// 返回 {bar: 动作}（交给选择栏的同名按钮）、{single: 动作}（作用于当前图），或 null：这一页没有对应的动作。
// 有选中时只返回本页选择栏上画着的按钮（ui/grid.js 的 renderSelection，docs/WEB_UI_MAP.md 第 4.1 节），选择栏上
// 没有的键返回 null，由键盘处理提示「这一页没有对应的操作」（#422 的 2-21）。
export function keyAction(key, {view, char = false, maybe = false, variants = false, n = 0, item = null}) {
  // 选择栏里有「设为角色」的页：默认选择栏（图库、重复、相似组）和未识别一类。
  const assigns = ['default', 'unmatched', 'skipped'].includes(view.bar);
  switch (key) {
    // 有选中时 ⌫ 交给选择栏里标着 ⌫ 的按钮：角色页「不是 TA」、默认选择栏（图库、重复、相似组）「不是…」；
    // 别的选择栏没有这个按钮。
    case 'remove':
      if (n) return char ? {bar: 'remove'} : view.bar === 'default' ? {bar: 'notlib'} : null;
      return char || item?.memberships?.length ? {single: 'remove'} : null;
    // C＝换成、A＝追加（用户 2026-11-14 定，#399）：图上已有角色时 C 换掉它，还没有角色时 C、A 都是设为角色。
    // 单图的「换成」在 singleAction 里看这张图有没有角色，没有就退成设为。有选中时：角色页换掉本页的角色；
    // 默认选择栏（图库、重复、相似组）换掉选中的图共同的角色（replace，没有角色时同样退成设为）；未识别一类的图
    // 还没有角色，C 是设为。「可能也是 TA」、随机抽查、已排除、待确认的选择栏上没有这几个按钮。
    case 'c':
      if (!n) return {single: 'swap'};
      if (char) return maybe ? null : {bar: 'swap'};
      return view.bar === 'default' ? {bar: 'replace'} : assigns ? {bar: 'assign'} : null;
    case 'a':
      if (!n) return {single: 'also'};
      if (char) return maybe ? null : {bar: 'also'};
      return assigns ? {bar: 'assign'} : null;
    // 造型按钮只在角色页、这个角色有造型时画出来。
    case 't': return char && n && !maybe && variants ? {bar: 'variant'} : null;
    case 'g': return n ? {bar: 'tag'} : {single: 'tag'};
    // F：收藏（#447）。每个选择栏都有「收藏」按钮；没有选中时切换当前这张。
    case 'f': return n ? {bar: 'fav'} : {single: 'fav'};
    case 'x': { const act = view.bar === 'excluded' ? 'include' : 'exclude'; return n ? {bar: act} : {single: act}; }
    // S：未识别一类是跳过；已跳过页是恢复到未识别，和已排除页的 X 恢复收录对称（#399）。
    case 's':
      if (view.bar === 'skipped') return n ? {bar: 'resume'} : {single: 'resume'};
      if (view.bar !== 'unmatched') return null;
      return n ? {bar: 'skip'} : view.skip ? {single: 'skip'} : null;
    default: return null;
  }
}

// 快捷键表：帮助对话框（index.html 的 #help）由它生成，所以帮助里写的就是这里写的。ui/grid.js 的键盘处理加减按键时，
// 同步改这张表（tests/help_keys.test.cjs 核对两边认的键一样）；每个键在哪一页、哪个模式下做什么的细节见上面的
// keyAction 和 docs/WEB_UI_MAP.md 第 5 节。
//   keys   帮助里显示的键；⌘ 在别的系统上显示成 Ctrl（core/format.js 的 platformText）
//   codes  键盘处理认的 e.key（字母小写）；鼠标、触控板的操作和命令面板里的键为空
//   does   做什么、在哪生效
export const SHORTCUTS = [
  {keys: '⌘K', codes: ['k'], does: () => t('跳转到角色')},
  {keys: () => isMac ? '⌘[ / ⌘]' : 'Alt+← / Alt+→', codes: ['[', ']'], does: () => t('后退、前进（只在这个图库的页面之间；鼠标侧键、标题左边的 ‹ 也可以）')},
  {keys: '/', codes: ['/'], does: () => t('搜索（图库、资料库）')},
  // 活页条（#706）：处理在 ui/rail.js 的 railKey（ui/grid.js 的键盘处理先交给它），所以这里不列 codes。
  {keys: '⌘1–4', codes: [], does: () => t('切到活页条的图库、资料库、画廊、整理')},
  {keys: () => isMac ? '⌥↑ / ⌥↓' : 'Alt+↑ / Alt+↓', codes: [], does: () => t('在活页条的二级层里移到上一项、下一项；资料库里按住 Shift 挪动钉住的项')},
  {keys: () => t('拖动 · Shift 点击'), codes: [], does: () => t('连续选择；在空白处拖动可框选')},
  {keys: '⌘A', codes: ['a'], does: () => t('选中全部符合条件的图片')},
  {keys: () => t('方向键'), codes: ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'],
    does: () => t('移动光标，按住 Shift 同时选中；详情栏打开时换图（↑ ↓ 换到上一行、下一行），大图里 ← → 换图')},
  {keys: '↵', codes: ['Enter'], does: () => t('整理页或选择模式下切换光标处那张的选中，浏览页打开它的详情')},
  {keys: 'Space', codes: [' '], does: () => t('打开或关闭大图；按组整理、首页「认一认」里看焦点所在或鼠标停着的那张')},
  {keys: 'I', codes: ['i'], does: () => t('打开或关闭详情栏（光标那张）；大图里关掉大图，在详情栏里看这张')},
  {keys: '⌫', codes: ['Backspace', 'Delete'], does: () => t('不是这个角色：有选中时作用于选中的图（角色页移出，图库、重复等页选「不是…」），没有选中时移除当前这张的角色')},
  {keys: 'C', codes: ['c'], does: () => t('换成别的角色：图上已有角色时换掉它（角色页换掉本页的角色），还没有角色时设为角色')},
  {keys: 'A', codes: ['a'], does: () => t('追加角色，原有的角色保留')},
  {keys: 'T', codes: ['t'], does: () => t('造型（角色页，选中图片时）')},
  {keys: 'S', codes: ['s'], does: () => t('跳过（「全部未识别」和各组；没有选中时跳过当前这张）；在「已跳过」页为恢复到未识别')},
  {keys: 'X', codes: ['x'], does: () => t('排除非插画内容（截图、照片等）；在「已排除」页为恢复')},
  {keys: 'G', codes: ['g'], does: () => t('添加或移除自定义标签（与自动标签分开保存）')},
  {keys: 'F', codes: ['f'], does: () => t('收藏或取消收藏：有选中时，只要有一张还没收藏就全部收藏，否则全部取消')},
  {keys: '1–9', codes: ['1-9'], does: () => t('选详情栏或大图里「也可能是」的第几项；按住 ⌥（Alt）改为追加')},
  {keys: 'Y / N', codes: ['y', 'n'], does: () => t('回答「是 TA 吗？」：随机抽查页回答选中的图，详情栏或大图里回答问题条')},
  {keys: '⌘+ / ⌘−', codes: ['=', '+', '-'], does: () => t('缩放图片网格；大图打开时缩放大图')},
  {keys: () => t('捏合 · ⌘ 滚动'), codes: [], does: () => t('缩放图片网格；在大图上缩放大图')},
  {keys: 'R', codes: ['r'], does: () => t('切换原比例 / 方形裁切')},
  {keys: 'H', codes: ['h'], does: () => t('隐藏 / 显示 R18 内容（页头的 R18 按钮还可以切换为只看 R18）')},
  {keys: '⌘Z', codes: ['z'], does: () => t('撤销')},
  {keys: 'Esc', codes: ['Escape'], does: () => t('关闭最上面的菜单、对话框或大图，然后是详情栏，最后取消选择；在输入框里先离开输入框')},
  {keys: () => t('命令面板里'), codes: [], does: () => t('↑ ↓ 移动，↵ 选中，Shift ↵ 暂存多个一起设，输入框为空时 1–9 直接选')},
];
