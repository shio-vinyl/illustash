# 用户标签、分级、WD 题材（词条第 3 步）

状态：现行参考，2026-11-12。[TERMS.md](TERMS.md) 第 4 节第 3 步：标签、分级与 WD 题材在词条核心里的表示；画师见 [TERMS_ARTISTS.md](TERMS_ARTISTS.md)。现有图库 2026-09-28 起切到核心；切换命令（`cutover-labels`，原 `legacy_labels::cutover`）已随 #201 第 4 步删除（2026-10-20），核心第 8 版拒绝没切换完的图库，新建图库由 `new_library` 一开始就在核心里（[TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 14 节）。

## 1. 映射

| 来源 | 核心 |
|---|---|
| `source.sqlite` 的 WD `labels`（dim=rating） | 分级证据：词条 kind=rating，链接 `wd_rating:<值>`；判定器取最高分、恰好一个 |
| `labels`（dim=general，WD ≥ 0.2） | 题材证据：kind=topic，链接 `wd:<标签>`；≥ 0.35 机器认定，0.2–0.35 也可能是 |
| 人贴的标签 | 用户概念：kind=concept，显示名来源 `user`；「加上」= 裁决 yes，「去掉」= clear（人收回，不是否定） |

WD 证据由 `native/terms/src/app_migrate.rs` 的 `deliver_wd` 从 source.sqlite 导出成证据包（每 1,000 张一包，控制两边内存），来源 `wd`、方法 = labels.source（`wd_v3`）、版本 = 模型版本（`swinv2_448`）；交付前 `wd_pending.rs` 按与导入相同的内容指纹预检，没变的图不再交。链接不用 `danbooru`：Danbooru 画师链接和 WD 通用标签绝不能解析到同一个词条。原始的 2,000 万条 WD 分数仍只在 source.sqlite。

## 2. 视图与写路径

- `asset_tags`、`general_hits`、`asset_rating` 是核心之上同名同形的视图（`crates/core/src/legacy_labels.rs` 的 `VIEWS`），由浏览筛选和角色判定读取。视图连接 `assets`，图库移除图片后相应结果随之消失。分页补用户标签时，只读取当前页的有效裁决，见[性能参考](TERMS_PERFORMANCE.md#普通图库分页与计数)。视图的兼容用途见 [TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 13 节。
- 用户标签：`POST /api/tag`（`native/terms/src/app_commands.rs`）解析目标（含重复组扩展），按名字找到或新建用户概念（`legacy_labels::concept_by_name`），写裁决和一步 `actions`；文案不变。撤销：核心自己的步骤和切换前的 `{uuid, tag, had}` 旧步骤都由核心撤（归属规则见 [TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 4 节）。
- 同步和识别导入新图：先把新图的 WD 结果交给核心（`--new-assets` 语义：图还没进应用库 `assets` 也先登记，见 [EVIDENCE_PACKAGES.md](EVIDENCE_PACKAGES.md) 第 2 节），再在应用事务里重算单人图、线索图和角色归属，通过视图已能看到新图。

**回退**：先停止服务并保留当前包与 WAL，再使用升级前的一致性备份恢复相容版本。现行库不依赖冻结旧表，不能通过改名视图或删除切换标记退回旧实现。

## 3. 验收（「我的图库」精简副本，2026-09-28）

library.sqlite 一致性拷贝，source.sqlite 只读抽出 `assets`、WD 分级（全部）与题材（≥ 0.2，另补 0.15–0.2 的 `realistic`，「照片」排除规则的阈值是 0.15）。约 4.8 万张图切换 129 秒，交 257 万条 WD 证据（分级 148,245、题材 2,423,556）。切换前后与从视图重算派生规则后读到的完全一致：`general_hits` 1,878,296 行、`asset_rating` 逐行相同，单人图、线索图、自动排除、排除、角色归属的数量，按题材、NSFW 及组合筛选的结果，总览、角色列表和用户标签列表。真实库里没有用户标签，这部分只由夹具和单元测试验证。

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

**同一批查询，旧表 vs 视图**（ms，结果全部相同；查询名是当时 Python 读取函数的名字）：

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
