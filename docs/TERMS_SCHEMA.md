# 词条核心 schema

状态：现行参考，2026-11-19。当前词条 schema 为 **11**，应用 schema 为 **36**，包格式为 **1**；第 1 版见第 1–4 节，第 2–7 版的增量见第 7–12 节，第 8 版见第 14 节，第 9 版见第 15 节，第 10 版见第 16 节，第 11 版见第 17 节，宿主退役后的兼容职责与恢复边界见第 13 节。画师、标签、角色 / 皮肤 / 作品、名字都已收编，第 8 版起 `term_*` 是唯一权威，旧兼容表已删除（第 14 节）：[TERMS_ARTISTS.md](TERMS_ARTISTS.md)、[TERMS_LABELS.md](TERMS_LABELS.md)、[TERMS_CHARACTERS.md](TERMS_CHARACTERS.md)。下面第 1–6 节是第 1 版交付时写的，已按现状更正的地方都有注明（2026-09-29，词条第 4、5 步 S6）。

2026-09-28，[TERMS.md](TERMS.md) 第 4 节第 1 步的交付。代码在 `crates/core/`（Rust crate `illustash-core`，rusqlite 自持连接）；命令行和原生宿主在 `native/terms/`（`illustash-terms`）。证据包格式见 [EVIDENCE_PACKAGES.md](EVIDENCE_PACKAGES.md)。第 1 版只建了新表和规则，画师收编是第 2 步。

```sh
cargo test --manifest-path crates/core/Cargo.toml
cargo clippy --manifest-path crates/core/Cargo.toml --all-targets -- -D warnings
```

## 1. 放在哪里

表建在 **library.sqlite** 里，名字都以 `term_` 开头（加上 `terms`），和应用库原有的 `assets` 等表并存（`entities`、`names`、`links` 等旧表第 8 版起已删除，第 14 节）。放同一个库是为了共用撤销历史：人工裁决和标签、角色、作品操作要在同一张 `actions` 里排先后。

共用的只有两张，建表语句与应用库原来的建表语句逐字一致（已存在时是空操作）：

- `actions`：统一撤销历史。词条核心写的步骤，`before_json` 是 `[{"terms": …}]`。
- `meta`：词条表版本记在 `terms_schema`，与应用 schema 版本各自独立。

`migrate()` 在一个 `BEGIN IMMEDIATE` 事务里建表或升级，失败不留半套表；库的版本比程序新时拒绝，不降级。连接打开时开外键、5 秒忙等待，不改日志模式（归宿主）。

## 2. 词条类型与类型策略

类型（`kind`）只是词条的一个属性，差异放在类型策略里（`schema.rs` 的 `Kind::policy`），不拆表。表上用触发器约束取值：加类型要一次迁移重建触发器。

| kind | 名称 | 判定器（本步） | 互斥 | 包能否新建 | 以后由谁收编 |
|---|---|---|---|---|---|
| `character` | 角色 | 已收编（第 4、5 步合并）：旧 hybrid / 未设置策略的规则由核心原样执行（旧 SQL 写兼容表），不经 `store::project`；v2 不支持 | 单人图一个（规则 SQL 里） | 能 | 第 4、5 步 |
| `skin` | 皮肤 | 同上（`form_of` → `contains`） | — | 能 | 第 4、5 步 |
| `work` | 作品 | 同上；作品树在 `term_tree`（`parent_id`、上锁），不是 `contains` 关系 | — | 能 | 第 4、5 步 |
| `artist` | 画师 | 只收强证据：`pixel_hash`、`gallery_match`、`iqdb`、`same_picture` 为机器认定（第 3 版起加上后两种），其余（OCR、关注、分组推荐）只到「也许是 TA」 | — | 能 | 第 2 步 |
| `rating` | 分级 | 取 WD 概率最高的一个 | 恰好一个 | 能 | 第 3 步 |
| `topic` | 题材（WD general） | WD ≥ 0.35 机器认定（同 `general_hits`），0.2–0.35 也可能是；< 0.2 不入证据表 | — | 能 | 第 3 步 |
| `concept` | 用户自建的概念 | 待收编（规则、例子学习） | — | **不能** | 第 3 步 |
| `mark` | 人工标记 | 有效的 `yes` 投影为 `confirmed`，`clear` 收回 | — | **不能** | 第 10 版 |
| `label` | 只当标签 | 待收编 | — | 能 | 第 3 步 |
| `pair` | CP | 待收编（由共现算出） | — | **不能** | 以后 |
| `duplicate` | 重复组 | 系统生成：有证据即机器认定 | 至多一个 | 能 | 分组算法留在外围 |
| `similar` | 相似组 | 系统生成 | — | 能 | 同上 |
| `cluster` | 聚类 | 系统生成 | — | 能 | 同上 |
| `session` | 保存段 | 系统生成 | 至多一个 | 能 | 同上 |

策略还记录是否参与提问（角色、皮肤、画师、概念）。「单人图一个」要读图上的 solo 证据，归角色判定器实现，本步只登记。

## 3. 表

| 表 | TERMS.md 的角色 | 要点 |
|---|---|---|
| `term_assets` | 资产 | `(namespace, key)` 唯一，`md5` 只是证据；移除只记 `removed_at`，证据和裁决保留 |
| `terms` | 词条 | 整数 `id` 做内部外键；`key` 永不改变（触发器保护），界面链接和证据指纹用它；`kind`；`origin`（`user`、`package:<producer>`、`legacy`……）；`attrs` JSON |
| `term_names` | 名字 | 沿用 `names`：语言、显示名/别名、来源、首选、顺序 |
| `term_links` | 链接 | 沿用 `links`：`(system, key)` 只属于一个词条；合并走 `same_as` 关系，不搬链接 |
| `term_relations` | 词条关系 | 只存端点和种类（`contains`、`same_as`、`different_from`、`excludes`），状态由证据和裁决算出；对称种类存成 a < b，`same_as` 两个方向算一条 |
| `term_evidence` | 证据 | 目标是 (图, 词条) 或一条关系，二选一；来源、方法、版本、分数与量纲、`data` JSON、指纹、来自哪个包。**只增不删**（更新、删除都被触发器拒绝）；同一目标、来源、方法、版本只有一条；来源不能是人。指纹是内容摘要的前 16 字节，只在同一目标内比较（区分「相同，去重」与「不同，冲突」），不建全局唯一索引：WD 证据有几百万行 |
| `term_verdicts` | 裁决 | 只来自人（`origin` 只能是 `user`、`album`、`legacy`）；追加写，每条挂一个 `action_id`；`yes` / `no` / `clear`（收回）；`reviewed` 记裁决时看到的证据 id（v2「决议所审证据」） |
| `term_projection` | 投影 | `(图, 词条) → confirmed / machine / maybe / rejected`、最高分、来源列表；随时可由 `project_all` 整表重建 |
| `actions` | 撤销历史 | 共用 |
| `term_packages` | 证据包账本 | 包指纹唯一，同一个包只导入一次 |

视图：`term_effective_verdicts`（每个目标最新一条、所在步骤未撤销、不是 `clear` 的裁决）、`term_relation_states`（关系的四档状态）。

共九张 `term*` 表加共用的 `actions`、`meta`。比 TERMS.md 估计的「约八张」多出名字与链接分开、证据包账本两项。第 2 版（第 7 节）又加了八张，共十七张。

## 4. 规则

- **证据只增不删，来源互不覆盖。** 新版本的算法写新行，与旧版本并存；同一版本结果不同就拒绝导入。
- **裁决只来自人，否定永久有效。** 投影时有效裁决先于任何证据：`yes` → 人工确认，`no` → 已否定，之后再多的机器证据也翻不回来。
- **撤销走统一历史。** 裁决与改类型各是一步 `actions`。撤销裁决不删行，只把那一步标为 `undone`，有效裁决随之回到上一条；改类型的撤销按 `before_json` 恢复原类型。Rust 的裁决返回值和核心撤销入口使用 `store::ActionId`，与图、词条及关系编号区分。读写 SQLite、解析撤销请求与输出 JSON 时按各自边界转换整数。
- **统一撤销入口的这一半**（`undo_last`）：同一事务里取最近一条未撤销的 `actions`；是词条核心的就撤，不是就返回 `Foreign` 原样交回调用方，不跳过它去撤更早的一步。
- **哪些步骤归核心撤**：`store.rs` 的 `owns` 判断——非空数组、每项只有 `terms` 一个键；画师切到核心之后，只有 `artist_verdict` 一个键的旧画师判断也算（[TERMS_ARTISTS.md](TERMS_ARTISTS.md)）；标签切到核心之后，恰好是 `uuid`、`tag`、`had` 三个键的旧用户标签步骤也算（[TERMS_LABELS.md](TERMS_LABELS.md)）。角色步骤另按 `character_commands::owns` 判断：新的角色信封 `{"terms": {"v": 2, …}}`，以及每一项都是角色形状（快照、`merge`、`identity_relation`、`identity_anchor`、`profile`）的旧步骤，交角色撤销；角色形状与别的形状混在一步里、或形状不认识，拒绝且什么都不改（[TERMS_CHARACTERS.md](TERMS_CHARACTERS.md) 第 3 节）。原生宿主在同一程序内直接调用核心撤销；命令行 `illustash-terms undo --db <library.sqlite> --action <id>` 在自己的 `BEGIN IMMEDIATE` 事务里核对这一步仍是最近一步再撤，否则按「操作历史已变化」拒绝。
  命令行退出码：0 成功（标准输出一行 JSON），2 输入不合法，3 历史已变化或冲突，4 库里没有词条表或版本不对，5 命令要调用方补一个新 id（`Error::NeedId`），1 其他错误。子命令的现行清单是 `native/terms/src/cli.rs` 的 `COMMANDS`；桌面外壳兼当这个命令行，第一个参数在清单里时只执行命令、不开窗口。原来的收编、切换与核对命令（`sync-*`、`cutover-*`、`verify-*`）随 #201 第 4 步删除。
- **投影 = 证据 + 裁决 + 类型策略。** 裁决、导入、改类型、移除图都在同一事务里重算受影响的图。「至多一个 / 恰好一个」的类型：有人工确认时，同类型的机器认定全部降为「也可能是」；否则只留分数最高的一个。
- **包不能新建用户的东西**：概念、CP 只能由人或核心建；按 key 引用的词条必须已存在。

同一个人被合并（`same_as` 已确认）后证据如何归到代表词条，由第 4、5 步按原 hybrid 规则的 canonical 投影实现（[TERMS_CHARACTERS.md](TERMS_CHARACTERS.md) 方案第 5.1 节）；本步只存关系。

## 5. 与现有表的对应（按实际收编结果更正，2026-09-29）

第 1 版这里写的是预想，有三处与实际不符，已更正：
- `relations` 是图片相似关系，不是作品层级，没有收编；作品树在 `term_tree`。
- 名字表在角色那一步**没有**收编（D4）；之后的名字那一步（第 5 版）收编了，见第 10 节。
- 角色的归属没有收编进 `term_projection`：归属表都还在，由核心维护、加写保护（原称**兼容表**，第 8 版起定为派生表，第 14 节），`term_projection` 只是人物 / 皮肤归属的镜像。

| 现有 | 收编到 | 切换后的旧表 |
|---|---|---|
| 溯源的 `artists`、`artist_links`、`artist_names` | `terms`（artist）、`term_links`、`term_names` | 溯源库里照旧，由溯源 worker 写；核心经证据包收 |
| `image_artists`（`auto` / `candidate`） | `term_evidence`（来源 `provenance`，按快照批次生效） | 溯源 worker 的缓存 |
| `artist_verdicts` | `term_verdicts`（沿用原 `action_id`） | 冻结 |
| `general_hits`、`asset_rating`（来自 source.sqlite 的 WD） | `term_evidence`（来源 `wd`）→ `term_projection`（topic、rating） | 同名视图 |
| `asset_tags` | `terms`（concept）+ `term_verdicts` | 冻结为 `legacy_asset_tags`，同名视图 |
| `entities` | `terms`（character / skin / work）+ `term_tree`（结构、属性、先后） | 第 8 版删除（第 14 节） |
| `links` | `term_links`（角色的 `wd` 存成 `wd_character`） | 第 8 版删除（第 14 节） |
| `work_review` | `term_tree` | 第 8 版删除（第 14 节） |
| `identity_baseline` | `term_identity_baseline` | 第 8 版删除（第 14 节） |
| `identity_relations`、`identity_evidence`、`identity_decisions`、`identity_decision_evidence` | `term_relations`（`same_as` / `contains` / `different_from`）、关系证据、关系裁决，旧 id 在 `term_legacy_ids` | 第 8 版删除（第 14 节） |
| `assertions` | `term_verdicts`（有步骤的挂原 `action_id`，没有步骤的进 `term_batches`） | 核心维护的派生表（第 14 节；原来是兼容表，第 4 版之前另冻结一份 `legacy_assertions`） |
| `candidates` | `term_evidence`（`wd.wd_character`、`ccip_index.char_ext`，按证据批次生效） | 核心维护的派生表（第 14 节） |
| `proto_claims`、`weak_claims`、`prototypes`、`learn_state`、`identity_canonical`、`identity_scopes`、`identity_suspended`、`identity_uncertain`、`identity_claims`、`identity_anchor_*` | 判定器私有状态，没有另建核心表 | 原表，核心写 |
| `memberships`、`tag_memberships`、`family_evidence`、`single_assets`、`lead_assets` | 由核心执行旧 SQL 算出；人物、皮肤那一片镜像进 `term_projection` | 核心维护的派生表 |
| `names`（角色、皮肤、作品的） | `term_names`（按词条；画师的名字本来就在这里） | 第 8 版删除（第 14 节） |
| `display_names` | `term_key_names`（按文字键） | 第 8 版删除（第 14 节） |
| `name_aliases` | `term_key_aliases` | 第 8 版删除（第 14 节） |
| 选名规则（旧 Python 实现的 `View`，移植到 `names_view.rs`） | 显示投影 `term_name_display`、`term_name_choice` | 显示读路径读投影 |
| `relations`（图片相似关系） | 不收编 | 原表 |

「先投影后收编」在角色这一步的实际做法：先用影子兼容表（`shadow_*`）逐表对照，再在切换时把旧表变成兼容表；没有经过「只读视图」阶段（标签那一步是视图）。

## 6. 还没做（2026-09-29 更正）

第 1 版这里列的接入、判定器、真实规模测量，都已在画师、标签、角色三步里完成。现在还没做的：

- ~~名字投影~~：已做（第 10 节，[TERMS_NAMES.md](TERMS_NAMES.md)）。
- ~~删冻结旧表~~：第 4 版已删。兼容表在第 8 版删除或定为派生表，现行职责见第 13、14 节。
- `concept` 以外的 `label`、`pair` 及系统生成的类型（重复组、相似组、聚类、保存段）的判定器。
- ~~Windows 上外壳兼当命令行的实机验收~~：2026-10-01 已验收（#72，见 [BACKEND_TASKS.md](BACKEND_TASKS.md)「Windows」）。
- 性能：见 [TERMS_PERFORMANCE.md](TERMS_PERFORMANCE.md)。

## 7. 第 2 版（2026-09-29，词条第 4、5 步 S1）

为角色、皮肤、作品收编加的表，方案见 [TERMS_CHARACTERS.md](TERMS_CHARACTERS.md) 方案第 2 节。`migrate()` 从第 1 版升到第 2 版是一个事务；中途失败回到第 1 版原样（表、视图、数据都不变）。桌面 App 启动时照常自动迁移。

| 变更 | 内容 |
|---|---|
| `term_batches` | 非用户批次（`baseline` / `album` / `undo_audit` / `migration`）：没有 action 的人工状态。`anchor` 是它在时间线上的位置，排在 id = anchor 的那一步之后。只增不改 |
| `term_verdicts` | 重建：`action_id` 可空，新增 `batch_id`（两者恰好一个）、`anchor`（action 裁决 = action_id，批次裁决 = 批次的 anchor，由约束和触发器保证）、`via`（原 origin 细分，自由文本，≤ 40 字）。原有裁决全部是 action 裁决，`anchor` = `action_id`、`via` 为空。**每条写入都要显式写 `anchor`**（没有默认值） |
| `term_effective_verdicts` | 批次裁决或未撤销的 action 裁决里按 `anchor DESC, 批次在前, id DESC` 取最新一条；最新的是 `clear` 就没有。只有 action 裁决时与第 1 版的结果相同。多出 `batch_id`、`anchor`、`via` 三列（加在末尾） |
| `term_relation_latest` | 每条关系最新的一条裁决，**包括 `clear`**（人工退回 proposed），「未复核继承层级」看它的 `reviewed` |
| 证据批次 | `term_evidence_sets`（一次成功运行：来源、方法、覆盖 `all` 或列出的图）、`term_evidence_set_assets`、`term_evidence_set_rows`，都只增不改；有效集 `term_active_evidence`（每张图、每个来源当前生效的批次，`store::rebuild_active_evidence` 可由前三张重建）。只接受可替换来源 `wd.wd_character`、`ccip_index.char_ext`（`schema::REPLACEABLE`，与表上的 CHECK 一致） |
| `term_current_evidence` | 视图：可替换来源只取有效集里的行，其余来源全部有效。投影改读它。可替换来源的证据**没有批次就不生效**：以后这两个来源只能经证据批次写入 |
| 私有表 | `term_identity_baseline`（= `identity_baseline`，`seq` 保留原 rowid 顺序）、`term_tree`（身份的结构 `parent_id`、`merged_into`、`work_id`、作品文字，第 8 版起还有属性列和先后 `seq`，以及作品复核三列；键都是词条 key 原样）、`term_legacy_ids`（旧关系、决议 id ↔ 核心 id） |
| 写保护 | 私有表的 INSERT / UPDATE / DELETE 在 `meta` 里没有 `terms_writer` 时拒绝（`schema::guard`，兼容表建出来时也用它）。核心写事务用 `begin_writing` 拿钥匙、`commit_writing` 交还后提交；别的连接看不到未提交的钥匙。`migrate()` 会清掉残留的钥匙 |

迁移里重建 `term_verdicts` 时用了旧式 `ALTER TABLE … RENAME`（`legacy_alter_table`）：标签切换建的 `asset_tags` 视图引用 `term_effective_verdicts`，新式重命名会先校验所有视图，在重建的中途因「视图引用的表不存在」而失败。旧式重命名不校验也不改写视图；视图按名字引用，重建完成后照常可用（有测试：第 1 版的库带着标签视图迁到第 2 版，视图读出的结果不变）。

## 8. 第 3 版（2026-09-29，main 的找回画师整合）

溯源的证据整个来源按批次生效（`schema::REPLACEABLE_SOURCES`，批次的方法记为 `*`）：一个溯源证据包是 provenance.sqlite 的全量快照，包里标为「已完成」的图上，这个来源当前有效的证据就是包里的行，没有行就是查无结果（`store::publish_snapshot`）。`term_current_evidence` 相应放宽；画师判定器的强证据加上 `iqdb`、`same_picture`。迁移时已有的溯源证据收进一个全库覆盖的初始批次，迁移前后有效的证据不变。为放宽 `term_evidence_sets` 的 CHECK，父表和三张子表一起重建，失败回到第 2 版原样。读路径和人工裁决的规则见 [TERMS_CHARACTERS.md](TERMS_CHARACTERS.md) 方案第 16 节「画师证据的整合」。


## 9. 第 4 版（2026-09-29，拆脚手架阶段 A）

不加表。按切换标记删掉切换时冻结的旧人工表：画师切过的删 `artist_verdicts`，标签切过的删 `legacy_asset_tags`，角色切过的删 `legacy_assertions` 等 6 张（`legacy_characters::FROZEN_COPIES`）；没切换的库什么都不删（`artist_verdicts` 在那里还是活表）。之后的三次切换本身也不再留冻结副本，也不再为画师已切换的库重建 `artist_verdicts`。回退只靠切换前的自动备份。

已有词条表的库打开前先跑一次 `migrate` 升到当前版本，否则 `character-command`、`import`、`judge` 等命令拒绝旧版本的库，任何一次升版都会让写操作失败。现在升级图库格式要显式进行（`http --upgrade` 或升级界面，第 14 节）。

## 10. 第 5 版（2026-09-29，名字投影与收编）

方案、用户决定和结果说明见 [TERMS_NAMES.md](TERMS_NAMES.md)。第 4 版升到第 5 版只建四张表，不动数据；没切名字的库这四张表可以是空的（或是影子收编留下的内容，没有读者）。

| 表 | 内容 | 谁写 |
|---|---|---|
| `term_key_names(kind, key, lang, source, value)` | 按文字键的名字（作品的 qualifier、皮肤的 variant），原 `display_names` | 第 8 版起是唯一权威，核心名字命令直接写，在名字版本触发器里（第 14 节）；第 5–7 版由核心写事务从兼容表补齐 |
| `term_key_aliases(kind, key, value, position)` | 按文字键的别名（搜索用），原 `name_aliases` | 第 8 版起是唯一权威，核心名字命令直接写（第 14 节）；第 5–7 版由核心写事务按本事务改到的键从兼容表同步 |
| `term_name_display(entity, skin_only, name, name_source, zh, ja, en, tag)` | 显示投影：每个身份的 `View.entity(id)` 与 `View.entity(id, skin_only=True)`（`crates/core/src/names_view.rs`） | 核心整表重建：切换时一次；之后凡是改了 `names_version` 的核心写事务，提交前在同一事务里维护（`legacy_names::after_write`）：按依赖闭包重写受影响的行、删掉失效的行（名字行、身份和链接的变化都一样；闭包过大等少数情况整表重建，[TERMS_NAMES.md](TERMS_NAMES.md) 第 3 节）；命令要在事务中途读名字时（历史文案）先重建投影 |
| `term_name_choice(kind, key, name, name_source, zh, ja, en)` | 按文字键的选名结果 `View.choose(kind, key)`，键的范围：人物的 tag、作品文字（身份的作品文字与 qualifier 链接）、皮肤 variant | 同上 |

按身份的名字进 `term_names`（第 1 版就有的表，画师的名字已经在里面），只收角色、皮肤、作品（`term_tree` 里的词条）的行；第 8 版起它是这些名字的唯一权威（第 14 节）。`meta.names_projected` 记投影重建时的 `names_version`。

切换（`cutover-names`，前提是角色已在核心）：一个事务里最后一次收编（必须零差异）、`names`、`display_names`、`name_aliases` 加写保护、写 `terms_names=core`、整表建投影。之后名字写入都走名字命令（`names_commands.rs`），显示读 `term_name_*`，搜索走 `names_search.rs`。这个命令和别的收编、切换命令已随 #201 第 4 步删除（第 14 节）。

## 11. 第 6 版（2026-09-30，派生表物理压缩）

不改任何业务列、证据、阈值或历史。角色已切换的库将候选 `candidates` 与归属证据 `family_evidence` 改为 WITHOUT ROWID，原主键、辅助索引、写保护触发器和依赖视图全部保留；未切角色的旧表保持原样，首次切换时执行同一转换。迁移事务内任何一步失败，全部表与版本回滚。

文件库的已有核心版本升级前，先用 SQLite backup API 生成包含 WAL、保留隐式 rowid 的一致性备份，位置为同目录 `library-backups/terms-v旧版-before-v6-唯一值.sqlite`，既有备份不覆盖；备份失败即停止升级。恢复时停止 App / 所有库连接，保留当前库，将备份完整还原为 library.sqlite，并一并处理当前库的 WAL / SHM，再使用对应版本程序。切换前的 App 备份机制保留。

2026-10-01 起，提交后的 VACUUM 先检查物理行安全性：存在无索引、无 INTEGER PRIMARY KEY 的隐式 rowid 表时跳过整库空间回收并清理 pending，保留物理行，不在每次启动重复尝试。已有 WITHOUT ROWID 派生表转换仍提交；文件可能保留空闲页。安全检查只在空间维护 pending 时读取 schema 元数据，不扫描业务行；正常启动不做整库检查。只有索引或整数主键保护物理行的表才允许 VACUUM；忙或空间不足仍保留待重试标记。升级和直接 cutover 的备份都用 SQLite backup API，保留 WAL 与隐式 rowid。

历史核验留下的持久 shadow_* 副本被清除（同一步也在第 8 版之前的升级里执行）；之后的核对用连接内 TEMP 影子逐表精确比较，结束即清除，不反复扩大主文件（这套影子核对随 #201 第 4 步删除）。提交后仅在 terms_compact_pending 标记存在时做 VACUUM；忙或空间不足时，升级已经成功、数据有效，空间回收延后至下次启动重试，成功后清标记。不会每次正常启动都 VACUUM。

合成库：191,152,128 → 143,118,336 字节，为旧库 90,427,392 字节的 1.583×。备份另占 179,621,888 字节，不计入活跃库比例；迁移仍需要备份及 SQLite 临时空间。没有降低题材证据入库阈值。

直接调用四类 cutover 命令时，核心也会在切换前生成 `library-backups/terms-before-cutover-领域-唯一值.sqlite`（这些命令已随 #201 第 4 步删除）。

## 12. 第 7 版（2026-09-30，放弃身份策略 v2）

删掉只有 v2 用的 `identity_claims`、`identity_calibration` 两张表，以及 meta 的 `identity_calibration_active`、`identity_bank_error`。hybrid 下它们一直是空的，删掉不改变任何结果。别的身份表、`identity_bank` 都不动。一个事务；升级前照第 6 版的做法先备份。核心里对这两张表的引用同时删掉：身份图重建时的认领拆分、学习快照指纹里的两个 meta 键、延迟刷新的图片范围、角色切换的写保护清单。

## 13. 宿主退役后的兼容结构职责（2026-10-01）

旧 Python 宿主退出生产包（之后随 #9 删除）不等于数据库读写消费者退出。下表是现行结构的保留规则，文件路径均相对 `native/terms/src/`；旧表名不作为删除依据。（2026-10-01 时核心版本为 7；之后的第 8 版见第 14 节。）

| 结构 | 结论与实际消费者 |
|---|---|
| `entities` | 第 8 版删除（第 14 节）：身份只在 `terms` / `term_tree` 里。（`links`、`work_review` 也在第 8 版删除。） |
| `assertions`、`candidates` | 保留，第 8 版起是核心维护的派生表（第 14 节）：权威分别是 `term_verdicts` 和证据批次。`character_commands.rs` 写人工结论时同事务维护 `assertions`、撤销时按有效裁决对齐；写证据的命令同事务按图刷新 `candidates`（`character_projection::refresh_candidates`）；判定 SQL、`http_browse.rs`、`http_characters.rs` 按索引读。 |
| `identity_relations`、`identity_evidence`、`identity_decisions`、`identity_decision_evidence` | 第 8 版删除（第 14 节）：关系、关系证据和决议只在核心里，读者按同形片段 `character_relations.rs` 读。 |
| `identity_canonical`、`identity_scopes`、`identity_suspended`、`identity_uncertain` | 保留（`identity_baseline` 第 8 版删除，基线只在 `term_identity_baseline` 里）。`character_graph.rs` / `character_projection.rs` 的基线、代表身份、成员范围和不确定归属；`character_commands.rs` 修改与撤销，`character_learning.rs` 快照指纹读取。基线顺序有语义。 |
| `identity_anchor_decisions`、`identity_anchor_history`、`identity_pairs`、`meta.identity_bank` | 保留。`http_anchors.rs` / `http_characters.rs` 读取锚点及距离，`character_commands.rs` 裁决和撤销追加锚点历史；`character_learning.rs` 使用锚点决议。`identity_pairs` 的持久距离仍供现行读取，不因没有日常写入口删除历史缓存。 |
| `memberships`、`tag_memberships`、`family_evidence`、`single_assets`、`lead_assets` | 保留。`character_projection.rs` 和 `character_sql/` 的判定 SQL 生成；`http_browse.rs` / `http_characters.rs` 读取；研究计算（`research_compute/`）使用隔离快照。 |
| `asset_tags`、`general_hits`、`asset_rating` | 保留为核心派生视图。`legacy_labels.rs` 切换时删除同名旧物理表并安装视图；`http_browse.rs` / `http_app.rs` 以及角色判定 SQL 读取。不能把视图当作已退役物理表删掉。 |
| `actions`、`meta`、`term_legacy_ids` | 保留。`actions.before_json`、`undone` 与物理行 ID 是统一历史/撤销输入；`character_commands.rs`、`store.rs` 用它们恢复旧动作，`character_relations.rs`、`character_identity.rs` 按 `term_legacy_ids` 给出旧关系/决议 ID。`meta` 保存版本、切换标记、写保护钥匙与恢复状态。 |
| `term_identity_baseline`、`term_tree` | 保留。核心正式结构状态：身份由 `character_entities.rs` 建出，结构、资料、合并、导入命令和 `character_graph.rs` 直接写；不是可丢弃的影子。 |
| `term_*` 证据、批次、有效集、名字与投影；`terms` | 保留。`store.rs` / `package.rs` 导入正式证据、裁决与投影；`names_commands.rs` / `names_view.rs` 维护名字。追加证据/批次与已撤销动作不能仅按当前投影裁剪。 |
| `artist_verdicts`、`legacy_asset_tags`、六张 `legacy_*` 冻结副本 | 已有 v4 条件删除（`schema::v4`）；新建图库由 `new_library` 删掉 `artist_verdicts`。六张表的确切名单见 `legacy_characters::FROZEN_COPIES`。 |
| 持久 `shadow_*` | 已切角色库沿用 v6 清理（按名字前缀删）。影子核对与收编预演随 #201 第 4 步删除。 |
| `identity_claims`、`identity_calibration` 及两个退役 meta 键 | 已有 v7 删除，见第 12 节；不重复建第二个删除迁移。 |

`family_confirmed`、`membership_rules`、`variant_memberships`、`ambiguous_claims`、`identity_nodes` 等派生视图保留：新建图库由 `app_create_schema.sql` 建出，第 8 版迁移把其中读 `entities` 的改为读核心表（第 14 节），用于现行归属规则和 `http_characters.rs` 读取，不是旧路由转发的存储。

应用库的其他旧名称表也仍有职责：`assets` 是原生浏览/同步资产；`imported` 是相册刷新状态；`deferred`、`asked`、`audit_sample`、`reviews` 是人工选择、审核及撤销数据（`character_commands.rs` / `http_browse.rs`）；`curation`、`duplicates`、`excluded`、`auto_rules` 是整理规则（`app_curation.rs` / `app_commands.rs`）。`prefs`、`name_settings`、`name_cache` 分别供 `app_settings.rs`、`app_accounts.rs` / `app_names.rs`、`app_name_sources.rs` 使用。`sync_runs`、`sync_added` 由 `app_sync.rs` / `character_import.rs` 维护，历史和浏览仍读取。`backend_effects` 是 `app_effects.rs` 的提交后可恢复事件，不能删除 pending 行。

`prototypes`、`weak_claims`、`proto_claims`、`learn_state`、`clusters`、`cluster_info`、`similar_groups`、`image_sigs`、`relations`、`similar_paired`、`similar_retry` 保留：`app_research.rs` 校验并导入研究结果，`character_learning.rs` / `character_projection.rs` 消费学习产物，`http_browse.rs` / `app_curation.rs` 使用分组和关系；研究计算只在隔离快照写缓存，不能直接写正式人工表。`worker_protocol.rs` 只提供共用协议读写、版本校验、路径隔离和退出码检查，不取代这些业务表的消费者；`research_runs` 是原生任务恢复日志，`app_research.rs` 重启时处理 pending；worker 缺席不意味着缓存或任务历史失去用途。

源库保留 `assets`、`labels`、`decisions`、`characters`、`character_aliases`、`runs`、`asset_links`、`files`：`app_migrate_data.rs` / `app_recognition.rs` 将来源事实导入核心，`app_sync.rs` / `app_folder.rs` 维护盘点，`app_create.rs` 克隆角色索引、别名和模型运行参考，识别计算（`recognition_compute/`）及离线工具仍使用来源格式。历史包的 `shards` 及未知附加表不属于宿主桥接清理，保留原 DDL、typed cells 和 rowid；没有消费者不代表获准丢弃用户包中的研究记录。源库无统一单独整数 schema 版本，升级检查所需列与索引。溯源库独立的版本与证据/任务恢复见 [PROVENANCE.md](PROVENANCE.md)，不与同名 source `runs` 混淆。

升级与回退按 [ARCHITECTURE_DIRECTION.md](ARCHITECTURE_DIRECTION.md)「已有核心库的显式升级边界」执行：词条 5/6/7 → 8 仅提交应用库，源库保持原样；没切换完的旧图库不再升级（第 14 节）。同步、识别、索引和 worker 各自提交、恢复，没有跨库原子事务。注册或提交后事件失败要报告已提交，不能冒称回滚。恢复前停止所有连接；词条升级恢复应用库备份，并处理旧 WAL/SHM。禁止直接用旧程序打开新格式。

## 14. 第 8 版（2026-10-19 起，#201）

#201 让 `term_*` 成为唯一权威、收掉旧兼容表（方案与用户决定见 #201；第 8 版的迁移内容随各 PR 累积，第一个公开测试版发布前定稿，中间形态不写迁移）。做法：第 2 步加升级核对和撤销提示的两列；第 3 步按领域（别名 → 显示名 → 名字 → 链接 → 身份与作品结构）逐个把权威切到 `term_*`，切完的旧表在第 8 版迁移里核对后删除；第 4 步删掉收编与切换的机制，身份关系表切到核心，`assertions`、`candidates` 定为派生表。现在角色、皮肤、作品的身份、结构、名字、链接、关系都只在核心里；留下的旧形表只有核心维护的派生表、判定器私有状态和标签视图（第 13 节）。

- **升级核对**：已有核心表的库（升级前版本 ≥ 1）里有旧表时，四个领域必须都已切到核心（`terms_artists`、`terms_labels`、`terms_characters`、`terms_names` 都是 `core`），否则拒绝升级并提示先用 `pre-v8` 版本（git 标签，指向 v8 之前最后一个 main）打开升级。切换完的库里，各张旧表在退役时各自与 `term_*` 核对（见下各领域）；有差异就拒绝升级、整体回滚，错误里带样例。还没有核心表的库（新建的库）不在这里拦。
- **撤销提示的种类和张数**：`actions` 加 `label_kind`、`label_count` 两列（可空）。按张数写提示的六种角色步骤（设为、移出、加上、去掉、换成、标为已检查，`crates/core/src/undo_labels.rs`）写入时一并存下；之前的步骤在升级时按已存的提示解析一次回填。撤销载荷 `before_json` 不动（旧步骤载荷是数组，没有地方加键）。读历史时（`http_app::action_label`）用这两列把提示里的角色换成现在的名字，不再解析提示。
- **别名**（第 3 步第一个领域）：`term_key_aliases` 是唯一权威。Danbooru 别名命令（`names_commands.rs`）直接写它，名字搜索（`names_search.rs`）和读取接口（`app_names.rs`）读它；核心写事务不再从旧表同步别名。升级时（`legacy_names::retire_aliases`）照旧表 `name_aliases` 整表同步一次，核对一致后删表，不一致就拒绝升级。新建图库不再建这张表。
- **显示名**（第二个领域）：`term_key_names` 是按文字键的显示名的唯一权威。
  - 写：名字命令（`names_commands.rs` 的 `put` / `replace`）直接写它。
  - 读：选名（`names_view.rs`）、名字搜索、读取接口（`app_names.rs`）都读它。选名结果与行的先后无关，所以不需要旧表的插入顺序，`term_key_names` 不加顺序列。
  - 名字版本 `names_version` 的触发器从 `display_names` 换到 `term_key_names`（`legacy_names::VERSIONED`、`version_triggers`），增量维护显示投影时按它记下改过的键。
  - 升级时（`legacy_names::retire_display_names`）照旧表整表同步一次，核对一致后删表并改装触发器，不一致就拒绝升级。
  - 新建图库不再建 `display_names`；22 条内置作品中文名在核心迁移之后写进 `term_key_names`（`app_create_names.sql`）。
  - 研究指纹照旧计入 `term_key_names`：之前镜像表也在指纹里，所以写显示名对研究结果的影响不变。
- **名字**（第三个领域）：`term_names` 是角色、皮肤、作品名字的唯一权威（画师的名字本来就在这里）。
  - 写：`character_names::put_name` / `alias`（资料、合并、结构、导入、名字命令都经它），以及资料撤销、合并撤销、名字命令的删除，都直接写 `term_names`，按 `terms.key` = 身份 id 找词条。
  - 新建的身份一建出来就是词条（实体一节的 `character_entities::create`），写名字时就有词条 id。
  - 名字的先后：旧表的 rowid 参与排序（`ORDER BY preferred DESC, value, rowid`），按总方案加显式顺序列 `term_names.seq`：升级时从 `names` 的 rowid 填入，新行取当前最大值加一（与 SQLite 分配 rowid 相同），`ON CONFLICT DO UPDATE` 保留原值；读者按它排序。画师的名字没有这个顺序语义，`seq` 为空。
  - 读：选名（`names_view.rs`）、名字搜索、`http_read`、`http_characters`、`app_names`、`app_profiles` 都读 `term_names`；整表扫描的读者限定在 `term_tree` 里的词条，不把画师的名字算进去。
  - 名字版本 `names_version` 的触发器从 `names` 换到 `term_names`，只在角色、皮肤、作品的名字变化时计数（触发器带 `term_tree` 条件），画师名字的写入不计数，与原来只数 `names` 相同。增量维护显示投影时按词条找回身份 id。核心写事务不再有名字镜像，`after_write` 只维护显示投影。
  - 升级时（`legacy_names::retire_names`）照旧表整表同步一次、逐行核对，一致就填 `seq` 并删表，不一致就拒绝升级。三张名字旧表都退役之后才装新的版本触发器，退役时的同步不计入 `names_version`。
  - `cutover-names`、`sync-names`、`verify-names` 随第 4 步删除（见下）。
  - 新建图库不再建 `names`。
  - 研究指纹照旧计入 `term_names`（之前镜像表也在指纹里），写名字对研究结果的影响不变。
- **链接**（第四个领域）：`term_links` 是角色、皮肤、作品链接的唯一权威（画师等别的词条的链接本来就在这里，`(system, key)` 全局唯一）。
  - 读：共用片段 `character_links::LINKS` 给出与原来 `links` 同形的行（系统名照旧写 `wd`，`entity_id` 是 `terms.key`，只含 `term_tree` 里的词条），另带存储里的系统名 `core_system`；按系统查找时过滤 `core_system`，仍走 `(system, key)` 主键。读者包括选名、名字搜索、名字命令、资料、结构、导入、合并、身份图、HTTP 读取层、应用层的索引 / 名字 / 引导 / 本地统计 / 资料，以及研究计算的锚点。
  - 写：资料、结构、导入、资料撤销直接写 `term_links`（`term_id` 由身份 id 找；新身份在建出来时已镜像成词条，见名字一节）。写入前检查这个 (system, key) 是否已属于别的类型的词条（`character_links::check_free`），报错与原来命令末尾镜像时相同，所以 `INSERT OR REPLACE` / `ON CONFLICT` 不会顶掉画师的链接。
  - 名字版本 `names_version` 的触发器从 `links` 换到 `term_links`，带 `term_tree` 条件（画师的链接不计数）；显示投影追踪「改动前的链接」的 BEFORE 触发器也改挂 `term_links`，按词条找回身份，记下的行仍是原来的形状。
  - 角色学习发布的输入指纹（`character_learning`）里 `links` 这一项改按共用片段、原来的列序读，数据不变时指纹不变。
  - 升级时（`legacy_characters::retire_links`）按原来的兼容表映射逐行核对旧表与 `term_links`：链接在第 7 版里由每条命令同步镜像，所以与角色核对一样，不一致就拒绝升级（不先同步）；一致就删表。`links` 移出角色收编的兼容表清单和写保护清单，收编预检里只针对 `links` 的几项一并删除。
  - 新建图库不再建 `links`。
- **作品复核与身份基线**（第五个领域的第一部分）：
  - 作品复核只在 `term_tree` 的复核三列里（`review_status`、`review_note`、`parent_locked`，三列同为空就是没有复核行）。结构、资料命令直接读写它（`character_review`：与原表同形的读法 `REVIEWS`、定位一个身份复核行的条件 `OF`、新作品的复核行 `OPEN`），读取层的作品列表读 `REVIEWS`。
  - 身份基线早已以 `term_identity_baseline` 为准，`character_graph::baseline` 不再给旧表追加副本；角色学习的输入指纹改按原来的列读核心表，数据不变时指纹不变。
  - 升级时（`legacy_characters::retire_work_review`、`retire_identity_baseline`）逐行核对（基线连 rowid / `seq` 一起），不一致就拒绝升级，一致就删表。两张表移出角色收编的兼容表清单和写保护清单。
  - 新建图库不再建这两张表。
- **身份**（第五个领域的第二部分，用户 2026-10-20 决定属性列和外键的做法）：`terms`（key = 身份 id、kind）和 `term_tree` 是角色、皮肤、作品的唯一权威。
  - 结构：`term_tree` 加属性列 `variant`、`artist`、`label_only`、`mascot`、`mascot_origin`、`original`（原来在 `terms.attrs` 的 JSON 里，升级时填入，这些词条的 `attrs` 清空；`attrs` 只留给画师等别的词条），以及身份的先后 `seq`（唯一索引）。`seq` 从 `entities` 的 rowid 填入，新身份取当前最大值加一；原来按 rowid 的顺序（名字拼接、「第一个只当标签的身份」、身份基线的追加顺序）都按它。
  - 读：共用片段 `character_entities::ENTITIES` 给出与原来 `entities` 同形的行（不含核心从不承载的 `name`、`external_key`、`tag`），另带 `seq`、`term_id`；选名、名字搜索、资料、结构、导入、合并、身份图、判定 SQL、HTTP 读取层、应用层和研究计算都读它，或在判定 SQL、小的过滤条件里直接连 `terms` / `term_tree`。片段在 LEFT JOIN 右边时改为直接连表，免得子查询不能展开。
  - 写：新身份由 `character_entities::create` 一次写进 `terms` 和 `term_tree`（原来的 `INSERT INTO entities` 加命令末尾的镜像）；改结构和属性直接改 `term_tree`，改类型改 `terms.kind`。`mirror_state`、`born` 随之删除。
  - 名字版本 `names_version` 的触发器从 `entities` 换到 `term_tree`（只在身份的列变化时计数，复核三列和 `seq` 不算）和 `terms`（只算角色、皮肤、作品改类型）；显示投影追踪「改动前的身份」的 BEFORE 触发器也改挂这两张表，记下的行是 `ENTITIES` 的形状。
  - 角色学习发布的输入指纹里 `entities` 这一项改按片段读（不含三列），数据不变时指纹不变。
  - 升级时（`legacy_characters::retire_entities`，放在别的旧表退役之后，因为外键开着时引用 `entities` 的表要先删）：核心里还没有身份（新建的库）就照旧表建一次；然后逐行核对（含 rowid 与 `seq`），不一致就拒绝升级（第 7 版每条改身份的命令都同步镜像过，不先同步）。引用 `entities` 的视图按核心表重写（表名换成片段，列逐一对应）；最后删表。引用 `entities` 的 `identity_relations` 在这之前已经退役（见下）。核心从不承载的三列随表删除。`entities` 移出角色收编的兼容表清单、写保护清单和预检。
  - 新建图库照旧由 `app_create_schema.sql` 建出 `entities` 和旧形的视图，同一个第 8 版迁移把它们改成上面的样子（建库与升级走一条路）。
- **身份关系**（第 4 步二的后一半）：`term_relations`、关系证据（带 `relation_id`、方法 `identity_evidence` 的 `term_evidence`）、关系裁决（带 `relation_id` 的 `term_verdicts`）和旧编号 `term_legacy_ids` 是唯一权威。
  - 读：`character_relations` 的四个片段 `RELATIONS`、`EVIDENCE`、`DECISIONS`、`DECISION_EVIDENCE` 给出与原来四张表同形的行（列序同原表；`RELATIONS` 另带核心关系 id `core_id`），是原来核心每次写后重算这几张表用的同一段映射。读者：提议与决议（找已有关系、读现在的状态）、身份图（已接受的关系、未复核的继承层级）、身份对比读取（`http_characters`）、角色学习的输入指纹（按原来的列序读，数据不变时指纹不变）。原来没写 ORDER BY 的读者按旧查询计划的实际顺序补上：待定关系按 `id`；找一对身份之间的重复关系时先 `a` 是第一个参数的方向（原来按两个方向依次走唯一索引）。
  - 写：命令只写核心；原来写完按核心重算兼容表的 `project_relation` 删除。
  - 升级时（`legacy_characters::retire_relations`，放在身份之前，因为 `identity_relations` 引用 `entities`）：四张旧表与片段逐表核对，第 7 版每条改关系的命令都重算过它们，所以不一致就拒绝升级；一致就删表（引用别人的在前）。四张表移出写保护清单。
  - 新建图库照旧由 `app_create_schema.sql` 建出四张空表，同一个第 8 版迁移删掉。
- **第 4 步（一）：删除收编与切换的机制**（用户 2026-10-20 决定）：
  - 删除的：命令 `sync-*`、`verify-*`、`cutover-*`（画师、标签、角色、名字），以及它们背后的收编同步、收编预检、按历史重建人工结论（S2）、画师从溯源库收编（`legacy_artists.rs` 整个删除，资产命名空间 `LIBRARY` 移到 `store`）、标签与名字的切换。第 8 版拒绝没切换完的图库，它们再也遇不到真实数据。
  - 新建图库：原来在空表上依次跑四个领域的切换，现在由 `new_library::switch` 在一个核心写事务里做完，结果与原来逐行相同：三个空的全库证据批次（溯源；候选的 CCIP、WD）、删 `artist_verdicts`、标签三张表换成视图、删 `assertions` 上的身份触发器、派生表与核心独占的表加写保护、派生表压缩、四个切换标记、`names_version` 从 0 起并建显示投影。新建图库不再产生切换前备份（没有要备份的数据）。
  - 保留的：各 `retire_*`，写保护名单 `GUARDED`，标签视图 `legacy_labels::VIEWS` 与 `concept_by_name`，名字投影的维护。读切换标记的运行时检查暂时保留。
  - 第 4 步二：四张身份关系表切到核心（见上）；`assertions`、`candidates` 定为派生表（见下）；Python 时代的输出习惯另见 #252，发布后做；核心 5/6/7 → 8 的升级路径保留。
- **人工结论与候选是派生表**（第 4 步二的前一半，用户 2026-10-20 决定）：`assertions`、`candidates` 与 `memberships`、`family_evidence` 同等对待，不算兼容层。
  - 权威：人工结论是 `term_verdicts`（`assertions` = 每个 (图, 身份) 的有效裁决），候选是证据批次（`candidates` = 当前有效证据按 (图, 身份, 来源) 取最大分；策略未设置时身份沿破坏式合并解析到代表）。核心命令在同一个事务里维护它们（`character_commands` 写结论与撤销、`character_projection::refresh_candidates` 按图刷新候选），表、索引、写保护照旧，判定 SQL 和读者按索引直接查。
  - 为什么不改成从核心现算：实测（#201 评论，合成 5 万张图，负载下只看量级）判定 SQL 的增量重算从约 0.5 ms 变成约 230 ms，浏览里逐张查「被否定过」从约 0.2 ms 变成约 650 ms。有效裁决视图要对全部裁决开窗，候选要对全部证据聚合，按图的条件推不进去。
  - 原来第 8 版升级时由核心主数据生成 TEMP 影子、与兼容表逐表比对的机制（`verify_in`、`COMPAT`、影子表）随之删除：派生表随时可以由权威重算，各张旧表在退役时各自核对。
- 升级沿用现有入口：桌面和网页的升级界面（#193）、`http --upgrade`、`illustash-terms migrate`；升级前备份 `library-backups/terms-v<旧版>-before-v8-*.sqlite`，失败整体回滚。应用层（`app_upgrade.rs`）对没切换完的旧图库（应用 schema 36 以前、切换标记缺失或未完成、更早的 source 布局或缺索引元数据）打开和升级都直接拒绝，提示先用 `pre-v8` 版本；原来为它们准备的分阶段成对发布路径已删除。
- 对照：新对照基准（`native/terms/tests/baseline.rs`、`contracts.rs`，规则见 `native/terms/tests/common/baseline.rs`）的每个 v7 检查点都经这次升级再重放，接口回复逐字节不变；退役的旧表（含基准录制之后才退役的四张身份关系表，它们录下的变化一并略去）不比，内容在核心表里比；新加的列（`actions` 两列、`term_names.seq`、`term_tree.seq`）在比较器里登记排除，由历史提示和读者排序的回复比较覆盖；身份的属性在比较器里照原来的形状算进 `terms.attrs` 比较（`term_tree` 上的属性列不另比）。`native/terms/tests/it/v8_upgrade.rs` 覆盖拒绝（身份、身份关系、链接或作品复核的旧表与核心不一致、没切换完）不留痕、回填，以及各张旧表的内容原样进入核心、视图改读核心表、身份关系按片段读出原来的行。


## 15. 第 9 版：明确收回的人工状态

`human_state_marks` 按 `(domain, asset_key)` 保存 `curation/reset`、`deferred/resumed`、`uncertain/cleared`，关联有效的本库 `owner_action_id`，保留来源及原操作时间。正值仍在原业务表；有正值、明确收回和从未决定是三个不同状态。普通收录／角色命令在同一写事务中维护它们，通用快照里的空值不会凭空生成收回标记。

`human_state_mark_undo` 按 `(action_id, domain, asset_key)` 保存完整标记的 before/after；同一步多次写入保留最初 before 和最终 after。原所有者恢复业务行后恢复标记，有独立变更时整体回滚。两张表持有核心写保护；当前标记变动同事务更新 `meta.human_revision`，撤销账本本身不触发修订。

升级仅采用 v8 已存的 `actions.label_kind` / `label_count` 和完整快照、当前状态回填。字段缺失、种类未知或状态不一致时保持未决定；不从展示文案、裁决或 reviews 重新推断 v8 元数据，也不单独核验 v8 以下历史。没有元数据的旧收录 reset、skip/resume 不补造无行标记，新版本本地命令仍按本节三态合同记录。正常 v5/v6/v7 升级入口保留。有效 before 能恢复先前已证明的标记，撤销不遗留已撤销动作的标记。升级不读原图，沿用必需备份和单事务迁移；回退使用升级前 `library-backups/` 的完整 SQLite 副本。


人工便携快照的核心投影在 `human_conclusions::export`：调用方必须持有一致读事务。它按真实裁决排序保留 yes/no/clear、人工三态、身份上下文、复核和锚点当前决定，并携带所需定义；结构来源无法证明时标记 `legacy_unknown`。关系记录区分身份图关系与普通关系，按身份图实际使用的旧编号保留相对顺序；非人工名称只作描述，合并带来的别名要求完整、未撤销且仍有效的历史证明。证据包能自报 `source=user` 的普通显示名标为 `legacy_unknown`，不据此声称人工来源；外链仅携带 system/key 等身份字段，原始 URL 不进入快照，以免带出查询令牌。账号配置、任意 attrs、机器载荷、源文件路径及全部撤销历史不进入便携文件。

快照与报告编号为不透明字符串，客户端原样保存和回传，不依赖 UUID 写法。JSONL v1 使用 `type` / `record_key` 封闭字段和有效状态模式；尾部同时校验原始字节 SHA-256、排除快照编号与生成时间的 `state_sha256`、分类计数和资产哈希覆盖。`asset_identity_complete` 只说明资产摘要是否齐全，不承诺目标匹配或历史锚点能恢复。当前结构与原始基线分别查环；hybrid 身份关系还校验重复代表、父级及“确认不同”的冲突。源锚点缺少参考版本时原样标为空，不能编造版本。宿主完成与本次内容版本绑定的哈希后，通过 `Snapshot::with_hashes` 填充；核心投影不读取原图。

宿主 `app_human_conclusions` 核验完整图片摘要，复用相同来源版本、大小和 mtime 的缓存或读取完整字节，并在提交前再次核对变化；跨库认回仍须 SHA-256、长度、输入形态一致且候选唯一。导入由 `human_conclusions::import` 在一次写事务中补目标从未决定的逻辑键，专用 `human_conclusions/v1` 历史信封记录精确 after。重复内容无新 action；普通 `/api/undo` 先核对全部 after，再撤去这一步的贡献，独立改动导致冲突并回滚。通用核心/CLI 撤销不能完成宿主的收录投影，因此不接受这个专用信封。

第 9 版把词条和关系的物理记录分别存为 `term_records`、`term_relation_records`，原名 `terms`、`term_relations` 是带写入触发器的有效视图。原有对象照常可见；仅导入新建的对象由 `human_object_contributions` 关联创建 action，撤销后从全部现行读者中失效。不可变裁决的外键仍引用物理记录。`human_entity_origins` 保留源图库/词条到目标的映射，但匹配只采用仍生效的贡献。新定义撤销移除名称、外链和投影，释放外链唯一归属；重新导入创建新 action 和新词条，旧 action 和旧裁决保持失效。后来新增机器证据或独立依赖、定义被修改，都会使整步撤销冲突并回滚。名称维护触发器挂在物理记录上；创建词条/关系后按唯一键取 ID，不依赖视图触发器外的 `last_insert_rowid()`。

`human_relation_review_context` 按 `(verdict_id, fingerprint)` 保存导入关系曾见过的指纹及是否已审，不创建机器证据。身份图的未复核层级由当前有效决议携带的上下文与本地证据共同决定；后来新增的本地指纹不会自动变成已审。普通关系复核接管便携指纹，普通撤销继续追加现行审计决议，不能当作退回导入时的审阅状态。导入按关系连通组预检真实图投影，不改变目标已有身份和范围；目标父角色否决与源子确认、目标子确认与源父否决均阻断依赖组，hybrid 按有效断言的代表与家庭核对，保护合并后仍保留原始键的判断。已撤销导入的关系被普通入口重新启用时复用原关系编号，旧裁决继续失效。

已有词条的名字按 `(词条, 语言, role)` 分组补缺，目标已有人工名字组优先；其它语言或类型独立处理。撤销核对名字字段与组内相对顺序，仅 `source=user` 的名字允许普通资料撤销重新分配 `seq`；后台可整批重建的 provider 显示名与 Danbooru 别名只留在源文件作为描述，不落为可撤销贡献：`seq` 可复用，不能证明所有权。已有词条受影响名字组报告未处理，新定义逐行报告并省略这些上下文行；其余名字仍核对字段与顺序。恢复时仍采用保存的原始顺序号；组内独立改动使撤销原子拒绝，组外新增名字保留。补入既有词条的旧合并别名在有效导入历史中保留来源证明；Danbooru 别名会由名称更新器整批重建，同字面值和顺序号均不足以证明当前行仍属旧合并，统一标为 `legacy_unknown` 并在往返中保留这一标记。名字组结果在 `definition_fields` 单列，不能和依赖词条数相加。

公共链接只补尚未占用的 `(system,key)`，保留目标已有归属与 `current` 状态；Danbooru 当前链接槽被普通资料入口明确清空时也受保护，以当前清空标记或未撤销资料历史证明，不从空链接集合推断人工意图；其它公共链接可独立补缺；撤销只删除本次新增且字段未被独立修改的链接。可解析且两端均能认回的身份跳过对映射到目标键；不透明或悬空旧键报告依赖无法认回。撤销只移除本次新增的跳过对，保留之后新增的其它对。旧锚点决定没有可核实的参考版本时报告未处理，不凭参考名称恢复。

结构定义按源相对顺序追加，词条 key、父级、作品及 hybrid 原始基线统一映射；目标已有结构须相容，身份策略不同或同一依赖组有冲突则整组报告未处理。导入同时同步角色人工归属、范围和暂停附件；目标已有图片判断的附件缺省也保留。`reviews` 第 9 版主键为 `(action_id,entity)`，允许一次导入保留多个人物的复核。普通默认合并遇到同 action 的两条复核时保留较新时间，撤销恢复两份原值，不移动该 action 的其它人物记录。

`human_asset_hashes` 保存完整 SHA-256、长度、输入形态、来源签名、文件时间戳及观察时间。缓存只在同一来源版本、大小和 mtime 下复用；来源绑定变化、重新插入资产或观察到文件新版本时失效。删除资产可保留尚未失效的历史摘要供源快照使用，目标匹配始终要求在线核验。`meta.human_revision` 在人工 actions、撤销、裁决（含相册批次）、人工名字/链接/词条、三态标记、无 actions 的结构命令及哈希覆盖变化的原事务中递增；机器证据与学习缓存不触发。生成快照期间新增的修订仍待保护，不能归入先前读快照。

`human_backup_state` 持久化自动调度状态。`human_import_reports` 和 `human_import_report_rows` 与导入及本次 `effect_id` 一起提交，`pending_effects` 只查询这个事件，无改动导入不归属事件；零人工改动也保留完整报告，导入撤销和重启不删除报告；导出报告保留本会话最近一次。资产、依赖定义、人工逻辑记录分别计数，已有定义的名字组和链接另列 `definition_fields`，对应实体明细带 `field_counts`，各口径不能相加。

HTTP 沿用本机认证与会话门槛：`POST /api/human-conclusions/export {}` 启动独立导出任务，`POST /api/human-conclusions/import {"path":"本机文件"}` 与 `POST /api/human-conclusions/restore {"backup_id":"清单编号"}` 共用原导入槽；不带 path 时打开原生选择器。导入使用 `GET /api/imports` 和 `POST /api/imports/cancel`，导出使用 `GET /api/human-conclusions` 内的 `export` 和 `POST /api/human-conclusions/export/cancel {"id":"任务编号"}`。状态 GET 的 `automatic` 返回自动状态及已校验快照清单，不创建目录或触发写入。`GET /api/human-conclusions/report?id=报告编号&offset=0&limit=100` 分页返回 `report` 与 `details`，每页最多 500 条。导入摘要的 `committed` 与 `pending_effects` 分开描述数据库提交和效果落地，只有有效人工写入才有 `undo`。格式/版本错误为 400，输入超过 256 MiB、单行超过 4 MiB 或超过 200 万记录为 413，会话/任务/升级/撤销冲突为 409，IO 或 SQLite 故障为 500；异步结果保留同样 code 及提交状态。

自动快照的节奏、轮换、离线边界和各备份目录分工见 [图库](LIBRARIES.md#人工结论快照)。文件先以私有权限独占创建同目录临时文件，写入、刷盘、完整回读验证后原子且不覆盖地发布，成功后才轮换；恢复重新核对清单中的快照编号与语义摘要。

## 16. 第 10 版（0.3）

第 10 版的迁移随 0.3 的 PR 累积，在 0.3 发布前定稿。所有改动追加到同一个 `v10` 函数。0.3 只执行一次定稿后的第 10 版升级，开发中的中间形态不单独迁移。

### WD 角色扩展候选（#549）

扩展加入可替换证据来源 `wd_extension.linear_head`，重建证据批次父表、子表和有效视图，原批次 ID、历史证据和有效集保持原值。原 WD 与 CCIP 的刷新照原来源发布，扩展以单独批次生效。扩展证据的版本带包 SHA-256 和本次命中的标签、分数指纹，量纲为 `wd_extension_probability`，候选表显示为 `WD_EXT`。

扩展默认关闭。应用库的 `meta.wd_extension` 记录包 SHA-256、首次启用和上次手动检查的 UTC 时间、上次手动检查的向量数。启用与更新证据同事务提交，零命中也持久化启用状态。撤回在清空有效证据的事务里移除该键。第 1 版固定 228 个头及阈值 0.8008725047111511，按 SHA-256 核验整包。有识别交付账本的向量需已完成投影，并匹配原始模型与词表的 SHA-256。早期无逐图账本的存量沿用已保存的 WD 向量。评分用来源库读快照，应用提交前短暂持有来源库写锁并复核 SQLite 的数据版本。新识别结果、内容归档失效或资产移除按图撤去旧候选。同步结束后，已启用且共享包就位的图库仅重算本次完成交付的图片。

打分（#582）按块进行：每块最多 1,024 张图（输出矩阵不超过 2²² 个数，16 MiB），向量和全部头的权重做一次 f32 矩阵乘粗筛，可能越过阈值的头再按原来的做法复算（f64 顺序累加、sigmoid，概率取 f32）。跳过条件由内积的舍入误差界保证不漏掉命中，证明写在 `native/terms/src/app_wd_extension/scoring.rs` 开头，所以命中和写进证据的概率与逐图逐头计算逐位相同，和用哪种矩阵乘后端无关。macOS 用系统 Accelerate；Windows 用完整安装版带的固定 OpenBLAS，加载不了时（如基础安装版）用可移植的循环。取消在读每一行和每块打分前检查。

扩展候选对现有 WD、CCIP、原型和人工归属的分数、排序与皮肤投影没有贡献。已有规则未给出归属且没有人工确认时，才从符合人工否决及身份限制的扩展候选中取一项机器归属。两个包都命中时保留仍有效的原扩展来源。没有在用的扩展来源时，先取 WD 扩展，再取 CCIP 长尾。分数只在同一来源内比较。`--withdraw` 追加空批次，使扩展证据及其候选和归属同事务失效。历史证据和可被人工复用的角色定义仍留在图库。相同内容的重复接入和已撤回时的重复撤回均不写新批次。

仍生效的扩展证据阻止撤销人工快照新建的角色定义。扩展证据失效后只保留对物理定义的历史引用，人工快照可按原有的其他依赖与独立改动检查撤销该定义。

扩展新建的角色词条记创建来源 `extension:character`，复用已有角色时沿用其来源。角色选择器只在这种词条有候选、归属、人工名称或人工外链时列出它。撤回后留下的纯扩展定义供历史引用与重新启用时复用。

角色资料包（#602）：WD 扩展是角色资料包的 WD 部分，`meta.wd_extension` 的包 SHA-256 就是图库用的那个 WD 文件，可以是内置的 `wd-extension-v1`，也可以是签名包目录里某个版本的（见 [开发说明](DEVELOPMENT.md#发布角色资料包)）。换包就是用另一个版本重新启用：沿用同一个 `wd_extension.linear_head` 来源的批次替换，旧版本的证据行留作历史，换回时同样的命中复用原来的证据行。还没有 WD 向量的图库启用时（新图库默认启用），只写 `meta.wd_extension`（`scored` 为 0），不写证据，之后识别结果交付时按图检查。`meta.character_pack_declined` 记下本图库明确不用角色资料包（默认启用的提示里选了不用，或撤回过），再启用时删掉。`meta.character_pack_default_pending` 记下新图库第一次同步时角色资料包还不在本机（#630）：包就位、基础角色索引也启用以后（首次准备时基础索引随识别模型下载，晚到时等它，CCIP 部分记下的就是启用后的索引代数），宿主照默认启用，和用户点「启用」走同一个任务，检查已有的识别结果；启用、撤回或选了不用时删掉，选过不用的图库不记。分级格式的二级头只出候选：它的证据 `data` 带 `"tier": 2`，证据版本的指纹也带上级别；一级头的证据和以前逐字节相同。候选表照常显示为 `WD_EXT`，补机器归属时只看一级证据支持的扩展候选（`character_projection::assignable_extension` 按候选的同一套 SQL 只算一级证据），二级候选不补归属，也不影响已有归属。没有表结构变化。

### CCIP 长尾角色参考包（#559）

词条升级在同一事务中为 `ccip_longtail.anchor_p75` 加入独立、可替换的证据批次。`ccip_longtail_package` 记的是图库用的那个 CCIP 文件的 SHA-256（角色资料包的 CCIP 部分，#602），换包和 WD 扩展一样是用另一个版本重新启用；还没有 CCIP 向量的图库启用时只记这一项和 `ccip_longtail_last_scan`，不写证据。旧批次和有效集逐行保留；包摘要 `ccip_longtail_package` 按图库存放，零命中也可区分启用和关闭。成功重扫在相同事务中记录启用时间、上次检查时间和评分张数（`ccip_longtail_last_scan`）。撤回追加空的有效批次并清除启用状态，历史仍可读取，也允许之后撤销仅被已撤回证据引用的人工导入定义。

### 人工标记（#447）

升级重建 `term_records` 的类型触发器，已有词条与历史保持原值。第一次收藏时，在裁决事务里建立 `mark:favorite`（`kind=mark`、`origin=user`）。归档包含全部裁决。人工结论快照按有效的人工状态携带收藏和收回，跨图库恢复按固定 key 找回词条、按原有内容校验匹配图片。撤销恢复会撤掉这批裁决，保留可供后续收藏复用的固定词条。收藏的读取和写入见 [API 契约](WEB_V2.md#api-契约)。

例图候选来源 `example` 在同一次升级中登记为可替换证据。方法是概念词条 key，按图、来源和方法发布有效批次，空批次只令同一标签的旧候选失效（#157）。


### 画廊（#445）

同一次升级创建 `galleries`。每行保存名称、规范筛选 JSON、手动封面 UUID、顺序和时间，自动封面按当前选区读取。排序相同的画廊按 ID 排序。每次新建、编辑或删除在同一事务写入统一历史，撤销恢复原行。画廊及其操作历史不参与学习输入指纹。画廊随整库备份和 `.pilib.zip` 归档保存，0.3 的人工结论快照只携带现有的人工裁决等状态（#533）。

### 相似组的看过了（#427）

同一次升级创建 `similar_seen(id, uuid, seen_at)`，主键 `(id, uuid)`，另有 `uuid` 索引。一个 `id` 是一次「看过了」，行是当时这组按调用方 R18 设置看得到的图。相似组的组号随 `similar_groups` 重建而变，所以按成员记：一组现在看得到的成员全部落在某一次标记里，就算看过。标为没看过时，只从覆盖这组的标记里删掉这组的图。标记和取消各在同一事务写入统一历史，撤销恢复原行，不插 `backend_effects`，不触发学习。表和它的历史都不算学习输入（`app_research.rs` 的 `UNREAD`）。随整库备份和 `.pilib.zip` 归档保存，不进人工结论快照：它记的是整理进度，不是对图的人工结论。


## 17. 第 11 版（0.4）

手选画廊（#727）在现有 `galleries` 上加 `kind`，取值为 `condition` 或 `manual`。升级先按原有流程备份，再保留每条画廊的编号、筛选、封面、顺序和时间，既有画廊都是 `condition`。新增画廊编号不再复用，旧筛选链接不会指到另一画廊。收藏仍由 `mark:favorite` 提供。

`gallery_members(gallery_id, uuid)` 以两列为主键。图片失去可见性或从资产表移除时仍保留成员记录。图库的统一筛选决定哪些成员可见，重复组继承任一在册副本的成员关系。增删成员时历史保存实际改动的 UUID。删除画廊级联删除成员，撤销恢复画廊及全部成员。画廊及其历史随整库备份和 `.pilib.zip` 归档保存，继续排除在人工结论快照（#533）和学习输入之外。接口见 [Web API](WEB_V2.md#api-契约)。
