// 页面注册表（#342）：每个页面一个声明，其余代码从这里读「这是哪类页面」，不再各自写页面名判断。
// 这里只有数据和不碰 DOM 的小函数：地址形态、主区、点击语义、侧栏高亮归属、子页签、工具栏组成、后端 pool 参数、
// 空状态文案、底栏提示、选择栏种类。需要 DOM 或 ui/ 的部分（渲染入口、标题、数据到达后的重画）由 ui/ 的各模块
// 在加载时用 implement() 登记进来，所以 core/ 不反向依赖 ui/。
// 新增一个页面要做什么，见 docs/WEB_UI_MAP.md「新增一个页面」。
import {isMac} from './format.js';
import {t} from './i18n.js';
import {count as conditionCount, queryParams} from './filters.js';
import {likeParams} from './like.js';

// 字段：
//   prefix     地址里的前缀，#/g/<id>、#/s/<id>；没有前缀的页面用页面名作路径（#/unmatched）
//   sections   地址可以带子页（#/characters/works、#/settings/library）
//   root       不带路径也不带参数的地址（#/）落在这页
//   area       主区：'grid' 或 index.html 里的某个面板 id
//   panel      不用 #grid 的页面：reload()、R、同步到达新图、R18 切换后的重绘都按它判断
//   keys       单图快捷键在这页的范围：'grid' 网格页全部生效（主区是 #grid 的页面不写也是它）；'shown' 只对详情栏或大图里
//              那张图生效，没有网格光标（首页）；主区不是网格、也没写的页面不响应（keyScope）
//   click      点卡片做什么：'browse' 打开详情（选择要主动进入）、'cleanup' 切换选中、'own' 页面自己处理
//   entry      属于活页条的哪个一级入口（core/rail.js 的 ENTRIES）：'library' 图库、'catalog' 资料库、'galleries' 画廊、'tidy' 整理、
//              'settings' 设置（底部的齿轮）；'' 哪个都不亮（画师抽查，入口在设置里）
//   item       在那个入口的二级层里选中哪一项的键（整理的五个队列）；随地址变的（某个画廊、钉住的角色）由 core/rail.js 的
//              railAt 按地址算，不写在这里
//   view       图库的三种视图：'days' 按天、'grid' 网格、'calendar' 日历（页头的分段切换按它画）
//   filters    读图库的筛选参数（has、not、from、to 和旧的单值参数）：图库三种视图和画廊；切换视图时这些参数跟着走
//   emptyIcon  空状态的大图标（ui/empty.js 的 ICONS 的键）；不写是「图片」
//   lead       未识别一类的「待认」页（全部未识别、已跳过、组页）：排序偏好共用、默认可按「有线索优先」排；在这些页上设角色之后
//              那张图会离开本页（详情栏、大图、卡片「+ 角色」建议都先飞走再消失）
//   skip       能跳过当前这张的页面（全部未识别、组页）：详情栏对还没有角色的图显示「跳过 S」，不选图时 S 跳过当前这张
//   tabs/tab   同一个侧栏入口下的子页签组，以及本页对应哪个页签
//   toolbar    工具栏组成：'none' 空、'tabs' 只有子页签、'index' 资料库、'library' 图库（搜索和筛选）、'grid' 网格页
//   sorts      排序下拉的选项
//   sortPref   改了排序记在哪个偏好里（core/route.js 的 sortKey）；不写就是这页自己的「sort.<页面名>」
//   pool/params 后端 /api/assets 的 pool 参数，以及该页其余查询参数
//   bar        选择栏的种类；同时决定 S／X／Y／N 几个快捷键在这页是否生效
//   card       卡片上额外的角标
//   directory  需要完整的角色目录（不是摘要行）
//   guideTick  进入本页算上手清单的哪一步
//   gallery    画廊页（#445）：地址总带着条件，不带查询时按保存的条件打开
const NEWEST = ['newest', 'oldest'];
const narrowed = r => !!(r.q || r.tag || r.utag || r.check || r.doubt || conditionCount(r) || r.from || r.to);

// 图库和角色页共用的一组字段：角色页是图库加 char 参数，不是独立页面。
const libraryBase = {
  area: 'grid', panel: false, entry: 'library', view: 'grid', filters: true, toolbar: 'library', sorts: NEWEST,
  params: r => {
    const f = {};
    if (r.day) f.day = r.day; if (r.work) f.work = r.work; if (r.artist) f.artist = r.artist; if (r.q) f.q = r.q; if (r.tag) f.tag = r.tag; if (r.utag) f.utag = r.utag;
    return Object.assign(f, queryParams(r), likeParams(r));
  },
  // 「只看 R18」时没有图，多半是这个筛选造成的：写明，旁边给「显示全部」（#386 的 2-16）。
  empty: (r, {nsfw} = {}) => nsfw === 'only' ? t('「只看 R18」开着，没有符合条件的 R18 图片。') : t('没有符合条件的图片。'),
  hint: () => t('点击查看 · 拖动或 {key} 点击多选 · Space 看大图', {key: isMac ? '⌘' : 'Ctrl'}),
  bar: 'default',
};

const list = [
  // 图库的三种视图（#706）：按天（原来的首页，#677）、网格（原来的图库）、日历。打开 App 落在按天。
  {name: 'days', root: true, area: 'homePage', panel: true, keys: 'shown', click: 'own', entry: 'library', view: 'days', filters: true, toolbar: 'none', emptyIcon: 'image'},
  {name: 'library', ...libraryBase, click: 'browse', sortPref: 'sortLibrary'},
  {name: 'calendar', area: 'calendarPage', panel: true, click: 'own', entry: 'library', view: 'calendar', filters: true, toolbar: 'none', emptyIcon: 'calendar'},
  // 画廊（#445）：全部画廊的封面卡片，和一个画廊里的图（就是带着它的条件的图库）。
  {name: 'galleries', area: 'groupsPage', panel: true, click: 'own', entry: 'galleries', toolbar: 'none', emptyIcon: 'gallery'},
  {name: 'gallery', prefix: 'gallery', ...libraryBase, view: '', click: 'browse', entry: 'galleries', sortPref: 'sortLibrary', gallery: true},
  {name: 'characters', sections: true, area: 'charIndex', panel: true, click: 'own', entry: 'catalog', toolbar: 'index', directory: true, guideTick: 'look', emptyIcon: 'person'},
  {name: 'settings', sections: true, area: 'settingsPage', panel: true, click: 'own', entry: 'settings', toolbar: 'none'},
  // 整理（#706）：未识别、重复、已排除三个页签（tabs 是页签组），随机抽查、身份整理有题时出现在二级层。各页签原来的子页保留。
  {name: 'unmatched', sortPref: 'sortUnmatched', area: 'grid', panel: false, click: 'cleanup', entry: 'tidy', item: 'unmatched', lead: true, tabs: 'unmatched', tab: 'all',
    toolbar: 'grid', sorts: ['hinted', ...NEWEST], pool: 'unmatched', bar: 'unmatched', skip: true, emptyIcon: 'check',
    empty: () => t('没有未识别的图。'), hint: () => t('点击或拖动选择 · Space 看大图 · C 设为角色 · S 跳过'), card: {hintMin: 70}},
  {name: 'skipped', sortPref: 'sortUnmatched', area: 'grid', panel: false, click: 'cleanup', entry: 'tidy', item: 'unmatched', lead: true, tabs: 'unmatched', tab: 'skipped',
    toolbar: 'grid', sorts: ['hinted', ...NEWEST], pool: 'skipped', bar: 'skipped', emptyIcon: 'check',
    empty: () => t('没有跳过的图片。'), hint: () => t('点击或拖动选择 · Space 看大图')},
  {name: 'groups', area: 'groupsPage', panel: true, click: 'own', entry: 'tidy', item: 'unmatched', tabs: 'unmatched', tab: 'groups', toolbar: 'tabs', emptyIcon: 'check'},
  {name: 'group', prefix: 'g', sortPref: 'sortUnmatched', area: 'grid', panel: false, click: 'cleanup', entry: 'tidy', item: 'unmatched', lead: true, tabs: 'unmatched', tab: 'groups',
    toolbar: 'grid', sorts: ['hinted', ...NEWEST], bar: 'unmatched', skip: true, emptyIcon: 'check',
    pool: 'cluster', params: r => ({cluster: r.id}), empty: () => t('这一组都整理完了。'),
    hint: () => t('点击或拖动选择 · Space 看大图 · C 设为角色 · S 跳过')},
  {name: 'duplicates', area: 'grid', panel: false, click: 'browse', entry: 'tidy', item: 'duplicates', tabs: 'duplicates', tab: 'all',
    toolbar: 'grid', sorts: NEWEST, pool: 'duplicates', bar: 'default', card: {dup: true}, emptyIcon: 'check',
    params: r => r.dup ? {dup: r.dup} : {}, empty: r => r.dup ? t('所有副本的角色都一致。') : t('没有重复的图片。'),
    hint: () => t('同色角标是同一组')},
  {name: 'similar', area: 'groupsPage', panel: true, click: 'own', entry: 'tidy', item: 'duplicates', tabs: 'duplicates', tab: 'similar', toolbar: 'tabs'},
  {name: 'sgroup', prefix: 's', area: 'grid', panel: false, click: 'cleanup', entry: 'tidy', item: 'duplicates', tabs: 'duplicates', tab: 'similar',
    toolbar: 'grid', sorts: NEWEST, pool: 'similar', params: r => ({cluster: r.id}), bar: 'default', empty: () => t('这一组已经没有图片了。'), emptyIcon: 'check',
    hint: () => t('点击或拖动选择 · Space 看大图')},
  {name: 'excluded', area: 'grid', panel: false, click: 'cleanup', entry: 'tidy', item: 'excluded', tabs: 'excluded', tab: 'all',
    toolbar: 'grid', sorts: NEWEST, pool: 'excluded', bar: 'excluded', card: {reason: true}, emptyIcon: 'check',
    params: r => r.reason ? {reason: r.reason} : {}, empty: () => t('没有被排除的图片。'), hint: () => t('点击或拖动选择 · Space 看大图 · X 恢复收录')},
  {name: 'pending', area: 'grid', panel: false, click: 'cleanup', entry: 'tidy', item: 'excluded', tabs: 'excluded', tab: 'pending', toolbar: 'grid', sorts: NEWEST, pool: 'pending',
    bar: 'pending', emptyIcon: 'check', empty: () => t('没有待确认的图片。'), hint: () => t('点击或拖动选择 · Space 看大图 · X 排除')},
  {name: 'audit', area: 'grid', panel: false, click: 'cleanup', entry: 'tidy', item: 'audit', toolbar: 'grid', sorts: NEWEST, pool: 'audit', bar: 'audit', card: {audit: true},
    emptyIcon: 'check', empty: () => t('抽查做完了。'), hint: () => t('点一张或 Space 看大图 · Y 是 · N 不是 · 看不清可以跳过，换一张')},
  {name: 'identity', area: 'groupsPage', panel: true, click: 'own', entry: 'tidy', item: 'identity', toolbar: 'none'},
  {name: 'artist-audit', area: 'groupsPage', panel: true, click: 'own', entry: '', toolbar: 'none'},
];

// 不再有的页面的旧地址 → 现在落在哪一页。新加入（#/recent）在 2026-10-02 由首页接手，页面本身在 #353 删掉；首页（#/home）
// 在 0.4 并进图库，是图库的按天视图（#706）；#/tidy 是整理的入口，落在未识别的按组整理（和二级层「未识别」去的地方一样）。
const RETIRED = {recent: 'days', home: 'days', tidy: 'groups'};

// 角色页：不是 route.page，而是 library 加 char 参数。点击即选择；排序多「可疑优先」；旧地址 #/c/<id>/<皮肤> 改写成它。
// 活页条上它属于资料库（角色在那里），不是图库；页头没有图库的视图切换。
const character = {name: 'character', virtual: true, legacyPrefix: 'c', ...libraryBase, entry: 'catalog', view: '', click: 'cleanup', sorts: ['suspicious', ...NEWEST], sortPref: 'sortCharacter',
  empty: (r, ctx = {}) => narrowed(r) || ctx.nsfw === 'only' ? PAGES.library.empty(r, ctx) : t('这个角色已经没有图片了。'),
  hint: () => t('点击或拖动选择 · Space 看大图 · ⌫ 不是 TA · C 换成')};

// 「更多像这张的」（#449）：也不是 route.page，而是 library 加 like（例图）参数，见 core/like.js。点击照常打开详情；排序多
// 「最相似」并默认是它；没有结果时，有别的筛选就照图库说，否则说没找到。#427 的「看过了」角标和按钮在这里另加 card、bar。
const like = {name: 'like', virtual: true, ...libraryBase, click: 'browse', sorts: ['similar', ...NEWEST], sortPref: 'sortLike', sortDefault: 'similar',
  empty: (r, ctx = {}) => narrowed(r) || r.work || r.artist || r.day || ctx.nsfw === 'only' ? PAGES.library.empty(r, ctx) : t('没有找到相似的图。')};

export const PAGES = Object.fromEntries(list.map(p => [p.name, p]));
export const CHARACTER = character;
export const LIKE = like;
const VIRTUAL = {character, like};
// 路由里的页面名没登记时（只有测试会这样）按没有特殊性质的网格页处理。
const UNKNOWN = {name: '', area: 'grid', panel: false, click: 'own', entry: '', toolbar: 'grid', sorts: NEWEST, bar: 'default'};
export const pageOf = route => PAGES[route.page] || UNKNOWN;
// 主区的全部种类（index.html 里各有一个元素），ui/dom.js 的 showArea 按它切换。
export const AREAS = [...new Set(list.map(p => p.area))];
// 屏幕上看到的那一页：带例图的是相似结果，角色筛选下是角色页，否则就是 route.page。char 默认取路由的 char；渲染标题和元数据重画时
// 要看角色有没有读到，传 state.char。
export const viewOf = (route, char = route.char) => route.like ? LIKE : char ? CHARACTER : pageOf(route);
// 单图快捷键在这页的范围（见上面的 keys 字段）：'grid'、'shown' 或 ''。
export const keyScope = view => view.keys || (view.area === 'grid' ? 'grid' : '');
// 本页的后端 pool 是不是这几个之一（条目离开本页的判断：设为角色之后离开未识别、排除之后离开已排除……）。
// 异步回应回来时：用户是不是还在（启动它的）这一页。守卫用它，不要在别处直接比较页面名（tools/check_frontend.py 规则 6）。
export const onPage = (route, ...names) => names.includes(route.page);
export const poolIs = (route, ...pools) => pools.includes(pageOf(route).pool);

// ui/ 登记页面的处理函数：{页面名: {render, refresh, heading, ...}}；character、like 是上面两个视图。
//   render({samePage, restore, scope})  进入本页时画主区；没有的页面是网格页，由 shell 调 reload(true)。scope 是这一页的
//                        作用域（ui/scope.js 的 page()）：监听、计时器、部件都登记在它上面，换页时一起释放（docs/WEB_V2.md「页面样板」）
//   subTabs(r)           页签行（整理三个页签下面的子页签）的 HTML；工具栏是 'tabs' 或 'grid' 的页面放在工具栏开头
//   prepare(route)       进入本页前要先读的数据；返回 false 表示已经导航走了
//   heading(r, ctx)      [标题, 副标题, 右侧链接 HTML]；角色页另用 drawHeading。副标题只放数量、状态和必须一眼看到的警告
//   wireHeading(h, r)    heading 画好之后给右侧的按钮挂上处理函数（相似组页的「看过了」）
//   help(r), helpHint    标题旁「?」里的说明（一行一句）；helpHint 为真时再接上本页的 hint，一个「·」一行（#527）
//   onMeta(info)         计数等元数据到达后重画什么；没有的页面重画工具栏
//   emptyNext(r)         网格空了时空状态下面的去处（#386 的 2-12）：[{label, href}]（链接）或 [{label, act}]（按钮），
//                        可以有几个；没有的页面只有图标和一句话（ui/empty.js 画）
//   emptyAct(act)        点了 emptyNext 里 act 的按钮（「显示全部」由 shell 自己处理）
//   readFailed(e)        网格读取失败时：返回一句话就当作没有结果、把它写进空状态，不弹错误条（相似结果的例图没有特征、
//                        画廊的条件失效）
//   failedNext(r)        readFailed 认下的失败在空状态下面给的去处，写法同 emptyNext（画廊的「删除画廊」）
//   reveal(uuid)         主区不是网格的页面把这张图滚进视野：详情栏、大图翻页时，和面板页借列表看的大图关掉时（#424）
//   refresh(why)         数据变了之后重画面板页：'edit' 改动之后、'undo' 撤销之后、'learned' 确认角色或同步结束后后台学习
//                        收尾之后（#400、#404）、'sync' 同步到新图之后、'nsfw' 切换 R18 之后、'view' 视图选项之后、
//                        'ask' 「偶尔问一句」开关之后
export function implement(hooks) {
  for (const [name, h] of Object.entries(hooks)) {
    const page = VIRTUAL[name] || PAGES[name];
    if (!page) throw new Error(`没有登记过的页面：${name}`);
    Object.assign(page, h);
  }
}

// ---------- 地址 ----------
// 把 #/ 之后的路径段读成页面：{page, id?, section?}、旧角色页的 {char, variant, legacy} 或已删页面的 {page, legacy}；
// legacy 的地址由 applyRoute 就地改写。认不出的是图库。
export function matchPath(parts, hasQuery) {
  if (!parts.length && !hasQuery) return {page: list.find(p => p.root).name};
  const [head, next] = parts;
  for (const p of list) {
    if (p.prefix) { if (head === p.prefix && next) return {page: p.name, id: decodeURIComponent(next)}; continue; }
    if (head !== p.name || p === PAGES.library) continue;   // 图库是认不出地址时的默认，不用自己的名字匹配
    return p.sections ? {page: p.name, section: parts[1] || ''} : {page: p.name};
  }
  if (Object.hasOwn(RETIRED, head)) return {page: RETIRED[head], legacy: true};
  if (head === character.legacyPrefix && next) return {char: decodeURIComponent(next), variant: parts[2] ? decodeURIComponent(parts[2]) : '', legacy: true};
  return {};
}
// 路由对应的路径（不含查询）。
export function pathFor(r) {
  const p = PAGES[r.page];
  return p?.prefix ? `#/${p.prefix}/${encodeURIComponent(r.id)}` : '#/' + r.page + (r.section ? '/' + r.section : '');
}
