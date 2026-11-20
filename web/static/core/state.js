// 页面共享的状态（#199）：路由、图库的列表和选择、各项计数以及每个任务最近一次的回应。不碰 DOM。
// 每个字段都在这里声明（tools/check_frontend.py 规则 2），按归属分组；每组只有一个写入者：组标题写的那个模块，
// 其余模块只读，要改就调用组标题里列的那个模块的函数（#344）。组标题的格式是「// ---- 说明：写入者文件（备注） ----」，
// 冒号和括号之间只写一个文件；规则 7 按它检查别的文件有没有给这一组的字段赋值。字段的初值要与第一次写入之前读它的代码对得上，
// 例如 doubtCount 是 null 而不是 0，「和上次一样就不重画」的比较才不会在第一次就跳过。
// 放在别处、各自只有一个写入方的：会话（session.js），编辑和撤销（edits.js），列表变化时保持在视野里的那张图
// （ui/grid.js 的 kept）。
// 图片交互的几个模式（浏览、选择、详情、大图）怎么由这些字段组成、当前图取哪个来源，见 core/modes.js。
import {prefs} from './prefs.js';
export const state = {
  // ---- 路由：core/route.js（setRoute 整体换掉、replaceRoute 就地改参数） ----
  route: {page: 'library'},

  // ---- 图库元数据：core/meta.js（refreshMeta 一起读；setChar、setIdentityCount） ----
  chars: [], charMap: new Map(), workNames: null, char: null, userTags: [],
  pools: {unmatched: 0, skipped: 0}, dupInfo: {groups: 0, mixed: 0}, exclInfo: {count: 0, reasons: {}}, pendingCount: 0, recentCount: 0,
  identityCount: null,
  // 撤销以后在等后台学习落定（#404）：计数旁显示「正在更新」。
  learning: false,

  // ---- 后台任务最近一次的回应：core/jobs.js（index、names 别处要换时用 setIndex、setNames、mergeNames） ----
  // syncLost：同步状态读不到了（轮询失败，正在按间隔重试），界面别把上一次读到的进度当成现在的。
  sync: {status: 'off'}, syncLost: false, syncHistory: [], syncDamaged: null, provenance: null, importTask: null, index: null, names: null,

  // ---- 设置页读写的数据：core/settings.js（settings 别处要换时用 setSettings） ----
  settings: null, about: null, accounts: null, fold: null, libraries: null, pixivWaiting: null, updates: null,

  // ---- 资料库的数据：core/catalog.js（localLinks 别处要作废时用 forgetLocalLinks） ----
  tagged: null, workTree: null, localLinks: null,

  // ---- 人工结论快照（自动快照的状态和最近一次导出，GET /api/human-conclusions）：core/human.js ----
  human: null,
  // ---- 用图动作（能不能拖出、最近一次导出到文件夹，GET /api/files）：core/files.js ----
  files: null,
  // ---- 图库完整性检查的回应：core/integrity.js ----
  integrity: null,
  // ---- 识别模型和基础角色索引（#607，GET /api/index/base，这个图库的启用状态）的状态：core/models.js ----
  models: null, characterBase: null,
  // ---- 角色资料包的状态（GET /api/packs，#602；合并了 #550 WD 角色扩展和 #559 CCIP 长尾包）：core/packs.js ----
  pack: null,
  // ---- 画廊列表（GET /api/galleries，#445）：core/galleries.js ----
  galleries: null,
  // ---- 上手引导的进度：ui/guide.js ----
  guide: null,

  // ---- 网格的列表：ui/grid.js（按天视图借用它放自己的图，用 showList；写入后作废读取用 listChanged） ----
  items: [], total: 0, totalReady: false, readEpoch: 0, countDirty: false, countReload: null,
  pages: new Set(), pending: new Set(), generation: 0, dataVersion: 0, gridKey: '',
  // 角色页「可能也是 TA」「可能认错」两个胶囊的张数。
  maybeCount: 0, doubtCount: null,

  // ---- 网格的布局与缩放：ui/grid.js（fit 用 setFit） ----
  // size 由知道缩放范围的 ui/grid.js 设置。
  cols: 1, cellW: 180, rowH: 250, gap: 12, size: 185, fit: prefs.get('fit', false),
  nsfw: ['', 'hide', 'only'].includes(prefs.get('nsfw', '')) ? prefs.get('nsfw', '') : '',

  // ---- 选择与键盘光标：ui/grid.js（cursor 用 setCursor；换页复位用 resetForRoute） ----
  selecting: false, sel: {all: false, ids: new Set(), exclude: new Set()}, anchor: -1, cursor: -1, armed: null,

  // ---- 详情栏与大图：ui/detail.js ----
  focused: null, detail: null, viewer: -1, viewerItem: null, question: null,

  // ---- 「第 N 组」页标题用的近似组信息和各组的先后（空了时「下一组」用）：ui/review.js（进入该页前读取） ----
  groupNear: null, groupNearId: null, groupTraits: null, groupOrder: [],
  // ---- 「相似组 N」页标题的「看过了」（#427）：ui/review.js（进入该页前读取） ----
  // {seen, size, nsfw}：这组算不算看过、读的时候看得到几张、按什么 NSFW 设置读的；读不到或组不在了是 null。
  sgroupSeen: null,
};
