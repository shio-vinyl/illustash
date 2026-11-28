# 数据模型：作品、角色、名字与外部 tag

定稿于 2026-09-24。分三步迁移：**链接、名字、作品三步都已完成**（schema 23 → 24 → 25）；外部作品对应关系与多上级歧义保留人工确认（第 9 节）。原第 7 节「迁移对照」随迁移完成删除（2026-11-14），原文在 git 历史里，节号不重排。验收记录已删除，见开源前私有历史中的 `docs/DATA_MODEL_ACCEPTANCE.md`。

本文的模型（身份、链接、名字三层）现行有效，但存储已换：2026-10-20 起（词条核心第 8 版，#201）身份在 `terms` / `term_tree`，链接在 `term_links`，名字在 `term_names`、`term_key_names`、`term_key_aliases`，第 4 节的 `entities`、`links`、`names` 和旧的 `display_names`、`name_aliases` 都已删除。表的对应见 [TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 5、14 节。

## 1. 为什么要改

现在一个角色有好几种写法，每一种都有地方拿它当 key：

- D 站 tag：`airi_(band)_(blue_archive)`；
- CCIP 短名：`airi_band_blue_archive`；
- `entities.name`：大多是短名，有 201 个是中文名，手建的角色则是随手打的文字；
- 作品：`nikke`、`kancolle`，是从角色 tag 括号里抠出来的后缀，不是 D 站作品 tag。

名字又分散在四处：`name`、`aliases`、`display_names`、`name_aliases`。

结果就是：
- 手建的角色对不上任何外部数据；
- 作品会出现 D 站上不存在的 tag；
- 以后每加一个数据源，都要再多一种写法。

## 2. 原则

1. **身份只认内部 ID。** 作品、角色、皮肤一经创建，ID 永远不变，不从任何字符串推导。改名、D 站改 tag、合并，都不会改变它。
2. **外部 tag 全部是「链接」。** D 站 tag、CCIP 索引名、WD tag、Bangumi ID、Photos 相簿名，都只是挂在身份上的链接。
   - 链接可以没有，可以之后再补，一个身份也可以挂多个。
   - **新角色在 D 站还没收录时，照样是完整的角色。**
3. **名字是单独的一层。** 多语言、多来源，分成显示名和只供搜索的别名。你起的名字永远优先，自动拉取不会覆盖它。
4. **导入时只按链接认人，不按名字认人。** 名字相似只能用来提出「是不是同一个？」让你确认，绝不自动合并。
5. **一键原则不变。** 新建角色时，作品、外部 tag 都不是必填项。

## 3. 概念

| 身份类型 | 说明 | 上级（`parent_id`） | 所属作品（`work_id`） |
|---|---|---|---|
| 作品 `work` | 游戏、动画、企划，也可以是系列 | 上级作品（可空）；可以任意层级嵌套，不能成环 | — |
| 角色 `character` | 一个人物 | 空 | 可空（还没归类，或者是原创角色） |
| 皮肤 `skin` | 角色的某套服装或形态 | 所属角色（必填） | 跟随角色 |

- **原创角色**是 `character` 加上标记 `original=1`，不属于任何作品。
- **作品是一棵树**，用来解决 fate、崩坏这类歧义。例如「Fate 系列」下面有 Fate/stay night、Fate/Grand Order；「崩坏系列」下面有崩坏3、星穹铁道。
  - 角色只知道后缀是 `fate` 时，先挂在「Fate 系列」上，以后可以改到具体作品。
  - 按某个作品筛选时，包含它整棵子树里所有作品的角色。
  - 唯一的约束：不能成环（一个作品不能挂到自己的子作品下面）。
  - 树的初始结构用 D 站作品 tag 之间的蕴含关系自动搭建（例如 `fate/grand_order` 蕴含 `fate_(series)`，只发送 tag 文字），你再修正个别地方。

## 4. 表结构（草案）

下面是 schema 25 时的写法，列的含义照旧；词条核心第 8 版起这些列分别在 `terms`（`key` = `id`、`kind`）、`term_tree`（`parent_id`、`work_id`、`original`、`merged_into` 等）、`term_links`（角色的 `wd` 存成 `wd_character`）、`term_names` 里，见本文开头。

```sql
-- 身份
CREATE TABLE entities(
  id TEXT PRIMARY KEY,                       -- UUID，永不改变
  kind TEXT NOT NULL CHECK(kind IN ('work','character','skin')),
  parent_id TEXT REFERENCES entities(id),    -- work → 上级作品（任意层级，不成环）；skin → 角色；character 为空
  work_id TEXT REFERENCES entities(id),      -- 只用于 character
  original INTEGER NOT NULL DEFAULT 0,       -- 原创角色
  merged_into TEXT REFERENCES entities(id),  -- 合并后指向保留的那个
  origin TEXT NOT NULL,                      -- 最初从哪来：index / wd / user / album / check
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);

-- 外部链接：同一个 (system, key) 只能属于一个身份
CREATE TABLE links(
  system TEXT NOT NULL,        -- danbooru / ccip / wd / bangumi / photos_album / qualifier
  key TEXT NOT NULL,           -- 按各系统的规范写法，见下表
  entity_id TEXT NOT NULL REFERENCES entities(id),
  current INTEGER NOT NULL DEFAULT 1,        -- 0 = 旧写法（D 站改名前的 tag 等）
  source TEXT NOT NULL,        -- import / user / check（定期检查时确认的）
  added_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY(system, key));
CREATE INDEX links_entity ON links(entity_id);

-- 名字
CREATE TABLE names(
  entity_id TEXT NOT NULL REFERENCES entities(id),
  lang TEXT NOT NULL CHECK(lang IN ('zh','ja','en')),
  value TEXT NOT NULL,
  source TEXT NOT NULL,        -- user / bangumi / index / builtin / danbooru / model
  role TEXT NOT NULL CHECK(role IN ('display','alias')),
  preferred INTEGER NOT NULL DEFAULT 0,      -- 每个身份、每种语言最多一个首选
  PRIMARY KEY(entity_id, lang, value, source));
```

### 链接系统

| system | 挂在 | key 的写法 | 例子 |
|---|---|---|---|
| `danbooru` | 作品 / 角色 / 皮肤 | D 站 tag 原文，小写 | `goddess_of_victory:_nikke`、`airi_(band)_(blue_archive)` |
| `ccip` | 角色 / 皮肤 | 角色索引文件名（短名） | `airi_band_blue_archive` |
| `wd` | 角色 | WD 模型词表里的 tag | `male_doctor_(arknights)` |
| `bangumi` | 作品 / 角色 | 条目或角色的数字 ID | `12345` |
| `photos_album` | 角色 | 相簿名原文 | `miyo` |
| `ccip_base` | 角色 | 索引里「基础角色 tag + 作品」这一对（JSON），用于索引缺少基础角色、只有皮肤时补建的父角色 | `["james_moriarty", "fate"]` |
| `qualifier` | 作品 | D 站角色 tag 括号里的作品后缀 | `nikke`、`kancolle`、`fate` |

- WD 词表是 D 站 tag 在某个时间点的快照，可能已经过时，所以单独作为一个系统。导入 WD 结果时，先按 `wd` 链接找；找不到再按同名的 `danbooru` 链接找。
- `qualifier` 用来解析 D 站角色 tag。导入 `rapi_(nikke)` 时，靠后缀 `nikke` 找到「胜利女神：妮姬」。它不是可以对外查询的 tag，**不能**当作 D 站作品 tag 去关注。

### 名字

- **来源优先级**：`user` > `bangumi` > `index` > `builtin` > `model`（机器翻译），数字越靠前越优先。
- `danbooru`（wiki 里的其他名称）只作 `alias`，不进入显示。这和现在「wiki aliases must never enter the display layer」的规则一致。
- 从 tag 推出来的可读英文名（`airi_(band)_(blue_archive)` → Airi）**不存储**，显示时现算。
- 你手动改名，就是写一条 `source='user'`、`preferred=1` 的名字。自动拉取只增加其他来源的名字，不改这一条。

## 5. 规则

### 显示

1. 显示语言顺序是一个设置项，默认 中 → 日 → 英。
2. 对每种语言，依次尝试：首选名 → 该语言里来源优先级最高的显示名。
3. 所有语言都没有名字时：用 `danbooru` 链接推出可读英文名；没有就用 `ccip` 链接推；再没有就显示「未命名角色」或「未命名作品」。
4. 机器翻译的名字，在悬停提示里标出「机器翻译」。
5. 悬停提示里显示链接的原文（D 站 tag），和现在一样。
6. 名字不要求唯一。跨作品同名时显示作品，如「明里 · 蔚蓝档案」。同作品的独立角色使用相同自动中文名时，界面上凡是写角色名字的地方（资料库、选择器、整理卡片、详情和大图底栏、按天视图、网格卡片、角色页）都用接口给的形态限定区分角色，如「阿米娅（近卫）」；悬停提示写成「阿米娅（近卫，明日方舟）」。图的归属只带 ID 和名字，限定按 ID 从角色列表取。放不下一行的窄头像卡（书架、角色页的「同作品」）把「近卫」另列在名字下方。优先用形态短词的译名，缺少译名时用可读限定词，人工名称原样显示。其他语言的可见名称仍相同时，接口也提供相应的限定。限定只用于展示，名称、别名、搜索索引和身份仍用原来的数据。

原生 `GET /api/characters`（含摘要和按 ID 请求）、`GET /api/entities` 的本地结果、`POST /api/suggest` 的角色建议，以及 `GET /api/identity/queue`、`GET /api/ambiguous` 和角色资料的 `kept_apart` 中的角色端点，按需附上 `name_disambiguation`，其中 `qualifier` 是按页面语言生成的限定词，`tag` 是用来生成限定词的识别标签。`name` 仍是原显示名。首选识别标签未入词表时，可用同一身份的其他当前链接唯一对应到的词表条目。重名以已有作品 ID 为准，缺少作品归属时按可验证的作品标签匹配本地作品，或按词表中的作品和形态关联比较。搜索结果、分级和队列分页都共用完整的重名范围。

### 搜索

以下任何一项匹配都算命中：
- 所有名字，包括别名；
- 所有链接的 key，下划线当作空格；
- 搜作品名时，能搜出这个作品和它下面所有作品的角色。

### 创建

- **你新建角色**：只需要一个名字。作品可选，不选就是「未归类」；链接可以以后补。
- **原创角色**：`original=1`，自动命名为「原创角色 N」（写成 `source='user'` 的名字）。
- **新建作品**：只需要一个名字。有 D 站作品 tag 的话，挂一条 `danbooru` 链接，这个作品才能被「关注」。新建和编辑作品的对话框里可以直接搜 D 站作品，选中后填好 tag（名称空着时一起填），并可一并关注（#293）。

### 导入（CCIP 索引、WD 结果、定期检查补进来的角色）

1. 先按链接找：`ccip` → `danbooru` → `wd`。找到了，就把新链接补挂到这个身份上。
2. 按链接都找不到，就新建身份。作品按后缀，通过 `qualifier` 链接找：索引来的角色用索引给的作品文字，WD 结果建的角色用 WD tag 末尾括号里的后缀。找不到时，照第 25 版迁移的写法新建一个待核对的作品（「链接待确认」），挂上这个后缀的 `qualifier` 链接；英文名取后缀的可读写法，按文字键已有的作品名和别名一并写成它的名字（2026-10-24 用户定，#296）。这样同一个后缀在新、旧图库里都归到同一个作品。
   - 不是作品的后缀不建，角色「未归类」，作品文字照旧保留。判断只看核心里的本地清单（`character_import::not_work`），导入时不联网：人物类别（`vtuber`、`real_life`、`male` 等）；服装（`1st_costume`、以 `_costume`、`_outfit` 结尾的）；皮肤词表能整段翻译的形态（`swimsuit`、`new_year` 等）。很多作品后缀本身不是 D 站作品 tag（`kancolle`、`fate`），所以不用 D 站标签类别来判断；作品的 D 站链接照旧在作品管理里核对。
   - 已经建好的角色不补：只对这一次新建的角色起作用。
3. 新身份和已有身份的名字相似（含你手建的角色）时，只生成一条「可能是同一个」的提示，由你确认后合并。

### 合并

- 保留一个身份，另一个的 `merged_into` 指向它。
- 名字、人工结论并过去。**链接留在被合并的身份上**：按链接找到它之后，沿 `merged_into` 走到保留的身份（`crates/core/src/character_commands.rs` 的 `resolve`）。这样撤销合并时不用搬动链接，以后的导入也照样落到保留的身份上。
- 两边的首选名冲突时，保留目标身份的，另一个降为别名。
- 可撤销（沿用现有的撤销机制）。

### 关注作品（资料库 › 作品）

- 关注列表存的是**作品 ID**。检查时用这个作品当前的 `danbooru` 链接去查。
- 作品 ID 只在自己的图库里有效，所以关注列表和「最少张数」**按图库存**，在应用数据库 `prefs` 表的 `index_watch` 键（2026-10-05 用户定，#205）。
- 没有 `danbooru` 链接的作品，在列表里显示为不可选，并注明「D 站上还没有这个作品」。
- 给图设完角色后，提示条上可以直接关注这个角色的作品；作品还没有 `danbooru` 链接时，打开编辑作品对话框先找到 tag（#293）。
- 联想搜索到 D 站作品 tag 后：如果这个 tag 已经挂在某个作品上，就关注那个作品；没有的话，先找后缀或名字对得上的本地作品，让你确认后挂上链接，再不行就新建作品。

## 6. 场景推演

1. **新角色，D 站还没有。**
   - 你标图时新建「X」，没有任何链接。
   - 标了几张以后，研究任务的学习（`native/terms/src/research_compute/learn.rs`）从你的标注里学会认 X。
2. **几个月后 D 站收录了 `x_(game)`。**
   - 定期检查发现它，按规则提示「D 站新角色 `x_(game)`，名字和你建的『X』很像，是同一个吗？」。
   - 你点「是」：`danbooru` 链接挂到 X 上，参考图也补给 X。身份不变，你之前的标注都在。
3. **系统已经先导入成了另一个身份。** 用「合并」把两个并成一个：名字、标注并过去，链接留在原处，沿 `merged_into` 指向保留的身份。
4. **D 站把 tag 改了名。** 新 tag 挂成 `current=1`，旧 tag 改成 `current=0`。用旧 tag 查过来的数据也能对上。
5. **同名角色**（好几个作品都有 Akane）。身份靠 ID 区分，显示时带上作品。
6. **后缀有歧义**（fate）。`qualifier:fate` 挂在「Fate 系列」上。角色先归到系列，以后可以改到具体作品。新图库里第一次导入带 `fate` 后缀的角色时，自动建一个待核对的作品，你在作品管理里改名、挂上级或核对 D 站链接。
7. **不是作品的后缀**（`vtuber`、`1st_costume`）。导入时不建作品，也不建 `qualifier` 链接（清单见第 5 节「导入」第 2 条），带这类后缀的角色就「未归类」，等你来归。第 25 版迁移当年没有排除它们，旧库里可能已有这类待核对作品，保留不动。

## 8. 不在本模型范围内

- **WD 画面标签**（white_hair 等）和**我的标签**：它们描述的是图片，不是身份，维持现状。
- **组合、团体**（乐队、hololive 的某一期）：以后需要时，可以作为一种新的 `kind` 加进来，不影响现在的结构。
- **画师**：溯源模块另行设计。


## 9. schema 25 的兼容边界与操作入口

- 库内 `kind` 使用 `work / character / skin`，原创由 `original=1` 表达。现有前端角色 API 暂保留 `kind='oc' / ''` 的兼容输出，并提供 `work_id`。
- `work_id` 是所属作品的唯一依据；作品文字（原 `entities.work`，现为 `term_tree.work_text`）保留为名称更新器的文字上下文。按文字键的名字（原 `display_names / name_aliases`，现为 `term_key_names`、`term_key_aliases`）中的作品、服装词汇保留为缓存，作品身份自己的名称同时存入按身份的名字（原 `names`，现为 `term_names`）。不再用文字决定作品身份。
- 历史作品字符串逐个保留为待核对作品及 `qualifier` 链接。只对远端确认的同名、非废弃、类别为作品的 tag 自动建立 Danbooru 链接。无匹配、非作品 tag、链接冲突与多上级全部保留待确认，不能把未核验后缀当作远端作品查询。
- 设置 → 角色索引 → 作品管理支持新建、改名、修改上级及确认外部链接；角色的更多菜单可调整所属作品。角色页的作品分组可直接查看整棵子树的图片。所有树调整事务内防环。
- 关注列表的 version 2 格式存作品 ID，未解析的旧 tag 保存在 `pending_tags`，检查时重新读取当前 Danbooru 链接。2026-10-05 起存在各图库的 `prefs.index_watch`；图库还没有这条记录时，从早期版本共享目录里的 `char-patch/watch.json` 推出初值（v2 文件只认领本库存在的作品 ID 并带上 `pending_tags`，v1 tag 列表用本库链接解析，没有文件时用原来的 8 个默认作品 tag），第一次保存时写入本库。共享文件只读、不再改写。
- 自动树初始化不会覆盖用户调整过的上级。一次性作品协调与迁移核验工具已于 2026-10-05 删除（#186），当前作品管理通过上述原生入口完成。
