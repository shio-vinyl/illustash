# 用户标签、分级、WD 题材收编（词条第 3 步）

状态：现行参考，2026-10-01。[TERMS.md](TERMS.md) 第 4 节第 3 步的标签、分级与 WD 题材收编契约；画师对应契约见 [TERMS_ARTISTS.md](TERMS_ARTISTS.md)。

当前生产入口由 Rust 负责，普通启动不做 Python 后台切换；升级显式使用 `http --upgrade`。本文提到的 Store/adopt_* 是保留的测试参考，原生现行行为见 [架构方向](ARCHITECTURE_DIRECTION.md#生产入口与测试参考)。

细粒度 ACG 概念的本地学习、meme 候选筛选及验证边界见[研究方案](CONCEPT_LEARNING_PLAN.md)；该研究尚未改变本篇的标签与裁决契约。

## 1. 映射

| 旧 | 核心 |
|---|---|
| `source.sqlite` 的 WD `labels`（dim=rating） | 分级证据：词条 kind=rating，链接 `wd_rating:<值>`；判定器取最高分、恰好一个 |
| `labels`（dim=general，WD ≥ 0.2） | 题材证据：kind=topic，链接 `wd:<标签>`；≥ 0.35 机器认定（= 旧 `general_hits`），0.2–0.35 也可能是 |
| `asset_tags`（人贴的标签） | 用户概念：kind=concept，显示名来源 `user`；「加上」= 裁决 yes，「去掉」= clear（人收回，不是否定）；按 `actions` 历史重建，每条挂在原来那一步上 |

WD 证据由 `tests/reference/web/wd_export.py（#9 已删除）` 从 source.sqlite 导出成证据包（每 1,000 张一包，控制两边内存），来源 `wd`、方法 = labels.source（`wd_v3`）、版本 = 模型版本（`swinv2_448`）。链接不用 `danbooru`：Danbooru 画师链接和 WD 通用标签绝不能解析到同一个词条。原始的 2,000 万条 WD 分数仍只在 source.sqlite。

## 2. 切换

`legacy_labels::cutover`（命令行 `cutover-labels`），一个事务：

1. 确认 `asset_rating` 里每张图都已有分级证据（没交齐就拒绝，报还差几张）；
2. 收编用户标签并全体核对（核心里有效的「加上」必须与 `asset_tags` 完全相等；找不到历史步骤的标签整次回滚）；
3. `asset_tags` 改名 `legacy_asset_tags` 冻结（回退快照）（2026-09-29 起，词条 schema 第 4 版：切换不再留冻结副本，已有的冻结副本在升级时删掉；回退只靠切换前的自动备份，见 [TERMS_CHARACTERS.md](TERMS_CHARACTERS.md) 第 4 节。）；删掉可从 source.sqlite 重建的 `general_hits`、`asset_rating`；
4. 建三个**同名同形的视图**（`VIEWS`），Python 现有读查询一行不改；视图 join `assets`，图移除后自动消失；
5. 写 `meta` `terms_labels=core`。

历史 Python 测试参考的服务启动时 `tests/reference/web/server.py（#9 已删除）` 的 `adopt_labels` 在**后台**做：建表 → 全量交 WD 证据（期间 App 照常用旧表）→ 拿 `Store._labels_lock` 补交期间同步进来的图 → 备份 → 切换。失败或找不到核心程序则留在旧表，下次启动再试。

**切换之后的写路径：**

- 用户标签：`Store.tag` 解析目标（含重复组扩展）后调用核心 `tag`；文案不变。撤销：核心自己的步骤和旧的 `{uuid, tag, had}` 步骤都交核心（`owns` 规则见 TERMS_SCHEMA）。
- 同步导入新图（`import_changes`）：先在 Python 事务之外把新图的 WD 结果交给核心（`--new-assets`：图还没进 `assets` 也先登记），再开事务；事务里不再写 `general_hits`、`asset_rating`，单人图、线索图、角色归属照旧重算，通过视图已能看到新图。
- `refresh_general_hits()`：重交全部 WD 结果（核心按指纹去重）后只重算派生（单人图、线索图、规则、排除）；`refresh_ratings()` 只更新时间戳。
- 同步导入、重建 WD、切换三者用同一把可重入锁互斥，切换不会落在「交证据」和「开事务」之间。
- Python 升级 schema 重跑建表语句时，跳过给这三个视图建索引的语句（SQLite 对视图建索引一律报错，即使 IF NOT EXISTS）。

**回退**：先停止服务并保留当前包与 WAL，再使用切换/升级前的一致性备份恢复相容版本。现行库不依赖冻结旧表，不能通过改名视图或删除切换标记退回旧宿主。

## 3. 验收（「我的图库」精简副本）

副本：library.sqlite 一致性拷贝；source.sqlite 只读抽出 `assets` 与 WD 分级（全部）、题材（≥ 0.2），另补 0.15–0.2 的 `realistic` 行（「照片」排除规则的阈值是 0.15）。真实库里没有用户标签（`asset_tags` 为空），这部分只有夹具和单元测试验证。

- 切换：按 App 启动同一路径，**129 秒**，约 4.8 万张图；257 万条 WD 证据（分级 148,245、题材 2,423,556）。
- 切换前后读到的完全一致：`general_hits` 1,878,296 行、`asset_rating` 逐行相同；单人图 29,036、线索图 10,346、自动排除 698、排除 872、角色归属 42,058；按题材（solo、1girl、chat_log、swimsuit）、NSFW（隐藏 / 只看）、组合筛选的全部结果；总览、角色列表（隐藏 NSFW）、用户标签列表。
- 切换后从视图**重算**派生规则（`refresh_general_hits`，39 秒，重交的证据全部判为已有）：以上全部仍一致。
- 测试：Rust 53 项；Python 全量（新增 `tests/test_terms_labels.py`：切换前后读到的一样、核心写标签与撤销新旧步骤、同步新图与移除、没有核心、切换失败、升级跳过视图索引）。

## 4. 体积与性能（留待优化）

**体积**（library.sqlite，真实规模）：

| | MB |
|---|---|
| 切换前 | 429 |
| 257 万条 WD 证据导入后（含投影） | 1,029 |
| 其中 `term_evidence` / claim 索引 / term 索引 | 305 / 95 / 39 |
| 其中 `term_projection` / 其索引 | 99 / 49 |
| 切换删掉的 `general_hits` + `idx_general_value` | 约 215（页面留在文件里待复用；VACUUM 才会缩小文件） |

指纹改成 16 字节、不再全局唯一之前是 1,366 MB。

**同一批查询，旧表 vs 视图**（ms，结果全部相同）：

| 查询 | 旧表 | 视图 |
|---|---|---|
| `overview()` | 45 | 50 |
| `overview(nsfw=hide)` | 68 | 164 |
| `characters()` | 465 | 463 |
| `characters(nsfw=hide)` | 606 | 1,139 |
| `assets()` 第一页 | 26 | 26 |
| `assets(nsfw=hide)` | 50 | 148 |
| `assets(tag=solo)` | 77 | 96 |
| `assets(tag=solo, nsfw=only)` | 87 | 257 |
| `home()` | 48 | 46 |
| 单人图规则全量 | 135 | 473 |
| 截图排除规则 | 45 | 207 |

**时间**：全量交 WD 证据 109 秒（导入 90 秒，导出 19 秒；精简副本上。真实 source.sqlite 每张图约 430 行原始标签，按 uuid 取会多读，导出会更慢）。同步导入新图时只交新图，很小。

**可以优化的方向**（尚未做）：

1. NSFW 相关变慢最明显：`asset_rating` 视图每行要经 term_assets → projection → terms → term_links → assets。可把分级物化成按 uuid 的小表（每图一行，由核心在投影时维护），或给视图用到的路径加覆盖索引。
2. 证据行里重复的文本（来源 `wd`、方法 `wd_v3`、版本 `swinv2_448`、量纲 `wd_probability`、采集时间）可以归一到一张小表，每行约省 45 字节（约 115 MB）。
3. 0.2–0.35 的题材证据占两成（约 54.5 万条证据 + 同样多的投影行），目前没有界面用它；若不做「搜索放宽」可把入库下限提到 0.35。
4. 全量交证据时每行一次查询加一次插入；可以按资产批量比较指纹，或对首次导入跳过逐行查重。
5. 切换后可以做一次 VACUUM 回收删表留下的页面。
