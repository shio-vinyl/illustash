# 本地图库 V2

状态：现行参考，2026-11-12；「前端整体升级」一节是进行中的方案（2026-11-19，#703），其中「渲染底座」一小节已是现行参考（#704）。本文记录图库数据模型、API 与浏览界面；原生入口及核心架构见 [ARCHITECTURE_DIRECTION.md](ARCHITECTURE_DIRECTION.md)。

## 浏览界面

图库提供正方形裁切和原始比例两种模式。原始比例按目标行高自动分行：目标行高取当前缩放档位下正方形格宽的 4/3（3:4 竖图与方格同宽），每行依次放入图片，直到再放一张会超出宽度时，选更接近目标行高的分法，然后整行等高拉满可用宽度。末行不拉满，保持目标行高并左对齐，结果只有一两张时也不会放大成巨图。版式按宽高比 0.5–2.4 计算，超出范围的全景图或长条图在格子内完整显示（留底色），不裁切。

卡片（0.4，#708）：图满格、圆角 8，下面没有标题条。图读完之前是这张图的主色（#717），悬停时上浮一点、投影带图的颜色；悬停或键盘光标在上面时底部压暗一条写角色名：一位写名字，几位写第一位加「另外 N 位」，宽不到 130px 的卡片只写「N 位」；宽不到 72px，或者左下有芯片（造型、「可能是」、抽查标签）时不写。选中、键盘光标、详情栏里那张用「选中」画法（强调色环，选中的再缩到 95%）。卡片是 `ui/grid_card.js` 的部件，行坐标在 `ui/grid_rows.js`，网格本身（挂哪些卡片、选择、键盘、缩放）在 `ui/grid.js`，选择栏在 `ui/grid_bar.js`；网格的取数（查询参数、分页、计数）在 `core/assets.js`。网格顶部可以放一块跟着滚动的头部（`ui/grid.js` 的 `gridHead()`，角色页的横幅用，#712）：它的高度算进行坐标，卡片整体往下挪。

日历视图（#708，`ui/calendar.js`，数据在 `core/calendar.js`）：一个月一页，看哪个月由地址的 `at` 定（没有时是本月），翻月换 `at`，后退能回到上一个月。月份标题写月、年、张数和几天有新图，背后是这个月的环境光（当月第一个有颜色的日子），旁边是这个月样图里出现最多的几位角色的头像；‹ › 去筛选后最近有图的月份。每天一格，按张数拼 1、2 或 4 张（`core/layout.js` 的 `cellTiles`），底色是当天的颜色；今天有强调色环；点一天去按天视图的那一天（`#/days?at=…`，带着筛选）。只看不选，方向键在有图的格子之间挪。这个月没有图时格子收起来，空状态给上一个、下一个有图的月份。

未加载图片按已加载图片的平均比例估算（尚无数据时用 3:4）。视口上下各约一屏的分页会提前读取，尺寸通常在进入视口前到达；行重新分配时以视口顶部那张图为锚点保持位置。直接拖动滚动条跳到未加载的深处时，可见行在数据到达后仍会重新排一次，这是不预取全量尺寸的代价。虚拟滚动、缩放定位、上下方向键（移到相邻行中水平距离最近的图）与框选共用同一套行坐标。行起点、行顶和行高缓存在复用的类型数组中，按位置或序号查行都用二分查找；仅保留视口附近的卡片（摘掉时放掉解码后的位图，#683），卡片按图复用，分页返回或写入后只改变了的地方。正方形模式可从工具栏切回，图片与数据格式不变。换视图（按天、网格、日历）时的淡入只有外壳一处（`ui/shell.js` 的 `applyRoute` 让 `#viewport` 淡入），各页不另做；缩放时卡片从旧的框滑到新的框（`ui/motion.js` 的 `flip`）。

图库的按天视图（`#/days`，原来的首页，#707）一天一段：大字日期、星期和张数，当天的角色芯片（一行放得下几个放几个，其余收成「另外 N 位」，点开换行显示全部），马赛克（`core/layout.js` 的 `dayMosaic`：一张大图高两行，右边两行小图，放不下的收成最后一格上的「+N 张」）。只挂视口附近的几天（`ui/virtual.js`），滚远的天摘掉、图片放掉位图；接近底部接着读更早的几天，依次淡入，读完写「最早的一张在 X 加入 · 共 N 张」。同步进来新图时只重读最新的两天，没变的天不动；「今天」那一段已经显示的图不动、不被挤进「+N」，每来一批新图就在下面多长一行（新图淡入），最多多长两行，再来的新图只加大「+N 张」，离开页面、刷新或窗口变宽变窄以后再按标准的马赛克排（用户 2026-11-20 定，#707）。大图在本页期间固定，缩略图档位只升不降，图挪位置时地址不变。改动、撤销之后重读视野附近的几天，重读范围里没了的天（筛选下最后一张被移走）去掉。读取期间来的刷新排队，读完再补一次；换了筛选、搜索或 NSFW 时取消还在路上的读取，晚到的回复按查询条件核对后丢掉。顶上两张卡是同步状态（点开任务面板）和「认一认」：收起时几张小图和问句，「不全是」展开后点掉不是 TA 的；「点掉的是…」用角色选择器把点掉的那几张一步记成「不是 TA、是选中的角色」（`/api/ask/answer` 的 `switch`，后端要求这一步不带「是」的图，所以其余几张留在卡上），一个撤销（#675）。认一认的写入成功以后才换下一批，写的时候这一批锁住；没写成时卡片和点选留着，可以再点一次。按天视图只看不选：点图开详情，双击看大图，多选到网格里做（「在网格里看 ›」带着筛选和这一天）。打开应用默认进入按天视图（用户 2026-10-02 决定打开进首页）：不带路径也不带参数的 `#/` 是它；只带参数的旧链接（如 `#/?artist=<id>`）仍打开网格。

机器识别默认计入角色归属；人工确认和人工移除继续独立保存。没有修改源模型分数，也没有将全部候选写成人工确认。

### 筛选条与画廊（2026-11-17，#445）

图库、角色页和画廊页的工具栏在搜索框后面是筛选条（`ui/filters.js`，条件的读写在 `core/filters.js`）：每个条件一枚胶囊，点名字在「含」和「不含」之间切换，× 去掉；日期一枚。「＋ 条件」打开一个浮层，一个输入框跨种类搜画面标签（中文页面显示 #446 的中文名）、我的标签、角色、作品、画师、出处和年份，另有收藏和「起止日期…」。条件写进地址：`has`、`not` 是和接口相同的 JSON 数组，`from`、`to` 是日期。详情栏点画师、作品、标签带来的旧单值参数（`tag`、`utag`、`work`、`artist`、`day`）照样显示在筛选条上，在筛选条上一改就整体改写成 `has`。

有条件或搜索词时工具栏出现「保存为画廊」：一键保存，名字由前端按胶囊上的文字生成（所以跟着界面语言），随后进入新画廊、标题处于改名状态。存下的是 `has`、`not`、`from`、`to`、`day`、`q`；在角色页上存时角色（或造型）也是一个 `char:` 条件。排序和 NSFW 不存。

侧栏「图库」下面的「画廊」只在有画廊时显示，进 `#/galleries`：封面卡片，按保存的顺序，封面按当前 NSFW 筛选批量读（`/api/galleries/covers`，#590；没有这个接口时退回读带封面的列表），张数逐个读 `/api/assets/count`。`#/gallery/<id>` 是带着保存条件的图库，点击是浏览；不带查询打开时按保存的条件，并把条件写进地址。改了条件后页头出现「更新画廊」「另存为新画廊」；标题点一下改名；「⋯」里有设封面（用详情栏里那张）、封面改回自动、往前移、往后移、删除。每一步都进撤销栈。保存的条件失效时（引用的作品、标签没了），卡片写「条件失效」；画廊页的网格读取回 400，空状态写「条件失效：{后端的原因}」，下面给「删除画廊」，不弹错误条，页头不另写（#647）。用户在筛选条上去掉那个条件后更新画廊。

## 前端模块（2026-10-18，#199）

页面脚本是浏览器原生 ES 模块，没有构建步骤：`index.html` 用 `<script type="module" src="/app.js">` 载入入口，`i18n.js`、`splash.js`、`theme.js` 仍是普通脚本，先于它执行。`i18n.js` 保持普通脚本，因为桌面外壳的启动页和 `splash.js` 也用它；模块从 `core/i18n.js` 导入 `t()`、`tb()`、`said()`、`I18N`，实现只在 `i18n.js` 一处。`app.js` 只是入口：导入各模块，把写入和任务回复的连带影响（`ui/effects.js`）接到 `onPost()`、`onJob()`，按原来的顺序调用各 `wire*()` 注册事件，然后启动页面（读会话与计数、打开地址指向的页面）。模块载入时不做事，事件都在 `wire*()` 里注册。每个模块开头的注释写它负责什么。

- `web/static/core/`：任何布局都能复用的一层（以后的移动端也用），不引用 `document`、`$()` 和元素 ID，可以直接在 Node 里导入。`format.js`（转义、数字、日期、快捷键名）、`keys.js`（键盘事件的公共判断，现在只有 `composing(e)`：输入法是否在组字，#388）、`prefs.js`（浏览器本地偏好）、`session.js`（`api()` / `post()`、会话令牌与过期：页面只属于启动时的会话，见 #33）、`edits.js`（最近一步可撤销的操作、撤销，以及「同一时间只做一项修改」的锁）。会话令牌和过期标记只由 `session.js` 写，撤销状态只由 `edits.js` 写；界面通过 `onStale()`、`onPost()`、`onLatest()` 得知变化，各返回一个取消监听的函数。
  `models.js`：识别模型（#168）的状态、下载进度轮询和请求，整个 App 共用；`onModels(before)` 在每次读取或改动后通知界面。
  `jobs.js`：后台任务（同步、检查新角色、角色名称、相似图片、找回画师、导入）的轮询、状态文字和开始 / 取消；同一任务的轮询重叠时旧回复不会把状态改回去（#34），任务在跑时轮询自己续上。每次回复生效后 `onJob(job, change)` 通知界面（`change.ended` 表示一次运行刚结束，同步的 `change.arrived` 表示进来了新图或运行告一段落）。`state.js`：页面共用的状态（地址、图库列表与选择、计数、各任务最近一次回复等），全部字段在这里声明，按归属分组，每组只有一个写入者（组标题写明），别的模块要改就调用写入者的函数（#344）。`modes.js`：图片交互的四个模式（浏览、选择、详情、大图）、当前图的来源和快捷键动作表，只读 `state`（见 [WEB_UI_MAP.md](WEB_UI_MAP.md) 第 6 节）。`events.js`：各模块通知界面用的监听表。`settings.js`：设置页读写的数据（启动开关、折叠、备份与缓存、界面语言、名称的联网授权与模型接口、图库与来源、账号与 Pixiv 登录等待、检查更新），`onSettings(what, detail)` 通知。`labels.js`：排除原因、相似关系等名称，以及角色名的悬停提示。`catalog.js`：资料库的数据（按作品分组的角色、近 30 天新增、列表的分档与排序、搜索、作品树、关注列表与作品的新建和编辑）。`pages.js`：页面注册表（每个页面一个声明：地址、主区、点击语义、侧栏高亮、子页签、工具栏、排序、后端 pool 参数、空状态、底栏提示、选择栏种类；`ui/` 在加载时用 `implement()` 登记渲染入口、标题和重画方式，见 [WEB_UI_MAP.md](WEB_UI_MAP.md)「新增一个页面要做什么」）。`route.js`：地址与路由（`#/页面/子页?参数` 的读写；`cleanup()`／`browsing()` 等「这是哪类页面」的判断从注册表派生）。`meta.js`：图库元数据（角色、待整理计数、用户标签、最近一步撤销）一起读取，读的过程中有写入时不会被旧回复覆盖，`onMeta()` 通知。`detail.js`：单张图的详情（带收藏和主色，同一张图、同一读取纪元并发只读一次）、角色每月张数、偶尔一问「是 TA 吗？」的节流和证据文字（#709）。`artists.js`：按需查找画师的请求和等它结束。会话令牌和过期标记只由 `session.js` 写（切换图库时用 `leaveSession()`）。
- `web/static/ui/`：桌面布局。`dom.js`（`$()`、`⌘` 改写为 Ctrl、提示条与其中的撤销、错误横幅、工具栏的撤销按钮、可折叠块；`wireDom()` 在启动时挂上停用横幅与撤销按钮）、`tasks.js`（任务面板：列表、等你确认的事、按钮；面板打开时顺带读取图库列表，每次刷新后做一次更新检查；`wireTasks()`）、`settings.js`（设置页的外壳和七节导航；`wireSettings()` 在启动时注册事件）、`settings_<节>.js`（设置各节的分组和行，写成配置表，#714）、`settings_group.js`（设置的分组：标题、范围标签、说明、行、折叠区）、`updates.js`（检查更新的文字和按钮，设置、侧栏和导览共用）、`catalog.js`（资料库：角色书架与列表、作品树、编辑作品与添加新角色的对话框、关注作品的选择框；`wireCatalog()` 在启动时注册事件）、`shell.js`（页面框架：按地址打开页面、侧栏计数、各页的标题与工具栏；`wireShell()`）、`palette.js`（对角色的操作：命令面板「换成 / 也是 / 设为角色 / 跳到角色」、原创角色、编辑资料、合并、判断所选图片的关系；`wirePalette()`）、`detail.js`（单张图：打开、翻页、关掉详情卡片，卡片和大图底栏上各个按钮的动作，单图快捷键；大图的函数也从这里转出口；`wireDetail()`）、`detail_card.js`（详情卡片的部件：图片和玻璃按钮组、角色身份、提问条、角色卡、属性列表和各小节、底部操作；大图底栏复用它的 `identities({compact: true})`、`askBar()`，#709）、`viewer.js`（看图器：三张轮换、从缩略图展开和缩回、缩放平移、底栏、面板页看大图；和网格、详情的配合由 `detail.js` 经 `bindViewer()` 传入，不回头导入它们，#709）、`grid.js`（图片网格：分页读取、虚拟网格与缩放、选择、写入与撤销、快捷键、NSFW 筛选；`wireGrid()`；`kept.hold` 是列表变化时保持在视野里的那张图，详情面板和大图也用它）、`grid_card.js`（网格的卡片部件，#708）、`grid_rows.js`（网格的行坐标：分行、按位置或序号查行、卡片的框，#708）、`grid_bar.js`（选择栏的按钮和分派，载入时用 `setBar()` 登记进 `grid.js`，#708）、`grid_view.js`（视图选项浮层，选项的值在 `core/view_opts.js`，#708）、`calendar.js`（图库的日历视图，#708）、`dialog.js`（运行时模态对话框的唯一生成函数 `openDialog`，以及建在它上面的确认对话框和选择本地作品；`catalog.js`、`palette.js` 的对话框也走它）、`overlay.js`（浮层管理器：浮层的栈、Esc、点外关闭、焦点归还，不 import 别的 ui/ 模块；`wireOverlays()` 要排在其他 wire 之前，规则见 [WEB_UI_MAP.md](WEB_UI_MAP.md) 第 3 节）、`days.js`（图库的按天视图，#707：页面模块，一天的部件在 `day.js`，顶上的同步卡和「认一认」卡在 `day_cards.js`，数据在 `core/days.js`；`wireDays()`）、整理各页（#713，数据在 `core/tidy.js`）：`groups.js`（未识别按组整理、组页进入前的读取）、`similar.js`（相似的图、相似组 N 页头的「看过了」）、`identity.js`（相似的角色对）、`artist_audit.js`（画师抽查），共用的浅底分组和看大图在 `tidy_group.js`（`wireTidyPeek()`）、`artists.js`（详情卡片里的画师一节和查找画师，第一次查找前先征得同意；请求在 `core/artists.js`）、`artist_dir.js`（资料库的画师页签：按画师浏览的头像卡片、搜索、排序和分页接着读，#448；工具栏和标题副标题由 `shell.js` 向它要，不导入 `shell.js`、`catalog.js`）、`guide.js`（新图库引导：导览、上手清单、适时的提问；`wireGuide()`）、`models.js`（识别模型的提示条、文字和按钮数据，#168；设置页的卡片由 `settings_data.js` 用行画；`wireModels()`）、`files.js`（用图动作：打开原文件、在访达中显示、拖到别的应用、导出，#450。拖出是 `wireDragOut(el, ids, when)`：`ids(down)`、`when(down)` 收按下时的 pointerdown 事件，返回 `off()`。常驻的节点直接挂在节点上；`keyed`／`virtual` 里来来去去的卡片在容器上挂一次，用 `down.target.closest(…)` 找按的是哪张，`off` 交给页面作用域）、`effects.js`（一次写入或任务回复对页面其他地方的影响：重读计数、重画图库与按天视图、设置卡片、引导）。
- `ui/` 模块之间互相导入，`core/` 不导入 `ui/`。`ui/` 模块之间有循环导入，但载入时都不调用其他 `ui/` 模块的导出，所以没有问题；新模块也要保持这一点。依赖方向、`state` 字段声明、全局监听、`<dialog>` 外壳和行长由 `tools/check_frontend.py` 检查，存量冻结在 `tools/frontend_baseline.json`、只能缩小（#320，规则见 [AGENTS.md](../AGENTS.md#前端结构约定)）。
- **输入框的键盘处理（2026-11-13，#388）**：用中文、日文输入法时，选候选词要按回车、挑候选要按方向键和数字键、取消组字要按 Esc，这些按键不是页面的操作。所以每个 `keydown`（以及 `keyup`、`keypress`）处理函数的第一句都是 `if (composing(e)) return;`，`composing` 来自 `core/keys.js`，判断是 `e.isComposing || e.keyCode === 229`。只判断 `isComposing` 不够：WebKit（macOS 桌面 App 的 WKWebView）里结束组字的那一次按键，`isComposing` 已经是 false，只有 `keyCode` 仍是 229（WebKit bug 165004）。这条判断只在 `core/keys.js` 写一处，别处不直接读 `isComposing` 或 `keyCode`；`document` 级的快捷键处理（`ui/grid.js`）和浮层管理器的 Esc（`ui/overlay.js`）同样先判断，组字时一律放行给输入法。新写的键盘处理函数没有调用 `composing(` 时，`tools/check_frontend.py` 规则 9 报错（没有基线）。输入框之外的元素（海报、按钮）上的键盘处理也要写，为的是规则不用区分元素；`input` 事件不受影响，组字过程中它照常触发。

页面、区域、浮层、快捷键和状态组合的现状清单见 [WEB_UI_MAP.md](WEB_UI_MAP.md)。

原生宿主按 `/core/<名>.js`、`/ui/<名>.js` 提供这两个文件夹里的模块（名字只含小写字母、数字、`-`、`_`），新增模块不用改路由。Node 测试用 `tests/web.cjs` 导入模块（`web.fakePage()` 提供一个由普通对象组成的假页面，供界面模块绘制）：它先运行 `i18n.js`（默认中文页面，`web.language('en')` 在载入前换语言，一个测试文件一种语言），`before(web.ready)` 后 `web.core()` 给出重置过会话的导出。测试不截取源码片段运行。

## 样式令牌（2026-11-12，#317）

`web/static/css/tokens.css` 是外观令牌的唯一来源（CSS 变量），规则里引用变量，不另写字面值；新增取值先在这里成对加一档，并改这一节。`tools/check_frontend.py` 规则 8（#349）拦 `app.css` 和 `css/` 下新增的字号、圆角、`gap`/`padding`、`z-index` 和颜色字面量，报错写明该用哪个令牌；下面登记的例外（`margin`、不在档位上的 18px 以上间距）不拦，`app.css` 现有的字面量（主要是颜色）冻结在基线里，`css/` 下没有基线。总跟踪见 #316，实物对照见样式清单页（#318）。

| 类别 | 令牌 | 档位 |
|---|---|---|
| 字号 | `--fs-N` | 10 / 11 / 12 / 13 / 14 / 15 / 16 / 20 / 22；新写的界面只用 11 / 12 / 13 / 20 |
| 展示字号 | `--fs-display-N` | 17 / 18 / 21 / 26 / 28 / 30 / 32 / 40 / 72，只给大标题、空状态、启动页这类一次性的大字，不用于正文；新写的界面只用 28 / 40 |
| 字重 | `--fw-400`、`--fw-600`、`--fw-750` | 正文；强调和节标题；展示字号。三档只对西文都分得开：中文字体只有两档（Windows 的微软雅黑只有常规和粗体，Mac 的苹方没有粗体、最粗是中粗），600 和 750 在中文上是同一档，层次靠字号拉开 |
| 圆角 | `--radius-N`、`--radius-pill`（999px）、`--radius-round`（50%） | 2 / 4 / 6 / 8 / 10 / 12 / 14 / 16 / 18；新写的界面只用 4（40px 以下的小元素）/ 8（缩略图、格子）/ 12（卡片、分组、弹出层）/ 18（内容页、对话框、活页条标签），加胶囊（能点的控件）和圆形（人），嵌套时内层 = 外层 − 间距；2 / 6 / 10 / 14 / 16 只留给旧样式 |
| 间距（`gap`、`padding`） | `--space-N` | 1（细缝）/ 2 / 4 / 6 / 8 / 10 / 12 / 14 / 16 / 20 / 24 |
| 控件高度 | `--h-N` | 28 / 32 / 36，只给按钮、输入框、下拉、药丸标签这类控件；头像缩略图、图标底板、条栏的尺寸不在其内 |
| 阴影 | `--shadow-hover`、`--shadow-pop`、`--shadow-dialog` | 悬停；弹出层；对话框。只给浮起来的东西，亮暗各一套；不带模糊的描边环（`0 0 0 2px var(--accent)`）不算阴影。旧的 `--sheet-shadow` 只留给旧样式 |
| 时长 | `--dur-1`、`--dur-2`、`--dur-3` | 140ms：出场、悬停；240ms：进场、展开、换页；380ms：共享元素、活页条标签。出场比进场短。系统开了「减少动态效果」时都是 0 |
| 缓动 | `--ease`、`--ease-spring` | `--ease`（`cubic-bezier(.2,.8,.2,1)`）给所有进出场、展开、换页；`--ease-spring`（`cubic-bezier(.34,1.42,.5,1)`）带回弹，只给活页条标签和「弹一下」这类强调动作（用户 2026-11-19 定） |
| 层级 | `--z-*` | 页面内局部 `page-1`…`page-6` < `bar`（选择条）< `popover` < `drawer`（窄屏侧栏）< `menu` < `overlay`（大图、任务面板）< `toast` < `guide-checklist` < `guide-nudge` < `guide-tour` < `splash`；只加档位，不改先后 |
| 颜色 | 基础色（`--canvas`、`--sheet`、`--ink`…）、语义色（`--tone-*-bg/-ink` 警告与关系类型色调、`--atag-*` 自动标签芯片、`--hint-*`、`--glass-*` 磨砂表面、`--bar-bg`/`--toast-bg`、`--seg-on`、`--switch-off`、`--on-accent` 强调色底上的文字、`--scrim` 海报的压暗渐变等）、固定状态色（`--ok`、`--bad`、`--err`） | 亮暗两套成对写在 `:root` 和 `:root[data-theme=dark]` 两块里，规则里只引用变量；暗色不再逐条覆盖 |
| 图的颜色 | `--tint`，派生的 `--tint-wash`、`--tint-soft`、`--tint-glow`、`--tint-shadow` | 图的主色（#717）只经 `ui/tint.js` 的 `setTint(el, color)` 写（规则 20）：它写 `--tint` 并给元素加 `.tint`，派生的几档在那个元素上用 `color-mix()` 重算（自定义属性里的 `var()` 在声明它的元素上就算好了），海报、头像上也会重算；没写时是中性色。wash：作品卡、日历格的底；soft：缩略图读完之前的占位；glow：环境光；shadow：`.tint` 里的 `--shadow-hover` 用它画彩色投影。不用相对颜色语法（Safari 18 之前和规范不一致）。界面上不出色块、色值、色带 |

数字就是 px 值。新写的值取最近的一档；两档一样近时取用得更多的一档。2026-11-12 第二段按用户答复把原先不在档位上的值归并进来：半像素字号 9.5 / 10.5 / 11.5 / 12.5 / 13.5px 分别归到 10 / 11 / 11 / 12 / 13；圆角 3 / 5 / 7 / 9 / 13px 归到 4 / 6 / 6 / 10 / 14；`gap`、`padding` 的 3 / 5 / 7 / 9 / 11 / 13px 归到 4 / 6 / 8 / 8 / 10 / 12；控件高度 30 → 32、34 → 36、38 → 36。

不进令牌、仍写在原处的：页面级的大间距（18px 以上的 `padding`、`gap`）、`margin`、头像缩略图、图标底板和条栏（`.nav-icon`、`.palette-item img`、`.caption`、`.footer`）的尺寸。要加新档位先改这一节。

仍直接写色值的一处：`:root[data-theme=dark] dialog::backdrop`（较旧内核里 `::backdrop` 不继承自定义属性，不能引用变量）。暗色下的可见性要求：文字对比度不低于 4.5:1，非文字（图标、边框、开关）不低于 3:1；新增暗色取值先按这个核对。

### 样式文件与层（#705）

- `web/static/css/` 下按层分文件：`tokens.css`（令牌）、`base.css`（公共部件：排版、表面、胶囊控件与「选中」、页头、空状态、海报、圆头像、玻璃），以及每块界面一个文件：`rail.css`、`library.css`、`detail.css`、`galleries.css`、`catalog.css`、`character.css`、`tidy.css`、`settings.css`、`overlays.css`，每个文件开头写明归 0.4 的哪一步。宿主按 `/css/<名>.css` 发这些文件（`text/css`，名字规则同模块，缓存和安全头同 `/app.css`），`index.html` 先引 `tokens.css`，再引 `app.css` 和其余各文件（规则 19 查第一个样式表和每个文件都引了）。
- 层的先后只在 `tokens.css` 第一行声明：`@layer legacy, tokens, base, pages, overlays;`。`app.css` 整个在最低的 `legacy` 层，新样式不用和旧选择器比权重；`base.css` 在 `base`，`overlays.css` 在 `overlays`，其余各页的文件在 `pages`。没有层的样式比所有层都强，所以 `css/` 下每个文件整个写在自己的层里，层里不再开层（规则 18）；层的声明是 `tokens.css` 的第一条、`tokens.css` 是第一个样式表（规则 19）。旧样式里按元素名写的规则（`button`、`input`…）仍会落到新部件上，部件把自己在意的属性写全。
- 重写一块，就删掉 `app.css` 里对应的旧规则；升级做完时 `app.css` 只剩没重做的部分，或者删掉。「减少动态效果」的总开关（`*{transition:none!important;animation:none!important}`）还在 `app.css`：`!important` 在最低的层里反而最强，管得到所有层的 CSS 过渡和动画；脚本里的动画由 `ui/motion.js` 自己判断。
- 公共部件的「选中」写在 `aria-pressed` / `aria-selected` / `aria-checked` / `aria-current` 上，不另起 `.on`、`.active` 这类类名（旧样式里到处有，会串）；变体用 `is-` 前缀（`.btn.is-primary`）。各部件的样子和写法见样式清单「新部件」一组。
- 检查（`tools/check_frontend.py`，写法见 AGENTS.md「前端结构约定」）：规则 8 查 `app.css` 和 `css/` 的字面量，`css/` 下给的改法只在新档位里找；13 查三档令牌、新代码的字号和圆角档位；14 不写类名子串选择器和后代形式的 `:has()`；15 `var()` 引用的变量要在 `tokens.css` 或登记的「由 JS 写的变量」（如 `--tint`、`--p`）里，`css/` 下不认文件里自己声明的局部变量（否则 `--x: 13px` 就绕过了 8、13；只有 `app.css` 还认旧的局部变量）；16 `backdrop-filter` 只在 `base.css` 的 `.glass`、`.glass-bar`；17 Safari 17.0 没有的 CSS 能力只写在登记的增强块里（探测它的 `@supports`，`@supports not` 不算，块前面紧挨着写明退路的注释），不用 CSS 嵌套、相对颜色语法和 `::backdrop` 里的 `var()`；18 `css/` 下的文件在自己的层里；19 样式表的先后；20 `--tint` 只经 `setTint()` 写。

## 前端整体升级（2026-11-19，#685、#469）

用户 2026-11-19 定：0.4 按 #685「方向」评论的第一至四节整体重做前端，不分批；渲染用自写底座（#469 2026-11-19 的评论），不引框架、不引 Node、不加构建步骤。分步、进度和验收见 #703。本节写各步共用的设计和约定；某一部分合进 main 时，它的内容并进本文对应的现行各节（「前端模块」「样式令牌」等），这里删掉。

### 界面结构

- **导航是活页条**：一级四个入口：图库、资料库、画廊、整理。选中的入口长出一张白色标签，和内容页连成同一张纸。二级层随一级变：图库是最近几天的日期封面，资料库是钉住的作品和角色（可以排序，接 #684），画廊是各画廊的封面，整理是未识别、重复、已排除三个队列，带数字。底部是同步圆环（就是任务按钮）和设置。⌘1–4 切一级，⌥↑↓ 在二级里移动。
- **图库**并入首页（#677），有按天、网格、日历三种视图，筛选条和搜索在三种视图里都生效；按天、日历只看不选，多选在网格里做。同步状态和「认一认」是按天视图顶上的两张卡片。打开 App 进按天视图。
- **画廊**分「按条件」（现在的保存条件）和「手选」两种，收藏是内置的手选画廊（#727）；图可以拖进手选画廊和收藏。画廊有拼贴、杂志、瀑布、展厅、色彩五种展示方式。
- **详情栏**是一张浮起的卡片：图片在顶，按钮组浮在图上；下面是角色身份、属性列表和底部两个操作。大图从缩略图的位置展开，关掉时回到缩略图。
- **资料库**三个页签用同一套语言：角色是海报；作品是作品卡（作品名大字 + 作品配色 + 一排角色头像 + 几条不同图的竖切片）；作品页顶部是切片拼的横幅。
- **角色页**：横幅、圆头像、大名字，下面是收集曲线、常一起出现的角色和最近加入，再下面是全部图。
- **整理**：未识别、重复、已排除合成一页，三个页签。
- **空状态**：大图标 + 一句事实 + 一个去处。

### 视觉语言

- 画是主角：角色用海报（满版图 + 渐变压暗 + 白字）表示；网格里不写字，卡片下面没有标题条。
- 形状有含义：圆形是人（角色、画师的头像）；胶囊是能点的控件（按钮、芯片、分段、搜索）；图和卡片用圆角矩形。
- 表面两层：画布和内容页。内容页里的分组用浅底，不描边、不套卡。阴影只给浮起来的东西。
- 「选中」一种画法：强调色浅底；分段控件里白底浮起。只有一个强调色。
- 每页的页头同一个结构：大标题 + 副标题 + 右侧控件组，下面最多一行工具。
- 颜色只用、不展示：图的颜色变成环境光（详情卡片的图片后面、角色页横幅往下、日历月份标题后面）、悬停时的彩色投影、作品卡和日历格的底色，以及缩略图读完之前的占位色。界面上不出色块、色值、色带。每张图的主色由后端给（#717），前端写进元素的 `--tint`，派生的底色用 `color-mix()`。
- 磨砂玻璃（`backdrop-filter`）只给浮层，卡片上不用：每张卡片上的模糊和投影滤镜都各占一块显存（#683）。

### 令牌、样式文件与层

已在 #705 做成现行参考，见[样式令牌](#样式令牌2026-11-12317)和其下的「样式文件与层」。

### 渲染底座

现行参考（2026-11-19，#704）；`ui/motion.js` 由 #705 加。新写的界面照这一节画：节点建一次，之后只改变了的地方；数据不进 HTML 字符串；先量后改；长页面只挂视口附近的节点。都是 `web/static` 下的原生 ES 模块，碰 DOM 的在 `ui/`，纯计算的在 `core/`。每个文件开头的注释写了完整的用法和例子，这里只写各管什么、怎么配合。

| 文件 | 做什么 | 用法要点 |
|---|---|---|
| `ui/el.js` | 模板与节点，往节点里写数据的唯一入口 | 模块顶层声明 ``const CARD = tpl`<div class="card"><b data-ref="name"></b></div>` ``，`CARD()` 克隆一份得到 `{el, refs}`。模板不带插值（写了 `${…}`、或者把 `tpl` 当函数调，运行时都抛错），第一次用到时解析一次、按模板字符串缓存；`data-t`、`data-t-attr` 在解析时翻译（和 `index.html` 同一套写法，键由 `tests/i18n.test.cjs` 从 ``tpl`…` `` 里收集）。数据只经 `text`、`attr`、`prop`、`value`、`cls`、`show`、`css` 写，它们记着上一次的值，没变不动 DOM；`value` 在焦点上时不覆盖。带链接、加粗的译文用 `rich(t('到 {where} 打开'), {where: 节点})` 拼节点，小节点用 `make()`。几个没有自己状态的小节点组成的一小块（一行链接、几枚只读芯片、几根柱子）用 `refill(node, 签名, build)`：签名没变不动，变了整块换掉；要按键复用、留住焦点或播进出场的用 `ui/keyed.js` |
| `ui/keyed.js` | 按键复用的列表 | `keyed(容器, {key, create, enter, exit, move, appear, scope})`，`set(items)`。复用的部件收到 `update(item, index)`；挪位置只挪最长递增子序列以外的节点，被挪的节点里原来有焦点就挪完放回去；第一次有数据的那次 `set` 不播进场（先 `set([])` 也一样），`appear: true` 才播；`create` 中途抛错时这一轮新建的部件释放掉、列表不变；退场的节点设 `inert`、留在原处，`exit` 返回的 promise 落定后再摘，`dispose()` 立刻释放全部。钩子只收函数，进场、退场、FLIP 由调用方从 `ui/motion.js` 传进来 |
| `ui/scope.js` | 生命周期 | `scope(名)` 或 `父.child(名)`；`on`（返回 `off`）、`timeout`、`interval`、`read` / `write`（帧任务）、`observe`、`add(部件或清理函数)`、`remove`、`signal`，`dispose()` 一次清掉、可重复调用。触发过的 `timeout`、跑完的帧任务自己从作用域里摘掉，所以长寿的作用域（`app`）可以反复用它们；`add()` 进去的要等 `dispose` 才走，不要每次滚动都 `add` 一个。`page(名)` 释放上一页的作用域、返回新一页的（页面框架换页时调用，#706）；全应用一直在的部件挂在 `app` 上 |
| `ui/frame.js` | 每帧一次的读写调度 | `read(fn, key?)`、`write(fn, key?)`，返回取消函数（部件里用作用域的 `s.read` / `s.write`）。同一帧先跑完所有读再跑所有写；带 key 的同一帧只跑最后一个；读里排的读、写同一帧跑，写里排的读、写都挪到下一帧，所以一个总在再排自己的写每帧只跑一次、不会卡住这一帧 |
| `ui/virtual.js` | 只挂视口附近的节点 | `virtual(视口, 内容, {create, margin, scope})`，`set(框, 总高)`；框是 `{key, x, y, w, h}`。挂视口上下各 `margin` 屏（默认 1），超出两倍才摘；按键复用部件；重新布局时以视口顶部那一张为锚；摘掉前去掉 `<img>`（包括根节点自己）的 `src`、`srcset` 和 `<source>` 的 `srcset`，放掉解码后的位图（#683）。调用方要知道：`content` 是定位元素（计算样式是 static 时它写上 `relative`）；部件根节点的 `transform`、宽高归它管；DOM 次序按框的先后插入（重新布局后已挂的节点不挪）；焦点在里面的节点滚远了先不摘，键从数据里没了照摘 |
| `core/layout.js` | 布局纯函数 | 输入宽高比和宽度，输出框 `{i, x, y, w, h}`：`justifiedRows` / `justified`（等高行，规则见「浏览界面」，图库网格就用它）、`masonry`（瀑布）、`dayMosaic`（按天：一张大图高两行、右边两行小图、放不下的收成最后一格的 `more`，界面盖「+N」；瀑布和马赛克把不是正的有限数的宽高比按 3:4 算）、`monthGrid`（某月的格子，周首默认周一）、`cellTiles`（格内 1 / 2 / 4 张）。FLIP 的起止位置用这里的框，不量 DOM。各页步骤在这里加自己的布局（画廊的拼贴、杂志、展厅由 #710 加） |
| `ui/rows.js` | 设置的几种行与配置表（#676） | `rows(容器, 配置表, {scope})`，`update(state)`。几种行：`switch`、`action`、`info`、`select`、`field`（输入框）、`list`；每行可带 `hidden(state)`、`disabled(state)`。画一次、按状态更新，轮询时焦点和打了一半的字不丢；改动后记一个「待提交的值」，`set` 返回的 promise 落定或 state 变成这个值之前，轮询带回的旧值不覆盖控件（#469）。配置表先整张校验再建节点；建好后立刻 `update` 一次。文字由调用方用 `t('…')` 翻译好传进来。设置页要的选项（#714）：`select` 的 `seg`，`action` 的 `buttons`、`dot`、`progress`，每行的 `href`、`link`、`warn`，`field` 的数字范围和 `enter`，`list` 的 `itemHref`、`itemTitle`；info 带 `href` 时整行是链接，action 必须有按钮。保存落定后这一行按最新的 state 重画，失败、取消时控件回到原值。样式在 `css/settings.css` |
| `ui/motion.js` | 动效（#705） | 时长和缓动取令牌，进场、退场、FLIP、共享元素过渡、数字滚动，「减少动态效果」；以及「这张图的缩略图在屏幕上哪里」的登记表 |

约定：

- **部件**：热路径（图库网格与按天视图、看图器、活页条、拖放、画廊布局），以及会被轮询刷新或带输入框的面板，写成自己持有节点的部件 `{el, update(props), dispose()}`：`el` 建一次；`update` 只经 `ui/el.js` 的写入函数改，所以每次把全部字段写一遍也不会多动 DOM；`dispose` 里调自己作用域的 `dispose()`。列表交给 `ui/keyed.js`，长列表交给 `ui/virtual.js`，它们都按这个约定调用部件。部件的作用域挂在页面作用域或 `app` 下面。
- **转义**：数据不拼进 `innerHTML`、`outerHTML`、`insertAdjacentHTML`（规则 10）。不常变、也没有输入框的内容可以照旧用这三个写静态的字面量，插值一律改走 `ui/el.js`。
- **先量后改**：读布局放进 `frame.read`，改 DOM 放进 `frame.write`；滚动、缩放的处理函数只排队。
- **动画**：只动 `transform` 和 `opacity`。`requestAnimationFrame` 只在 `ui/frame.js` 和 `ui/motion.js` 里调，`el.animate`、`startViewTransition` 只在 `ui/motion.js` 里调（规则 11）。系统开了「减少动态效果」时直接到终态。
- **虚拟化**：`content-visibility` 在 macOS 14 上没有，长页面靠 `ui/virtual.js`。现有图库网格（`ui/grid.js`）的行坐标缓存、虚拟滚动、选择和键盘照旧，只有分行改调 `core/layout.js` 的 `justifiedRows`，和按天视图、画廊共用一份。
- **分层不变**：请求和状态在 `core/`，`state` 的写入者规则照旧；部件只画，不发请求；异步回来判断还在不在这一页照旧用 `onPage`。页面注册表、浮层管理器的规则不变。
- **开发模式**：在开发者工具里运行 `localStorage['pi.dev'] = 'true'` 后重新载入页面（关掉：`localStorage.removeItem('pi.dev')`）。开发模式下 `ui/scope.js` 记着所有还活着的作用域，换页后上一页期间建的根作用域还没释放（不属于 `app`，也不属于新一页）时在控制台提示 `[scope] 换页后还有 N 个作用域没释放`；`live()` 返回还活着的个数和各名字的数量。偏好存在浏览器本地，不进图库、不随包发出，正常使用不受影响。
- **测试**：每个模块一组 Node 测试（`tests/el.test.cjs`、`keyed`、`scope`、`frame`、`virtual`、`rows`、`layout`）。`tests/dom.cjs` 是底座用的最小假 DOM：有真正的节点树、`<template>` 的静态 HTML 解析、事件和写入计数（测「值没变不动 DOM」）；`tests/web.cjs` 的 `fakePage()` 只记 `innerHTML` 文本，旧模块的测试照旧用它。`core/layout.js` 用固定输入比对输出的框，等高行另和网格原来的分行逐项对照。`ui/frame.js` 在 Node 里没有 `requestAnimationFrame`，测试用 `setScheduler()` 换成手动的。

### 页面框架与活页条（#706）

- **注册表**（`core/pages.js`）：每个页面声明属于活页条的哪个一级入口（`entry`：`library`、`catalog`、`galleries`、`tidy`、`settings`，画师抽查是 `''`）和在二级层里选中哪一项（`item`，整理的五个队列）；随地址变的那一项由 `core/rail.js` 的 `railAt(route)` 算：某个画廊、收藏（只按收藏筛选的图库，在画廊下）、角色页和作品页（只按一部作品筛选的图库，F #711 另做作品页之前就是它，都在资料库下，钉住了就选中）、按天和日历的 `at`。图库有三种视图：`days`（按天，`#/days`，原来的首页；`#/`、`#/home`、`#/recent` 改写到这里）、`library`（网格，`#/library`，地址和查询照旧）、`calendar`（日历，`#/calendar`）；`view` 写是哪一种，`filters` 标出读图库筛选的页面（三种视图和画廊）。整理的入口 `#/tidy` 改写到按组整理。
- **图库三种视图共用筛选**（#703 第 2 问）：网格的工具栏是 `library`（搜索、筛选条、排序、选择、显示方式）；按天、日历是 `filters`，只有搜索框和筛选条（条件胶囊、「＋ 条件」、保存为画廊）。筛选变了只有一个入口：筛选条上加、去、翻转条件是换地址，搜索词是就地改进地址，两种都让面板页收到 `render({samePage: true})`（网格页照旧 `reload`）。按天、日历取数时把 `core/pages.js` 的 `libraryFilters(route)` 带进 `/api/home`、`/api/calendar`（和 `/api/assets` 同一组参数，#718）；它不含 NSFW 设置，另带 `nsfw: state.nsfw`。切换视图（`core/rail.js` 的 `viewRoute`）时筛选和搜索都跟着走，包括旧的单值参数 `day`（三种视图里都是「只看这一天加入的」）；排序、角色、例图不带。
- **看哪一天：`at`**：按天、日历的地址可以带 `at=YYYY-MM-DD`：按天视图滚到那一天，日历显示它所在的月（C1 #707、C2 #708 实现）。它不是筛选，切视图时不带过去；活页条按它选中时间轴的那一项。
- **活页条**（`ui/rail.js`，数据在 `core/rail.js`，样式 `css/rail.css`）：全应用一个，挂在 `app` 作用域上。各页要换二级层的内容用 `provide(entry, fn)`（`fn()` 返回项的列表，`refresh(entry)` 时重新调），选中某项用 `select(key)`，按天视图滚到某一天用 `selectDay(day)`，点了二级项要自己处理用 `onPick(entry, fn)`，改队列的数字用 `count(key, n)`，拖放时临时换一组项用 `mode(name, items, hint)`。每一项的字段写在 `core/rail.js` 开头。资料库钉住的项按图库存在浏览器本地（`libraryPrefs`，键 `railPins`）；读不到图库 id 时不读也不写。⌘1–4（Windows 上 Ctrl，按 `e.code`）切一级，⌥↑↓ 在二级里移动，⌥⇧↑↓ 挪动钉住的项，走 `ui/grid.js` 现有的键盘处理。窄窗口（≤760px）二级层收进抽屉，页头左边的按钮打开；拉宽窗口时自己收起。二级层比窗口长时，里面的滚动区（`#railScroll`；外层 `#railLayer` 不滚，窄窗口里是抽屉）自己滚动，上下边缘用遮罩渐隐：上边只在往下滚过时有、下边只在下面还有内容时有（`core/rail.js` 的 `railEdges`，用户 2026-11-20 提）；键盘焦点到了边缘附近的项、或选中了某项时，滚到它完整露在渐隐以外（`railReveal`）。指针在二级项上停 0.3 秒（停留判定，划过去不出；卡片出来以后换到相邻项直接滑过去）或键盘焦点到了二级项上（立刻）时，旁边滑出一张预览卡（`ui/rail_peek.js`，画在二级层外，不被滚动区裁掉）：上面一行图按原图宽高比排成一样高（`core/rail.js` 的 `peekRow` 挑图、定行高，`core/layout.js` 的 `strip` 排），放得下几张放几张，卡片随内容变宽；下面是标题和张数；项的 `peek` 给图（作品是角色的圆头像，`peekRound`），`color` 给主色，经 `setTint` 变成彩色投影和很淡的环境光。没有图的项（整理的队列）只有文字。在相邻项之间移动时卡片滑过去，不重新弹出（#706）。一级、二级都是 `<a href>`，在 macOS 上 Tab 能走到它们，前提是 WKWebView 打开了 `tabFocusesLinks`（现在由 wry 0.57 打开）；换掉 wry 或关掉这一项，Tab 只会在搜索框和图片区之间转。
- **图库的时间轴**：最近 5 天各一项，更早的同一年按月、再早的按年（用户 2026-11-19 定）。从这个月起按 `/api/calendar` 的 `prev` 一个月一个月往前读，带着 `libraryFilters` 和 NSFW 设置；参数没变、也没有写入时不重读，同步到了新图只重读最新的月，写入和撤销之后整条重读，读失败留着上一次的结果。**点一项**：在日历里去那一天所在的月（`#/calendar?at=…`）；在按天、网格里去按天视图的那一天（`#/days?at=…`），都带着现在的筛选。已经在按天视图里时，C1 用 `onPick` 直接滚过去、不换地址。**⌥↑↓**：按天、日历里走到哪一项就去哪一项；网格里只移动选中（不把人从网格带走），回车或点击才去。
- **页头**：各页用 `head(r, {n})` 返回 `{title, sub, alert, actions}`：标题、副标题、警告是纯文字，由 `ui/shell.js` 用 `ui/el.js` 写进 `.title-page`、`.page-sub`；`actions` 是这一页自己的按钮节点。旧写法 `headingHTML`（返回 HTML 字符串，拼进 `innerHTML`）和自己画整个页头的 `drawHeading` 是存量，记在规则 10 的基线里，各页步骤改到哪页就迁到 `head`（图库网格的标题等 C2 把 `ui/filters.js` 的 `conditionTitle` 改成节点）。图库三种视图之间（活页条落在「图库」时）、整理三个页签之间的分段切换由框架画，是链接写法的 `.seg`；右侧是全局的控件组（跳到角色、NSFW、视图选项、撤销、快捷键）。下面的工具栏是 `#toolbar`，空的页面藏起来。页头的样式在 `css/base.css`。
- **空状态**：`ui/empty.js` 的 `drawEmpty(box, {icon, fact, next})`；网格页按注册表的 `emptyIcon`、`empty`、`emptyNext` 画，`emptyNext` 返回 `[{label, href}]` 或 `[{label, act}]`。

### 页面样板（#706）

一页一个模块，后面各步照这个写（#703「可维护性」）：

1. **登记**：在 `core/pages.js` 的 `list` 里加一项（地址、主区 `area`、活页条的 `entry`、工具栏、空状态……），在 [WEB_UI_MAP.md](WEB_UI_MAP.md) §2.1 总表和 `tests/pages.test.cjs` 的 `ROWS` 各加一行。数据字段写在注册表里，`ui/` 只用 `implement` 登记函数（`render`、`head`、`refresh`……）。
2. **取数**在 `core/<页>.js`：导出返回数据的函数，参数里带 `signal`；请求的参数、并发合并、缓存都写在这里，Node 里能直接测。`ui/` 不从 `core/session.js` 导入 `api`、`post`、`shared`，也不调 `fetch()`（规则 22）。
3. **画**在 `ui/<页>.js`：开头的注释写它负责什么；静态结构写成模块顶层的 ``tpl`…` ``，部件是 `{el, update, dispose}`；文件末尾 `implement({页: {render, head, help, refresh, …}})`。一个文件不超过 600 行（规则 21），一页里自成一块的部件（卡片、浮层、工具栏）拆成自己的文件。
4. **部件建一次**：`render({samePage, scope})` 收到这一页的作用域（换页时框架调 `ui/scope.js` 的 `page()`，释放上一页的）。`samePage` 为假是刚进这一页：在 `scope` 上建这一页的部件（列表、监听、计时器都登记在它上面）。`samePage` 为真是同一页又画一次（换了子视图、筛选变了），作用域沿用：**不再建一份**，只按新地址更新已有的部件（`list.set()`、`update()`），不然上一份要等离开这一页才释放。主区已经由框架切好，不用调 `showArea`。不挂 `document`、`window` 级的监听（规则 3）。
5. **异步回来**先判断还在不在：`if (!scope.alive || state.route !== route) return`，成功和失败都这样判断（换了页，作用域已经释放；同一页换了子视图、改了筛选条，地址换了，路由对象也换了；搜索词改了地址是就地替换，框架同样换一个新的路由对象，`core/route.js` 的 `reRoute`）。这样先发的请求后回来，结果不画、错误也不弹。撤销、学习落定、轮询这类重读用的是同一个路由对象，挡不住：**同一路由上的重读只认最后一次**，每个视图记一个读取序号（`const seq = ++page.seq`），回来时 `seq !== page.seq` 就丢掉，轮询也只由最后一次读取续排（`ui/groups.js`、`ui/similar.js`）。别处的回调照旧用 `onPage(state.route, '页')`。
6. **数据变了**：`refresh(why)` 和 `samePage` 一样只更新已有的部件，值没变不动 DOM；`why` 的取值写在 `core/pages.js` 的 `implement` 说明里。筛选变了不走 `refresh`，走 `render({samePage: true})`。
7. **页头**：`head(r, {n})` 返回纯文字和节点（上一节「页头」），不拼 HTML。

最小的例子（「笔记」页只是示意，不存在）：

```js
// core/notes.js：笔记页的数据
import {api} from './session.js';
export const loadNotes = (route, signal) => api('/api/notes?' + new URLSearchParams({q: route.q}), {signal});

// ui/notes.js：笔记页（列表和空状态）
import {t} from '../core/i18n.js';
import {loadNotes} from '../core/notes.js';
import {implement} from '../core/pages.js';
import {readError} from '../core/session.js';
import {state} from '../core/state.js';
import {$, error} from './dom.js';
import {show, text, tpl} from './el.js';
import {drawEmpty} from './empty.js';
import {keyed} from './keyed.js';

const PAGE = tpl`<div><ul class="notes" data-ref="list"></ul><div data-ref="empty" hidden></div></div>`;
const ROW = tpl`<li class="note"><b data-ref="title"></b></li>`;
function noteRow(note) {
  const {el, refs} = ROW();
  const update = x => text(refs.title, x.title);
  update(note);
  return {el, update};
}
// 这一页的部件：进页面时在页面作用域上建一次，之后只 load。
function notesView(scope) {
  const {el, refs} = PAGE();
  $('notesPage').replaceChildren(el);
  const list = keyed(refs.list, {key: n => n.id, create: noteRow, scope});
  return {scope, async load(route) {
    let notes;
    try { notes = await loadNotes(route, scope.signal); } catch (e) { if (scope.alive && state.route === route) error(readError(e)); return; }
    if (!scope.alive || state.route !== route) return;
    list.set(notes);
    show(refs.empty, !notes.length);
    if (!notes.length) drawEmpty(refs.empty, {icon: 'check', fact: t('还没有笔记。')});
  }};
}
let view = null;
implement({notes: {
  // 刚进这一页（或作用域已经释放）才建；同一页又画一次（samePage：子视图、筛选变了）只按新地址重读、更新列表。
  render: ({samePage, scope}) => { if (!samePage || !view?.scope.alive) view = notesView(scope); return view.load(state.route); },
  refresh: () => view?.load(state.route),
  head: () => ({title: t('笔记'), sub: ''}),
}});
```

`tests/pages.test.cjs` 有两项照这个例子在日历页上走：同一页画几次以后，活着的作用域数不涨、部件只建一次；搜索时先停在 a 再打成 ab，a 的回复晚到不画。

### 兼容底线

- macOS 14.0 的 WKWebView 按 Safari 17.0 算；Windows 的 WebView2 是常青版。macOS 14 上的 WKWebView 不随 Safari 单独更新，只随系统走，最高到 Safari 17.6 的水平（2026-11-19 在 macOS 14 虚拟机里确认，#703 的评论）；所以照 17.0 写。磨砂玻璃照旧写带前缀的 `-webkit-backdrop-filter`（Sonoma 上的 Safari 也只认带前缀的）。
- 只作增强、要有退路的：View Transitions（Safari 18.0）、`@starting-style`（17.5，`@supports` 测不出来）、`transition-behavior`（17.4）、`linear()` 缓动（17.2）、`content-visibility`（18.0）、`@scope`（17.4）、滚动驱动动画（26）、`Promise.withResolvers` 和 `Object.groupBy`（17.4）。Safari 没有的 `requestIdleCallback`、`scheduler.postTask` 不用。
- 退场动画用 Web Animations 做。判断动画结束用 `animation.finished`，另按时长加计时兜底（Safari 17 的 `transitionend` 偶尔不来）。
- 缩略图到大图：WAAPI 的 FLIP 做底，View Transitions 只在支持的平台上用。共享的两端带 `view-transition-class: share`，新旧快照按 cover 裁（方形缩略图到竖图时不拉伸、不重影）；不认 class 选择器的旧版本退回默认的拉伸交叉淡化。各视图在缩略图 `<img>` 上 `thumbs.mark(img, uuid)`，看图器才找得到从哪里展开、缩回哪里（#709）。
- WebKit 里频繁切换 `hidden` 特性有代价：样式里带 `:has()` 的选择器（如 `body:has(#taskPanel:not([hidden]))`）让 WebKit 在 `hidden` 一变时重查（#709 逐段计时时看到大图翻页因此变慢）。热路径上的显隐用类名：看图器的三张槽、大图底栏的角色行都只换类名；`#viewer`、`#inspector` 原来的两段 `:has()` 已换成 body 类 `viewing`、`inspecting`。

### 检查

实施时加进 `tools/check_frontend.py`，同时写进 AGENTS.md「前端结构约定」（#704、#705）。前三条已在 #704 落地，脚本里的编号是 10–12：

1. 规则 10（`inner-html`）：数据不拼进 `innerHTML`、`outerHTML`、`insertAdjacentHTML`：右边只能是不带插值的字符串字面量（方括号写法同样算），`setHTMLUnsafe`、`createContextualFragment`、`parseFromString`、给 `srcdoc` 赋值、把 `tpl` 当函数调按名字报；`ui/el.js` 放行；存量按文件计数进基线。
2. 规则 11（`animation-entry`）：`requestAnimationFrame` 只在 `ui/frame.js`、`ui/motion.js` 里调，`.animate(`、`startViewTransition` 只在 `ui/motion.js` 里调；存量按文件计数进基线。
3. 规则 12（`safari17-js`）：Safari 17.0 没有的 JS 能力只出现在登记的增强处（`SAFARI17_ENHANCEMENTS`，现在只有 `ui/motion.js` 的 `startViewTransition`）：import 属性（17.2）；`Promise.withResolvers`、`Object.groupBy`、`Map.groupBy`、`AbortSignal.any`、`checkVisibility`（17.4）；`startViewTransition`、`URL.parse`（18.0）；`Promise.try`（18.2）；迭代器方法和 `Iterator.from`（18.4）；`setHTMLUnsafe`（26）；Navigation API（26.2）；`Temporal`、`CloseWatcher`、`using` 和 `Symbol.dispose`（27.3）。Safari 正式版没有的 `requestIdleCallback`、`scheduler.*`、`Element.moveBefore` 哪里都不用。版本按 MDN browser-compat-data 核对，条目写在脚本注释里。模块和经典脚本 `i18n.js`、`splash.js`、`theme.js` 都查。没有基线。

4–8 已由 #705 做成 `tools/check_frontend.py` 的规则 8（扩到 `css/`）和 13–17，另加 18（层）、19（样式表的先后）、20（`--tint` 的写入者）；现行写法见[样式令牌](#样式令牌2026-11-12317)「样式文件与层」的最后一条。

#703「可维护性」的两条由 #706 做成规则 21、22（见上面的[页面样板](#页面样板706)）：

- 规则 21（`ui-file-length`）：`ui/` 下的文件不超过 600 行；现有的大文件按超出的行数进基线，只能缩小（`ui/settings.js` 已由 #714 拆到 600 行以内，移出基线）。
- 规则 22（`ui-request`）：`ui/` 不直接发请求：不从 `core/session.js` 导入 `api`、`post`、`shared`（改名导入、`import * as` 也认），不调 `fetch()`（`window.fetch`、`globalThis.fetch?.()` 也认）、`navigator.sendBeacon()`，不建 `EventSource`、`XMLHttpRequest`、`WebSocket`；存量按文件计数（导入的名字和 `fetch` 调用各算一处）进基线。

### 样式清单要加的

`tools/styleguide/` 随各步加：活页条的标签和二级项（#706，已加）；详情卡片、提问条和大图底栏（#709，已加，调用生成）；作品卡（#711）。新令牌表和公共部件已在 #705 加好（「新部件」一组）；旧部件在重做完之前标成「旧样式，重做中」。

## 新图库引导（2026-10-12，#96）

引导在真实界面上进行，不另开页面（用户 2026-10-12 决定）。新图库第一次打开、启动动画结束后，界面变暗并在中间显示欢迎卡片；选「带我看看」后，依次点亮任务按钮（实时识别进度）、首页第一张图（认出的角色、按 C 纠正、撤销）、资料库、未识别，最后说明上手清单。亮圈和气泡在各站之间平滑移动，系统开了「减少动态效果」时只淡入淡出。任何一站都能跳过（Esc 同效），「设置 › 关于 › 新手引导」重新播放。引导只读取状态、调用已有接口，不自动授权联网，也不启动同步以外的任务（同步本来就在打开图库时自动开始）。

- **上手清单**：右下角「上手 n/5」，展开后五项：看看认出的角色、纠正一张认错的图、撤销一次、确认一组没认出的图、关注一部作品。做过就自动打勾（打开资料库；`/api/change`、`/api/reassign` 成功；在未识别分组页确认；`/api/undo` 成功；关注列表加入作品），全部做完稍后自动收起，也可以「不再显示」。
- **需要时才问**：第一次认出角色后，底部浮出「要联网查它们的中文名吗」（只发文字，允许后对图库长期有效）；首次同步结束、还没有关注的作品时，浮出图片最多的可关注作品，点「关注」即加入「检查新角色」的关注列表，检查与添加时仍单独确认联网。每个问题回答（包括「暂不」）后不再出现。
- **问题条**：没有识别组件或识别自检失败时，在页面顶部显示说明。自检进程失败退出时显示它写入标准错误的最后一个非空行，最多 300 字，可点「重新检查」。同步被取消、超时或出错时说明已加入和识别的图都保留、再同步会接着做，来源文件夹不存在时指向「设置 › 图库」，并给出「再同步一次」（#97）。问题解决前一直在；读不到的位置和逐张读取失败仍由任务面板按同步结果显示。
- 进度按图库存在 `prefs` 的 `onboarding` 键：`{status: active|skipped|done, step, ticked[], seen[], hidden}`。没有这条记录时，还没有一次同步走完（`sync_runs` 里没有 done / needs_recognition / needs_windows）的图库算新图库，打开时写入；升级上来的旧图库和导入的归档不出现引导与清单；导出和导入都会清除这条记录（#116），接收方不会继承发送方答过的提问和清单。测试图库没有引导。
- `GET /api/onboarding` 返回 `guide`、`capability`（识别与索引 worker 是否配置、共享索引里的角色参考数、本次会话的识别自检状态）、`results`（可见图片、已识别即有 WD 分级、认出角色的图片与角色数、未识别）；带 `?works=1` 再返回推荐作品。`POST /api/onboarding` 的 `action` 为 `step`（带 `step`）、`skip`、`done`、`reopen`、`tick`（带 `item`）、`seen`（带 `key`：names、works）、`hide`、`check`；`check` 用 `--check` 启动一次识别 worker（实际加载模型），结果只留在本次会话，没有 worker 时直接报告缺失。
- 推荐作品：按已认出角色的图片数取前 5 部。角色已归到作品的按作品计；否则取角色 tag 末尾的作品限定词（`name_(作品)`），与 WD 普通画面标签同名的限定词（如 `swimsuit`）不算。没有限定词的角色本地不推断。「关注」用 `/api/works` 的 `from_tag` 建好（或复用）链接 Danbooru tag 的作品，再加入关注列表。

## 识别模型（2026-10-19，#168）

发布用的安装包不带识别模型（下载、校验和存放见 [安装与平台](BACKEND_TASKS.md#独立安装与可选计算包)「发布用的小安装包」）。和检查更新一样是整个 App 共用：状态和下载进度在 `core/models.js`（`/api/models`，下载或导入进行时每秒读一次），文字和按钮在 `ui/models.js`。只有 `on_demand`（小包）才会出现下面这些：

- **提示条**：模型没就位时，每个图库的页面顶部都显示（不论引导是否开着），写明约多大、没有模型时能做什么（建库、浏览、手动整理），按钮为「下载模型（约大小）」「从文件导入…」，大小按平台清单计算，并链到 [手动下载说明](INSTALL.md#识别模型)。下载中显示已下 / 总量、速度、当前来源（经代理时写代理地址）和「取消」；失败时写出是哪个文件，逐条列出每个来源的原因，给出重试、导入和「删除未完成的部分」。导入后逐个列出所选文件的结果：已接收、SHA-256 不符被拒、不是需要的文件、已经有了。这时识别组件的问题条和自检让位，模型就位后才自检。
- **设置 › 数据 › 识别模型**：同样的状态和按钮，加上镜像地址（保存或恢复默认）、存放位置和本平台文件的大小与 SHA-256。原始模型就位即允许识别；macOS 缺少可选 WD 加速文件时说明暂用 CPU，并提供下载和导入入口；开着加速而上次 WD 没用上 Core ML 时提供「修复 WD 加速文件」和导入。加速文件下载失败保留原模型的可用状态。自带模型的完整包（模型在 App 包内）模型就位后没有下载、修复和导入入口，不往包里写文件，状态行照旧说明 WD 暂用 CPU。模型就位后有「删除已下载的模型」（用户 2026-10-19 要求）：先确认，说明删除后要重新下载或导入才能识别新图、已有结果保留；正在同步识别、更新角色索引、下载或导入时按钮不可用，宿主也会拒绝。自带模型的完整包没有这个按钮。
- **设置 › 数据 › 识别模型 › 使用 GPU 加速识别**（#270，不论 `on_demand` 都有）：只在这个版本有加速后端时显示（`/api/models` 的 `acceleration.available`；Apple 芯片的 macOS 上 WD 与 CCIP 走 Core ML，Windows x64 上走 DirectML），默认开。存在 `preferences.json` 的 `acceleration`（`true` / `false`，没有这个键或取值不对时按开），整个 App 共用；识别和索引 worker 启动时读，所以改了从下次识别起生效。关掉就全部用 CPU。上次 worker 没能用上加速、改用 CPU 时，开关下写出原因（`acceleration.last`，即共享目录的 `acceleration.json`，按模型记录请求的、实际用的后端、原因和时间 `at`）。每次同步结束都重新读取模型状态，原因跟着最近一次识别变。WD 加速文件装好（下载或导入后改名进模型目录）之前写下的那条 WD 回退记录，`/api/models` 不再报告：宿主比较记录的 `at` 和加速文件的修改时间，早于它的去掉（`app_models.rs` 的 `drop_superseded_wd`，只读 `acceleration.json`），所以重开页面或 App 后也不显示原因和「修复 WD 加速文件」；下一次识别写入新记录后照常报告。CCIP 的记录不受影响。编译缓存和回退见 [安装与平台](BACKEND_TASKS.md#识别的执行后端)。
- **设置 › 网络与隐私**：表里加一行「下载识别模型」，服务列出原始模型镜像、ModelScope 及本平台执行变体来源的主机名，发送的是一次普通的 HTTPS 下载，不带图库信息，「只在你点下载时」。
- **任务面板**：模型没就位或正在下载时列为一项，显示同一行状态、进度条和主按钮；同步停在等待识别时写「识别模型还没下载」，不再写「识别服务未启动」。
- 一次下载或导入让模型从没就位变成就位时，任务结束后提示一次，自动同步一次把等着的图识别掉；开着引导的图库再做一次识别自检。首次下载在同一个任务里先激活原始模型、再下 WD 加速文件，加速文件成功、失败还是取消都算；页面在任务中途打开、无从判断时也算。任务开始前模型已就位的（补下或修复加速文件、就位后的导入）不提示、不同步，失败照旧提示。判断在 `core/models.js` 的 `readyWhenStarted` 和 `modelsArrived`。提示条只有一个，所以模型刚到而加速文件没下成时并成一句「识别模型已就位，开始识别；WD 加速文件没能下载，WD 暂用 CPU。」；没下成的是可选文件时提示里写「WD 加速文件」，不写路径（`ui/models.js` 的 `finishedNote`）。没有点按钮就不下载。

## 界面语言（2026-10-18，#173）

界面语言整个 App 共用，不按图库或浏览器分开：存在应用数据目录的 `preferences.json`（和 `libraries.json` 同一处，`ILLUSTASH_APP_SUPPORT` 可改），`{"language": "system" | "zh-CN" | <译文表的语言代码>}`，默认 `system`。文件缺失、读不了、内容不对，或设的语言的译文表已经不在，都按 `system` 处理，不影响页面打开；写入与 `libraries.json` 相同，持锁读改写、临时文件改名，保留文件里其他键。在「设置 › 通用 › 语言」切换（跟随系统、简体中文，以及每份译文表的语言，语言名用各自的语言写，不翻译），保存后页面重新载入。页面从 `/i18n.js` 得到当前设置；`GET /api/settings` 不含它，回复保持旧录制的字节。`POST /api/preferences` 只接受 `language` 和 `acceleration`（[识别模型](#识别模型2026-10-19168)的加速开关，布尔值）两个键，`language` 取值只能是 `system`、`zh-CN` 和 `web/static/locales/` 里现有且能解析的语言，其他键或取值返回 400、什么都不写，成功返回 `{"preferences": {"language": …, "acceleration": …}}`。偏好不授权联网、不启动任务。

检查更新（#191，A 档：只提示；`native/terms/src/app_updates.rs`）也是整个 App 共用：选择存在 `preferences.json` 的 `update_check`（`"auto"` 或 `"manual"`，没问过时没有这个键），上次结果存在旁边的 `updates.json`（检查时间、最新版本、失败原因、用户点了「暂不」的版本），两个文件的写法同上。默认关闭：新图库的首次引导问一次（不论在哪个图库回答，别的图库都不再问），「设置 › 网络与隐私」有开关和说明；打开后宿主进程每次启动查一次，一直开着时距上次检查满 24 小时再查一次（用户 2026-10-18 定，macOS 上应用可能长时间不退出）；关闭时「设置 › 关于」可以手动检查。检查是不带账号的 `GET https://api.github.com/repositories/1407079068/releases`：按仓库的数字 ID 查，请求不跟随重定向，而仓库改名或转移后 ID 不变，所以已经发出去的版本仍查得到（#358；对仓库的约束见 [发布](DEVELOPMENT.md#发布) 第 10 步）。GitHub 明确答复这个地址查不到时（3xx 或 4xx；403、429 是限流，不算）再按 `repos/shio-vinyl/illustash/releases` 查一次，以这一次为准；连不上、限流和 5xx 不换地址，回复里的跳转地址从不使用。请求只发给 `api.github.com`，User-Agent 固定为 `Illustash-update-check`，不带版本号和任何图库信息；跳过草稿，正式版不提示预发布版（GitHub 标为 prerelease 或 tag 带 `-beta.1` 这类后缀），只认 `vX.Y.Z` 形式的 tag，按 SemVer 取最大。Release 页地址由程序按版本号拼成，不取回复里的链接。有新版本时页面底部提示，任务进行时不显示，点「打开下载页」在浏览器打开；点过「暂不」或打开过的版本不再提示，更新的版本照常提示。`GET /api/updates` 返回状态；`POST /api/updates` 接受 `{"action": "auto", "allow": …}`、`{"action": "check"}`（手动，总会联网）、`{"action": "check", "auto": true}`（只在已打开且到时才联网）、`{"action": "dismiss"}`，以及下面 B 档的 `download`、`cancel`、`install`，其他内容返回 400。联网请求不占用图库会话，不影响切换图库。测试用 `ILLUSTASH_UPDATE_BASE` 指向本机 HTTP 端口的假服务器（B 档的 latest.json 和包也从它取）。

下载并重启（#279，B 档；外壳的 `desktop/src/updater.rs`，Tauri 更新插件）：只在桌面 App 里。`GET /api/updates` 的 `install` 是外壳经本机桥（`/platform/update/status`）报的状态：`available`（这个安装能不能自己更新；不能时 `reason` 是 `unsigned` 没有更新公钥的构建、`unpackaged` 不是打好的包、`translocated` macOS 上没拖进「应用程序」就打开，被系统隔离到随机位置）、`stage`（`idle`、`downloading` 带 `received`/`total`、`ready`、`failed` 带 `error`、`installing`）和 `version`；不在桌面 App 里时为 null。能自己更新时，有新版本的提示和「设置 › 关于」多一个「下载并重启」，「打开下载页」变成次要按钮。侧栏「任务」上方另有一个更新入口（用户 2026-11-14 要求，`drawUpdateButton`）：有新版本就一直显示，不受「暂不」和任务的影响，没有就隐藏；点一下直接开始下载（显示「下载 N%」，这时点了打开「设置 › 关于」），下好后显示「重启更新」，点了先确认再重启；不能自己更新的安装点了打开下载页。点了才联网：外壳下载检查查到的那个版本 Release 里的 `latest.json`（`https://github.com/shio-vinyl/illustash/releases/download/vX.Y.Z/latest.json`，地址由 `app_updates::package` 按版本号拼出，所以通道规则与检查相同，也不依赖哪个 Release 标成 Latest），latest.json 说的版本必须就是它，本平台的包必须在同一个 Release 里；下载跟随 GitHub 到它文件服务器的跳转，用系统代理，只带插件固定的 User-Agent。下好的包用 `desktop/tauri.conf.json` 里的公钥校验签名，签名还要绑定这个版本号（`requireSignedVersion`），然后留在内存里等用户确认；退出 App 就丢掉。下载时提示里显示进度和「取消下载」，页面每秒读一次状态；这期间有任务开始，提示也不收起。下好后「重启并安装」：有任务在运行时按钮不可用；点了先确认，外壳把窗口换成启动页（「正在安装 X.Y.Z…」），停掉本地服务、等它把任务收尾，再安装：macOS 原地替换 `.app` 后重启，Windows 后台运行 NSIS 安装包（被动模式，只有进度条，装完带着原来的启动参数自己打开）后退出；插件留在 `%TEMP%\<产品名>-<版本>-updater-*` 里的安装包，由新版本启动时在后台删掉。下载、校验或安装失败时显示原因，按钮换成「打开下载页」和「重试」；安装失败会重新启动本地服务。密钥和发布步骤见 [发布](DEVELOPMENT.md#发布)。

- **翻译方式**：用中文原文当键（gettext 式）。前端写 `t('中文原文')`，参数用 `{n}`、`{name}` 占位：`t('{n} 项进行中', {n})`。页面语言的译文表里有译文就显示译文，没有就照常显示中文，所以可以分块迁移，没迁到的文字不会坏。`t()` 只返回文本，插进 HTML 时照旧由调用处转义；需要带链接或加粗时把 HTML 作为参数传入，不写进译文。同一句中文在不同地方要译成不同的词时加语境（gettext 的 msgctxt）：`t('移除', {context: 'column'})` 在译文表里查 `"column\u0004移除"`，中文页面照样显示「移除」。英文里两句直接拼接时由译文自带句末空格。CSS 不写界面文字：要生成的文字放在元素属性里由 `t()` 填，样式用 `content: attr(data-more)`。静态页面里的文字用 `data-t`（元素文本）和 `data-t-attr="title aria-label"`（列出的属性）标记，`i18n.js` 在 `app.js` 之前替换一次。
- **译文表**：每种语言一份 `web/static/locales/<语言代码>.json`（现有 `en.json`），文件名就是语言代码（`en`、`ja`、`pt-BR`，小写主语言加可选的连字符子标签，大小写须与文件名一致）；中文是原文，没有表。表里的保留键 `"@name"` 是这种语言用自己写的名字（`"English"`），用于设置里的选项；代码里的 `t()` 不用 `@` 开头的键。加一种语言只需放一份新表，不用改代码。各语言共用的用词见 [中英术语对照](I18N_TERMS.md)。
- **复数**：译文可以写成按 `Intl.PluralRules` 类别分开的对象，由参数 `{n}` 选：`"{n} 张": {"one": "{n} picture", "other": "{n} pictures"}`。`{n}` 可以是数字，也可以是 `number()` 格式化过的整数（`1,234`）；必须有 `other`，类别只能是该语言有的。中文没有复数，键仍是中文原文，调用处不变。英文目前只把最显眼的几处（`{n} 张`、`{n} 位角色`、`{n} 组`、`{n} 个皮肤`、`共 {n} 份副本`）写成复数，其余仍是单一译文。
- **数字与日期**：`number()` 按页面语言用 `Intl.NumberFormat`；月日、年月日、年月和星期在非中文页面用 `Intl.DateTimeFormat`（英文 `5/3`、`5/3/2025`、`May 2026`、`Sun`），中文页面保持原来的写法（「5 月 3 日」「2026 年 05 月」「周日」）。
- **加载**：`/i18n.js` 由原生宿主拼出：先写三行，`self.ILLUSTASH_LANGUAGE`（当前设置）、`self.ILLUSTASH_LANGUAGES`（可用语言 `{代码: 名字}`）和 `self.ILLUSTASH_TABLES`（`{代码: 译文表}`），再接 `web/static/i18n.js`，不缓存。设置为具体语言时只带那一种表；设置为 `system` 时带全部表，由页面选：最终语言由浏览器的 `navigator.languages` 决定，宿主不知道，而页面要在 `app.js` 运行、第一次绘制之前同步拿到表，事后再取会让 `app.js` 等待并先闪一下中文（#182）。表都解析后重新序列化；坏了的表不列入可用语言，它的文字保持中文。`index.html` 在 `splash.js`、`app.js` 之前加载它。`system` 按 `navigator.languages` 的顺序找第一项有表的语言：先比完整代码，再比主语言（`en-GB` → `en`），`zh` 开头的没有对应表时用简体中文；都对不上时有英文表用英文，否则中文。页面的 `lang` 属性跟着设成选中的语言代码。
- **后端发来的文字**（报错、任务和同步的消息、撤销提示、导入导出状态、来源名称、版本渠道、诊断组件名等，#173 第 3 阶段）：后端照旧发送和存储中文，回复字节与数据库内容不因语言改变，翻译消息也不需要后端知道页面语言。后端 Rust 源码里的中文字面量就是模板，用具名占位写（`format!("已将 {n} 张移出 {name}")`，`{n}` 只放数字），和前端共用同一份译文表。页面用 `tb(文字)` 显示：整句是键就直接翻译；否则找文字吻合的模板（`{name}` 代表任意文字，自身文字最多的一条优先），占位分两类（#198，名单在 `i18n.js` 的 `BACKEND_NESTED` / `BACKEND_DATA`）：装着后端自己的嵌套消息的（`{reason}`、`{error}`、`{e}`、`{m}`、`{label}`、`{step}`、`{duration}` 等）同样翻译；其余是用户、图库或外部给的数据（标签名、角色名、作品名、路径、键、数字），原样显示，即使碰巧和译文表的键相同（名为「未识别」的标签）；数据占位不跨「；」。没有模板整句吻合时按「；」分成能对上的最长几段分别翻译；都对不上的照原样显示。所以同步记录、撤销栈里早先存下的中文，只要模板还在也会翻译。模板要写成整句，不在后端把半句拼起来；嵌在 SQL 里的提示改为参数传入。判断后端消息用 `said(文字, '模板')`：比的是后端发来的原文，与页面语言无关，也能对上「前缀：」之后的部分；前缀只认纯汉字的词（「冲突：」「同步失败：」），数据里的「：」不算前缀。冻结录制逐字节比对回复，并对每张表算摘要（见 [ARCHITECTURE_DIRECTION.md](ARCHITECTURE_DIRECTION.md)），所以没有改为发错误码或存模板加参数。后端自己显示的少数文字（macOS 回退的文件选择框标题）由 `app_preferences::tr()` 按请求头里的语言查同一份表。
- **角色和作品名**：页面不是中文时，`api()` 请求带 `X-Illustash-Language`；后端在只读请求里（含只读的 `POST /api/suggest`）按「你自己填的名字 → 英文名 → Danbooru tag 的可读写法（`hatsune_miku` → Hatsune Miku，日文角色即罗马字）→ 原选名」显示角色、皮肤和作品名（`http_read.rs` 的 `entity_name`、`entity_info`、`work_name` 与浏览页的名称表）。不带请求头的请求、写入请求和后台任务看到的名字与原来相同，写进数据库的内容不受页面语言影响。
- **桌面外壳**（#173 第四阶段）：外壳（`desktop/src/i18n.rs`）自己读 `preferences.json`，生成新文案时读取当前语言，切换后新打开的选择框和新产生的报错随即生效；已经创建的菜单和启动页初始化脚本沿用创建时的语言。`system` 按系统首选语言（macOS `CFLocaleCopyPreferredLanguages`，Windows `GetUserPreferredUILanguages`）用和 `i18n.js` 相同的规则选表。译文表由 `desktop/build.rs` 在编译时从 `web/static/locales/` 读进外壳，不另存一份，所以资源缺失时的启动错误也能翻译。外壳自己的文字（菜单「刷新页面」、文件夹选择框标题、启动错误页的原因、完整安装包检查、Pixiv 登录窗口标题和它的错误）用 `tr("中文原文")` / `tr_with("…{error}", …)`，键和图库页共用。启动页 `desktop/dist/index.html`（欢迎页、加载中、启动失败）加载和图库页同一份 `i18n.js`（`build.rs` 复制，与 `splash.js` 相同）；主窗口的初始化脚本只在本地启动页注入那三行，`ILLUSTASH_LANGUAGE` 是外壳算好的具体语言，表只带那一种；图库页在桌面里调用的文件夹选择框也由外壳弹出。启动页上来自后端的文字（启动进度的阶段名、建库与导入的报错、出处名「其他」、没能登记进最近图库的原因）和图库页一样用 `tb()` 显示；失败页下方的日志是子进程原样输出，不翻译。macOS 默认菜单（关于、编辑、窗口）由 Tauri 生成，中文和英文下都是英文。
- **安装程序**（#173 第五阶段）：Windows 的基础版和完整版 NSIS 安装程序都带英文和简体中文（`packaging/tauri-bundle.json`、`tauri-full.json` 的 `languages`，英文在前），不弹语言选择框，由 NSIS 按 Windows 显示语言选：中文（含繁体）用简体中文，其他语言用英文，与 `i18n.js` 的 `system` 规则一致。只看显示语言这一项：显示语言是没有译文表的语言、首选语言列表后面才有中文时，安装程序是英文而 App 是中文。界面文字是 NSIS 和 Tauri 自带的译文，仓库不另存。Tauri 的卸载程序从注册表 `HKCU\Software\<制造商>\Illustash` 的 `Installer Language` 取语言，没有这个值就先弹英文的「Please select a language」选择框，而 Tauri 的安装程序从不写它，所以共用钩子 `packaging/nsis-hooks.nsh` 在安装后写入这次安装用的语言（完整版钩子 `nsis-full-hooks.nsh` 引入它）；卸载时勾选删除应用数据会连同这个值一起删掉。安装程序不写 `preferences.json`：语言设置照旧是 `system`，首次启动的欢迎页按同一规则显示，和安装界面一致。没有翻译的：MSI（WiX 一种语言一个文件，保持英文，用户安装用 NSIS）；便携测试包里的 `first-start.cmd` 提示、`START-HERE.txt`（英文）和 `INSTALL.md`（本仓库中文文档的副本），给测试者用，不是普通用户的安装途径；快捷方式和「应用」列表里的名字是产品名 Illustash。macOS 是直接拖动的 App，没有安装界面。
- **检查**：`tests/i18n.test.cjs`（本机全量的 Node 测试）收集 `web/static` 下各 `.js`（含 `core/`、`ui/` 的模块）里的 `t('…')` 字面量和 `index.html` 的标记文字，以及桌面启动页的 `t('…')` 与标记文字、`desktop/src` 里的 `tr("…")` 字面量（外壳的 `tr()` 也只能用字面量），要求 `t()` 只用单引号或双引号字面量、语境写成 `语境\u0004中文`；另收集 `native/terms/src`、`crates/*/src`（测试模块以外）的中文字面量和 `native/terms/application.json` 的渠道名作为后端文字。对 `locales/` 里的每一份表分别检查：有 `@name`，键要么是前端用的文字、要么是后端字面量，译文的占位符与中文一致，复数类别合法且有 `other`；每种语言分别报告前端和后端文字的覆盖率（后端不计命令行、测试工具、日志和 SQL），没翻译的条目只报告数量，不算失败。另外三条会失败：样式表的 `content` 里写了中文；代码拿中文字面量做 `/…/.test`、`includes`、`startsWith` 一类判断（英文页面上永远不成立，应改看状态字段，后端消息用 `said()`）；`said()` 的模板不是后端源码里的字面量；后端模板（有自己的中文字样的）里出现的占位名不在 `BACKEND_NESTED` 或 `BACKEND_DATA` 里，或两边都在，或名单里有已经没有模板用的名字。页面模块的测试用 `tests/web.cjs` 选语言（见「前端模块」）。
- **后端不看自己的文字做判断**（2026-10-18，#204）：`tests/backend_text_logic.test.cjs` 检查 `native/terms/src`、`crates/*/src`、`desktop/src` 测试模块以外的代码，不许拿中文字面量做比较（`==`、`!=`、匹配模式）或查找（`contains`、`starts_with` 一类）；读的是别人的数据（如 Bangumi 的字段名）时在测试里列为例外。要按失败种类走不同分支的，失败本身带上种类（Pixiv 账号查询的 `PixivError`、证据 JSON 的 `http_json::Refusal`、研究 worker 的 `Stall` 等），显示的文字、回复字节不变。撤销提示里的张数只存在提示本身（撤销载荷里没有操作种类和张数）：几种句式在 `crates/core/src/undo_labels.rs` 一处定义，写提示和把提示里的角色换成现名都用它，读的是已存数据的格式；#201 在载荷里存下种类和张数后，这段解析删掉。
- **范围**：`web/static` 里前端自己写的文字已全部迁移（导航、顶栏、首页、图库与网格、详情面板、大图、资料库与作品书架、未识别分组、重复与相似、身份整理、画师抽查、设置各页、任务面板与前端拼出的状态行、各种对话框、新手引导与上手清单、启动画面的「准备好了」），英文全部译完。几部常见作品的中文名（`WORK_NAMES`）也经 `t()`，英文页面显示官方英文名。后端发来的文字和角色、作品名按「后端发来的文字」「角色和作品名」两条显示，英文全部译完。桌面外壳（欢迎页、启动画面与启动进度、启动错误、菜单、选择框）也已迁移（第 4 阶段）。外壳与核心之间的协议文字（`bridge.rs` 的报错）和终端日志不翻译；macOS 默认菜单由 Tauri 生成。Windows 安装程序见「安装程序」一条（第 5 阶段）。

## 数据模型与迁移

- `entities.parent_id` 指向父角色，`variant` 保存子标签。实测按 **相同 base_tag + 相同 copyright + skin 为空** 唯一匹配父角色，不能用 char_id 直接等于 base_tag。没有唯一匹配时创建稳定的本地父实体，不猜角色。在当前快照上得到 540 个变体、19 个本地父实体。
- `family_evidence` 是可重建的家族证据汇总：来源集合、最高分、来源数、是否仅 CCIP。迁移与 refresh 在事务中重建。索引覆盖 `(entity,uuid)` 和 `(uuid,entity)`；源候选保留各变体证据。
- `family_confirmed`、`memberships`、`variant_memberships` 是即时派生视图。人工改判后撞脸隐藏立即重新计算，不保存隐藏标记，也不写拒绝记录。已离开素材范围的人工记录继续保存，但不计入当前成员数。
- `reviews(entity,reviewed_at,action_id)` 记录「检查完了」。一次操作把当时该父角色全部机器归属写为 `confirmed / sweep`，与 action 同事务提交；撤销恢复各图片此前的断言及暂记未知状态，并撤回本次检查记录。重复检查允许记录零张，新增机器候选仍显示为未检查。

版本记录在 `meta.schema_version=2`。已有库改表前用 SQLite backup API 备份，重新打开备份验证 `PRAGMA quick_check=ok`；失败即停止。DDL、层级、证据与版本号在同一个 `BEGIN IMMEDIATE` 事务中迁移，避免 executescript 的隐式提交。重开 V2 库不重复迁移/备份；拒绝未知较新版本。

迁移不改写任何旧 `assertions / deferred / actions` 行，旧 action JSON 仍可撤销。旧相簿确认若恰好挂在变体实体上，通过读视图归到父角色；不把相簿身份解释为服装确认。新相簿导入直接挂父角色。`imported` 保证刷新不会重新导入已移除或撤销的旧相簿结论。

## 归属规则

父角色的人工移除优先，屏蔽该角色及全部子标签。其余情况下，父角色或变体的人工确认产生人工归属；有任一父/子机器候选产生机器归属。机器证据只有 CCIP、且图片已人工确认为其他父角色时隐藏该机器归属；家族内任何 WD 候选均豁免此隐藏。人工确认优先于隐藏，多角色确认继续支持。

子标签仅在父角色仍归属时显示：人工确认子标签，或存在该子标签候选且未人工移除子标签。移除子标签保留父角色归属。父角色已被移除时，添加子标签返回 400，要求先显式恢复父角色；不会暗中清除父角色移除记录。

`unknown` 仅保留暂记未知标志，不删除现有身份。未识别池 = 没有任何当前归属，且未暂记未知。全部查询、搜索、筛选写操作共用同一套归属规则。

**设置 › 角色资料 › WD 角色扩展**（#550）的开关和重扫记录按图库保存。扩展包在共享目录的 `extensions/wd-extension-v1` 下，使用与识别模型相同的续传和 SHA-256 校验，只有点击下载才请求 GitHub。下载及导入走独立任务槽；移除文件后，已有候选保留。包重新就位后，后续同步继续自动检查新批次，缺包期间的结果可用整库重新检查补齐。

- `GET /api/wd-extension` 返回 `package`、共享的 `download`、图库的 `library` 和 `task`。图库的 `vectors`、`images`、`assigned` 为现算数；`library.blocked` 为 `null`、`frozen`、`no_vectors`、`syncing` 或 `busy`。
- `POST /api/wd-extension/package` 的 `action` 为 `download`、`import`（可选 `paths`，缺省打开原生选择器）、`cancel`、`discard` 或 `delete`。选择器取消返回 `{cancelled:true}`，其余成功应答返回完整状态。
- `POST /api/wd-extension/enable` 和 `/withdraw` 各接收空对象并返回完整状态。手动检查在 `scan` 阶段通过 `POST /api/wd-extension/cancel` 传 `id` 取消，进入 `write` 后完成事务。完成报告用 `result.changed` 表示候选有变的图片数。同步期间，每批 WD 扩展结果随该批识别投影一同提交。中断后保留已发布的结果和评分计数，下次同步继续未发布的交付。
- **设置 › 角色资料 › CCIP 长尾包**（#559）紧跟在 WD 角色扩展后面，用同一套卡片、确认框和任务面板项（`web/static/ui/packs.js`、`core/packs.js`）。`/api/ccip-longtail` 的路由和回复形状与 `/api/wd-extension` 相同；参考包放在共享目录的 `extensions/ccip-longtail-v1/`，启用状态和候选按图库保存。`library` 另带 `needs_scan`（角色索引更新后要重新检查）和 `error`（上次自动检查失败的原文）。已启用时重新检查失败，卡片仍提供撤回。
- **角色资料包**（#602）：2026-11-17 起，上面两张卡片合成一张「角色资料包」卡片（`packCard`，`web/static/ui/packs.js`、`core/packs.js`），一个包、一个版本、一个开关，默认启用。页面只读 `GET /api/packs`：`{catalog:{configured,serial,checked_at,error}, current, offer, download, location, library, task, wd, ccip}`。`current` 是卡片上的版本（本图库启用的，否则本机已有的最新的，都没有就是最新的），`offer` 是比它新的最新一个（只按修订号比）；两个都带版本名 `version`、`stage`（`preview` 是测试版，卡片上版本号后面写「（测试版）」，否则是 `release`）、`components.wd`、`components.ccip`（各自的格式、头数、文件）和两个文件在本机下载了多少（`status`：`ready`、`partial`、`missing`；`have`），`offer` 另带 `downloading`。`download` 是两个组成部分的下载或导入合成的一个状态，导入结果按文件名合并；首次准备时宿主先读一次包目录再下载（#630），读目录期间 `download` 是 `{kind:"catalog",status:"running"}`，卡片写「正在读取角色资料包目录」，没有取消按钮。`task` 是启用、换包、撤回的任务（`part` 是正在处理的组成部分；两个组成部分各自提交，WD 部分提交以后 CCIP 部分失败或取消时，`result.wd` 留着 WD 部分的结果）。`library` 写本图库启用了哪个版本、两个组成部分各自启用没有、合起来的 `images`（按图去重）、`assigned`、`vectors`、`checked_at`、`blocked`，以及默认启用的提示 `prompt`：`auto`（还没有识别结果，第一次同步前记成启用）、`pending`（新图库第一次同步时包还没到，#630：包就位后宿主等后台任务结束、基础角色索引启用以后就启用，检查已有的识别结果；包已就位时页面每秒读一次，直到看到启用任务）、`ask`（已有识别结果、没决定过，卡片提示一次）、`declined`（选了不用或撤回过）、`null`（已启用）。`wd`、`ccip` 是两个旧路由的完整回复。
- `POST /api/packs` 的 `action`：`download`（可带 `version`）、`import`（可带 `paths`，缺省打开原生选择器；离线包里的 `packs.json`、`packs.json.sig` 先验签采用，结果列为 `catalog`、`catalog_old` 或 `catalog_invalid`）、`cancel`（不带 `id` 停下载或导入，带 `id` 取消扫描阶段的检查）、`discard`、`delete`、`enable`（可带 `version`，就是换包；图库还没有某种向量时，那个组成部分只记下启用了哪个文件）、`withdraw`、`decline`。成功时回复完整的 `GET /api/packs`。
- 目录只在检查更新真的去查了 GitHub 时（`POST /api/updates` 的手动检查，或领到的自动检查）顺带读，结果记在 updates.json 的 `packs_checked_at`、`packs_error`，不进 `/api/updates` 的回复；`GET /api/packs` 只读本机缓存。卡片上「有新版本：N 个角色」一行在按钮和折叠区之间：没下载时「下载新版本」（按还要下的大小写），下好后已启用的图库「换成新版本…」（确认框写要检查的张数、换掉什么、几个角色只给候选）。页面画卡片时读一次，检查更新的时间变了、下载或任务结束后再读。
- **补角色以资料包为主**（#606，2026-11-18）：「设置 › 角色资料」的顺序是角色资料包、检查新角色、角色名称、名称翻译。检查新角色只补两次发包之间新出现的角色，入口显不显眼按上次检查的覆盖情况（`GET /api/index/coverage`，#561）定：有关注作品、上次检查时有结果的作品都不缺、检查没有在跑也没有暂停或出错时，检查新角色卡片只留一行说明，其余收进「检查设置与记录」折叠区；还缺时，资料包卡片底部写「关注的作品还缺 N 个角色」，上次检查找到的还没添加的整行链到资料库的「作品」，否则带「检查新角色…」按钮（联网前照常确认）。还没检查过、检查早于 #561 或 Danbooru 不认识全部关注作品时不知道缺不缺，检查新角色卡片照常展开，资料包卡片上不写。覆盖数字是上次检查时的，之后启用资料包或改关注作品要再检查一次才更新。
- 两个旧路由 `/api/wd-extension`、`/api/ccip-longtail` 留作两个组成部分的状态和操作，回复形状不变，`package` 说的是卡片上那个版本里的这个组成部分：本图库启用了、或本机已有某个版本时，读到目录前后逐字节相同；本机一个版本都没有时，直接说明最新的版本。它们的 `package` 下载和 `enable` 也认 `version`（角色资料包的版本名）。
- 详情栏「也可能是」把 `WD_EXT` 候选放在原有强线索和弱线索之间。同一角色已有强线索时保留原来的候选项。CCIP 长尾候选排在 WD 扩展候选前，两种包指向同一角色时只显示 CCIP 长尾项，两条证据都保留。人工确认和否定照常保留，撤回操作通过设置完成。

### 可疑优先

按以下元组升序排列，最后以 UUID 稳定打破平局：

```
(人工确认 ? 1 : 0,
 来源数 == 1 ? 0 : 1,
 仅 CCIP ? 0 : 1,
 最高分（空值取 1，按 12 位小数比较）,
 存在其他人工父身份 ? 0 : 1,
 UUID)
```

角色页只取该父角色的键；全图库取各身份中最靠前的键，无归属图片排在前面。来源分数只用于该排序与证据展示，不解释为身份正确率。SQL 用定宽字符串一次计算元组，避免逐图重复查询五次。

## API 契约

### 读取

- `GET /api/characters?q=&min=0`：父角色数组，字段 `id,name,work,count,unchecked,cover,variants`；每个子标签含 `id,name,count,parent_id`。`unchecked` 为当前机器归属数。
- `GET /api/assets`：`entity,variant,q,tag,pool,status,order,offset,limit`。`status=all|confirmed|machine|unknown`；`order=suspicious|newest|oldest|hinted`（`hinted`：有未被否定的机器线索的图排前面，强证据先于弱线索、分数高的在前，其余按最新）；每项带 `hint`：尚无归属的图给出最强未否定线索 `{id,name}`，否则为 `null`；`pool=unmatched`。`variant` 使用实体 ID；也兼容把变体 ID 作为 entity 传入。分页上限 200。 `count=0` 可跳过总数统计，此时 `total` 为 `null`，图片字段与排序不变；前端仅在同一列表读取周期内复用总数，重新加载时重新统计。
- `GET /api/assets/count`：使用图库相同筛选参数（搜索使用 `q`，不接收分页参数），仅返回 `{total}`，不加载图片附属信息。
- `GET /api/home` 和 `GET /api/calendar` 共用 `/api/assets` 的筛选。按本机加入日分组，组内图片按指定的 `order` 取样，默认先取当天最新加入的。两条接口仍按日期排日组。按天的 `days` 默认 5，范围 1–20，`before` 是下一页的排除上界。当天的计数、角色和取样使用同一个选区。
- `GET /api/calendar?month=YYYY-MM` 返回 `{days,total,prev,next}`。月份必填，日组按日期升序，只返回有匹配图的日子。每组为 `{day,count,samples,color}`，`day` 为 `YYYY-MM-DD`，`samples` 最多 4 张，与按天视图的图片对象一致，并接受 `marks`。`total` 是全月匹配图数，`prev`、`next` 是筛选后最近的有图月份（`YYYY-MM`），没有对应月份时为 `null`。
- 组合筛选（#445）在原筛选上叠加 `has`、`not`、`from` 和 `to`。`has` 与 `not` 是 URL 编码的 JSON 字符串数组，分别要求全部满足和全部排除。可用 `char:<id>`、`work:<id>`、`artist:<key>`、`tag:<WD 标签>`、`utag:<我的标签>`、`source:pixiv|x|other` 和 `fav`。条件只按首个冒号分隔，重复项去重，合计至多 20 条，每条至多 160 字符。日期区间沿用加入日期，两端包含，可只提供一端。无出处的图归入 `other`。列表、计数、ID、按筛选选图的写入及画廊封面使用同一选区。
- `GET /api/user-tags?limit=` 可指定 1–1,000 条，默认 60 条。
- `GET /api/galleries` 按 `position,id` 返回 `[{id,name,params,position,cover_manual}]`，`position` 是保存的整数顺序值，可能不连续；创建和更新回复的 `gallery` 同样带 `position`。保存的条件失效时仍返回该画廊。手选画廊另带 `kind:"manual"`，`params.has` 是包含 `gallery:<id>` 的 JSON 数组字符串。条件画廊沿用原字段。
- `GET /api/galleries/covers?ids=&nsfw=` 按需读取当前封面。`ids` 为 URL 编码的 JSON 整数数组。按画廊顺序返回 `[{id,cover}]`，重复 ID 去重、缺失的 ID 跳过。手动封面仍在当前选区且符合调用方的 `nsfw` 时使用它，否则取排序第一张。空选区或失效的保存条件返回 `cover:null`，打开图片列表时沿用筛选错误响应。
- `GET /api/similar?kind=&nsfw=&seen=1`（#427）：带 `seen=1` 时每组多一个 `seen` 布尔值，表示这组看得到的成员是否都在某一次「看过了」里；不带时回复和以前相同。
- `GET /api/identity/count?nsfw=...`：复用身份待判断队列的筛选、去重与跳过规则，仅返回 `{total}`，不排序或加载展示名称、样例图。
- 每张图片返回 `memberships`，每个父角色包含 `status,origin,evidence:{sources,max_score},variants`。状态区分 `confirmed` 与 `machine`；`confirmed` 数组为前端兼容字段。
- `GET /api/detail?id=` 另返回 `ownership`，包含有效归属与 `rejected` 父角色记录，每项带 origin 和证据。原始 assertions/candidates 继续返回，保留历史人工结论和逐来源证据。
- `GET /api/ids` 使用相同筛选和排序，直接返回 UUID 数组；支持 offset/limit，limit 最大 5000，供跨区段选择使用。
- 收藏（#447）：`has=["fav"]` 只看收藏，`not=["fav"]` 排除收藏。实际请求对 JSON 数组做 URL 编码，条件与其他筛选取交集，`has` 和 `not` 合计至多 20 条。列表、计数、ID 和按筛选选图的写入都用这一口径。图片本身或重复组内任一份有有效收藏，即算收藏。`GET /api/assets`、`GET /api/home`、`GET /api/calendar`、`GET /api/detail` 带 `marks=fav` 时在每个图片对象末尾附布尔字段 `fav`，详情在顶层附该字段。省略 `marks` 时沿用原回复。
- `/api/entities` 仍用于查找或创建实体；`/api/review-pools` 仅返回「未识别」。`/api/tasks` 返回 404；pending/conflicts 筛选返回 400。
- 图片主色（#717）：`/api/assets`、`/api/home`、`/api/detail` 带 `marks=color` 时在图片对象末尾附 `color`（`#rrggbb` 或 `null`）；可和收藏合用 `marks=fav,color`。详情再附最多五项 `palette: [{color,share}]`，占比取 0–1，未就绪时给空数组。画廊封面、`/api/characters` 和 `/api/artist-directory` 在同一对象上附封面主色。`/api/works?marks=color` 附 `covers: [{uuid,color}]`，从作品与子作品中取最多四张最近加入的可见图片，样图接受 `nsfw`。省略颜色标记时沿用原回复。
- `order=color` 按红、黄、绿、青、蓝、紫分段，段内明度升序，灰度随后，未算颜色的图排在最后。同键按加入时间从新到旧，再按 UUID。图库打开、导入和同步后补算缺失的颜色，优先读已有缩略图，缺失时复用缩略图解码从原图取样。派生结果在该包的隐藏缓存 `.illustash-colors-v1.sqlite`，与 JPEG 缩略图分开。删除颜色缓存后可补算，回退应用版本无需迁移图库。

### 写入与撤销

`POST /api/change` 接收 `action=confirmed|rejected|unknown|resume|add_variant|remove_variant`；后两者的 entity 必须是子标签 ID。`action=confirmed` 可改传 `entities`（1–20 个角色 ID，与 `entity` 二选一），一次设为多个角色，作为一条撤销记录。`POST /api/reassign` 接收不同的父角色 `old,new`；一次拒绝旧角色并确认新角色，不触碰其他身份。单图和批量纠正统一使用 `/api/reassign`。

change/reassign 的选择方式二选一：

```json
{"ids":["uuid-1"],"entity":"角色 ID","action":"rejected"}
```

```json
{"filter":{"entity":"角色 ID","variant":"子标签 ID","q":"","tag":"solo","order":"suspicious"},"exclude":["uuid-1"],"entity":"角色 ID","action":"rejected","confirm":true}
```

服务端获取 `BEGIN IMMEDIATE` 后解析筛选、排除、校验并写入，选集与写入不会分离到两个事务。实际选集超过 500 张必须明确 `confirm:true`；一次只有一个 action，整批撤销。空选集或非法 ID 返回 400，不留下部分写入。筛选对象不接受未知字段，防止拼写错误意外扩大操作范围。

`POST /api/sweep {"entity":"父角色 ID"}` 检查该角色全部机器归属，不受当前前端分页/筛选影响。`POST /api/undo {"id":123}` 撤销最近未撤销 action；跨重启保留。写接口继续要求同源与 CSRF token。

`POST /api/favorite` 沿用 `/api/tag` 的 `ids` 或 `filter` 选图、`exclude` 与 `confirm` 规则。默认收藏，`add:false` 取消。对选中的重复组写入全部副本，提示张数按组计。回复 `{"undo":…}`，在从未建立收藏词条的库里直接取消会返回 `{"undo":null}`。人工裁决与快照规则见 [词条数据模型](TERMS_SCHEMA.md#人工标记447)。

相似组的「看过了」（#427）：`POST /api/similar/seen {"gid":组号,"size":张数,"nsfw":"","seen":true}` 记下这组现在看得到的成员（按 `nsfw` 筛选，和列表同一口径），`seen:false` 把这组的图从覆盖它的标记里去掉。`size` 是界面显示的张数，和现在的成员数对不上时回 409（组刚变过），组不存在或只剩一张回 400，已经是要的状态也回 400。返回 `undo`，撤销恢复原来的行，不触发学习。存储见 [词条 schema](TERMS_SCHEMA.md#相似组的看过了427)。

条件画廊将当前筛选保存到图库。对 `POST /api/galleries` 传 `action=create` 和 `params`，可带 `name`，省略时由条件生成。传 `action=update`、`id` 和待改字段 `name,params,cover,position`，其中 `cover:null` 恢复自动封面。传 `action=delete` 与 `id` 可删除。每次操作随 `undo` 返回一步历史，创建和更新另返回 `gallery`，写入回复中的封面按隐藏 NSFW 解析。手选画廊用 `action=create, kind=manual, name` 新建，可带 `ids` 或整批筛选选区。传 `action=add` 或 `action=remove` 与 `id`，目标按 `/api/change` 的 `ids` 或 `filter` 加 `exclude` 给出，超过 500 张以 `confirm:true` 确认。放入已在画廊里的图时成员保持唯一。增删成员各返回 `gallery`、`undo`。条件和手选画廊共用改名、排序、封面和删除操作，手选画廊的成员通过 `add`、`remove` 改动。图库的 `has` 和 `not` 接受 `gallery:<id>`，其中 `id` 是手选画廊编号。现行存储和备份口径见 [词条 schema](TERMS_SCHEMA.md#17-第-11-版04)。

## 阶段 1.5–3（2026-09-23，Claude 直接实现）

用户在第 1 阶段后决定：单人图只保留一个角色、全站显示英文 tag 原样、「暂记未知」改为「跳过」、角色页点击即选中。随后三个阶段合并实施。`schema_version` 升为 4；迁移前照旧备份并 `quick_check`，派生对象（`family_evidence`、`single_assets` 与三个视图）每次迁移先删后建，人工表只做加列/建表。

### 单人图规则
- `single_assets` = general_hits 含 `solo`，且不含 `multiple_girls / multiple_boys / multiple_others / 2girls–6+girls / 2boys–6+boys`。`multiple_views`、`multiple_persona` 等同一角色重复出现的标签不算多人。
- `family_evidence` 增加 `wd / ccip / rank`：每张图的机器家族按「WD 与 CCIP 都有 > 只有 WD > 只有 CCIP，再比 WD 分数、CCIP 分数、实体 ID」排名。单人图只有 rank=1 计入归属；已有任何人工父身份时，机器归属全部降为「也可能是」。人工可以明确给单人图确认多个角色。
- 人工移出排名第一的角色后，不自动提升第二名（避免移走一个又冒出一个）；该图进入未识别，详情里仍列出「也可能是」。
- 真实库只读副本前后对比：单人图多角色约 1 万 → 0；机器归属约 7.3 万 → 5.3 万；有归属图片只少 1 张；撞脸隐藏不变。相簿单人图首位正确率 91%（排除一个模型不认识的角色后 95%，难例 82%）。

### 显示名
- `entities.tag`：索引角色取 `characters.danbooru_tag`（缺失时 char_id），`wd:` 实体取 WD tag，`album:` 实体取相簿 key，本地新建角色为空（显示用户起的名字）。变体显示为「父 tag · skin」。
- 相簿原名不再覆盖显示名，只作别名；日/中/英名、相簿名都能搜到，`/api/entities` 返回 `matched` 说明命中的别名。

### 新增 API
- `POST /api/suggest {ids|filter,exclude}`：所选图片共有的「也可能是」，按出现次数排序，供选择器建议。
- `GET /api/characters?id=`：按 ID 取单个角色；返回增加 `reviewed_at`。
- `GET /api/assets` 增加 `utag`、每项 `user_tags`；`pool=skipped` 为已跳过；`/api/review-pools` 返回 `unmatched` 与 `skipped`。
- `GET /api/detail` 增加 `alternatives`（带 wd/ccip 分数）、`single`、`user_tags`。
- `/image?size=` 支持 `thumb 400 / medium 800 / preview 1600 / full 3000`。
- 人工通用标签：`asset_tags(uuid,tag,origin,created_at)`，`GET /api/user-tags`，`POST /api/tag {ids|filter,exclude,tag,add}`；撤销快照记录原有标签行，刷新不影响。
- 操作标签改为可直接显示的句子，例如「已将 23 张从 A 换成 B」「已跳过 3 张」。

### 前端
- 路由：`#/`、`#/characters`、`#/c/<id>[/<variant>]`、`#/unmatched`、`#/skipped`，查询参数 `q / tag / utag / sort`；刷新与前进后退恢复视图（打开的图片与缩放档位不进 URL，档位存 localStorage）。
- 侧栏：图库 / 角色 / 未识别 + 置顶 + 最近看过 + 我的标签。角色索引按作品分组，少于 5 张的折叠。（2026-10-05 起「角色」改为「资料库」，分「角色 / 作品」两页，见 README「能做什么」。）
- 缩放：5 档（96/140/185/260/380 px），滑块、⌘±、触控板捏合（wheel+ctrlKey 与 Safari gesture 事件），以指针下的图片为锚点；最小档强制方形，次小档可切方形；大两档用 medium 缩略图。
- 选择：拖过卡片按阅读顺序连续选择/取消（起点状态决定），空白处框选，贴边自动滚动；Shift 跨未加载区段调用 `/api/ids`；⌘A 为「筛选条件 + 排除集」。超过 500 张时主按钮就地变红要求再点一次。
- 写操作：先淡出、服务端成功后本地收拢空位，再静默换入新数据，不跳滚动位置；提示条带「撤销」，悬停不消失。
- 选择器（命令面板）：换成 / 也是 / 设为角色 / 标签 / 跳到角色共用。空输入时依次给出「也可能是」「最近使用」「常用」；「最近使用」的角色和标签各记最近 8 个，按图库分开存在浏览器里（键带 `library.json` 的图库 ID，#523），换了图库只列当前图库用过的；1–9 直选；底部一行说明将发生什么，Enter 直接执行（不再多一步确认）。标签模式可采纳 WD 通用标签或新建。
- 详情与大图：角色胶囊（空心点机器、实心点人工），悬停 ⇄ 换成、× 不是 TA；子标签小胶囊；「也可能是」点击换成、⌥ 点击追加；大图支持捏合/⌘ 滚轮/双击放大、拖动平移，显示本地副本实际尺寸，操作后停在同一位置即自然前进到下一张。

### 与提示词的偏差
- 未做「只看已选」与可疑排序中的「未检查 / 已检查」分隔线：虚拟网格按索引计算位置，插入分隔行会破坏选择与缩放锚点；标题处已显示未检查数。
- 选择器选中后不再单独确认一步，改为底部实时说明 + Enter 执行，少一次按键。
- 图库页没有单独的「设为角色」与「也是」两个按钮（在图库中两者等价），合并为「加角色…」。

## 缩放重做与 NSFW 筛选（2026-09-23）

- **缩放**：原 5 档滑块有三处问题——无指针时以视口中部为锚点，反复缩放后滚动位置逐步漂移（从顶部开始也会跳到数万 px）；最大档在窄窗口下变成单列，网格高度约 4,400 万 px，超过浏览器约 3,300 万 px 上限后滚动失效；档位跳变生硬。现改为连续目标列宽（72–480 px，滑块按对数刻度），实际布局按整列吸附；每次列数变化用 FLIP 动画让卡片从旧位置滑到新位置。锚点为指针下的图片，无指针时为首个可见行；在顶部缩放保持在顶部。网格高度超过 3,000 万 px 时自动增加列数。⌘± 每次增减一列；触控板捏合按帧合并。列宽 <110 px 强制方形，<200 px 可切换方形，列宽乘屏幕倍率（`devicePixelRatio`，按 1 封底）超过 260 物理像素使用 medium 缩略图（#452；首页按行高、详情栏按小图高度，同一个 `core/format.js` 的 `thumbSize`），图库页 ≥150 px 显示角色名。
- **NSFW**：`asset_rating` 保存每张图 WD 评级的首位（可重建，随通用标签索引刷新；首次启动自动建立）。NSFW = `questionable` 与 `explicit`，`sensitive` 仍显示。筛选键 `nsfw=hide|only` 适用于所有列表、`/api/ids` 与按条件批量操作；隐藏模式下角色封面只取非 NSFW 图片。顶栏按钮循环「显示 → 隐藏 → 仅看」，H 键一键隐藏/显示，选择记在本机浏览器。`schema_version` 升为 5（迁移前自动备份）。

## 归属结果存表（2026-09-23）

- **问题**：`memberships` 原为视图，每次查询都对全库重新求值归属规则。侧栏角色列表 415 ms、未识别页 306 ms、可疑优先 174 ms；且每次写操作后前端都会重拉角色列表与侧栏计数。
- **做法**：规则本身改名为视图 `membership_rules`（唯一的规则定义，未改动任何规则）；`memberships` 改为存储其结果的表，并附 `skey`（可疑优先排序键，与原先查询时拼接的元组完全相同）。迁移、`--refresh`、通用标签重建时全量重算；`change / reassign / sweep / undo` 在同一写事务内只重算涉及的图片（归属只依赖该图自己的断言与证据）。人工标签不影响归属，不触发重算。`variant_memberships` 仍为视图，读取存表。
- **一致性**：`tests/` 的 `tearDown` 在每个测试后逐行比对存表与 `membership_rules`；故意去掉撤销时的重算会让 18 项测试失败。直接改库（手工 SQL）后必须重算存表（核心 `crates/core/src/character_projection.rs`）。
- `schema_version` 升为 6；迁移前照旧自动备份，人工表不变，仅重建派生对象（真实库约 2 秒）。
- 改成存表后（真实库只读副本，5 次中位数），侧栏角色列表、可疑优先首批和未识别页首批从 170–420 ms 降到 50–80 ms，侧栏计数从 150 ms 降到 15 ms。

## 重复图（2026-09-23）

- **口径**：Photos 原图稳定哈希（源库 `assets.stable_hash`）相同即为一组。README 记录的核查结果为同哈希 SHA-256 逐字节相同，因此这是完全相同的副本，不含缩放版或差分图（这类近似重复由下文「全库找相似」和「按关系折叠」处理）。只统计当前素材范围内的图；已删除的副本不算，所以剩下一份的组不显示。真实库：约 1,900 组 / 3,900 张（绝大多数 2 张一组，最多 7 张一组）。
- **数据**：`duplicates(uuid,grp,size,pos,grp_added)` 为可重建派生表，迁移与全量刷新时从源库重建；`pos` 按加入时间排序（1 为最早一份）。`schema_version` 升为 7。
- **API**：`pool=duplicates` 只看有副本的图，按「组内最新副本的加入时间」排组，同组相邻、组内从早到晚（选可疑优先时不强制相邻）。`dup=mixed` 只看角色不一致的组：同组副本的父角色集合不同即算，人工确认与机器识别的差别不算。每张图返回 `dup:{grp,size,pos}`；`/api/detail` 返回 `duplicates`（其他副本及其角色名）；`/api/review-pools` 增加 `duplicates:{count,groups,mixed}`。筛选也适用于 `/api/ids` 与按条件的批量操作。
- **前端**：侧栏「重复」；卡片右上角 `1/2` 角标，同组同色；顶部「全部 / 角色不一致」切换；点击看详情（和图库一样，选择需要 ⌘ 点击或拖动），详情里列出其他副本，点击切换。
- **真实库**：角色不一致 48 组 / 106 张。分页首批 29 ms，不一致筛选 64 ms，计数 31 ms。
- **没做**：删除副本。Photos 只读，删除请在 Photos「实用工具 → 重复项」里合并，然后再同步。也没做「同步副本归属」按钮：可以选中同组两张后用「加角色」/「不是…」手动对齐。

## 折叠副本（2026-09-23）

方向（用户拍板）：图库是收藏的**索引**，Photos 只是来源；应用不删除 Photos 里的图。同一组字节相同的副本视为同一件作品。

- **折叠**：`duplicates.rep` 为代表图（分辨率最高，同分取最早加入）；`hidden = rep=0 AND 角色一致`。隐藏的副本不出现在除「重复」页以外的任何列表、`/api/ids`、按条件批量操作，也不计入总数、角色数、子标签数、未识别/已跳过数和我的标签数。`hidden` 与归属一样在写事务内按组重算；测试 `tearDown` 核对它与重新计算的结果一致。
- **角色不一致的组先不折叠**（方案 B）：同组全部副本继续可见，等用户在重复页对齐后自动折叠；不自动取并集。
- **写操作同步到整组**：`change / reassign / tag` 把所选图片扩展到同组全部副本，同一事务、同一次撤销；提示条按作品计数（1 张而不是 2 张）。`reassign` 只校验用户选中的那几张原角色归属；添加子标签时，父角色已被移出的副本保持移出、不加子标签。`suggest` 不扩展。`sweep` 本来就覆盖该角色全部机器归属。
- **前端**：其他页面有隐藏副本的卡片右上角显示 `×N`，悬停提示操作会同步到全部副本；重复页选中图片时，选择栏写明改动会同步到同组的全部副本（#528）。
- `schema_version` 升为 8。真实库（只读副本核对）：隐藏 1,973 份，图库约 4.7 万 → 4.5 万；未识别相应减少；4 组代表图因分辨率更高而不是最早一份；角色不一致仍为 48 组。角色列表 74 ms、可疑优先 59 ms、未识别 67 ms、侧栏计数 41 ms。

## 收录状态与排除规则（2026-09-23）

图库只收插画。截图、照片等「不是插画」的图从所有列表和计数中移到「已排除」；这只是索引里的标记，不改动原图（照片图库或来源文件夹）。「没有角色」的插画（风景、物件）照常收录，不属于排除对象。

### 规则（按抽样拼图逐张目视核对后确定）

| 原因 | 条件 | 真实库 |
| --- | --- | ---: |
| 真人/照片 `photo` | 源库 WD `realistic` ≥ 0.15（不看有没有角色：38 张同时有角色，全是照片或新闻截图，角色是外部索引硬配的） | 75 |
| 截图 `screenshot` | 有 `chat_log / fake_screenshot / fake_phone_screenshot / user_interface`，且没有人物标签，也没有角色归属 | 150 |
| 文字图 `text` | 有 `text_focus / chinese_text / korean_text / english_text`，没有截图标签和人物标签，也没有角色归属 | 305 |
| 待确认（不排除） | 有截图标签和人物标签（`1girl / 1boy / solo / multiple_girls / multiple_boys / 1other / male_focus`），但没有角色 | 63 |

- 核对结果：截图规则抽 64 张全是截图；文字图抽 64 张，只有 2–3 张是简笔梗图；`realistic` 0.15–0.35 这一段抽 27 张，只有 1 张是水彩静物。待确认组大约一半是带头像/贴纸的截图、一半是插画，所以只列出来，不自动处理。
- **刻意不用的标签**：`cellphone_photo`（抽样 41 张全是画出来的手机拍照构图）；`photo_background`（多为贴在实景上的插画）；`cosplay`（Danbooru 里指画中角色穿别人的衣服，不是真人）。
- 规则依赖 WD 通用标签与当时的角色归属，在迁移和重建通用标签时重算；平时的人工操作不触发规则重算。

### 数据与 API

- `curation(uuid,status,origin,created_at)` 是人工决定（`included / excluded`），**不可重建**，永远优先于规则，刷新不覆盖。`auto_rules`、`excluded` 为派生表；`excluded` = 人工排除，或规则排除且没有人工决定。写操作在同一事务内按图重算；测试 `tearDown` 核对它与重新计算的结果一致。
- `VISIBLE` = 未被折叠 且 未被排除，用于全部列表、`/api/ids`、按条件批量操作和各类计数。`pool=excluded`（可加 `reason=photo|screenshot|text|manual`），`pool=pending`；重复页不显示已排除的图。这个条件、内容筛选（NSFW 指评级 `questionable`、`explicit`）以及其他 HTTP 读取共用的 SQL 条件，只在 `native/terms/src/sql_fragments.rs` 里定义一处（#205）。
- `POST /api/curate {ids|filter,exclude,action}`，`action=exclude|include|reset`；会扩展到同组副本，整批撤销。每张图返回 `excluded`（原因）；详情返回 `excluded:{reason,origin}`、`curation`、`pending`；`/api/review-pools` 增加 `excluded:{count,reasons}` 与 `pending`。`schema_version` 升为 9。
- **前端**：侧栏「已排除」，按原因筛选，右上角进入「待确认」；已排除页选中后「恢复收录」，待确认页「排除 / 保留」；其他页面的选择栏和详情里有「排除」，快捷键 X（在已排除页为恢复收录）。
- **真实库**：图库约少 1%，未识别相应减少；有图的角色少了 28 个（只出现在照片/截图上）。角色列表 97 ms、未识别 86 ms、已排除页 23 ms、侧栏计数 54 ms。
- **尚未做**：纳入 Photos 里其余非 X/Pixiv 来源的图，需要先在 Windows 上跑 WD（源库目前只有 X/Pixiv 图的标签）。

## 原比例显示（2026-09-23）

- 两种网格，和 iOS 相簿一样可以随时切换（工具栏「原比例」或 R 键，偏好存在本机浏览器）：**方形**把图裁成正方形；**原比例**在 3:4 竖格里完整显示整张图，不裁切。库里 82% 是竖图，宽高比中位数 0.70，所以用 3:4 竖格比正方形格更少留白；横图（12%）上下留白。
- 仍然是等大格子，不是按行拼接：虚拟滚动、拖动/Shift 连选、捏合缩放锚点都依赖「第 i 张在第几行第几列」的固定换算。按行拼接需要重写这几部分，缩放时图片也会跳行。
- 原比例模式按每张图的宽高（`assets.width/height`）算出画框，选中圈、勾选圆点、角标都画在画框上，而不是整个格子上。
- 去掉旧规则「列宽 < 110 px 强制方形、≥ 200 px 强制 4:5」：现在任何缩放都按所选模式显示。

## 与 Photos 增量同步与设置页（2026-09-23）

用户拍板：每次启动同步；只收 X / Pixiv；**不写回 Photos**。原先「少量增量在本机识别、大批量交给 Windows」的上限已删除（2026-10-12，#100），现在总是在本机识别。

### 流程

现行的同步由原生宿主完成，协议、进度、取消和恢复见 [ARCHITECTURE_DIRECTION.md](ARCHITECTURE_DIRECTION.md)「原生来源同步与应用刷新」。

1. **只读扫描**：`native/terms/src/app_sync.rs` 在后台只读 `Photos.sqlite`（`crates/platform/src/platform_photos.rs`；macOS 需要给 App 照片权限）。X（`com.atebits.Tweetie2`）和 Pixiv（`net.pxv.iphone`）的新图写进源库 `assets`，优先用 Photos 的本地中等副本；从 Photos 消失或进废纸篓的图标记 `trashed=1`（判断「消失」时比较的是 Photos 里所有来源的图，不只看 X/Pixiv）；恢复的图改回 0。
2. **识别**：在范围内、却还没有 WD `rating` 标签的图就是待识别队列，所以中途停止也能续跑。可选识别 worker 逐张跑 WD 与 CCIP，交回结果包（[RESULT_PACKAGES.md](RESULT_PACKAGES.md)）；WD 1024 维和 CCIP 768 维特征存进源库 `embeddings`，供聚类和学习使用。读不到的图（多半还在 iCloud 里）不写任何结果，下次同步重试。没有 worker 时照常盘点，状态为待识别。
3. **增量导入**：每批结果导入后由核心的 `import_changes`（`crates/core/src/character_import.rs`）只重算这批图的候选、通用标签、分级、证据、单人图和归属，与全量刷新共用逐行逻辑；重复分组、折叠、收录规则和排除全量重算。
4. **记录**：`sync_runs`（每次同步的发现/移出/导入/读不到数和各来源统计）和 `sync_added`（每张新图属于哪次同步），供「新加入」页和设置页使用。一次同步如果中途被停掉，下次启动会标记为「中断」。

### 与全量结果保持一致

- **外部索引看最近 9 个锚点**：原生认领保留最近 9 个锚点、严格向上别名跳转和按来源选择阈值的固定契约，见 [结果包](RESULT_PACKAGES.md)。
- **识别结果核对**：同一张图，新代码的 CCIP 特征和旧特征差 < 2e-4；WD 前 5 个标签和角色标签一致（分数差异来自测试时用的是服务端重新压缩过的 JPEG）。
- **增量导入 = 全量刷新**：两种方式导入同一批新图，结果应逐表一致（原生同步测试见 `tests/test_native_sync.py`）；证据来源的拼接顺序固定排序（`CCIP,WD` 不随导入方式变化）。
- `schema_version` 升为 10。

### 界面

- 同步状态显示在侧栏「任务」面板里（2026-10-05 起，原先在侧栏底部的设置入口上）：检查中 / 识别新图 n/N · 约几分钟（预计剩余时间满两小时按小时显示，如「约 13 小时」，2026-11-12，#360）/ 已同步 · 时间 / 出错。同步期间每 2 秒刷新一次，新图导入后各处计数随之更新，完成时弹提示。
- **新加入**（`#/recent`，`pool=recent`）：最近一次同步加入的图，和图库一样浏览、整理；没有新图时不显示这个入口。
- **设置**（`#/settings/<分组>`，2026-10-05 起分页）：「图库」页有同步开关、iCloud 照片按 App 的收录统计、排除规则各排除了多少张（可点进对应页面）；「通用」页是显示偏好（方形/原比例、NSFW、默认排序）；「数据」页是备份（立即备份、最近的备份）和数据位置与大小。「立即同步」和同步记录在任务面板里。
- API：`GET/POST /api/sync`、`GET /api/settings`、`POST /api/backup`。服务参数：`--no-sync`（不同步）、`--no-auto-sync`（只关自动触发）、`--sync-limit N`（每次最多识别最近保存的 N 张，试跑用）；Photos 图库路径在 `library.json` 的来源配置里。

### 撤回方式

同步只往源库里**新增**图片、识别结果和特征，并给消失的图打 `trashed` 标记，不修改已有的人工结论。要回到同步前，用升级前或手动备份恢复图库。

## 证据强弱与「可能也是 TA」（2026-09-23）

- **按人工结论校准过的置信度**（相簿、人工确认、人工移除作为标准答案）：WD ≥ 0.95 判错 0.0%（2,893 例），0.90–0.95 判错 0.7%，0.80–0.90 判错 12.5%，0.80 以下约 30–80%（样本少）；外部索引分数 ≥ 0.95（CCIP 距离 ≤ 0.05）判错 0.1%，0.90–0.95 判错 11.8%。
- **排名规则**（`family_evidence.rank`，决定单人图算谁）：两种来源都认 > 有一个强证据（WD ≥ 0.9 或外部索引 ≥ 0.95）> 弱证据；同档内 WD 优先，再比分数。以前是「只要有 WD 就压过只有外部索引的」，所以 WD 0.4 的弱猜测也能压过距离 0.01 的外部匹配。改规则前在真实库上算过：约 1 万张有多个候选的单人图里，第一名变化的只有 35 张。逐张看过，大约 30 张改对（多是外部索引把发色、服装相近的角色认错），2–4 张拿不准。`schema_version` 升为 11。
- **「可能也是 TA」**：`status=maybe`（需要同时指定角色），列出机器证据指向这个角色、但图算给了别人或谁都没算、且没被人工移出过的图，证据最强的排在前面。角色页顶部有这个开关，显示数量；选中后批量「是 TA」或「不是 TA」，处理掉的图从列表里消失。这是修正「新角色的图被错认成别的角色」的主要入口。

## 补外部角色索引（2026-09-24）

角色索引由原生计算程序补充公开图片与标签，不上传图库；先下载、清洗参考图，再重建锚点和重扫认领。补充索引用 p75 阈值，既有索引用 p90；人工放弃与回滚由原生索引编排和备份协议管理，见 [结果包](RESULT_PACKAGES.md)。

## 合并角色（2026-09-24）

两个角色其实是同一个人时，在角色页点「合并到…」，选择目标角色。

- 合并（核心 `crates/core/src/character_merge.rs`）在一个事务里完成，并记为一次可撤销的操作：人工结论移到目标（目标对同一张图已有结论的，保留目标的）；子标签改挂到目标下；别名、原名和 tag 加为目标的别名；置顶、「已检查」记录、机器候选一并转过去；受影响的图重算归属。
- 被合并的角色不删除，只写 `entities.merged_into`。查找角色、导入候选、导入相簿结论、建立皮肤层级时都会顺着它转到目标，所以以后的同步和刷新不会再冒出第二个同名角色；它也不再出现在任何列表里，搜旧名字会找到目标。`schema_version` 升为 12。
- 首次使用：把一个相簿角色（`album:…`）并入对应的 Danbooru 角色，移动数百条人工结论；两边都已确认的图保留目标的。

## 新角色流程：相似分组与边标边学（2026-09-24）

- **特征**（`native/terms/src/research_compute/features.rs`）：每次研究运行从 `full-features.npy` 加上源库 `embeddings` 读入全库 CCIP 特征（约 4.8 万 × 768）。
- **相似分组**（`native/terms/src/research_compute/groups.rs` 的 `cluster`）：未识别池按密度分组（半径 0.06，每组至少 3 张），两组中心距离 ≤ 0.08 时提示「可能是同一人」。结果写进 `clusters` / `cluster_info`，所以单组页可以直接用 `pool=cluster&cluster=<id>` 复用网格、全选和批量操作；组里的图有了角色、被跳过或被排除，就会离开这一组。分组只在启动时、同步进新图后、手动点「重新分组」时更新，免得正在处理的组突然变了。真实库：216 组，共 1,167 张。
- **原型学习**（`native/terms/src/research_compute/learn.rs`）：一个角色有 ≥ 5 张人工确认的图（相簿、确认、检查完了都算），就从中取最多 600 张，按密度分出最多 8 个原型，覆盖常服、泳装等不同造型。每个原型的认领范围是其成员距离的 p90，限制在 0.03–0.08 之间，并且比该角色被你移出过的任何一张图更紧（留 0.9 的余量）。认领结果进 `proto_claims`，在证据里记作 `PROTO`（界面上显示为「你的标注」），和外部索引站在同一边参与排名；人工结论永远优先。每个角色的签名是人工结论的条数加最后改动的行号，签名变了才重新学习。确认、移出、换成、检查完、合并、撤销之后由研究任务批量重学。
- **判别式认领**（`native/terms/src/research_compute/discriminate.rs`，2026-10-16，#156）：原型只覆盖每个角色最密的那团图，在真实图库副本上只能认出约四成确认图。现在每次学习另给每个有 ≥ 5 张确认图的角色训练一个 L2 逻辑回归（C=10，类别按 balanced 加权，截距不罚），全部角色共享样本行一起训练（批量 L-BFGS，矩阵乘走研究计算的 BLAS）。负例是该角色被移出的图、其他角色的确认图，以及外部索引只给出其他角色候选的未裁决图（伪负例），后两者跳过与该角色确认图同一作品组的图。强认领需要同时满足三条：概率 ≥ 0.99；离该角色某张确认图的 CCIP 距离 ≤ 0.06；离它比离任何被移出的图更近。分数是「1 − 到最近确认图的距离」，与原型认领合并取大值，写进同一张 `proto_claims`，界面和归属规则不变。弱认领（「也可能是」）仍只由原型外圈和补充锚点产生。库里没有外部索引的 CCIP 候选时不启用：只用已标注角色当负例时，用户没标注过的相似角色、同作品角色会被大量误认。学习签名升为 `identity-v3:` / `v4:`，升级后第一次学习会全部重学一次。
  - 口径（真实图库副本，约 5 万张，按作品组 5 折留出，角色分开发半与测试半，参数只在开发半选定）：外部索引没覆盖的确认图上，召回从约 0.23 提到约 0.55；对其他角色确认图的误认领率与原型同一量级（测试半约每千张 0.05，原型约 0.03）。模拟用户没标注过的相似角色和同作品角色时，误认领约为原型的 3 倍（每千张约 0.2）。本机 Mac release 构建全量训练加认领约 6 秒。
  - 回退：删掉 `learn.rs` 里对 `discriminate::claims` 的调用并再升一次学习签名，下次学习即恢复只有原型的认领。
  - 测试（用户 2026-10-18 决定，#156）：`tests/test_native_research_compute.py` 的固定答案出自只有原型的旧算法，这组测试设置只供测试的 `ILLUSTASH_TEST_PROTOTYPES_ONLY` 关掉判别式认领，固定期望只把签名前缀改成 `v4:`；产品不设置它。判别式认领由 `discriminate.rs` 的单元测试覆盖；`tests/test_native_research.py` 另有一项走 App 研究发布路径：确认图自己也会被认领，分数不能因 f32 舍入超过 1，否则核心发布校验会拒绝整批学习结果。
- **真实库首次运行**：11 个角色、14 个原型，认领 5,678 张；19 张未识别的图有了角色，163 张的归属被改判。改判抽样全部看过，都是用你的判断纠正了外部索引的错：几个角色各有几十张原来被判成发色、服装相近的其他角色。
- **界面**：未识别页「按相似分组 →」；分组总览（`#/groups`）每组一张卡片，有 4 张缩略图、剩余张数和「可能和第 N 组是同一人」；单组页（`#/g/<id>`）和未识别页一样，可以设为角色、跳过（S）、排除（X）。给一组设为某个角色后，这个角色的人工确认增加，原型随之重学，全库里同一个角色的其他图会被自动认领。
- `schema_version` 升为 13（新增派生表 `clusters`、`cluster_info`、`prototypes`、`proto_claims`、`learn_state`）。API：`GET /api/clusters`、`POST /api/regroup`。
- 后来补上的：中档候选见下一节「中档候选进『也可能是』」，外观标签辅助分组见「用 WD 外观标签辅助分组」，偶尔提问就是首页的「认一认」（[新图库引导](#新图库引导2026-10-1296)）。

## 中档候选进「也可能是」（2026-09-24）

- **弱认领**（`weak_claims`，派生表）：把握不够、只作线索的匹配。**不参与归属计算**，只出现在三个地方：图片详情的「也可能是」（排在强证据之后，界面上画成虚线、偏淡）、角色页的「可能也是 TA」（排在强证据之后），以及选择器里的建议（`suggest`）。三处统一查 `HINTS` = 强证据 ∪ 弱认领。你移出过的图不会再作为这个角色的线索出现。
- **两个来源**，都在研究任务里用已有特征算，不需要重扫全库：
  1. 学到的原型的外圈：距离在认领阈值和 min(1.5 × 阈值, 0.11) 之间，同样不越过你移出过的图。
  2. 从 Danbooru 补的角色的中档：距离在 p75 和 p90 之间（这些角色的认领只到 p75）。
- **修正**：以前只有人工结论变了才重新认领，所以同步进来的新图要等你下次操作才有「你的标注」这一路证据。现在启动时和同步后都会强制重新认领一次。
- **真实库**：弱认领 3,921 条（补充角色中档 1,186 条，原型外圈 2,735 条），归属表前后完全一致。抽样：一个角色的 12 条线索里约 9 条确实是她；双胞胎角色的线索都指向另一位。很多「可能也是 TA」的图被外部索引标成了别的角色，看画面其实是她。`schema_version` 升为 14。

## 用 WD 外观标签辅助分组（2026-09-24）

- **标签**：从 `tags_v3.csv` 核对过的外观标签，包括发色（按色系归并：白/灰、蓝/浅蓝/深蓝/水蓝、棕/浅棕 各算一系，因为 WD 常把相近的颜色混淆）、瞳色、光环、兽耳、角、尖耳朵、尾巴、翅膀、发型、异色瞳、X 形发饰等（`web/learn.py（#9 已删除） TRAITS`）。
- **实测先行**：在当时 191 组上验证：没有一组同时含两种发色；91 条「可能是同一人」的提示里只有 1 条发色冲突（棕发 vs 金发）。中心距离 0.08–0.12 之间的 148 对组里，按标签稀有程度加权后画像相似度 ≥ 0.85 的约 10 对，看图大约六成是同一个人（同发色同服装的组多数对；两组黑白漫画、两组普通黑长发不对）。
- **规则**：
  1. 一组里有两种发色各占 ≥ 30%（且各 ≥ 3 张）就按发色拆开（现有数据里没有触发，是个保护）。
  2. 「可能是同一人」：中心距离 ≤ 0.08，且两组的主色系（占 ≥ 60%）不冲突。
  3. 「外观相似」（较弱的提示）：中心距离 ≤ 0.12，且加权画像相似度 ≥ 0.85，同样不能有发色冲突。
  4. 每组显示最有辨识度的最多 4 个特征（占 ≥ 50% 的标签，按稀有程度排序），卡片和单组页标题都有。
- `cluster_info` 增加 `near_kind`（`look` / `trait`）和 `traits`；`schema_version` 升为 15。真实库重新分组：156 组，其中 143 组有特征；提示 56 条「可能是同一人」、1 条「外观相似」。

## 画师目录

`GET /api/artist-directory` 返回 `{items, total, offset}`，每项有画师的 `id`、`name`、`homes`，
以及可见图数 `count`、最近收图日期 `last` 和这张图的 UUID `cover`。日期相同时用 UUID 最小的图作封面。
目录只读图库里的画师核心投影。机器认定和人工确认计入图数，候选、人工否定、已移除、排除和折叠的图片均不计入。
画师按词条 key 统计，继续沿用 [画师词条](TERMS_ARTISTS.md) 的归属口径。

`nsfw=hide` 或 `nsfw=only` 同时影响图数、封面和入选的画师；不传时显示所有分级。
`q` 最多取 80 字，对显示名、X 账号、Pixiv ID 和 Danbooru tag 做 Unicode 大小写折叠后的子串匹配。
`order` 可取 `count`（默认）、`recent` 或 `name`（按大小写折叠后的显示名排序），同序时按 key 排定。
`offset` 默认 0，`limit` 默认 60（1–200）；空页仍返回查询对应的总画师数。

界面是资料库的「画师」页签（`#/characters/artists`，`ui/artist_dir.js`）：一次读 120 位，滚到底接着读；排序选过的记在偏好 `artistOrder`；张数跟着全局 NSFW 开关；点一位画师进入 `#/library?artist=<key>`。从那里后退回目录时沿用已经读过的页和滚动位置。

## 画师 OC（2026-09-24）

相似分组里有不少组是同一个画师反复画的原创角色（OC）。这些图不属于任何作品，以前只能跳过。现在可以把它们标成「画师 OC」：

- **怎么标**：
  - 分组页标题旁的「整组标为画师 OC…」；
  - 未识别页、分组页、已跳过页选中图后，点选择栏里的「画师 OC…」；
  - 角色选择器里新建时选「新建画师 OC」；
  - 已有的本地角色，在角色页用「标为画师 OC… / 编辑画师…」补标或改画师，也可以取消。
- **填什么**：OC 名字和画师，至少填一个。只填画师时，名字默认是「画师 的 OC」。画师输入框会补全已经填过的画师。
- **数据**：`entities.kind='oc'`；画师单独存在 `entities.artist`，不塞进 `work`（schema 16）。这样以后做「找回画师」时，一个 OC 的所有图都能直接对应到画师。
- **显示**：
  - 角色页里 OC 单独归到「画师 OC」一栏，卡片带 OC 标记并显示画师；
  - 按画师名搜索，能搜到这个画师的所有 OC。
- **学习**：OC 和普通角色一样，由人工结论学出原型，之后同步进来的同一 OC 的新图会被自动认领。

## 定期检查新角色（2026-09-24）

设置页的「检查新角色」由原生 `/api/index` 提供；计算子进程使用原生 `illustash-worker`。

- **检查**：
  - 每次点「检查新角色」都会说明联网目的并确认，只授权当前运行；取消后不发起检查，不保存授权。命令行 `--index-network` 可明确授权当前进程的自动检查，默认桌面启动不自动联网；
  - 本地读取失败显示「读取失败：具体原因」，不以空结果替代失败；可选的本地链接探测保持静默。
  - 只发文本查询：对每个关注的作品调用 Danbooru 的 `related_tag`；
  - 条件与上次一致：≥ 最少图数（默认 100）、与该作品重合度 ≥ 0.5、本地没有、是本体；
  - 以前放弃过的角色（`char-patch/rejected/`）不再列出。
- **关注的作品**：默认是上次那 8 个作品；可以在卡片上增删 Danbooru 作品 tag，调整最少图数（当时存在 `char-patch/watch.json`；2026-10-05 起按图库存，见 [数据模型](DATA_MODEL.md)）。Danbooru 不认识的作品 tag 会标出来。
- **补进来**：
  - 用户勾选后点「补进选中的 N 个（下载约 X MB）」，按钮上写明下载量，超过 500 MB 另外提示；默认全选，可以取消不要的角色（用户 2026-10-21 确认，#287）；不设流量预算；
  - 添加做到一半重启时没有联网授权，任务停在「已暂停」，已下载的参考图保留；任务面板里「继续下载…」确认联网后接着下，也可以「放弃」（#287，细节见 [角色索引](ARCHITECTURE_DIRECTION.md#原生角色索引维护)）；
  - 原生编排先备份、下载，计算子进程清洗、重建锚点和重扫，再由宿主验证并导入结果；
  - 下载、清洗和重扫都只处理选中的角色；清洗时和所有已有角色比对，包括以前补进来的；
  - 可用的单人图不足 3 张的角色，现在会记为放弃，以后不再反复出现；
  - 一个都没补进来时，跳过重建和重扫。
- **只重导变化的图**：
  - 重扫前后对比 char_ext 认领，只把变了的图交给核心的 `reindex_characters`（`crates/core/src/character_import.rs`）；
  - 它和全量刷新共用逐行逻辑：新角色的实体、皮肤层级、tag 和别名，以及这些图的候选、证据和归属；
  - 完成后，研究任务重新读取补充锚点，重新认领和分组。
- **放弃**：「最近补过的」列表里每个角色显示认领了多少张，可以点进角色页看图。觉得不对就点「放弃」（点两次确认），由原生计算程序移除索引并重建；重扫后它认领的图恢复原样，以后检查也不再提它。
- **互斥**：更新索引和 Photos 同步都会写 char_ext，所以不同时进行。检查只是查询，不受限制。
- **备份**：每次更新或放弃前，在 `char-patch/backup/<时间>/` 备份锚点、角色表和全部 char_ext 认领，保留最近 5 份。
- **实测**（2026-09-24）：
  - 8 个作品检查一次约 27 秒，只找到上次因为「单人图不足 3 张」而跳过的 6 个角色，说明这一天 Danbooru 上没有新角色；补这 6 个用了 19 秒，都被记为放弃。
  - 单独运行一次「重建锚点 + 重扫」用了 67 秒，74,109 条认领和之前完全一致，变化 0 张。
- 服务参数 `--no-index-check`：启动时不自动检查。

### 更新：原创角色简化（2026-09-24）

用户反馈填画师和 OC 名字太麻烦，现在改成**一键标为原创角色**：
- 分组页的「整组标为原创角色」和选择栏的「原创角色」都不再弹窗，也不要求填画师和名字（现在设为哪个角色见下一节）；
- 角色页的按钮改成「标为原创角色 / 取消原创角色」，直接切换；
- 角色页里原创角色单独成一栏，卡片带「原创」标记。

`entities.artist` 字段和接口保留（schema 不变），界面暂时不用，以后做溯源时再用。

### 更新：原创角色统一标注、按需细分（2026-09-26）

区分原创角色彼此是谁拖慢标注，而以后做画师回溯时可以用图像特征（再加上画师信息）重建，所以界面上只做统一标注。用户 2026-11-19 定（#694）：原创角色机制还要打磨，界面上原创角色只作为一个整体类别，不开放设置各个不同的原创角色。

- 界面上凡是「标为原创角色」，都把图设为唯一的共享角色「原创角色（未细分）」，不新建角色。它是**标记而不是人**（`entities.label_only=1`，schema 29）：不学习原型、不产生机器候选或「可能是」线索、不进身份待判断队列；已有的模型在下次学习时清掉。入口：选择栏的「原创角色」；组页标题的「整组标为原创角色」和按组整理页每组组头的「原创角色」（「是 X 吗？」的组和还不知道是谁的组都有）；角色选择器里输入名字后列在末尾的「原创角色（未细分）」（本地结果里已有它时不重复列出；和「新建」一样只是兜底，不会默认选中）。都可以撤销。
- 整组也只设为共享角色，不新建独立的原创角色：按长相聚出的一组常混着几位画师各自的 OC，整组存成一个独立原创角色会把不同的人当成一个，还会参与学习、按混出来的原型自动认领以后同步进来的图。
- 后端仍保留独立原创角色的能力（`kind='oc'` 的实体、`POST /api/entities {kind:'oc'}`、`POST /api/kind`），界面不再用它新建。已有的独立原创角色照常显示，角色页「⋯」里的「标为原创角色 / 取消原创角色」照旧给已有的角色改类型。以后怎么细分原创角色见 #695。
- 「按相似细分」的界面入口已隐藏（#694）。接口 `GET /api/oc/groups?entity=` 保留：现算共享角色里长得像的分组（与未识别分组同一套聚类，至少 2 张），不落库。
- `POST /api/oc/shared` 取得（不存在则创建）共享原创角色；它不能被取消原创角色标记。

## 自建角色的资料（2026-09-26）

以前「新建角色」只存一串你输入的文字：不关联 Danbooru、没有作品和别名，也不会自动补中文名。现在：

- **从 Danbooru 新建**：选择器输入名字时，本地结果之后会出现「Danbooru 角色」（`GET /api/entities?q=&remote=1`，只发送输入的文字；已在图库里的 tag 不重复列出）。选中即 `POST /api/entities {tag, typed}`：关联 tag；作品先按 tag 的后缀（`_(blue_archive)`）找本地作品，找不到再查 Danbooru 的 related_tag（取几乎覆盖该角色全部图的最小作品 tag），仍没有就自动建立作品（关联 Danbooru tag，名称自动补）；你输入的写法存为别名。随后自动拉取这个角色的显示名。只在本地新建的选项仍在列表末尾，默认不会被选中。
- **编辑资料**（角色页「⋯ → 编辑资料…」；未关联的角色标题旁有「关联 Danbooru…」）：显示名（含原创角色改名）、别名、所属作品（可搜索本地与 Danbooru 作品，或新建）、Danbooru 关联。`GET/POST /api/character/profile`，一次保存是一条撤销记录（快照包含用户名称、Danbooru 链接和作品）。关联了 Danbooru 时显示名可以留空，用自动名称；一个 tag 只能属于一个角色，重复时提示用「合并到另一个角色」。作品框里选 Danbooru 作品走 `POST /api/works {action:'from_tag'}`。
- **存量整理**：角色页出现「N 个自建角色能对上 Danbooru」（`GET /api/local-links`，只查这些角色的名字，结果缓存）。名字就是 Danbooru tag 的默认勾选；按名字搜到的最像的一个标「可能是」、默认不勾；已在图库里的给出「合并到……」。`POST /api/local-links` 一步关联所选（可撤销），原来的名字转为别名，好让自动补来的名称显示出来。
- Danbooru 连不上时接口返回 502 和提示，已有功能不受影响。

## 图片关系判断（2026-09-24）

选中 2–12 张图后，选择栏会出现「判断关系」按钮。系统两两比较这些图，判断它们属于完全相同、重复、局部重复（裁剪）、差分、局部编辑、构图相似，还是不同的图；并在图上框出不同的地方（红框），以及小图在大图里的位置（蓝色虚线）。最初在 Python（`web/relate.py`，已随 #9 删除）实现，现在由研究计算的 `native/terms/src/research_compute/relations.rs` 实现，判断规则和阈值不变；接口是 `POST /api/relate`。

不用模型，只做像素比较，在本机计算（最初每对约 0.2 秒，经下文「阈值复查」后约 0.5 秒）：
1. **画布比例相同**：把两张图缩到同一尺寸（长边 512），各做一次 3×3 模糊后逐像素比较，再按 16px 的格子统计哪里变了。
   - 如果变化较多，先对齐再比：拿右图中间 88% 的区域去左图里找位置和缩放，只比较两张重叠的部分。这样可以处理重新导出、偏了几个像素、裁切略有不同的情况。
   - 变化 < 0.4% → **重复**。
   - 逐通道做一次曲线校正后变化 < 0.4% → **局部编辑（调色）**。
   - 变化只在边缘 7% 以内 → **局部编辑**（加字、水印、边框）。
   - 变化区域在一张图上几乎没有细节 → **局部编辑**（打码、遮挡、模糊），并指出改动在哪张。这一步在原始解码分辨率上判断，因为缩放后马赛克的硬边会被抹掉。
   - 其余的局部变化 → **差分**。
   - 变化超过 45% 时看灰度相关系数：≥ 0.75 说明还是同一张画，只是大面积重新上色或修改，判为**差分**；0.5–0.75 判为**构图相似**；再低就是**不同的图**。
2. **比例不同，或者同比例但对不上**：在 28 个缩放比例下，用 FFT 归一化互相关，在一张图里找另一张。两个方向都找，因为同一张原图的两个裁剪版可能互相都包含对方的大部分。分数 ≥ 0.86 判为**局部重复**，再对重叠部分做第 1 步的比较，看有没有改动。
   - 颜色平坦的窗口直接排除，否则方差接近 0，会被放大成假峰值。

**校准**（真实库）：
- 抽样来源：CCIP 距离 < 0.02、但文件哈希不同的图对共 75,691 对；按距离分 5 层，每层抽 16 对，全部看图核对。
- 距离 < 0.001 的 16 对：15 对判为重复或完全相同，1 对差分，都对。
- 其余层里的局部重复、差分、局部编辑共 13 对，看图后都成立，包括：
  - 带手写字的裁剪版；
  - 下半部分被灰块遮住的版本；
  - 重新上色的版本；
  - 动画里相邻的两帧，背景人物动了。
- 修正了几类错判：
  - 两个互相裁剪的版本被判成「无关」：原因是只找了一个方向；
  - 偏了几个像素的重新上色版本被判成「无关」：原因是没对齐，而且用的是对错位敏感的拉普拉斯相关；
  - 两页不同的文字被判成「构图相似」。
- 同一角色的不同画，绝大多数判为「不同的图」。文字截图的 CCIP 距离没有参考意义，这类图也都判成了不同的图。

对全库自动找差分组、近似重复组见下文「全库找相似」和「按关系折叠」。

### 阈值复查：「很接近的图被分成差分」（2026-09-24）

用户反馈：有些几乎一样的图被判成了差分。从库里另外抽了 180 对 CCIP 很近的图，逐对看图核对，原因有三个：

1. **对齐太粗（主要原因）**：
   - 找裁剪位置时，缩放比例每档相差约 6%，线条边缘就对不齐；
   - 缩放方式不同、偏了不到 1 个像素，也会在线条边缘留下一圈差异；
   - 结果是本来一样的图被判成「差分 10–30%」，裁剪版的改动面积高达 25–60%。

   修正：
   - 逐像素差异改为**容忍 1 像素错位**：每个像素和另一张图周围 3×3 范围里最接近的那个比，两个方向都算；
   - 定位改为**先粗后细**：先在 256 分辨率下粗找，再到 512 分辨率下只在最佳位置附近细调，缩放比例按约 1% 和 0.25% 的步长搜索；
   - 同比例的图也先对齐再比较。

   结果：这类图回到「重复」，裁剪版的改动面积降到 0 左右。
2. **「打码」误判**：把张嘴和闭嘴、加眼镜、换衣服当成了打码。看图后的结论是：除了纯色块遮挡和规则的马赛克格子，打码和内容变化在像素统计上分不开，比如加眼镜和把眼镜涂掉是对称的。所以现在只有下面这些情况判为局部编辑，其余局部改动（包括贴纸）都算差分：
   - 变化的像素在一边是单一颜色（色块遮挡）；
   - 最大的变化区域里有至少 3 行、3 列贯穿整个区域的直边（马赛克）；
   - 改动只在边缘；
   - 整体调色。
3. **文字截图被当成裁剪**：一行字几乎在哪里都能匹配上。现在规定小图至少要占大图的 10% 才算局部重复。

另外把**改动面积 < 2%** 的差分单独分成一档，叫「细微差分」，包括嘴型、眼镜、签名、小物件这类改动，好和大改的差分分开看。

代价是每对比较从约 0.2 秒变成约 0.5 秒。

## 从例图找相似（#449）

`/api/assets`、`/api/assets/count` 和 `/api/ids` 接收 `like`（含 1–20 个图片 UUID 的 JSON 字符串数组）和可选的 `like_by`（`wd` 按画面，省略时采用；`ccip` 按角色）。例图缺少该空间的特征时跳过，全部缺失时返回 400。示例和示例在「重复」里的同组副本都从候选中去掉。

现有选区先应用筛选、分级、折叠与排除条件，再对有特征的候选按例图质心的余弦分数取前 500。同分按 UUID 稳定排序。`order=similar` 是相似检索的默认排序，`newest`、`oldest` 在同一批入选图片内按加入日期排序。响应仍用普通网格的形状。按 `filter` 选图的命令走同一套筛选和排名，在自己的写事务里取得选区，随后执行命令原有的选区操作。

界面（`core/like.js`、`ui/like.js`）：详情栏预览图下面、大图底栏和选择栏（选 1–20 张、非全选）有「更多像这张的／这些的」，没有快捷键。结果是图库的一个视图，地址 `#/library?like=[…]`（`by=ccip` 按角色），工具栏和图库相同，搜索、筛选胶囊和标签照常叠加；排序多「最相似」，默认是它。网格是浏览语义，选图后照常加角色、标签。标题右侧是例图缩略图和「按画面｜按角色」。例图全都没有特征时网格空着，写明「例图还没有特征，识别完成后再试。」；有两张以上例图时，界面对每张单独问一次 `/api/assets/count`，没有特征的缩略图变淡，副标题写「N 张例图没有特征」。各页的说明见 [WEB_UI_MAP.md](WEB_UI_MAP.md)。

按画面读取源库现有的 WD general 标签概率，使用 IDF 加权、L2 归一化的稀疏向量。IDF 中以有原图路径的在库图数为基数，沿用离线评估口径，只有概率至少 0.35 的图片计入标签文档频次。按角色读取 CCIP，按 UUID 合并基础矩阵和 SQLite 向量，以后者为准。宿主按会话懒加载使用到的空间，切换图库或闲置 10 分钟释放。缓存会检查源库连接的 SQLite `data_version` 和基础特征文件的大小与修改时间。WD 稀疏矩阵另存为图库 `cache/` 下的派生文件，源库或 WAL 变化后重建，不随归档导出。选区排名只存于当前 SQLite 连接的临时表。整个检索使用本地已有识别结果。

## 全库找相似（2026-09-24）

相似扫描由研究任务完成（`native/terms/src/research_compute/similar.rs`）。启动时按图库的 `startup.similar` 自动开始；每次同步导入新图后，只处理新增的部分；「相似的图」页上也可以点「重新检查」。

1. **签名**：每张图解码一次（JPEG 用 draft 快速缩小），存画布比例和 16×16 灰度缩略图，放在 `image_sigs` 表。全库约 4.7 万张，每秒约 70 张。
2. **候选图对**：
   - 画布比例相同、缩略图相关系数 ≥ 0.9 的图对，每张图最多取 12 个最像的，几乎纯色的图跳过；
   - CCIP 距离 < 0.006 的图对，用来覆盖裁剪版。

   字节相同的副本、比较过的图对、已排除的图都不再比较。
3. **比较**：逐对调用关系判断（`native/terms/src/research_compute/relations.rs`），每对约 0.5 秒。所有结论都存进 `relations` 表，包括「不同的图」，以后不重复比较。
4. **分组**：除了「构图相似」和「不同的图」，其他关系都会把两张图连成一组，写进 `similar_groups`。这是派生表，升级 schema 时根据 `relations` 重建。

界面：「重复」页右上角有「相似的图 →」入口（`#/similar`），相似组可以按关系筛选：重复、细微差分、差分、局部重复、局部编辑。点进一组（`#/s/<组号>`），就是普通的图片网格，可以选图、排除、加角色；图不超过 12 张时，标题旁有「判断关系」按钮。

看完一组可以点标题旁的「看过了」（#427）：回到列表，这组不再出现，提示条上可以撤销。组里进了新图（包括和别的组连成一组）就重新出现；组里有图被排除或移走，仍算看过。列表状态行末尾的「看过的 N 组」（`#/similar?seen=1`）只列看过的组，打开其中一组，标题按钮是「标为没看过」。「更多像这张的」的结果没有「看过了」：那是按例图临时算出的排名，没有固定的组。

`image_sigs`、`relations` 是非派生表（schema 17）。设置里关掉启动时的相似扫描（`startup.similar`）即不在后台扫描。

**第一次全库扫描**（2026-09-24）：
- 耗时：签名约 11 分钟（全部图都读到）；2,718 对候选逐对比较约 35 分钟，其中 2,669 对属于同一张图的各种版本，说明候选筛选很准。
- 结果：共 1,982 个相似组，涉及 4,140 张可见的图。按组里出现的关系统计（一个组可能同时有几种）：
  - 重复 1,339 组、完全相同 163 组（像素一样但文件不同）；
  - 差分 360 组、细微差分 61 组；
  - 局部重复 91 组、局部编辑 38 组。
- 抽检：细微差分、差分、局部重复、局部编辑各抽 6 对看图，都成立。有一对是一张多了一个小插图、另一张那里是空白，被判成了「色块遮住」；归类不准，但分成一组是对的。

### 按关系折叠，可在设置里选（2026-09-24）

用户决定：「重复」和「完全相同」自动折叠，「细微差分」先不折叠；哪些关系折叠，由设置决定。

- 设置页新增「折叠」卡片，可以勾选：完全相同、重复、细微差分、局部编辑、局部重复、差分。默认勾选前两项，存在 `prefs` 表的 `fold_kinds` 键。字节完全相同的副本总是折叠。
- 重建副本组（`native/terms/src/app_curation.rs` 的 `refresh_duplicates`）：用并查集把 Photos 哈希相同的副本和折叠关系连起来的图合成一组。组里有哈希时沿用哈希作为组号，所以原来的副本组号不变。之后的规则和原来的副本一样：
  - 代表图取分辨率最高的一张；
  - 组里的图角色不一致时不折叠，留在「重复」页的「角色不一致」里；
  - 改任意一张，会同步到整组。
- 以下时机会重建折叠：改设置时、扫描写入新关系时（`refresh_similar`）、同步导入时、全量刷新时。schema 升为 18，升级时就按新规则重建。
- 真实库按默认设置迁移后的变化：
  - 图库可见的图减少约 3%；
  - 「重复」页从 1,910 组、4,003 张变成 3,228 组、6,902 张，其中 142 组角色不一致，等待对齐；
  - 试着勾上「细微差分」，会再折叠 52 张，已恢复为默认设置。
- 接口：`GET/POST /api/fold`。
