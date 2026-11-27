# 名字：核心数据、显示名来源与收编（词条第 4 步的名字这一半）

2026-09-29，[TERMS.md](TERMS.md) 第 4 节第 4 步里角色那一步留下的名字（[TERMS_CHARACTERS.md](TERMS_CHARACTERS.md) 的 D4）。做法与前几步相同：先投影对照，再收编，然后切写路径和读路径，最后在真实库副本上验收。

状态：现行参考，2026-11-14（并入原 `WEB_NAMES.md` 与 `TERMS_NAMES_PLAN.md`）。名字收编已完成并合入 main；核心第 8 版（#201）起三张旧名字表删除，`term_names`、`term_key_names`、`term_key_aliases` 是唯一权威，收编与切换命令也已删除（[TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 14 节）。第 2–7 节写的是当时的切换、接线和验收，Python 部分是历史测试参考。

本文是结果说明。显示名的来源、联网补全与后台更新见第 9 节（原 `WEB_NAMES.md`）；方案、用户确认的决定（方案第 5 节）和各段（N0–N6）定下的设计在下文「方案」一节（原 `TERMS_NAMES_PLAN.md`，节号沿用）；schema 在 [TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 10、14 节。

**范围**：
- 角色、皮肤、作品的名字（`names`）、按文字键的名字与别名（`display_names`、`name_aliases`）；
- 选名规则（旧 Python 实现的 `View` 与词表，移植到 `crates/core/src/names_view.rs`）、名字搜索；
- 历史文案的生成，以及剩下写入口的预演（拆脚手架阶段 C 里依赖名字的部分）。

**不在范围**：
- `name_cache`、`name_settings`：名称更新的联网缓存与模型设置，外围（现由原生宿主维护，见第 9 节）；
- 画师的名字：画师那一步就在核心的 `term_names` 里；
- 名称的联网查询本身：当时是 Python 的 worker，现由原生宿主执行（第 9 节），结果都经名字命令交给核心。

## 1. 映射

原则是**等价迁移**：界面、HTTP API、选名结果、搜索结果、历史文案逐字不变。旧表变成由核心维护的**兼容表**（同名同形，加写保护），核心另存主数据；核心第 8 版起兼容表删除，核心直接读写主数据。

| 旧 | 核心主数据 | 切换后 Python 读到的 |
|---|---|---|
| `names`（角色、皮肤、作品的行） | `term_names`，按原样存（来源、语言、display / alias、首选、顺序；用户名字就是来源 `user`，决定 N3） | 核心第 8 版起删除，核心直接读写 `term_names`（[TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 14 节） |
| `display_names` | `term_key_names` | 核心第 8 版起删除，核心直接读写 `term_key_names`（[TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 14 节） |
| `name_aliases` | `term_key_aliases` | 核心第 8 版起删除，核心直接读写 `term_key_aliases`（[TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 14 节） |
| 选名规则 `View`（`choose`、`entity`、`base`、`own_name`）与词表 `SKINS`、`WORK_NAMES`、`readable`、`skin_zh` | 逐条移植（`crates/core/src/names_view.rs`），算成显示投影 `term_name_display`（每个身份的完整名与皮肤短名）、`term_name_choice`（每个文字键） | 显示读路径读投影（第 3 节） |
| `names.search_hits` | 逐条移植（`names_search.rs`），命令 `names-search` | `search_hits` 调核心 |

Python 里的词表副本仍给 NameUpdater 用，两边由测试钉住相同。`str.casefold()` 按码位表移植（`casefold.rs`，由 `tools/gen_casefold.py` 从服务 App 的 Python 生成）。

## 2. 切换

**这条命令和收编代码已随 #201 第 4 步删除（2026-10-20）**：核心第 8 版拒绝没切换完的图库，新建图库由 `new_library` 一开始就在核心里（[TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 14 节）。下面是当时的过程，现有图库都是这样切过来的。

- 命令 `cutover-names`，前提是角色已在核心。一个事务里完成：最后一次收编（必须零差异）、三张名字表加写保护、写 `meta.terms_names=core`、整表建显示投影。
- 当时的 Python 服务启动时按「标签 → 角色 → 名字」顺序切换（`adopt_names`）。切换前先备份；失败就留在旧表，下次启动再试。
- 已切换的库每次启动先跑一次核心 `migrate`（拆脚手架阶段 A 的 `upgrade_terms`）。真实图库现在是第 3 版，第一次用这个分支启动时会升到第 5 版，同时删掉冻结的旧表。

## 3. 切换之后

**写**：
- NameUpdater 的整批替换、Danbooru / Bangumi 别名、索引种子，以及相册标题别名，都走核心命令 `names-command`。报错文字与旧代码相同，核心拒绝时转成 `ValueError`。
- 用户改名、别名、合并、作品编辑，本来就在角色的核心命令里。
- 所有核心写事务的统一提交点 `schema::commit_writing` 会检查：`names_version` 变了，就在同一事务里维护显示投影。核心第 8 版起名字、按文字键的显示名和别名都由命令直接写 `term_names`、`term_key_names`、`term_key_aliases`，没有要同步的旧表（之前的增量镜像见 #102）。测试构建（`illustash-core` 的 `verify-incremental` 特性）设了 `ILLUSTASH_TEST_VERIFY_INCREMENTAL` 时，每次局部维护投影之后在保存点里整表重建核对一遍，必须一致；录制重放和固定契约都打开它。
- 显示投影：本事务只改了名字行（`term_names`、`term_key_names`）时，从记下的身份和键出发按索引求依赖闭包（身份 → 它的人物键、子身份、它并入的身份；键 → tag / variant / qualifier 是这个键的身份），只为闭包和选名要读的身份（父身份、并入者、同 tag 的身份）载入快照，重写这些身份和键的投影行；`entities` 上没有索引的 `variant`、`merged_into`、`work` 各一条扫描语句，只在闭包里有对应的键或身份时执行，按用户决定只记录、不加索引。闭包碰到空 tag 的人物键或空 variant 的皮肤键时整表重建（2026-10-13，#103）。
- 身份或链接变化（作品移动、新建、合并与撤销合并、父子与变体、导入）也局部维护：TEMP 触发器在一个身份第一次被改动时存下它事务开始时的行和链接（`INSERT OR REPLACE` 会顶掉的行也先存下），闭包从变化前、变化后两侧出发——两侧的人物键、皮肤键、作品键都重算，变化前并入的身份也重算；不再存在的身份删掉两行显示投影，库里不再出现的键删掉选择行。事务中途为历史文案刷新过投影的，那次写过的身份和键在提交时按最终状态再算一遍。只改投影不读的列（如 `entities.name`、`links.source`）不算结构变化。闭包大到全库八分之一以上时（例如批量导入）整表重建（2026-10-14，#104）。
- 测试构建打开上面的开关时，每次局部维护后在保存点里整表重建，两张投影逐行逐列比较，按只改名字、改过结构、退回整表三类分别计数。
- Python 直接写名字表会被写保护拒绝。

**读**：
- `names.view(c)`：名字已在核心时给 `ProjectedView`，接口与 `View` 相同，按 `names_version` 在连接之间共享快照；否则给 `View`。三个 SQL 函数 `entity_name` / `entity_info` / `work_name` 经它注册，嵌着它们的 34 处查询一行没改。`Store._display_reader`、`names.action_label`、NameUpdater 的统计与翻译前检查也都用它。
- 投影只收库里出现过的键。别的键（任意 tag 的 `work_name`、词表里有但库里没有的作品、空 tag 的人物键）按这个键自己的行现算（`names_view::choose_key`）：它在 `term_key_names` 里的行，人物键另加 tag 是它、会把显示名并进人物键的身份的显示名；选名结果与行的先后无关，不必整库载入。空 tag 的人物键要扫描 `links` 找没有 tag 的身份，只记录、不加索引（2026-10-14，#105）。
- 名字更新器不再整库载入：选作品名、模型预演、皮肤翻译前检查都用 `choose_key`。模型预演仍在回滚的事务里写入待提交改动，再按键现算，读到的是改动后的名字，不必先刷新显示投影。提交时投影按闭包维护；闭包碰到空 tag 或空 variant 时整表重建（这时整库载入一次）（2026-10-14，#105；#102 的整批快照已删）。
- `/api/names` 的统计：名字已在核心、且 `names_version` 等于 `names_projected`（投影与名字表一致）时，身份和键的筛选在 SQL 里做，各键的选名从 `term_name_choice` 按种类汇总，只有投影外的键（空 tag 的人物键）交给 `choose_key`；否则退回一次整库载入（2026-10-14，#105）。
- 测试构建打开上面的核对开关时，`choose_key` 的每次选名、`/api/names` 的投影统计都与同一事务里整库载入的结果比较，分别记为 `choice`、`names_counts`。
- 搜索：`search_hits` 调 `names-search`，旧实现保留为 `search_hits_python`，没切换的库照用，也作对照的标准答案。

**历史文案与预演**：
- 角色命令的历史文案由核心按旧代码逐字生成（`character_labels.rs`）。名字读显示投影；名字还没切的库，核心用移植的选名规则现算。
- 写入口都不再「先演一遍旧代码」：构建器只做 Python 才看得到的参数检查、按筛选条件选图、联网查版权，其余检查由核心按旧顺序、用旧文案做。
- 新建作品的 id 在核心走到那一步时向 Python 要，uuid 的抽取次数和先后与旧代码相同。

## 4. 回退

只靠切换前的自动备份（`library-backups/`）。切换是单向的：切换后 main 的旧代码写名字表会被写保护拒绝。

## 5. 与旧行为的差别

- **非法输入的一处**：`set_work` 或改资料时 `work` 传列表或对象，旧代码是 SQLite 绑定参数失败（`ProgrammingError`，HTTP 500），现在核心报「作品不存在」（400）。前端不会发这种值。**2026-09-30 用户确认接受。**
- 除此之外，规则、文案、结果都没有差别。拒绝对照、双跑、真实副本上的三方对照都是逐字比较。
- 预演原来挡在前面、核心自己的检查与旧代码不一致的几处，拆掉预演时已在核心补齐：tag 冲突的报错里是角色名字、Python 的 `strip` 语义、`work` 的真值、`uncertain` 选图时的拒绝。

## 6. 验收

**测试**（全量：cargo 77 + 3、clippy 零警告、JS 40、Python 816 项，跳过 11）：
- 契约、投影、收编、写入双跑、拒绝对照（原 `test_terms_names_contract`、`test_terms_names_projection`、`test_terms_names_sync`、`test_terms_names_writes`、`test_terms_character_refusals`）：2026-10-07 起改为核心调用重放（#9）；2026-10-19 起由新对照基准 `native/terms/tests/baseline.rs` 中同名模块接替（#201，`test_terms_names_sync` 只测旧表同步，已退役），比较规则见 `tests/common/baseline.rs` 开头。
- 选名规则：`native/terms/src/app_names.rs` 的 `matching_tests`、`sources_tests` 在原测试保存的图库状态上用同样的提供方回答驱动原生名字任务，核对原期望；传输层的重试与节奏见 `app_name_sources.rs` 的 `network_tests`。
- 每一段都做了变异检查（故意改坏一处规则，对照能报出）。

**真实副本**（N6）：
- 投影与旧 `View` 全量零差异：9,276 个身份、10,718 个文字键；收编零差异，再跑不改。
- App 路径切换成功。三方对照零差异：名字全量、4,057 个搜索查询、4,300 多项读接口，含全部 1,478 条历史文案。
- 双跑 26 步、连续撤销 50 步（其中 42 步是真实历史），每一步都零差异。
- 故意破坏投影或主数据，都能报出。
- **真实数据里的新情况**（都不是差异）：
  - 图库的核心 schema 是第 3 版，一次升到第 5 版；
  - `library.sqlite` 带着一个没检查点的 34 MB `-wal`，拷贝方法因此改为连同 `-wal` 一起只读拷贝；
  - `term_names` 里另有 80 行画师的名字，按设计不在名字收编的范围里。

**桌面 App 在真实副本上走一遍**（2026-09-30，合入 main 之后，用户同意）：
- 副本：两个库连同 `-wal` 只读拷出后，在副本上用备份接口生成；`source.sqlite`、缩略图、特征用 APFS 克隆。`library.json` 改名为「名字切换桌面验收副本」。拷贝前后原库的 mtime 与大小不变。
- 启动：main 的桌面外壳（debug 构建，产物放在 scratchpad，不碰主目录的 `desktop/target`）。参数 `--library <副本> --no-sync --no-names --no-similar --no-index-check`；`ILLUSTASH_APP_SUPPORT` 指到 scratchpad，事后核对全局的 `libraries.json` 没被修改。
- 启动时自动完成：核心 schema 3 → 5（删冻结表），名字切换（含备份）13 秒；`names_projected` 等于 `names_version`。
- 界面：图库首页正常（角色显示中文名），控制台没有错误。
- 经同一个服务的接口：
  - 搜索「圣园」得到圣园未花；
  - 把她改名为「验收改名」：历史文案「已更新 验收改名 的资料」，按新名字、旧名字都能搜到；
  - 撤销后名字回到「圣园未花」。再改名、撤销两轮，结果相同。
- 关闭 App 前 `verify-names`、`verify-characters` 都零差异。副本、备份与构建产物用完已删。
- **记下的性能**：
  - App 里改一次名约 1 秒（验收脚本测的是 0.3 秒，差在服务端接口与名字快照重载）；
  - **撤销一次改资料约 9.4 秒**：撤销资料步骤总是全量重建身份图、重算证据与归属。旧代码也是这样（`_profile_changed(c, True)`），不是新差异。后来已优化（撤销改名 / 改链接在合成数据上约 40 / 114 ms，见 [TERMS_PERFORMANCE.md](TERMS_PERFORMANCE.md)）。

## 7. 体积与性能

2026-09-29 在真实副本上的测量（单次中位数，早于 #102–#105 的增量优化）：
- **库体积**：没变。新表约 6.5 MB，用的是删冻结表腾出的空页。
- **搜索**：64.7 → 34.3 毫秒。
- **只读名字的写入口和读路径**：稍快。
- **写名字的操作变慢**：NameUpdater 单次整批替换 4 → 195 毫秒，改资料 53 → 298 毫秒。每次都要启动核心、补齐主数据、整表重建投影；改资料时，事务中途为了文案还要多重建一次。
- **名字切换**：App 路径 10.4 秒，其中 9.6 秒是切换前的备份。

## 8. 没有做的

- **性能优化**：名字镜像（#102）、显示投影（只改名字行 #103、身份或链接变化 #104）、名字更新器与读侧的选名（#105）已改成增量或按键读取。名字更新器提交单项前只核对这个文字键相关的身份、链接及名称来源；hybrid 下每次决议仍整体重建身份图，不在 #38 的名字主线内。
- ~~拆脚手架阶段 C 的其余部分~~：都已完成。旧 Python 写路径随参考实现删除（#9、#186）；新图库直接以核心形态创建（`new_library`）、收编与对照命令删除、名字兼容表删除，都在核心第 8 版（#201）。

## 9. 显示名来源与后台更新

显示名由 Rust 名称命令更新正式名称及其投影，不改变实体身份、memberships 或 assertions；名称请求和配置保留在 `name_cache`、`name_settings`。显示名的读取走核心的显示投影（第 3 节）；创建实体的去重谓词继续使用原始 tag / 既有 aliases。

### API 与运行

- `GET /api/names` 返回 `state`、`counts`、`model_api`。状态为 `idle / running / done / error / off`，进度为 `done / total`；错误只含本地生成的说明或异常类型。
- `POST /api/names` 使用现有 token / Origin 校验。`refresh` 可传 `entity`；单实体刷新包含其父角色、同族皮肤及所属作品。后台忙时显式刷新返回 400，自动同步通知会排队补跑。
- `model_api` 操作可带 `key`（新 key；`null` 清除；省略则保留）、`model`（默认 `deepseek-chat`）、`base`（任何兼容 OpenAI 的接口地址，默认 `https://api.deepseek.com`；须为 https，本机服务可用 http）。保存仅修改设置；后续拉取才会启用翻译。`model_api_test` 使用固定小请求，返回 `{ok: true}` 或 `{ok: false, error}`。GET 只返回配置状态、末四位、模型和接口地址。旧版的 `deepseek_*` 设置会自动沿用。
- 启动及 Photos / 索引导入后自动补拉；`--no-names` 关闭后台拉取和刷新，已有显示名仍可使用。

角色、作品优先采用 Bangumi 的中文或日文；无结果时依次使用 index 中文、index 日文、wiki 中文、模型 API 中文、可读 tag。角色的 wiki 中文（`source=wiki`）仅在有旁证时按 Bangumi 档次参与显示选择。无旁证且无歧义时，它排在 Bangumi 和 index 的日文名之后。user 数据优先于自动来源；`names.en` 保留来源提供的英文名，英文显示回退仍按约定由 tag 生成。皮肤按中文 → 可读 variant 回退，未知复合词整体回退。同来源内中文优先于日文；皮肤内置译名先于模型 API。Danbooru 原始别名（source=danbooru）不参与显示选择，只有下面规则挑出的 wiki 中文可以。本地自建实体及标为 OC 的名字保持原值。`name_source=model` 是机器翻译标识（schema 22 起由旧的 `deepseek` 迁移而来）。

`characters / entities / detail / assets / similar / suggest / index` 的实体对象附原始 `tag`，原有 `work` 不变，另附 `work_name`。角色列表及详情的 `variants` 只显示皮肤部分；完整实体名使用父角色名和皮肤名。新操作及可从历史实体 ID 还原的撤销文案使用当前显示名，原始历史文本和撤销快照不被改写。

### 原生运行与恢复

原生宿主直接提供 GET/POST `/api/names`、entities/works 的远程查询和 local-links 建议。启动按当前包的 names 偏好补拉，`--no-names` 禁止后台与显式刷新，仍可读取已有显示名及模型设置。名称文本查询（角色/作品远程搜索、后台补拉中文名、作品版权解析）的授权有两种来源（2026-10-06，#1）：`--names-network` 仅为本次进程授予；图库的持久授权由用户在界面上同意（`POST /api/names {"action":"network","allow":true|false}`），存在 `library.sqlite` 的 `prefs` 表 `names_network` 行（`{"granted_at":…}`），与启动偏好分开，撤销即删除该行。授权在每次请求时实时读取，撤销后运行中的任务下一次请求即被拒绝。`GET /api/names` 返回 `network:{authorized,this_run,granted_at}`。未授权的非本机请求报「显示名联网尚未授权」（HTTP 502），前端据此显示授权入口，其他失败显示具体原因。保存 key 和修改启动偏好不授予网络。模型 HTTP 401/403 等以本地安全错误报告，不回显服务响应、凭据或请求头。模型设置的存储和回退见下文。

模型名和接口地址保留在 `name_settings`。模型 API key 使用 macOS 钥匙串或 Windows 当前用户 DPAPI，SQLite 只保存随机引用。凭据绑定当前机器上的图库路径和接口地址；复制、移动图库或改用其他接口后需要重新填写，除非保存新地址时同时提供 key。删除或替换设置只影响当前包里的凭据。读取设置只返回尾号，保存也不会授予联网权限。

现有可写图库首次打开时迁移 `model_key`，缺失时沿用旧 `deepseek_key`，`deepseek_model` 仍可作为模型名。写入本机存储后才提交引用并启用 `secure_delete` 删除两个旧密钥行。提交后尝试截断 WAL。本机已有可读取的凭据时保留它；条目缺失时迁移旧明文，存储读写失败时保留原行。设置卡会显示凭据异常，可以保存以重试或清除。系统拒绝删除时，清除操作报错并保留引用供重试。名称拉取在需要模型补译时才读取凭据。迁移范围是当前图库的有效密钥行，已有副本、备份和更早写入遗留的磁盘字节需分别处理。回退旧版时需要重新填写模型密钥，旧版仍会保存明文。

`backend_effects.kind=names:{"entity":...,"refresh":...}` 持久绑定事件所在包，entity=null 为全范围；单实体包含父角色、同族皮肤和作品。公共 Task 身份包含 canonical 包路径、generation、会话摘要及 task_id；切库先验证替代包，再取消并等待旧任务，退出同样等待。任务与同步选择器捕获原包路径及取消标记，迟到响应不能写入新包或返回新会话。底层 HTTP 每次 25 秒超时，取消期间等待在途 I/O 超时后收尾；等待节流/退避时每 20 ms 检查取消。

自动任务先查图库中有归属或候选的角色，把角色及其造型的照片合并去重后按张数从多到少排。处理每个角色前同时查它所属的作品。识别导入发布新批次时，新进入范围的角色在当前查询结束后排到最前。已授权且名称任务空闲时，这类批次也会启动名称任务。全进程按主机串行限制请求，沿用下面的最小起点间隔、重试、冷却和响应限制。原生请求不持有应用事务。成功的自动名称、搜索别名、正式投影和版本 1 完成键在同一事务提交。完成键的缓存值记录单项的来源指纹，新增角色无需作废已完成的条目。0.3.0 的时间戳完成值在整轮来源仍一致时继续使用，来源变化时按当前来源重算。手动全范围刷新在开始时清掉完成键，刷新中断后重用已写下的键。提交前重查查询来源，变更时保留可重试事件。完成事件另有确认边界，确认失败及进程中断后幂等重放。失败事件保留 pending 与安全错误，重启或手动刷新可重试。周期重试从失败的那一轮结束时算起，等 1 分钟，连续失败每次加倍，最长 30 分钟，成功后复原。新的事件继续处理。数据库错误报「名称数据库操作失败：」加原始错误，日志另记 SQLite 原始错误；名称获取只写 library.sqlite，读 source.sqlite 失败另记一行。每个条目在应用库写事务之前读源库的索引名并与查询前比对，写事务内只核对应用库来源，不在写锁内等源库。索引名的初次写入也在应用库事务内复核身份、查询 tag、作品和 CCIP 键，按复核过的身份写入。只因未授权而没查成的事件同样保留 pending，但任务状态记为 `unauthorized` 而不是 `error`（同一轮有其他失败时仍报那个失败），任务页显示「未允许联网查询」和授权入口所在位置，不显示为获取失败；处于该状态且仍未授权时不做 60 秒周期重试，只处理新事件，授权后立即重试（2026-10-16，#166）。本机内置名、索引名和已有缓存在未授权时照常写入。后续同步/索引导入方可同事务入队相同 names 请求，不直接确认名称完成。

`ILLUSTASH_COPYRIGHT_BASE` 与 `ILLUSTASH_BANGUMI_BASE` 只允许显式端口的本机 HTTP 合成服务覆盖默认地址。生产查询沿用 HTTPS，缓存键保留旧 urlencode/JSON 格式；本地版权快捷路径在外查前运行。

### 拉取边界

只处理有 memberships / candidates 的外部实体、其父角色、作品及皮肤词；全库刷新不会抓整个外部索引。图库搜索匹配所有来源名字、可读 tag、既有 aliases 和作品名，作品搜索返回同作品角色。

角色自动名仍按当前主标签共享显示。联网任务按链接系统优先级选择活跃顶层角色写入，同一个标签的查询作品须与所有实际写入目标一致。作品不一致时跳过该项，并保留后续取名机会。联网期间出现这类歧义时，待提交的结果按来源变化丢弃。`names-command` 按文字键仍可修改合并成员用来借名的记录。

Danbooru 仅请求 wiki `title,other_names`。全部别名作为搜索数据存入本地。挑角色的中文显示候选时，统一空白、全半角和间隔号，去掉末尾的作品限定词，排除假名、谚文、作品名及常见合称。来自同一作品两个及以上角色的共用别名留给搜索，另有可靠来源印证的本名仍可使用。索引、Bangumi 或模型 API 提供的中文同名可以旁证生僻字；只有日文同形时，写法须符合现有简中文字形检查。已知中文原作中通过筛选的候选也可提档。无旁证时按 wiki 顺序取首个可用中文别名作为低优先级候选。悬停提示沿用「来自 Danbooru 别名」。共用别名用各角色已经取回的缓存判定，同作品有新别名入库时同步复核。角色查询优先用 index 的 name_ja，其次按 wiki 原顺序选含假名或汉字的词，排除括号和超过 40 个规范化字符的词，最多三个不同关键词。

角色目录（[TERMS_CHARACTERS.md](TERMS_CHARACTERS.md) 第 9 节）给了这个 tag 的 Bangumi 角色 ID 时，先按 ID 调 Bangumi `characters/{id}`，不按名字搜（#837）。中文名按目录的取名提示挑：
- 默认用信息框的「简体中文名」，原名按文字记成日文或英文；
- `name` 取原名当中文名（国产作品，「简体中文名」这时只记成搜索别名）；
- `p0`、`p1` 取「A／B」写法的第几段（一个条目写了平时名和变身名的角色），原名取同一段。

来源照旧是 `bangumi`。目录未收录或条目缺失时，退回下面的按名字搜索。有效条目缺中文名时，保留已取得的原名，按名字搜索只允许同一 Bangumi ID 补中文。Bangumi ID 查询超时、限流或其他网络失败时保留待重试。角色 ID、取名提示和规范标签纳入对应条目的完成标记，已有图库升级后自动重新查询使用这些目录信息的角色。只有旧标签的身份按规范标签查 Wiki，以规范页的别名和可读标签辅助 Bangumi 搜索。仍在原身份上保存新结果，旧标签、已有显示名的搜索入口及旧别名也可使用。名称任务首次运行按缓存复核旧 wiki 自动名，保留人工名和搜索别名。

角色独立调用 Bangumi `search/characters?limit=10`，结果原名须与任意 index/wiki 候选名规范化后完全相等；优先采用与本次查询词精确相等的结果，防止 wiki 里的宽泛别名拉进无关角色。单一命中直接采用搜索结果的 name 和 infobox，不再请求角色详情。中文原作的 Bangumi name 存为 zh，infobox 简体中文名仅作为搜索别名；已知日本系列（例如东方、Fate、VOCALOID、Persona 3）的全汉字角色名仍存为 ja。中文原作的判断使用作品原作语言、同名 subject 和 wiki 精确别名证据；纯韩文原名不写入 zh/ja，纯拉丁字母原名写入 en。同名多结果查询各自关联 subjects，用作品候选名精确比对 name/name_cn；无法由作品区分时，只有全部候选的中文显示名完全一致才采用。作品查询失败不阻止角色查询。

作品有目录 ID 时先取 Bangumi `subjects/{id}`，已有图库自动重取对应作品的自动名。详情缺失时按名字搜索；有原名、缺中文时只接受同 ID 的中文补充。详情临时失败且同 ID 搜索也没有有效结果时保留待重试。按名字查询同时匹配 name 和 name_cn，因此中文候选名可匹配带副标题的原名。多个命中若 name_cn 一致直接采用；否则只保留 name_cn 命中候选名的结果，再有歧义则放弃。规范化处理全半角、空格、中点及 Danbooru 的下划线空格编码。

未匹配 Bangumi ID 的人物，可离线显示词表中经独立核对的 Wikidata 简中标签（`source=builtin`），包括只有旧 tag 的图库。词表不发起名称网络请求。已有用户命名、Bangumi 或索引中文名继续优先。

系列级版权 tag 使用确定的内置中文名（source=builtin，用户自定义名仍优先）；Bangumi 为这些 tag 找到的单一动画或游戏条目不覆盖系列名。搜索先用 SQLite 的实体、别名和名字表生成匹配实体集与精确/前缀/包含排序，再查角色和图片，不在每一张图片或每一个角色上逐个调用搜索函数。

请求和负结果永久缓存在本地。重新拉取绕过选定范围缓存；网络失败不写负缓存，下次再试。每个主机最多两个并发文本请求；Danbooru 请求起点至少间隔 0.35 秒，Bangumi 至少间隔 0.6 秒，模型 API 及其他主机至少间隔 1 秒。作品全部处理后才用作品语言判定角色。有目录 ID 的内置系列可保存条目提供的名称，供原文和搜索使用；其余内置系列跳过不必要的 Bangumi 作品搜索；重复查询键在同一次运行及本地缓存中去重。25 秒超时，单响应最大 2 MB。超时、连接错误、HTTP 429/5xx 最多尝试三次，指数退避附抖动并遵守 Retry-After；每个主机连续失败 20 次后暂停 180 秒。失败不写负缓存，状态保留安全的错误类型；重启继续补拉。不设流量预算（用户 2026-10-21 决定，#287：名字请求每次只有几 KB，原先终身累计的 500 MB 计数没有批准入口，用完就再也取不到名字）；旧图库里残留的 `name_settings.traffic_reserved` 不再读取。网络期间不持有数据库事务。模型 API 同样只接收 tag、日文名、作品名，无图片、图片路径或特征。

Key 存在本地 SQLite，随用户现有数据库备份机制进入本地备份；清除操作删除当前库的设置，不修改历史备份。请求错误不回显远端响应体、请求头或 key。

参考规范：[Bangumi API](https://github.com/bangumi/api/blob/master/open-api/v0.yaml)、[Bangumi User-Agent](https://github.com/bangumi/api/blob/master/docs-raw/user%20agent.md)、[Danbooru WikiPage](https://github.com/danbooru/danbooru/blob/master/app/models/wiki_page.rb)。

2026-09-24 的新种子 60 角色、44 部作品实网抽样耗时 111 秒，261 个逻辑查询、268 次实际尝试；7 次代理隧道 HTTP 503 均在重试后成功，终态失败为零。按作品和角色两个阶段的实测吞吐，647 部作品加 3,354 个角色约需 76 分钟；这是抽样推算，没有跑全库。同样本回放旧版查询逻辑时，缺失的缓存由真实服务补足，共需 277 个唯一查询；优化后为 261 个（少 16 个内置系列的作品搜索）。旧版另一组 52 角色实网耗时 230 秒，不同样本的耗时只供粗略比较。

## 方案：名字投影与收编

以下是 2026-09-29 起草、同日经用户确认方案第 5 节并完成的方案里，别处没有、仍然有效的设计和用户决定（2026-11-14 从 `TERMS_NAMES_PLAN.md` 并入本文）。方案原来让旧名字表成为核心维护的兼容表，这部分已被词条核心第 8 版（#201，2026-10-20）取代。前置契约是角色那一步的决定 D4（[TERMS_CHARACTERS.md](TERMS_CHARACTERS.md) 方案第 14 节）。代码注释里的 N0–N6 指方案第 4 节的分段，方案第 6 节记各段定下来、现在仍然成立的设计；逐段的实施流水和测量在 git 历史里。

### 方案 1. 起点

收编之前，名字数据是三张表：按身份的 `names`（`entity_id, lang, value, source, role(display/alias), preferred, position`）、按文字键的 `display_names`（`(kind, key)` = 作品的 qualifier、皮肤的 variant）和别名 `name_aliases`。选名规则（`View` 的 `choose`、`entity`、`base`、`own_name` 与词表）注册成 SQL 函数 `entity_name()`、`entity_info()`、`work_name()`，嵌在 34 处查询里；搜索、批量显示和历史文案也直接用它。`name_cache`、`name_settings` 是名字更新器的联网缓存和设置，不算名字数据。

### 方案 2. 目标

1. **选名规则进核心**：核心按与 `View` 逐条相同的规则算出显示投影，在每个写事务里维护，显示读路径都读投影。
2. **名字主数据进核心**：名字更新器只把结果交给核心，不直接写表。
3. **历史文案由核心生成**，写入口不再先演一遍旧代码。

界面、HTTP API、选名结果、搜索结果、历史文案逐字不变（等价迁移）。

### 方案 3. 设计要点

- **显示投影表**（核心维护，物化）：按身份一行 `term_name_display`（完整显示名、皮肤短名 skin_only、名字来源、zh / ja / en、tag）；按文字键一行 `term_name_choice`（`(work, qualifier)`、`(skin, variant)` 等的 `choose` 结果，供 `work_name(tag)` 和皮肤名用）。
- **失效依赖**：身份的属性（kind、original、parent、variant、merged_into）、当前链接、名字行、文字键名字、词表。凡是改到这些的核心写事务，在同一事务里维护投影；判断依据是 `meta.names_version`（`entities`、`links`、`names`、`display_names` 上的触发器递增它）。最初是整表重建，之后改成按依赖闭包局部维护（#103、#104），见第 3 节。
- **词表**：`SKINS`、`WORK_NAMES`、`readable()`、`skin_zh()` 移植进核心（`names_view.rs`）。
- **写路径**：用户名字经角色核心命令；名字更新器的结果经名字命令交给核心（方案第 5 节 N2），按来源整批替换，成功的空结果让旧名字失效。

### 方案 4. 分段

| 段 | 内容 |
|---|---|
| N0 夹具 | 在旧代码上钉住选名行为：qualifier 作品读 `display_names`、词表例外、英文回退、合并代表借名、本地 / 原创只用自己的名字、皮肤名与「以前的人物变成的皮肤」、作品的用户名字、`put` / `put_name` / 替换的写入规则、历史文案的重新渲染、搜索 |
| N1 投影 | 核心按 `View` 规则建显示投影（`names_view.rs`，命令 `names-project` 整表重建），与旧 `View` 全量逐字对照 |
| N2 名字收编 | `sync-names`：名字三表收编进核心主数据（已随 #201 删除） |
| N3 核心写名字 | 名字更新器的交付、所有写名字的入口改走核心（`names-command`）；核心命令同事务维护投影 |
| N4 读路径与搜索 | 显示读路径改读投影；名字搜索进核心（`names_search.rs`，命令 `names-search`） |
| N5 历史文案进核心 | 历史文案由核心按投影生成（`character_labels.rs`）；写入口去掉「先演一遍」 |
| N6 真实副本验收 | 结果见第 6 节 |

### 方案 5. 用户确认的决定

2026-09-29 用户确认。

| # | 问题 | 决定 | 理由 |
|---|---|---|---|
| N1 | 搜索这一步是否进核心 | 原建议留在 Python。**用户决定：这一步就搬进核心** | 它只做字符串匹配，没有选名规则；用户希望一并离开 Python |
| N2 | 名字更新器怎么把结果交给核心 | **核心命令**（像角色命令一样，标准输入给 JSON：按来源整批替换、成功的空结果、display / alias / 首选 / 顺序），不走证据包文件 | 名字是显示数据，不参与归属判定；证据包的格式和按指纹去重是为机器证据设计的，名字用不上 |
| N3 | 名字在核心里怎么存 | **按原来的形状收进 `term_names`**（来源、语言、角色、首选、顺序），用户名字就是来源 `user`，不拆成证据和裁决 | 选名规则本来就按来源排优先级；拆成证据 / 裁决不改变任何结果，只增加复杂度 |
| N4 | 历史文案进核心、写入口去掉预演放在这一步吗 | **放在这一步的 N5 做** | 名字规则进了核心之后顺手就能做 |

### 方案 6. 各段的设计

#### N1 显示投影

`crates/core/src/names_view.rs` 逐条移植 `View` 和词表，结果写进 schema 第 5 版的 `term_name_display(entity, skin_only, …)`、`term_name_choice(kind, key, …)`。照搬的细节：

- `skin_zh` 按词长排序时，同长的词保持词表原顺序（稳定排序）；
- `readable` 的去 qualifier 正则、Python `str.title()` 的大小写规则；
- `min` 取第一个最小值；wiki 的中文按 bangumi 排，只对中文生效。

投影只收库里出现过的键；别的键按这个键自己的行现算（`names_view::choose_key`，第 3 节）。

#### N3 名字写入走核心

- **提交钩子**：所有核心写事务的统一提交点 `schema::commit_writing` 调用 `legacy_names::after_write`：`meta.names_version` 与上次投影时记下的 `names_projected` 不同，就在同一事务里维护显示投影。日常命令不碰名字表，只多两次 meta 查询。
- **`names-command`**（`crates/core/src/names_commands.rs`），标准输入一个 JSON `{"op": …}`，一个事务，报错文字与旧代码相同：
  - `replace`：按来源整批替换，空列表就是成功的空结果；
  - `danbooru_aliases`：人物按 tag 找身份写；作品写按文字键的别名，并同步到 qualifier 对应的作品身份；
  - `bangumi_alias`；
  - `seed_index`，以及种子用到的 `put`；
  - `alias`：相册标题用；
  - `batch`：多条放在一个事务里。

#### N4 读路径与搜索进核心

- 显示读路径读投影；投影外的键（任意 tag 的 `work_name`、词表里有但库里没有的作品）按键现算。
- **搜索**（`crates/core/src/names_search.rs`，命令 `names-search`，标准输入 `{"query": …}`，输出 `{"hits": {身份: 排名}}`）逐条移植旧 `search_hits`：
  - `LIKE` 预筛（SQL 语句逐字照搬，所以只对 ASCII 不分大小写这一点也相同），再按 Python `str.casefold()` 比较；
  - 排名：完全相同 2、开头 1、包含 0；人物 tag 另比可读形式；`WORK_NAMES`；作品树（`works.TREE` 的递归，`UNION` 防环）；`skin_zh`；「父 · 皮肤」组合；
  - `str.casefold()` 是逐码位的完全大小写折叠，Rust 的 `to_lowercase` 不同（ß、ﬁ、ſ…）：码位表 `casefold.rs` 由 `tools/gen_casefold.py` 生成（Python 3.12.12，Unicode 15.0）；
  - 照搬了一处旧代码的细节：作品的名字 / 链接命中那一段按行的顺序后者覆盖前者（`abc` 完全相同、`abcd` 只是包含 → 0 而不是 2）。
- 核心的子命令都要列在 `native/terms/src/cli.rs` 的 `COMMANDS` 里：桌面外壳兼任核心程序时只把表里的子命令当命令跑，其余当成开窗口。有测试钉住 `run` 认的每个子命令都在表里。

#### N5 历史文案进核心、拆掉预演

- **核心生成历史文案**（`crates/core/src/character_labels.rs`）：
  - `Shown::name` = SQL 函数 `entity_name(id)`，读 `term_name_display`；这个事务改过名字（`names_version` 变了）就先在事务里重建投影（`legacy_names::refresh_projection`）。**事务中途只维护投影**，其余留到提交时。
  - `works` = 按复制品组去重的张数；`lang_of`：打字的名字按假名 → ja、汉字 → zh、其余 → en 归语言。
  - 每条命令按旧代码逐字拼文案：`change` 六种、`answer` 三种、随机抽查、换人、检查完了、身份未定、锚点、改资料（用改完之后的名字）、关联到 Danbooru、策略未设置时的合并（用合并之后的名字：先占位、最后改）。`set_kind` 标成原创角色时固定下来的显示名，也由核心读。命令不收 `label`。
- **写入口不再先演一遍旧代码**：调用方只做它才看得到的检查（按筛选条件选图、联网查版权），其余检查由核心按旧顺序、用旧文案做（[TERMS_CHARACTERS.md](TERMS_CHARACTERS.md) 方案第 8 节）。新建作品的 id 在核心走到那一步时向调用方要（`Error::NeedId`，退出码 5）。
- 原来靠预演挡在前面、由核心补齐的几处：tag 已被别的角色关联时，报错里是那个角色的**名字**；名字、别名按 Python `str.strip()` 去空白（多去 `\x1c`–`\x1f`）；`set_work` / 资料的 `work` 按 Python 真值判断；`uncertain` 的选图拒绝；锚点的 post ID 用包含 `str.isdigit` 的 `char::is_numeric`（比如 `５` 也接受）。
