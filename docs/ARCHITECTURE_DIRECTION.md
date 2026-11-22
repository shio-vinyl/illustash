# 架构现状与已批准方向

状态：现行参考，2026-11-15。记录稳定应用现在的架构（Rust 应用宿主、Core、Platform、可选原生 worker）和用户批准、仍然有效的方向；词条抽象见 [TERMS.md](TERMS.md)，安装交付见 [BACKEND_TASKS.md](BACKEND_TASKS.md)，检查与合并门槛见 [LOCAL_CHECKS.md](LOCAL_CHECKS.md)。各次实施、测量和验收过程在对应 PR，不在本文。2026-10-06 开源之前的 Issue / PR 编号指向开源前的私有历史（见 [AGENTS.md](../AGENTS.md#公开库规则)）。

## 目标与边界

采用 **Rust 模块化单体 + 计算 worker + 现有前端**。计算 worker 在 macOS/Windows 全部原生（2026-10-05 起），Linux 已冻结（见 [README](../README.md)）。2026-09-28 用户定：新增功能模块用 Rust；研究 worker 的语言不限。目标是稳定应用核心能够独立运行，不以整个仓库的 Rust 代码占比作为指标。

稳定应用不依赖 Python：没有 Python 也能启动、浏览、管理图库、执行全部人工整理和撤销。仓库里的 Python 测试与工具不是运行依赖。可选 worker 缺失时，溯源、OCR、模型推理可以不可用，但不能阻断这些基础功能。平台专属能力仍需分别适配，Rust 不自动解决跨平台问题。

保留 SQLite、本地 HTTP 与现有前端。暂不引入微服务、消息中间件、全面 ORM 化或通用插件框架。改动尽量保持 API、数据语义和用户行为稳定；数据模型调整单独设计迁移及验收。

## Core / Platform Service / App 边界

Core 持有连接内规则、资产、证据、人工结论、历史/撤销和投影；平台差异不进入这些业务规则。Platform Service 提供原生文件选择、存储位置读取等平台能力，并明确 Supported / Unsupported。应用宿主管理图库实例、generation、任务取消/授权、持久事件恢复和正式发布；App（网页或桌面 UI）负责展示、用户选择及能力不足时的手填路径，不自行推断出处。服务返回选择结果不等于允许写库，宿主仍负责输入校验、会话隔离与提交。

来源的「存储位置」属于平台能力，分**本地文件夹**与**云相册**。本地文件夹使用路径；云相册通过统一的枚举资产、读取原图/元数据、报告变更与不可用状态的接口接入，iCloud 照片（经 macOS Apple Photos）是第一个适配器。存储位置与 Pixiv / X / 其他「出处类型」分离：文件夹出处由用户指定；云相册按每张图片元数据确定，无法确定时为其他。现有只读适配器为 `platform_folder` / `platform_photos`；`app_folder` / `app_photos` 掌管来源归属、资产身份和差异提交。出处类型由 `origin` 统一提供（#111），没有新增云厂商或改变来源数据库结构。

原生选择服务：`app_platform::choose(kind,multiple,stopping)` 接受 `folder`、`library`、`zip`、`archive`。桌面通过 `app_desktop` 共用 Pixiv 的 loopback 通道，请求 `POST /platform/choose?kind=folder&multiple=1`；启动时随机口令只给外壳和宿主，网页不接触口令。外壳在 bridge 工作线程调用 Tauri dialog 插件，文件夹支持多选；文件和图库保持单选。桥返回 `paths` 列表，不读选定目录、猜出处或记录私人路径。桥调用失败时报「桌面 app 没有响应或拒绝请求：」加失败的那一步：桌面 app 回复的错误原文（如选择窗口的失败）、连不上本机通道、等待超时或连接中断（2026-10-15，#147）；Pixiv 账号逻辑按这个前缀识别桥失败。没有桌面通道时，CLI 网页的 macOS 使用 `osascript`，其他平台明确 Unsupported、由用户手填，不悄悄回退到另一条通道。

`POST /api/libraries/choose` 默认沿用单选响应：`path`（取消为 null），文件夹成功时还有 `name` 和固定为 `other` 的中性 `preset`，保持既有表单字段兼容，绝不按目录名猜值。显式 `multiple:true` 仅适用于 `folder`，响应增加完整 `paths`（取消为空列表）；`path` 保留首项兼容，新增消费者应使用 `paths`。

弹窗等待期间不持图库会话锁；请求仍绑定接收时的 generation，并在返回后再验证，旧会话结果返回 403，不交给替换图库；归档导入另在取得写锁后复验 token/generation。停止宿主不会等待选择窗口满 600 秒；桌面按现有 stdin/子进程生命周期收尾。

能力矩阵记录已收拢的平台能力，选择窗口证据见 PR #120，凭据存储证据与未覆盖平台见 PR #132，文件系统、时间与系统信息的合成验收见 PR #133（Linux 未实测）。编译通过不代替原生窗口实测。

| 能力 | macOS | Windows | Linux |
|---|---|---|---|
| 桌面文件/文件夹原生选择 | Tauri；合成图库、文件夹与 ZIP 路径返回已实测 | Tauri；合成文件夹与 ZIP 路径返回已实测 | Tauri / GTK；合成文件夹与 ZIP 路径返回已实测 |
| 桌面文件夹多选 | 已实测两个合成目录返回列表 | 已实测两个合成目录返回列表；取消、切库旧结果拒收及关窗退出通过 | 已实测两个合成目录返回列表；取消、切库旧结果拒收及正常关窗退出通过 |
| CLI 网页原生选择 | 保留 osascript；显式多选解析已测试，CLI 窗口未验 | Unsupported，手填路径 | Unsupported，手填路径 |
| iCloud 照片 | 既有 Apple Photos 适配器；未验真实相册权限 | 待 #112 查证 | 无现行适配器 |
| 私有文件/目录 | 文件创建 0600；临时目录创建后设 0700 | 保留系统默认权限，不新增 ACL | 同 macOS 的 Unix 权限规则 |
| worker 响应读取 | O_NOFOLLOW 拒绝最终路径符号链接 | 保留普通只读打开；识别输入另有既有 metadata 检查 | 同 macOS |
| JPEG 缓存扩展名 | 大小写敏感 | ASCII 大小写不敏感 | 大小写敏感 |
| 本地日期转换/系统描述 | libc mktime/localtime_r；uname | UCRT _mktime64/localtime_s；OS/架构 | 同 macOS 的 Unix API |
| 账号凭据 | Illustash 钥匙串；`ILLUSTASH_NO_KEYCHAIN=1` 使用原 0600 文件 | 当前用户 DPAPI，按账号类型绑定用途，独立 `.dpapi` 密文 | 原 accounts 文件，Unix 权限 0600 |

## 共享 Core 与宿主的中性能力边界

兼容文本位于 `compat_text`：共用 Python 引号、转义、Unicode 可打印规则及扩展空白裁剪；错误和桌面建库不依赖 HTTP 工具。Unicode 15 表保留历史文件名 `http_unicode.rs`，以中性 `compat_unicode` 模块加载。HTTP 与欢迎页共用 `app_create::creation_error`，保留创建成功后登记、登记失败保留包和原错误转换。

带路径的文件操作错误由 `error::filesystem_message` 显示为「文件操作失败」，保留操作系统原始原因、错误号和源／目标路径；错误类型、HTTP 状态及校验顺序不变。SQLite 整数绑定越界显示「整数超出数据库支持的范围」。前端按完整模板翻译这些消息，系统原因、数字和路径作为数据保留。新产生的同步错误沿用此格式和原有 300 字符上限；`sync_runs.message` 的历史文本不迁移、不重写（2026-11-15，#252）。

`app_curation` 和 `app_identity::skip` 的新动作载荷使用结构化紧凑 JSON；skip 保存的旧 `identity_skipped` 偏好仍是字符串，撤销按原文恢复。既有 `actions.before_json` 不重写，其他核心动作载荷不变。学习发布戳包含动作原文，因此新编码参与的戳会与旧编码不同（2026-11-15，#252）。机器溯源的分组和本地匹配仅对新写入的 `image_artists.evidence` 使用结构化紧凑 JSON，交付前仍先解析再规范化计算 v4 版本；字段值和数组顺序相同的两种编码交付同一份核心证据。候选画师理由只在证据 `count` 为正整数时显示张数，其余取「有线索指向 TA」；此规则同时适用于核心和旧证据读取，不修改证据或裁决。

扫描取消由宿主 `app_sync::check` 提供，保留 atomic cancel 或宿主进程停止状态的原谓词；文件夹、Photos 及只读函数仅调用传入的检查。文件夹仍先开启源库即时事务再盘点/哈希、发布 pending journal 后提交；Photos 仍先只读枚举显式来源再开启源库事务，并在原逐行位置探测路径。原检查点、频率、来源碰撞拒绝和应用库后续刷新边界不变。原生选择由宿主传停止谓词，保留 100ms 轮询、600s 超时及 kill/wait 收尾。

`crates/core` 的 `illustash-core` 通过 path dependency 被 `native/terms` 的 `illustash-terms` 使用。Core 承载连接内的词条 schema/迁移、证据包解析与验证、人工裁决、角色/身份关系、历史/统一撤销、名字搜索与投影，以及 Error、Origin、严格 JSON、Unicode 与生成字符表。SQL 资源跟随规则归 Core；宿主以窄转导出保持旧 Rust 模块路径，折叠 SQL 通过连接内函数共用，不复制算法或 SQL。Core 可独立 fmt/clippy/test/locked build，进程内客户端不需要 HTTP/CLI/Tauri、平台 crate 或参考宿主；应用协调与 HTTP/CLI 留宿主；已有来源读取、路径/VFS、文件权限、进程/凭据/系统集成由 `crates/platform` / `illustash-platform` 提供。包名、bin 名、构建入口和 JSON/SQLite 格式不变，无根 workspace。

`backup::RequiredBackup` 是 Core 的具体必需能力，字段私有：Platform 提供私有文件预留、VFS 打开和自身失败文件清理，Core 执行既有 SQLite Backup step100、DELETE journal 与 quick_check，保留已提交 WAL 和隐式 rowid。文件库不能以空适配或内存连接确认备份成功；只有 SQLite main filename 为空的内存库保留免备份语义。现有宿主公共 migrate/cutover 固定调用 `platform_backup::capability()`；Core 独立 API 必须显式提供该能力。宿主已持有 writer transaction 的升级以独立 reader 完成备份并获得字段私有的 `PreparedUpgrade`，规则核对原路径/版本后在原事务迁移；目标版本在任何写入前校验，没有公开的不带备份的 migrate_in。ATTACH URI 原编码规则仍在 Core，宿主给 ATTACH 提供 Windows 规范化操作路径，同时保留原请求路径作校验与错误文本；只读和事务顺序不变。测试生成旧版本也走带能力入口，合成夹具只清其自身 setup 备份后测实际升级。

`illustash-platform` 独立提供实际被宿主调用的进程/Windows Job、文件锁、目录与路径、SQLite 长路径/VFS、凭据/DPAPI、私有权限与不跟随链接、时间与系统信息，以及文件夹和 Photos 只读适配。Platform 仅引用 Core 的 Error/Result 与 RequiredBackup（测试复用合成标识生成），不依赖宿主、HTTP、CLI 或 Tauri；Core 不依赖 Platform。宿主保留原模块路径的窄别名，desktop 直接调用 Platform 进程能力；Windows Job 与宿主任务组合回归仍在宿主，原身份句柄/一次内核等待及结果断言保留。

Platform picker 以 Selected/Cancelled/Unsupported 表达原生回退结果，宿主保留 600 秒政策、JSON、loopback、token/generation 和旧结果拒收；Tauri dialog 与窗口仍在 desktop。`capabilities::photos()` 统一 desktop_shell 和 HTTP 的既有 macOS 可用性判断，Windows/Linux 仍保留原 Unsupported 文案，iOS/Android 未支持。普通文件夹读取可用于三种桌面平台；Photos 仅 macOS 具有现行产品入口，跨平台合成 SQLite 读取不代表其他系统具有 Photos 权限或集成。读取未改为全量缓存后提交；来源事务、差异/删除恢复、冲突/身份与正式发布仍由宿主业务层掌管。

## 应用服务与客户端入口

实际依赖为 `desktop → illustash-terms / illustash-platform`，`illustash-terms → illustash-core / illustash-platform`，`illustash-platform → illustash-core`；Core 不引用 Platform、宿主、HTTP/CLI 或 Tauri。`tools/check_architecture.py` 检查四份显式 manifest 的普通、build、dev、target 条件依赖和重命名包，禁止反向依赖及 Core/Platform 传输或 UI 依赖；各 crate 保持独立 manifest 与 path dependency，没有根 workspace。

`native/terms` 继续是第三个后端 crate（应用宿主）。`app_service::Application` 私有持有 Session、实例、generation、任务和导出：显式 `open(path, Options)` 只校验/登记，`start` 初始化既有任务和恢复，`recover` 在该实例锁内恢复持久事件，`switch(received_generation, path)` 在写锁内复验后按原顺序打开、登记、停止旧任务、替换并发布新 generation，`shutdown` 停止并回收任务/导出；Drop 复用同一收尾。关闭后新的服务调用拒绝，重复关闭可安全调用。`Options::startup` 的 `SessionStartup` 供首次开库与切库共用，分别设置功能启用、宿主联网权限和后台自动执行。任务请求中的单次联网授权由对应任务处理，偏好不会自行授予网络权限。进程内客户端须在接受工作时取 generation，再通过 `with_library` 的锁内复验执行 Core 操作，回调取得的 Library 经打开时固定的物理实例访问包内文件，显示别名重指不能改写另一包；任务结果还须沿用完整 TaskIdentity、持久幂等与原所有者的正式提交校验。库 ID 相同不能代替实例路径、generation 或会话身份。

HTTP 前门仅装配服务与 socket，保留 Host/Origin/token 检查、请求体解析、路由、状态码、响应序列化及等待后安全复验，原 403 和成功 JSON 字节契约不变。会话转 HTTP LibraryContext 留在传输适配；慢 body 不持图库锁，释放锁执行的旧库快照不能重新绑定替换实例。连接有上界（2026-10-12，#25）：tiny_http 0.12 本身没有每连接超时，且会在自己不限量的线程池里先读完不超过 1024 字节的请求体，所以宿主用 `native/terms/vendor/tiny_http` 的补丁副本（`[patch.crates-io]`，`native/terms` 与 `desktop` 两个构建根都要带）。单次读或写等待超过 30 秒即关闭连接（尚未开始请求时回 408）；同时最多 64 个连接，超出的连接立即得到 503 并关闭；应用层同时最多 128 个请求处理线程，超出的请求回 503「服务繁忙」。补丁只新增 `Server::from_listener_with_limits`，默认的 `from_listener` 行为与上游相同（外壳登录桥仍用它）。`ILLUSTASH_HTTP_TIMEOUT_MS` 仅供测试缩短超时。回退：去掉两处 `[patch.crates-io]` 并把 `serve` 改回 `from_listener`。CLI 保留原命令/bin，连接内命令调用 Core；原角色命令事务 helper 移到应用服务，CLI、工厂、迁移与常驻 JSON 通道消费同一函数。`app_service::create` 与欢迎页、HTTP 多来源建库共用唯一 `app_create` 工厂及 optional reference 规则；登记使用工厂返回的实际绝对路径，相对目标不受之后 cwd 变化影响；发布后登记失败仍保留完整包，建库不自动打开、扫描或授权。登记失败时 HTTP 建库返回 200 和 `{path, name, listed: false, error}`，欢迎页的建库命令返回 `{created, error}`（2026-10-12，#97），界面提供「打开这个图库」（打开时再登记一次）和「仍然新建一个」，不再让重试另建带编号的包。打开图库（启动或切换）时登记失败同样不挡住图库（2026-10-18，#176）：照常打开，stderr 记一行，`/api/libraries` 只在这时多带 `unlisted`（原因；升级场景写明「升级已提交」和升级前备份的位置），页面启动后提示一次；之前是服务直接启动失败。建库、导入、打开三处的登记失败原因都写成一句「最近图库记录（libraries.json 路径）写不进去：原因」，没有写入权限时说明可能是只读，不带「读取失败」「不合法」或 Python 风格的 errno（2026-10-18，#179）。来源库、应用库、事件与文件发布各自的提交边界不合并。

未来进程内客户端可直接链接 Core 的连接内 API，或使用宿主服务复用现有图库/任务生命周期；不要求启动 HTTP、Tauri 或 Python。desktop 继续只消费 cli/desktop_shell 与 Platform 进程接口，Tauri dialog、窗口和 bridge 留在外壳；不增加 UI。现行能力矩阵只表示已实现及各次实际验收范围，iOS/Android、分享、通知和移动后台没有适配或目标构建证据；抽库及 Unix cfg 不表示移动支持。

检查分级与合并门槛见 [AGENTS.md](../AGENTS.md#约定) 和 [LOCAL_CHECKS.md](LOCAL_CHECKS.md)。合成客户端、构建和历史平台证据不能替代未执行的平台、GUI 或真实图库验收，限定例外须逐项由用户决定。

## 文件系统、时间与系统信息边界

`crates/platform/src/platform_filesystem.rs` 提供私有文件创建选项、既有文件权限恢复、私有临时目录创建、只读不跟随符号链接选项和扩展名比较。业务模块保留文件路径、独占创建/覆盖/截断、大小限制、清理及错误处理。Unix 文件创建模式为 0600，凭据写入对已存在文件也恢复 0600；同步临时目录仍先 create_dir，再设置 0700。Windows 不新增 ACL 或符号链接拒绝能力；O_NOFOLLOW 只影响 Unix 最终路径组件，不保证父目录无符号链接。JPEG 缓存清理仍比较最后四个编码字节，Windows 忽略 ASCII 大小写。

`platform_time.rs` 统一宿主本地时间转换：浏览日期下一日按中午 12 点和 tm_isdst=-1 交给 libc 规范化，Windows 使用 _mktime64，返回 -1 仍由业务转为原来的 mktime 错误；角色统计仍使用当前时间减固定秒数后调用 localtime_r/localtime_s，保留失败时零初始化结果。`platform_system.rs` 提供 about 的 Unix uname 描述及其他平台 OS/架构回退。不新增日期库、改变时区或扩大平台支持范围。

## 应用生命周期

普通打开只接受已初始化的 `.pilib` 包：Rust 持有图库实例、会话和切库状态。包格式、应用 schema、词条 schema 与 core 标记分别校验；需要建库或升级时拒绝启动，不通过读取入口隐式修改数据库。

`POST /api/libraries/new` 的 `name` 缺省、为 null 或裁剪后为空时使用「新图库」；其他值必须是字符串，布尔、数字、数组和对象在创建目录前返回 400「图库名称必须是字符串」。名称仍按既有扩展空白规则裁剪，含 NUL 时返回「无法创建图库：名称不能包含空字符（NUL）」（2026-11-15，#252）。

入口为 `./start-web.command --library /绝对路径/已有图库.pilib` 或 `illustash-terms http --library ...`，无需 `--native`。省略 `--library` 时重新打开最近登记的有效包；无记录则明确提示指定包，普通启动不创建图库或隐式升级。`--native` 保留为兼容别名。启动参数见 `http --help`；任务和网络授权参数沿用既有协议，`--no-open` 继续可用。旧宿主的 `--python`、`--upstream`、`--route`、`--refresh`、`--source`、`--db`、`--cache`、`--identity`、`--no-similar` 与参数分隔透传 `--` 明确报退役错误，不打开图库、不连接上游、不启动 Python。图库读、写、设置、备份和管理都在原生宿主中完成。

当前请求在应用接收时绑定 generation；旧 token 的 POST、读取 body 期间切库的写请求及等待锁后已失效的请求不能写入替换图库。读取从请求选定的旧库完成后释放会话锁，网络慢读不阻断切库；旧只读快照可以完成送达，不重新绑定新库。每次切换分别验证目标包、生成新令牌、成功更新注册表后才激活；正式写连接为请求局部；溯源状态轮询保留只读连接，以 SQLite data_version 感知外部提交。同 ID 的不同包实例使用独立会话。同一物理图库同时只属于一个宿主：打开或切入时在包内 `.illustash-owner.lock` 取得操作系统文件锁，并在该库的任何恢复逻辑运行前取得；另一宿主（含经别名）打开时明确拒绝「已在另一个 Illustash 中打开」，不会把正在运行的任务当作崩溃残留改成 interrupted。重开同一物理库沿用原锁，会话关闭（切库、shutdown）即释放，进程崩溃由系统释放，没有需要手工清理的永久锁。最近图库注册表的读改写在 `libraries.lock` 下串行，多宿主不会互相覆盖（2026-10-11，#24）。网页只认启动时取得的令牌：之后 bootstrap 返回不同令牌，或请求被拒为「会话已过期」，说明图库已在其他窗口或调用方切换、重开，页面不接受新令牌，丢弃该批及之后到达的响应，前端拒绝全部读写，常驻提示重新载入；未保存的输入不提交。本页自己切库在成功后同样停止读写，直到整页重载（2026-10-11，#33）。进入单个分组页时读取的分组数据在路由已改变后丢弃；同步状态轮询只采用最新一次请求的回答，较早的回答迟到也不会让状态倒退；新建图库表单的名称与勾选在输入时即存为草稿，设置卡片在输入框获得焦点时不重建（2026-10-12，#34）。原生入口已有名称、识别、同步和溯源协调；溯源任务/授权见 [PROVENANCE.md](PROVENANCE.md) 第 8 节。持久 startup 偏好与本次 `forced_off` 分离，保存偏好不启动任务、不授权联网。原生 POST 当前接收 UTF-8 常规 JSON，复用有序大整数/非有限数字解析；孤立 surrogate 明确拒绝。

普通校验、设置与图库列表不创建应用 schema 或缓存目录。SQLite 的正常只读 WAL 连接可能创建空 `-wal` 和 `-shm` 协调文件；这些文件由 SQLite 管理，不用 immutable 绕过并发锁，也不在失败时擅自删除其他连接可能使用的文件。正式数据库、非空 WAL、schema 和人工记录的失败不变性分别验收，SQLite 锁区字节不作为正式数据。[SQLite WAL 文档](https://sqlite.org/wal.html)

备份是显式操作，只备份应用库 `library.sqlite`，使用 SQLite backup API 保留已提交 WAL 与隐式 rowid；随机文件名以独占创建避免覆盖，校验失败仅删除本次文件。不宣称源库和应用库跨库原子性。设置写入在一个即时事务内合并和提交；提交成功后 HTTP 统计或网络传输失败不能证明写入未发生，不能通过补偿写覆盖其他进程的后续改动。

已有库升级使用显式 `--upgrade`（见「已有核心库的显式升级边界」），新建见「显式 fresh 建库边界」。可选 worker 的取消、恢复和进程树清理由原生协调器负责。

## 进程平台边界

进程生命周期由 `crates/platform/src/platform_process.rs` 及其 Windows 子模块提供，业务任务负责取消、超时和结果状态，平台模块负责启动、观察和收尾。Unix worker 使用独立进程组并在所有退出路径杀掉剩余后代；Windows worker 先以挂起状态启动，分配私有 kill-on-close Job 后恢复线程，收尾等待 Job 活跃进程归零并回收领头进程。领头先退出不免除后代清理；其他平台保留拒绝启动 worker 的错误。

Windows Job 的终止请求与后代进程对象信号完成有异步间隔，不能把零超时等待未信号直接等同于后代仍执行。用户 2026-10-03 决定保持生产收尾及超时语义；生命周期验收夹具在领头退出前，把原后代进程句柄复制到观察进程，保留 PID 和创建时间身份，并在现有 5 秒观察界限内作一次内核等待，再断言原身份、退出信号、Job 终止退出码及任务结果。此观察不依赖按 PID 重开，不用增加 sleep 或重试次数。

桌面外壳复用该模块，保留既有管道和 10 秒停止宽限。Unix 强制收尾按 PID/PPID 枚举所属后代再终止进程组；Windows 保留启动后分配私有 Job 的方式，外壳异常退出时由内核关闭 Job。停止信号保持 Unix SIGINT/SIGTERM 与 Windows 控制台处理器；父进程变化检测仅用于 Unix，Windows 仍依靠桌面 Job 和 stdin EOF。可执行文件的 Windows `.exe` 后缀和 Unix worker `exec` 也在此边界提供。

## 生产入口与测试参考

读取对照的合成夹具遵守宿主文件名规则：POSIX 保留含 `?` 的包名，Windows 使用 `%` 并继续覆盖空格、Unicode 和 `#`。Windows 账号读取样本通过本机原生保存接口预种 DPAPI，旧 Python 参考录制时使用独立明文种子及相同元数据。测试读宿主没有原生退出入口或 Windows 进程组，仅其测试收尾在 Windows terminate 后等待；原生正常退出和进程树清理的断言不改变。

CLI 备份拒绝的跨平台合成夹具在回收临时目录前显式提交并关闭各 SQLite 连接，保留 schema、typed rows、rowid 和阻塞文件断言；Windows 句柄释放与迁移/切换拒绝分别核验。

Python 合成测试在导入 `tests` 时为 `urllib.request.OpenerDirector.open` 安装守卫，只允许 HTTP/HTTPS 的 `localhost` 和数值 loopback 地址；外部服务必须 mock，跳转到外网也会被拦截。`python -m tests.run_offline -v -b` 跑完整发现套件并汇总外网尝试，测试代码即使捕获了拦截异常也不能使验收通过。名字写入夹具 mock `NameUpdater.wiki` 的空结果，保留别名解析器的合成答案和现有 golden。守卫只覆盖 Python urllib；原生进程与其他网络库仍由各自合成服务夹具约束。

`native/terms/src/http.rs` 只保留原生入口、请求上下文、JSON/安全响应；停止信号委托进程平台模块，`app_http` 持有应用会话；不再有 Python state/readiness、旧路由转发或 Python 宿主启动。资源沿用 `--root` / `ILLUSTASH_ROOT` 下 `web/static` 和 `tags/tags_v3.csv`，参考库来自 `ILLUSTASH_SHARED/reference.sqlite`。stdout 的 `图库已启动：http://127.0.0.1:PORT` 与 `/api/bootstrap` 是桌面复用的启动信号；账号桌面登录桥与显式配置的可选研究 worker 保留。

2026-10-08（#9 第 4 阶段）起，历史 Python 应用（HTTP、Store 编排、识别导入、Rust 命令桥、溯源 Runner 与联网来源，原在 `tests/reference/`）和根目录 `web/*.py`、`provenance/` 下的旧 Python 研究、索引、溯源计算脚本均已删除，原样副本归档在私有库；`web/` 只剩静态界面 `web/static`。测试不再导入旧 Python 代码，只读取录下的固定期望（见下两段）。读路由语料测试使用只服务核心读路由的测试读宿主（`native/terms/src/read_harness.rs`，`cargo build --manifest-path native/terms/Cargo.toml --features test-read-host --example read-http`），不转发、不启动 Python，不属于交付 CLI；普通构建不启用该 feature。`tools/` 下旧 Python 索引脚本及其自测已按用户决定删除（#53），现行脚本职责见 [开发说明](DEVELOPMENT.md#tools-目录)。安装研究包的资源见 [安装交付](BACKEND_TASKS.md#独立安装与可选计算包)。

固定契约：`native/terms/tests/contracts.rs` 重放 `native/terms/tests/fixtures/baseline/contracts/`（2026-10-19 起为 #201 的对照基准，由 main 在四个场景上录下），接口回复按字节、除退役旧表以外的全部表按行比较，规则见 `native/terms/tests/common/baseline.rs` 开头。测试只恢复输入、启动原生宿主、重放 HTTP，并核对两库全部表的列名、typed cells、rowid（包括历史、撤销载荷、事件与任务行），不录制期望值；输入 SQL 显式保留 rowid 间隙。四个场景覆盖基础 API 读取与拒绝、旧库升级/备份/旧历史撤销、标签写入和撤销、已有 running 同步记录转为 interrupted 后完成新运行并再次运行。成功 JSON 的全部键和值按列出的归一化规则比较，检查四个响应头；错误正文只比较状态码，同时检查数据库不变量。

时间仅按夹具列出的表/列及 JSON 时钟键解析后映射，资产日期、输入源运行日期和其余字符串不泛化；新用户概念 UUID 按固定名称、kind、origin、source 绑定，检查唯一性与 UUID 格式；session token 检查格式并映射，包内路径映射至 `@ROOT@`。输入中的既有 UUID 保持原值，整数 ID、NULL、历史顺序、载荷与引用不放宽。`tools/test_native_contracts.sh /absolute/contracts-executable` 运行已编译测试：runner 空 PATH/独立 HOME，原生子进程清空继承环境，仅配置临时 HOME、support、shared 和失败的 Python 名称哨兵；Windows 上另外只透传 Winsock 初始化所需的 `SystemRoot`，Python 名称哨兵仅 unix 有（Windows 的 `Command` 只按 `.exe` 解析且会再查系统目录，脚本哨兵拦不住），临时根去掉 `\\?\` 前缀并按 JSON 转义写入 `library.json`（#61）。该隔离运行不调用 Python/Cargo，不等于在本机卸载所有解释器。`.github/workflows/ci.yml` 的 contracts job 从本次 Cargo compiler-artifact 提取可执行文件，不猜测缓存路径；Actions 触发与本地合并门槛见 [AGENTS.md](../AGENTS.md#约定)。固定性能口径与阈值统一见 [TERMS_PERFORMANCE.md 第 5 节](TERMS_PERFORMANCE.md#5-固定原生宿主基准)。契约输入与期望不得用被测原生输出刷新。

原生黑盒测试与旧 Python 参考的比较都已改为固定期望（#9）。第 3 阶段在参考实现仍存在时按调用顺序录下参考响应、参考库状态（每表一个摘要，含 schema、存储类型、值、rowid 和视图行）、参考建出的包（数据库按一致性字节，文本按占位符）和其余参考计算值，存为 `tests/fixtures/reference_recordings/<模块>.tar.xz`，由 `tests/fixed_expectations.py` 只读回放；第 4 阶段把借参考造数据的测试改为原生命令或提交的夹具，旧算法给出的比较值固化在 `tests/fixtures/*_values.json.gz`／`.json.xz`（图像、索引、研究、溯源等，像素数组按 SHA-256 比较），显式生成图索引专项的旧算法期望另存于 `tests/fixtures/platform_index_values.json.xz`（八张生成图的旧算法期望来自删除前标签 `python-index-tools-before-issue-53`，Windows 差异值保存在同名 `windows/` 覆盖文件），同样只读；标签、画师、核心调用的录制重放原在 `native/terms/tests/fixtures/replay/`，2026-10-19 起由 #201 的新对照基准 `native/terms/tests/fixtures/baseline/` 接替；上述整库摘要在核心 v8 下改为按同一套规则与 `tests/fixtures/baseline_states/` 比较（`fixed_expectations.py` 末尾）。临时根、回环 origin、会话令牌等一次性字符串以占位符保存（数据库文本里的临时根也是，录制副本重建后不留旧内容），恢复时换成本次的；夹具不含录制机器的路径。参考与原生在不同时间运行，凡形如日期时间（到分或秒）的文本按 `<clock>` 比较；没有日期时间的字段、计数、rowid、键和字节仍逐一比较。原生侧需要可复现的键时设置仅测试用的 `ILLUSTASH_TEST_KEY_COUNTER`（计数在锁文件下递增）；按今天算的窗口（角色接口的「近 30 天」和「近 12 个月」）录的是录制当天的结果，读路由的测试宿主和生命周期测试设置仅测试用的 `ILLUSTASH_TEST_TODAY=2026-10-07`，把今天固定为录制日的当地正午（#301），产品不设置它，正式运行照常按当前日期；测试图片与 ICC 配置文件用提交的固定字节，并按内容设定修改时间。参考已删除，这些期望不能重录，被测原生输出也不得用来刷新它们；某项期望若需改变，只能按产品决定单独修改并在 PR 说明。2026-10-11 用户决定（#40）：人工操作已提交、其后的 verdict 事件确认失败时，响应改为 200 并带最新撤销，不再沿用旧参考的 500「未提交的操作已回滚」；录制的数据库状态与事件恢复期望不变，三个提交后故障测试只改响应断言。例外：静态网页文件（`/`、`app.js`、`app.css` 等）的响应正文对照仓库当前文件，录制只约束状态码和响应头，`Content-Length` 对照文件长度，前端改动不需要改期望（用户 2026-10-08 于 #39、2026-10-11 于 #33 决定）。录下的期望来自 macOS，Windows 上路径段按本机分隔符还原，少数与录制平台固有差异相关的比较在 Windows 跳过（明细见 PR #51）。旧算法在 Windows 上本来就给出不同结果的值（浮点累加顺序、图像缩放与 TIFF 字序、存储的比例），在删除参考前用同一参考在 Windows 验收机上另录了一份，只保存与 macOS 不同的条目，放在 `tests/fixtures/windows/`，由 `fixed_values()` 仅在 Windows 上按路径替换。

程序回退：停止新程序，保留当前包与已提交数据，再用升级前备份恢复对应版本；有不兼容格式时禁止旧程序直接打开升级后的包。无格式变化的程序回退仍需使用与该包版本相容的旧安装产物。角色、名称、身份和研究结构的逐项职责见 [TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 13 节，现行版本见其开头。

## 目标职责

**Rust 应用核心**负责启动、HTTP、静态文件、资产定位与缩略图；图库创建、打开、切换、导入导出、备份和升级；全部人工操作、历史、统一撤销、正式数据与投影；设置、账号状态、任务调度、进度、取消和恢复；稳定外部查询、名称与索引更新、结果验证导入。一次命令统一负责校验、目标解析、事务、历史和必要投影，不把一项逻辑写入拆到两个连接分别提交。保存凭据不能自动授权联网。

**Rust／原生平台适配**负责文件夹扫描、Photos 接入和同步流程、文件可用性与权限。核心使用明确的资产引用，不依赖某个平台的内部路径布局；文件改名、移动和重新扫描必须保留资产身份与人工判断。

**可选 worker（语言不限，现行均为原生）**负责模型推理、OCR、学习、聚类和实验性溯源算法，可维护研究缓存、特征和机器证据；优先复用现有证据包与结果包契约，交回证据和候选。worker 不得直接修改人工裁决和核心投影，正式结果由 Rust 校验、去重、导入并安排可恢复后处理；重复、迟到或旧图库结果必须拒绝或幂等处理。worker 缺失或崩溃不得阻断已有缓存浏览和人工操作。源库与应用库保留各自提交和恢复边界，不宣称跨库原子事务。

`crates/iqdb`（`illustash-iqdb`）是 IQDB 签名计算的命令行模块，由原生溯源 worker（`native/terms/src/provenance_compute/iqdb.rs`）以 `--hash-only` 调用；经授权的 Danbooru IQDB 查询由 App 执行。

溯源研究与画师归属管理分开：人工的「是 TA / 不是」是核心 `library.sqlite` 的画师裁决，进统一的 `actions` 历史、可以撤销，没有 worker 也能操作；溯源 worker 只产出机器结果，经证据包交给核心（[EVIDENCE_PACKAGES.md](EVIDENCE_PACKAGES.md)、[TERMS_ARTISTS.md](TERMS_ARTISTS.md)），不读也不写人工表。机器候选和人工确认保留不同来源。

## 推进顺序

2026-10-01 用户批准稳定应用整体迁移到 Rust，分六个阶段（开源前的私有 Issue 跟踪）：M0 固定全部入口与写入的最终归属，M1 图库包、初始化、升级、备份、设置与会话，M2 全部人工写入，M3 本地查询、图片、静态资源、导入导出与扫描，M4 同步、名称、索引、账号、任务与结果导入，M5 删除 Python HTTP、Store 编排和桥接。M0–M5 均已完成：生产路径不含 Python，旧实现与对照实现于 #9 删除，兼容表由 #201（词条核心第 8 版）清理，见 [TERMS_SCHEMA.md](TERMS_SCHEMA.md) 第 14 节；干净 macOS、Windows 机器的安装验收见 #7、#6。

基础版交付 Rust 核心与桌面资源；研究版另加可选 worker，依赖缺失明确显示不可用。

共享撤销历史只有一个入口：标签、角色和作品等操作共同影响最近一步，统一入口在同一事务内检查最近动作并按类型分派。

## 桌面外壳、启动与完整性检查

Tauri 桌面外壳（`desktop/`）只负责启动核心 HTTP 命令、管理窗口、Pixiv 请求通道和用图动作的系统调用（见下文「用图动作」），不承载业务逻辑。
桌面直接调用自身链接的 `http --native --no-open` 命令，透传图库、端口及网络授权参数，保留原生账号模块实际使用的 `ILLUSTASH_DESKTOP_BRIDGE/TOKEN` 登录桥。资源优先使用显式 `--root` / `ILLUSTASH_ROOT`，安装包使用 Tauri `resource_dir()/resources`，仅 debug 构建允许源码根回退；启动前检查 `web/static/index.html` 和 `tags/tags_v3.csv`。核心链接进外壳，无额外核心边车；资源或执行文件缺失、启动失败、端口冲突与服务异常退出显示本地错误页及重装、检查参数后重启的操作提示。

macOS 外壳创建主窗口和 Pixiv 登录窗口时显式开启 WKWebView 的 `tabFocusesLinks`，使 Tab 能聚焦页面链接（#761）。

### 关窗、后台任务与窗口记忆（#451）

宿主（`native/terms/src/app_activity.rs`）每秒读一次各任务在内存里的状态，「进行中」的判断与任务面板相同（`web/static/core/jobs.js` 的 `*_ACTIVE`，单元测试对照两边的列表）。有任务开始、结束或进度变化时，经登录桥 `POST /platform/activity` 告诉外壳：在跑的任务（种类、状态、进度），以及刚结束的任务（状态、数字、失败原因前 200 字），不含路径和图库内容。换图库时上一个图库的任务不算结束。外壳（`desktop/src/background.rs`）据此：

- 关主窗口时，宿主报告有任务在跑或启动页正在导入图库，就隐藏窗口，任务继续；否则照旧退出。隐藏期间宿主出错退出时，窗口重新显示出错页。
- 隐藏期间任务都结束、又安静 10 秒（同步做完会接着找回画师之类），按 `preferences.json` 的 `after_tasks` 办：`quit`（默认）退出，`stay` 留在后台。设置在「设置 › 通用 › 关闭窗口」，只在桌面 App 里显示（`GET /api/settings` 的 `desktop`）。
- 窗口隐藏、最小化或不在前台时，任务结束发系统通知（`desktop/src/notify.rs`）：同步（有新图或识别了图）、找回画师、检查新角色（发现可添加的角色）、添加角色、导入、导出和识别模型的完成或失败；角色名称和相似图片不发。点通知回到窗口并打开任务面板（图库页的 `window.illustashOpenTasks`）。macOS 用 UNUserNotificationCenter，第一次转到后台时请求权限，被拒绝就不发，只有从 .app 包运行时才有；Windows 用 toast，按应用 ID `app.illustash.desktop`（安装包给开始菜单快捷方式设了同一个 ID）发，没装的开发构建不显示。
- macOS 隐藏后留在 Dock，点 Dock 图标回到窗口；有任务时向系统声明用户发起的活动，免得 App Nap 拖慢 Pixiv 窗口里的请求，不阻止睡眠。Windows 隐藏时任务栏按钮消失，通知区域出现托盘图标（提示进行中的任务，单击回到窗口，右键「打开 Illustash」「退出」），窗口回来后收起；再次打开 Illustash 回到已在运行的那一个（`tauri-plugin-single-instance`）。
- 退出（⌘Q、托盘「退出」、任务做完后自动退出）与关窗退出相同：关 stdin，宿主停任务、收尾 worker，10 秒后强制结束进程树。

主窗口的位置、大小和是否最大化（macOS 不记最大化：铺满屏幕的窗口也被报成缩放）存在外壳自己的配置目录（`app_config_dir()/window.json`，物理像素），不进图库；移动、缩放停下 0.8 秒后写一次，退出时再写一次（`desktop/src/window_state.rs`）。打开时记下的窗口顶部要有一块标题栏落在某个显示器的可用区域里才放回原处，放到重叠最多的显示器上，比它大就缩小、超出就挪进来；否则（显示器拔掉了、分辨率改了）按默认大小居中。Pixiv 登录窗口不记。

### 启动信号与会话 cookie

原生宿主完成启动恢复和自动任务初始化后输出并 flush `图库已启动：http://127.0.0.1:PORT/?k=<一次性码>`；外壳只接受当前端口的精确本机 ready URL，加上 `launched=1` 后让主窗口打开它。本机宿主的每个请求（含 GET、静态文件和 `/image`）都要带会话 cookie（2026-10-21 用户定，发布前安全自查）：127.0.0.1 对本机所有进程和用户开放，Host/Origin 检查只挡得住浏览器里的网页。宿主启动时生成会话秘密和一次性码；`GET /?k=<码>` 第一次访问时设置 `illustash-<端口>=<秘密>; HttpOnly; SameSite=Strict; Path=/`（cookie 不分端口，名字带端口免得两个宿主互相覆盖），码随即作废，回复一个用 meta refresh 转到去掉 `k` 的同一地址的页面。不用 302：桌面启动页（`tauri://localhost`）发起的是跨站导航，302 续上的请求按 SameSite=Strict 不带 cookie，刷新则是 127.0.0.1 页面自己发起的导航。缺 cookie 或不对一律 403（「请用启动时显示的地址打开图库」），检查在 Host/Origin 之后、会话与 CSRF 之前；写请求照旧另要 Origin 和 `X-CSRF-Token`。宿主重启（换库、升级、抢救、重新打开 App）就换新秘密和新码，外壳照常从新的就绪行进入；同一进程里切换图库不换 cookie。CLI 由 `start-web.command` 加 `--open` 用默认浏览器打开带码的地址；浏览器丢了 cookie（关掉重开）要重启宿主拿新地址。测试客户端走同一条路（`tools/verify_read_http.py` 的 `enter()`、`native/terms/tests/common/entry.rs`）。

### 完整性检查

完整性检查（`PRAGMA quick_check(1)`）要读完全部页面，数 GiB 的图库要几十秒，所以开库时按需做（2026-10-18 用户在 #212 选 C，`app_integrity`）：包内 `.illustash-checks.json` 由持有包锁的会话维护，开库时记下「开着」，正常收尾（换库、桌面外壳关窗后宿主收到管道 EOF、CLI 收到 SIGINT/SIGTERM 后的收尾）时清掉；panic 展开时不清。下次开库时它仍是「开着」（崩溃、强退、断电、外壳 10 秒后强杀），或记录里有检查发现的损坏，就在开库前对两个库都做完整检查；否则开库只做版本、core 标记和源库基础结构的校验，不做完整检查。就绪后（等 5 秒让图库页先加载）后台检查 `source.sqlite`（开库前刚检查过的除外），`library.sqlite` 距上次检查通过满 7 天才查；设置的「数据」卡可以「立即检查」两个库（`GET/POST /api/integrity`）。后台检查用独立只读连接，换库或退出时中断，不算结果。`source.sqlite` 不是 WAL，检查持有的读锁会让写入方（同步、识别导入，忙等待 5 秒，见下文 #275）提交失败，所以后台检查每 20 毫秒看一次有没有写入方的回滚日志 `source.sqlite-journal`，出现就中断检查、放开读锁，等日志消失 2 秒后从头再查。导入、备份路径上原有的检查不变，`Library::open` 本身仍做完整检查，只有会话开库（`app_service::open_owned`）按上述规则；升级的检查见「已有核心库的显式升级边界」。开库前发现损坏：拒绝打开，包不变，损坏记进记录；桌面外壳在服务未就绪就退出时读这条记录，显示处理办法（`window.showDamage`，按钮同启动失败；损坏的是 `source.sqlite` 时换成「抢救重建」），页面切库时报同一段话。后台或「立即检查」发现损坏：记进记录，会话立即拒绝写请求（409，回复处理办法；只放行切换/新建/导入/忘记图库、备份 `library.sqlite` 和应用级偏好），一秒内由恢复循环停掉全部任务、取消导入和导出，不再做持久任务恢复；图库页常驻提示。处理办法分库：`library.sqlite` 引导退出应用、把它连同 `-wal`/`-shm` 移走，再换回包内两处备份之一（#492）：`backups/` 里设置页「立即备份」的 `library-*.sqlite`（同一文件夹的 `source-damaged-*` 不是整理记录），或 `library-backups/` 里核心升级前的必需备份 `terms-v旧版-before-v新版-*.sqlite`（内容停在升级前，换回后要再升级一次；需要 pre-v8 版本升级的换一份），选修改时间最近的，之后的整理丢失。记过损坏的包因格式需要升级而打不开时，`open_owned` 先对两个库做完整检查，完好就清除损坏记录，桌面外壳随后显示升级提问而不是再显示损坏提示；`source.sqlite` 说明整理记录不受影响，引导「抢救重建」（下一段）。单独换回旧的 `source.sqlite` 会让同步按旧快照重算资产，所以不提供。记录里的损坏让之后每次开库都先检查，直到检查通过或抢救完成。

检查进行时桌面宿主（设置 `ILLUSTASH_DESKTOP_STDIN`）在 ready 之前输出 `启动进度：<千分比> <阶段>`，阶段为「上次没有正常关闭，正在检查图库数据（文件名）」：检查进度按两库文件大小加权；库内进度取连接的页请求数（页缓存命中加未命中）除以 `page_count × 1.5`，由 SQLite 在完整性检查中调用的 progress handler 上报。quick_check 先逐页检查 b-tree，再逐行扫描各表检查约束，所以读取量超过页数：在真实图库副本上，未命中数约为页数的 1.4–1.6 倍；默认 2 MiB 页缓存下大库几乎每次请求都未命中，所以这个比例也是请求数的比例。第二遍可以命中第一遍留下的页，命中也计入进度（#84）。#382 起内置 SQLite 3.54 不再启用 `SQLITE_ENABLE_MEMORY_MANAGEMENT`，页请求的统计口径照旧。同一千分比和阶段只输出一行；CLI 不输出。使用 SQLite 3.46 时，约 10 GiB 的合成大库（2026-10-18，Apple M2，release，热缓存，三次中位数）在异常退出后从启动到就绪约 55 秒，正常开库约 0.04 秒（#212）。

### source.sqlite 抢救

`source.sqlite` 的应用内抢救（2026-10-19 用户在 #228 选 B，`app_rescue`）。识别结果可以重做，但文件夹图片的 uuid 是首次扫描时随机生成的，整理记录按它关联，所以抢救要保住 uuid：（1）用独立的只读连接读损坏的文件，按它自己的建表语句在包内建 `source.rescue.sqlite`，逐表按 rowid 顺序复制读得出的行（uuid、rowid 原样），读到损坏就向后跳着探，从下一个能读到的 rowid 接着读；行复制完再建索引、触发器和视图；连表清单都读不出时按建库方式建空库（参考库在就补角色表）；角色表有读不出的行时从参考库补回。（2）每个文件夹来源在新库上重新扫描：先读取各来源原图的 MD5，在全部来源中先按路径加内容、再按剩余的一对一内容认回原 uuid。读不全时只认路径和内容都对得上的记录。对照表来自抢救出的 `assets`/`files` 行和 `library.sqlite` 已有的 `assets.src_path` 与 `meta` 里的 `folder_hash:<uuid>`（写入后不删除），每份 `library.sqlite` 备份都带着，所以不需要加表或另存文件，`#201` 之前也不改表结构。同一路径换了内容时给新文件独立 ID，原有结果留在原 ID；扫描不到的文件夹资产标为已移除；没有任何记录认得的 uuid 的识别结果删掉；没有 rating 标签的资产清掉 `recognition_assets` 账本，覆盖到这类资产的 `recognition_imports` 记录也删掉：否则重新识别的导入会被跳过或拒绝，同一模型对同一批图重新识别得到的包摘要相同，会被当成重复包、什么也不写。Photos 资产的 uuid 就是 Photos 标识，留给打开后的同步。任何文件夹来源读不到就拒绝抢救，包不变（否则那个来源的图以后会拿到新 uuid）；可用磁盘空间不到 `source.sqlite` 的大小加 256 MiB 也先拒绝，说明需要多少。（3）新库 `quick_check` 通过后才替换：损坏的 `source.sqlite`（连同 `-journal`）移到 `backups/source-damaged-<时间>.sqlite`，不自动删除；新库改名为 `source.sqlite`，扫描留给下次刷新的变更记录并入 `source.scan-pending.json`，检查记录清掉损坏、记下 `source.sqlite` 已检查。打开后照常同步，抢救不到识别结果的图由识别补上。入口只在损坏的是 `source.sqlite` 时出现：桌面启动页「抢救重建」以 `--rescue-source` 重启本地服务，抢救进度作为 `启动进度：` 行显示，完成后照常打开（失败时显示失败原因，包不变，重试会再次提供抢救）；图库页横幅的「抢救重建」调 `POST /api/integrity/rescue`，在后台建新库（会话仍停写，进度在 `GET /api/integrity` 的 `rescue`），完成后页面重新打开当前图库，新库在重新打开时、会话的连接都关闭后换上；页面切到已记录损坏的图库时确认后带 `rescue` 打开，先同步完成抢救再打开。抢救结果（读回、按记录找回、新 uuid、待识别的数量和损坏文件位置）由 `/api/libraries` 的 `rescued` 给页面提示一次。抢救耗时（2026-10-19，#228）：6.23 GiB 的合成 `source.sqlite`（约 5 万张图的规模，Apple M2，release，热缓存，不含重算图片哈希）三次中位数 158 秒；现在抢救还会读取各来源图片做内容校验，该项耗时未计入上述历史测量。

### 启动页、欢迎页与退出

外壳启动页播放与图库页相同的 logo 折叠动画（`web/static/splash.js`，由 `desktop/build.rs` 复制进 `desktop/dist/`，两边只有一份），进度条按启动进度走到 90%，进度行不进入错误日志尾部。收到 ready 后，启动页等折叠播完，再以带 `launched=1` 的地址打开图库页；图库页看到这个参数就从成形的图标开始，进度条从 90% 接着走，不再重播折叠。启动页脚本 8 秒内没有跳走（例如窗口隐藏时动画暂停），由外壳直接导航。外壳页拿不到图库页保存的主题设置，跟随系统深浅色。启动失败、升级询问、损坏提示或欢迎页出现时动画停下（`splash.halt()`），图标还没成形就直接跳到成形的样子，不再按加高了的整列居中压在提示文字上（#521）。外壳启动时：命令行给了 `--library` 就打开它；否则打开最近图库里第一个还能打开的（`desktop_shell::startup_library`）；一个都没有时显示欢迎页（#107）。欢迎页列出最近图库，并分开三个入口：新建图库（按出处分栏添加文件夹，macOS 可加 iCloud 照片）、打开已有图库、从导出包导入（2026-10-12，#95）。建库与设置页共用 `app_create::create_sources` 和同一参考库规则（`desktop_shell::create`）。添加文件夹时当场只读检查（`desktop_shell::inspect`，与同步同一套文件遍历，只读文件名和大小，每个文件夹最多数 3 秒，超时报「至少 N 张」）：不存在、没有权限、不是文件夹、没有可读图片分别提示，不阻止创建；与已添加文件夹重叠按建库的同一规则（`app_paths::overlaps`）判断，同一出处里的直接去掉被包含的那个，跨出处的标出来由用户去掉一个。从导出包导入复用设置页的归档导入器（`desktop_shell::import`，阶段与取消相同，发布前取消会删掉已解压内容），成功后直接打开新图库并由打开登记到最近图库。创建或导入失败时表单和已选文件夹保持不变。新建和导入每次点击带一个请求编号，外壳对同一编号只执行一次，再次送达的调用等第一次的结果：Tauri 的自定义协议请求失败时会改用 postMessage 重发同一调用，而这两个命令在返回前就让页面跳转，跳转可能使原请求失败，曾因此生成两个带编号的包；选择窗口打开期间再点不会打开第二个（2026-10-13，#115）。加载中可以「选择其他图库」；启动失败时显示原因、日志尾部和「重试 / 选择其他图库 / 新建图库」。每次启动本地服务取一个新的代号，取消、重试或换库后，旧服务的进度、ready 和退出一律忽略；换库前先按正常收尾停止旧服务（`Child::stop`）。这些命令（`welcome_*`、`startup_*`）只开放给主窗口的本地启动页（`desktop/capabilities/main.json`），127.0.0.1 上的图库页调用不了。启动超时按「30 秒内没有任何输出行」判定，不再限制总时长。登录桥与业务页保持原契约；只读请求的节流与网络授权由原生调用方负责。桌面设置内部 `ILLUSTASH_DESKTOP_STDIN=1`：宿主先等一字节启动闸门，再监听管道 EOF；外壳完成进程归属后释放闸门，关闭及异常父进程退出使管道关闭，宿主复用已有任务 stop/join。CLI 未设置该变量时不读 stdin。正常桌面退出等待宿主，10 秒超时才终止自身子进程；Unix 使用独立宿主进程组，超时先按 PID/PPID 枚举仍归属该宿主的后代并终止（覆盖独立 worker 组），再终止宿主组；枚举失败会输出诊断并退回宿主组终止。Windows 使用私有 kill-on-close Job 并在初始化任务前完成归属；归属失败会终止并 wait 新宿主，再显示重启提示。研究 worker 保留现有独立进程组与平台限制，不因桌面 Job 推断 Windows worker 已受验。

直接 SIGKILL 原生宿主时不能保证执行 Unix worker 析构收尾（见「公共后台任务与会话结果边界」），桌面父进程异常退出由存活宿主处理 EOF。Windows 桌面启动、关窗收尾、stdin EOF、端口冲突和父进程异常退出的验收结果与已知限制见 [BACKEND_TASKS.md](BACKEND_TASKS.md)「Windows」。程序回退使用此前发布的应用与既有备份恢复规则。

### 用图动作（#450）

图库页可以打开原文件、在访达或资源管理器中显示、把图拖到别的应用、把选中的图导出到文件夹（2026-11-16，#450）。底线是不动原图：只读、复制，不移动、不删除、不改原文件。安全边界：

- **页面只按资产 ID 指图**。页面能调的路由是 `POST /api/files/open`、`reveal`、`drag`、`export`、`export/cancel`、`export/reveal` 和 `GET /api/files`，照常要求会话 cookie、Origin 和 `X-CSRF-Token`，正文里只有资产 ID（导出另有选择栏的筛选条件和子文件夹的分法）。宿主从当前图库 `library.sqlite` 的 `assets.src_path` 查路径（`native/terms/src/app_asset_files.rs`），图不在当前图库、没有路径、文件不在或不是普通文件就拒绝；页面给的路径一律不收。导出的目录由宿主弹系统的选目录窗口取得，任务自己记下，「显示导出的文件夹」不收路径。
- **本机桥的文件路由只有宿主能调**（`desktop/src/bridge.rs` 的 `/platform/files/open`、`reveal`、`folder`、`drag`）。口令只在宿主的环境变量里，页面拿不到；路径放在 POST 正文里，不进地址，`app_desktop::send` 只放行这几个固定路由。外壳再查一遍：路径是绝对路径，指向存在的普通文件；「打开」只放行图片扩展名（`illustash_platform::platform_files::OPENABLE`），不会被拿去启动程序；日志里不写路径。这些动作在外壳进程里做：Windows 上从拥有前台窗口的进程打开资源管理器才不会被压在后面，拖放也只能在外壳里起。浏览器模式（没有外壳）由宿主直接打开和显示，不能拖出。
- **拖出只允许复制**。外壳起系统的拖放：macOS 用 `NSView beginDraggingSessionWithItems:event:source:`，拖放源的操作掩码只有 `NSDragOperationCopy`；Windows 用 `SHCreateShellItemArrayFromIDLists` 取系统的数据对象，再 `SHDoDragDrop(…, DROPEFFECT_COPY)`。拖进访达、资源管理器只能复制，不能移动。一次最多 100 张。
- **导出只读原图**。用 `create_new` 写进用户选的文件夹，从不覆盖；同名文件内容相同就跳过（算「已存在」），不同就在扩展名前加编号；写到一半出错或取消，删掉这个写了一半的文件。可以按角色或画师建子文件夹：一张图有几个角色（画师）就在每个文件夹里各放一份，没有的放进「未识别角色」「画师未知」（按界面语言翻译）；文件夹名去掉文件系统不允许的字符、避开 Windows 保留名。一个会话同一时间只有一个导出，换库、关闭或发现损坏时停下；进行中的导出算后台任务（`app_activity` 里的 `files`），关窗时外壳隐藏而不退出，做完发结束通知。
- **照片来源的文件不交出位置**。它们在「照片」的图库包里，在访达里动那里的文件、用别的应用编辑存回，都会弄坏照片图库：不提供「在访达中显示」；打开和拖出用系统临时文件夹 `Illustash/open-copies/` 里的只读副本，起「照片」里的原文件名，下次做副本时清掉一天前的；导出时本机有原图就复制原图（只读打开 `Photos.sqlite` 查位置），只在 iCloud 里的复制派生图，完成后写明几张是派生图。
- 「打开」交给系统的默认应用的是文件夹来源的原图本身：在那个应用里编辑、保存是用户自己的操作。

## 恢复条件与验收标准

每批保留现有测试与 golden，不降低判等标准；验证读、写、数据库变化、历史、统一撤销和待处理事件。随机选择比较候选范围与业务约束，不要求独立随机调用抽到同一资产。真实图库和副本验收须另行授权，默认仅用隔离合成夹具，禁止默认启动真实库。

故障与并发覆盖写入失败、锁竞争、请求中切库、进程中断、重复结果、备份与恢复；性能在相同数据、构建和负载下记录延迟及内存。显著退化必须解决或由用户明确接受，语言迁移不能代替性能证据。未执行、数据口径与平台限制须在对应 PR 明确列出。

涉及不兼容升级时，从升级前备份恢复，禁止直接用旧程序打开新格式。任务清单、交接和验收数字只写 GitHub，本文仅维护仍有效的设计和决定。

### 迁移覆盖与对照工具

`tests/migration_inventory.json` 记录生产入口与写入的最终归属、当前运行时、存储域、验收及删除条件；研究、测试和生成代码按目录或类别保留，生产可达工具另列例外。`python -m tools.migration_inventory check` 只在新增 HTTP 路由/分支、数据库写入点、启动/CLI 入口、归属缺失或文件不是规范格式时失败，普通实现变化只报告。规范格式是顶层每个键、顶层对象或列表的每一项各占一行（`sources` 指纹每个文件一行，之间再空一行，因为 git 把相邻行的改动算作冲突），让改不同文件指纹的分支能被 git 自动合并；`fmt` 按此格式写回，`sync-sources` 只把报告用的 `sources` 刷新为当前指纹。`expand` 按当前源码位置展开归属，`report` 输出覆盖摘要；静态候选含动态 SQL/包装调用，数量不代表正式写入数量或迁移完成率。工具要求 Python 3.12，不属于基础版运行依赖。

`tools/verify_read_http.py` 对确定性读取严格比较状态码与原始响应字节，拒绝将随机 `/api/ask` 加入字节等同语料。

随机读取分别按调用者提供的独立候选契约检查状态、范围、数量、唯一性、元数据、尺寸及固定 hint 顺序，独立抽样不要求资产相同。单一标签夹具在克隆前预置随机 ID，并以合成触发器固定动作时间，不对响应归一化；不代表正式数据库时间行为。逐步 HTTP 延迟与运行内存可作为工具基线记录，单次用例不能证明迁移性能收益。

## HTTP 读路径（2026-09-30）

原生前门位于 `native/terms/src/app_http.rs`，CLI、启动脚本与桌面共享。原 27 个确定性 GET 的响应由核心独立组装；下列分组用于测试语料，不能按数量代替完整调用链验收。

### 默认核心读取

- **R1–R2（7 个）**：`/api/user-tags`、`/api/general-tags`（本地 CSV）、`/api/artists`（OC 画师统计）、`/api/artist`、`/api/artist-audit`、`/api/entities`、`/api/works`。画师读取保留旧/核心来源、来源帖子、候选原因及固定 SHA1 抽样；本地搜索的 `remote` 缺失或空值分支直读。
- **R3（7 个）**：`/api/assets`、`/api/assets/count`、`/api/ids`、`/api/detail`、`/api/home`、`/api/fold`、`/api/review-pools`。包括现有筛选、排序、分页、可见性、NSFW、作品树、画师和 `pool=cluster|similar` 的持久化分组查询；详情组装包括源库标签和来源库帖子。
- **R4（8 个）**：`/api/characters`、`/api/tagged`、`/api/character/profile`、`/api/character/activity`、`/api/character/stats`、`/api/identity/count`、`/api/identity/queue`、`/api/bootstrap`。保留未设置/hybrid 策略、summary、封面分配、首次命中、去重、分页、历史重渲染和会话字段；其他读取见下节。

`GET /api/character/together?id=<角色>&nsfw=<筛选>&limit=<数量>` 返回指定角色的同框次数。结果为 `[{id, count}]`，按次数降序、同数按角色 ID 升序排列。`limit` 为正整数，默认 6。`nsfw` 与角色统计共用分级和可见性条件。双方均须在同一张图上有正式归属（人工确认或机器认定），并排除自身及只当标签的角色。

`GET /api/ambiguous?nsfw=<筛选>`（#786）按有向角色对（A，B）列出打平的归属：A 是当前机器归属，B 是证据几乎一样强的备选角色。结果为 `{groups: [{a: {id, name}, b: {id, name}, count, items: [uuid, …]}]}`，只含可见图，排除最近七天对（图，A）答过 `kind:"ambiguous"` 的「以后再说」的项；组按张数降序、同数按 A、B 的 ID，组内按入库时间降序、同一时间按 UUID 升序（和不带 `order` 的 `/api/assets` 同序），`count` 等于 `items` 的长度。`GET /api/review-pools` 在数组末尾追加 `{kind:"ambiguous", name:"可能认错", count, cover:null}`，`count` 是同一筛选下各组张数之和。「都不是」的写入见下文[人工标签与统一撤销](#人工标签与统一撤销的原生入口边界)的 `/api/ask/answer`。

### 本地、外部与任务读取

`/api/ask`、静态/图片、设置、图库、宿主信息、同步、名称、索引、账号和溯源状态均由原生实现处理。本地分组和已有特征/缓存可以在 worker 缺席时读取；按需研究计算走显式可选 worker，不启动旧 HTTP 宿主。未知接口或缺失研究能力明确返回 unavailable；Host/Origin/会话校验失败直接拒绝。远程名字/作品查询、网络授权及各任务具体协议见本篇对应章节。全部人工 POST、统一历史与撤销由原生业务模块提交。

`tools/verify_read_http.py` 只接受系统临时目录内的合成夹具。成功响应严格比较状态码与原始响应字节，不归一化；错误按用户 2026-10-01 决定只比较状态码。golden 不重录，随机 `/api/ask` 沿用候选范围与选择约束。默认宿主专项另用仅包含失败 Python 哨兵的 PATH 验证不启动 Python；R1–R4 读路由语料由上述测试读宿主按录下的旧 Python 响应逐字节核对（#9），不表示生产路径。

核心名字投影按请求实际使用的键查询，并只在本次读事务内记忆结果；未投影的作品文字仍按旧规则选名。资产分页批量读取本页身份、作品显示名和 hybrid 节点，保留字段覆盖顺序；HTTP JSON 使用单一输出缓冲区，浮点规则和持久化 JSON 均未改。没有跨请求名字缓存，人工改名和切库后的请求重新读取当前快照。

## 显式 fresh 建库边界（2026-10-01）

建库与普通读取分离。Rust 提供 `illustash-terms library-create --dest <新包.pilib> --folder <来源目录> --reference <参考.sqlite> [--name <名称>] [--preset x|pixiv|other]`；目标包、来源文件夹与只读 reference 路径必须由调用方提供，不能隐式打开默认图库。新包须构造完整应用 schema、名称默认值和源库，再复用词条迁移、四类切换及核心刷新，完成后才发布 `library.json`。来源 reference 的表定义与索引保留，只复制 characters、character_aliases、runs 的数据；不扫描来源目录、不启动推理或联网。普通新库保留未设置的身份策略；名称缺省沿用原规则，不给 `--preset` 时出处为「其他」（不按文件夹名猜，见 [LIBRARIES.md](LIBRARIES.md)「来源管理」）。核心切换备份在包内 `library-backups/`，与设置页的 `backups/` 分开。

创建独占取得新目录；失败只清理本次创建的目录，已有目标、来源文件夹、reference 与当前会话均不改。各核心初始化仍有自己的事务，不宣称源库与应用库跨库原子提交。中断留下的不完整新目录不能作为可打开图库呈现。已有库升级和恢复沿用独立设计，不能套用新目录删除策略。显式原生服务的 `POST /api/libraries/new` 已接同一 fresh 构造函数，支持 `kind=folder` 与 `kind=photos`，不扫描或启动 worker。新包按既有规则选唯一名称、创建并登记到 recent 末尾，不改变当前库或令牌；只有后续 `/api/libraries/open` 才切换会话。创建失败只清理本次目录；发布完成后若注册表失败，返回既有 400 并保留完整新包，当前会话不变。普通 JSON 参数、名称/路径、错误和注册顺序复用旧契约；显式创建与切库由会话锁串行协调。创建本身不启动同步；打开后的来源同步见「原生来源同步与应用刷新」。

Photos 新库沿用既有来源契约：`s1`、`kind=photos`、宿主主目录下的 `Pictures/Photos Library.photoslibrary`，App 映射为 `com.atebits.Tweetie2 → x`、`net.pxv.iphone → pixiv`。创建仅登记来源路径，不解析链接、不检查存在性、不读取 Photos 数据，HTTP 请求中的 `folder`、`photos`、`preset` 字段沿用旧接口的忽略行为，来源保持默认值；Photos 读取继续由原生平台适配负责。两种来源共享完整 fresh 初始化、发布和失败清理边界。

## 原生图库管理与归档

显式原生服务直接处理 `POST /api/libraries/choose`、`preset`、`sources/add`、`sources/remove`（见 [LIBRARIES.md](LIBRARIES.md)「来源管理」）、`forget`、`import`、`export` 和 `GET /api/libraries`；桌面外壳链接相同核心，显式原生模式复用这一调用链。选择窗口见「Core / Platform Service / App 边界」：取消返回空路径，选择期间不占用会话锁，返回结果仍须属于原会话，退出时终止选择进程。来源预设沿用 x/pixiv/other，仅修改指定文件夹来源及其资产；配置先原子保存，再提交源库更新，保留既有可重试边界，不宣称配置文件与 SQLite 跨介质原子提交。最近图库移出只修改注册表，拒绝当前显示路径，不删除包。

归档仅允许 `library.json`、`source.sqlite`、`library.sqlite`、`features/full-features.npy`、`features/full-manifest.csv` 和 `album-char-map.json`，不包含图片、缩略图、备份或 provenance。导入支持旧归档的 stored、deflate、bzip2 和 ZIP-LZMA，拒绝绝对路径、上级路径和未知内容，流式解包并校验 CRC；保留 library_id、来源和配置顺序，增加 imported_from，沿用唯一目录命名，完整发布后登记到 recent 末尾，不切当前库。失败只清理本次独占创建的目标目录；登记失败保留完整已发布包，导入任务照常结束为 done，结果为 `{path, name, listed: false, error}`，设置页提供「打开这个图库」（打开时再登记一次），不再以错误结束让重试另导入一个带编号的包（2026-10-13，#111，与建库的 #97 相同）。导入不隐式初始化或升级已有数据库，旧归档后续打开仍须走现有显式升级入口。

可分享归档与个人备份的边界：导出只清理 SQLite 副本，清空 `name_settings`（本机凭据引用、历史明文密钥、接口地址、模型名、流量计数与将来的连接设置均不随包分享），删除 `prefs.names_network` 和 `prefs.onboarding`（首次引导记录：答过的提问、上手清单，2026-10-13，#116），保留其他偏好、人工记录和识别结果；原图库设置不变。清理启用 secure_delete，并压实副本以移除历史写入遗留的秘密字节；对无索引表临时建立索引，压实后删除，以保留隐式 rowid 与最终 schema。导入包括旧版 ZIP 在发布前执行同一清理，不继承发送者的名称联网授权，模型接口与授权由接收者重新设置。模型凭据和账号凭据均位于归档范围外的应用凭据存储。停止程序后备份完整图库目录可保留包内个人设置；在新位置或设备恢复后需要重新填写模型密钥。

导出捕获包实例路径和会话 generation；配置 ID 相同的不同包不共用任务状态。SQLite 使用备份 API 包含已提交 WAL；压缩前先复制全部数据库，并以各库的 `data_version` 确认从第一个复制开始到最后一个复制结束期间没有任何提交，否则重取；读取版本或备份时遇到写入者的 SQLite BUSY 也重做整组（最多 5 次，仍在写入则明确报错，不发布部分归档），所以包内两库是同一时刻的状态，不会出现应用库领先源库的混合版本；配置与特征文件不在这份快照里（2026-10-12，#23）。导入在解包、清理并写入配置之后、发布与登记之前，按打开图库的同一套校验（配置必需字段、两库完整性与结构；旧核心按显式升级的校验）检查新包，打不开的包删除新目录并报「导入的图库无法打开」（#42）。导入的清理连接和两库只读校验连接均在处理 schema 前关闭 `trusted_schema`、开启 `SQLITE_DBCONFIG_DEFENSIVE`。导入是后台任务（见[导入任务](#导入任务)），不占会话锁；`library.json` 不超过 1 MiB，条目只认导出白名单（ZIP 索引按名称去重），每个条目最多写出其声明的大小，开始前要求目标卷可用空间不少于声明总量、`library.sqlite` 压缩所需的同等空间和 256 MiB 余量，不足则在创建目录前报「磁盘空间不足」。取消、切库或退出时，解包、清理与校验在下一个检查点中止并删除新目录，不登记（2026-10-12，#26、#88）。数据库与特征流式写入同目录独占暂存归档，完整关闭后原子替换最终产物；错误或发布前取消保留原有产物，并清理本次暂存目录。启动响应为既有 running，重复请求在任务运行期间返回 400，列表发布 running/done/error；切库先验证并登记替代包，再取消并等待旧导出，新的会话显示 idle。退出拒绝新导出、取消并等待当前任务，备份分批检查取消并限制锁等待；普通网络慢响应不拥有导出任务。

归档验收比较解包后文件内容及数据库 DDL、typed cells、rowid 和正式投影，不比较 ZIP 时间戳或压缩容器字节。确定性 HTTP 成功响应保持原始字节；错误只比较状态码。未知内容、路径和格式校验沿用 400，损坏 ZIP、CRC 与非对象配置沿用旧宿主的 500。

## 已有核心库的显式升级边界

原生升级与普通打开分离；使用 `http --upgrade --library <包>` 显式请求。2026-10-19 起（#201，用户决定），只升级四个领域都已切换到核心的图库：应用 schema 36、terms 5 到当前版本的前一版、source 是核心 5 时代以来的布局、索引与名称已初始化；一次应用库事务，事务前用 backup API 备份。更早的旧图库（应用 schema 1–35、没切换或切换没完成、旧 source 布局、缺索引元数据）打开和升级都拒绝，提示先用 `pre-v8` 标签的版本打开升级；Python 时代的分阶段成对发布路径（原 `app_legacy_upgrade.rs`，以及 `app_migrate.rs` 里的旧库迁移）随之删除，留在 git 历史里。无需升级的当前包沿用普通打开；未知未来版本、未完成的核心标记或损坏的既有写保护在写入前拒绝。普通打开不补跑这些迁移。

界面入口（#193，2026-10-18）：普通打开仍不升级，用户确认后才走与 `--upgrade` 相同的预检、备份和一次提交。桌面应用启动时本地服务打不开图库，外壳只读预检（`app_upgrade::prepare`，不写任何东西）这个图库能否升级；能的话不显示启动失败页，改显示升级确认页：说明升级后旧版本打不开、升级前会自动备份、升级没有完成时图库保持原样，按钮是「升级并打开」和「选择其他图库」。确认后外壳带 `--upgrade` 重启本地服务；升级失败时显示失败页和日志（含「升级未提交：…；升级前备份：…」），重试会再次询问。图库页在「设置 › 图库」切换到旧格式的图库时，`POST /api/libraries/open` 在打开失败后同样预检，能升级则返回 409 和同一段说明；页面确认后带 `"upgrade": true` 再请求，升级后切换，失败返回 400「升级未提交：…」，当前图库保持打开。升级后的会话在 `GET /api/libraries` 里带 `upgraded.backup`，页面在每个浏览器里提示一次备份位置和回到旧版本的办法。只在打开失败之后才预检，当前格式的图库打开不多花时间。网页形态启动时（`start-web.command`）仍在终端报错，按提示用 `--upgrade`。

升级只提交应用库，走单应用库迁移与提交后空间维护路径，维护失败仍使用待重试标记，source 和配置不写。

升级的完整性检查与进度（#267，2026-10-20）：问「要不要升级」只读版本、core 标记和两库结构（`app_upgrade::prepare` / `needed` / `pre_v8`），不做完整检查，所以大图库也立即出现确认页。升级本身在持有应用库写锁（`BEGIN IMMEDIATE`）之后对两个库各做一次完整检查，发现损坏就停，不备份、不写；然后备份、迁移、提交、空间维护；提交后只把写过的 `library.sqlite` 再完整检查一次，没写的 `source.sqlite` 不再查。桌面宿主（`ILLUSTASH_DESKTOP_STDIN`）把升级的阶段作为 `启动进度：` 行输出：「正在检查图库数据（文件名）」占 0–60%（按两库大小加权），「正在备份图库」60%，「正在升级图库」70%，有待回收空间时「正在回收图库空间」75%，「正在核对升级后的图库数据」80–95%。备份、迁移和空间回收在 SQLite 里一步完成，不能报进度；这期间宿主每 5 秒重发最后一行进度，外壳的「30 秒没有输出」判定只在宿主真的不出声时才停掉服务。`--upgrade` 打开的若已是当前格式（例如上次升级已提交、后续失败），按普通开库处理。测试可用 `ILLUSTASH_TEST_UPGRADE_PAUSE_MS` 让备份前的一步停住指定毫秒。约 6.3 GiB 的合成 terms 6 图库（2026-10-20，Apple M2，release，热缓存，三次中位数）从启动到就绪约 27 秒，其中两库检查约 23 秒、备份 library.sqlite 约 3 秒；真实图库冷缓存、迁移和空间回收都会更慢。

terms 5（`5e7e7f7`）/6（`d9c49b4`）的历史源库定义均含可为 NULL 的 `assets.stable_hash`；文件夹建库从该完整参考 schema 克隆，词条升级无需补该列。精简读取夹具缺列时，升级仍可成功，默认同步会在应用刷新阶段记录 `error` 与缺列原因；这类夹具不代表历史完整扫描输入。迁移、历史、撤销及备份恢复的精确对照使用 `--no-sync` 隔离独立后台任务；默认自动同步另以完整历史 schema 和合成图片验证，每次启动均新增 `sync_runs`，不能把该任务记录算作迁移差异。当前 schema 上重复 `--upgrade` 只表示迁移无操作，仍遵循启动同步偏好。`--upgrade` 与 `--backup` 冲突在预检阶段拒绝，提示为 `--upgrade 不可与 --backup 同用`，不写库或备份。

## 人工标签与统一撤销的原生入口边界

全部人工命令由原生会话在完整 HTTP 调用链内处理。`POST /api/tag` 的用户标签增删及 `POST /api/undo`：Rust 在命令事务内完成参数校验、filter/ids/exclude 解析、重复资产扩展、作品计数、现有核心写入、历史及有序响应。沿用旧目标和标签规则，不改分类政策；人工操作提交之后的 verdict 事件确认失败只记日志，事件保持 pending 由下一次操作、启动或定期恢复处理，响应仍按已提交返回 200 与最新撤销；网页撤销失败时重新读取最新撤销状态（#40）。请求在会话锁内重查 generation 和令牌，旧请求不能写入切换后的图库。

原生会话另接管 `POST /api/artist/verdict`、`/api/curate`、`/api/fold`，覆盖画师裁决、人工收录与排除、重复关系折叠。画师裁决仅选择已有资产，不扩展重复组；人工整理沿用标签目标选择并在同一应用事务写历史、curation 与 excluded。折叠配置按既有稳定哈希和关系规则重建 duplicates，并沿用成员差异可见性规则；源库只读快照与应用库提交之间保留独立边界，不宣称跨库原子事务。折叠沿用现存无历史语义；人工标签、画师裁决和整理统一参与最近一步撤销。持久化 JSON 的 ASCII、空格与字段顺序保持原契约，和 HTTP 序列化分开。整理及其撤销共用 bulk excluded 重建规则，保留乱序目标下的物理 rowid 顺序。

原生会话接管 `POST /api/entities`、`/api/character/profile`、`/api/local-links`、`/api/works`、`/api/character/mascot`、`/api/kind`、`/api/oc/shared`。实体创建先按现行名称、别名及 Danbooru/WD 链接复用已有身份；无名 OC 按未占用的「原创角色 N」命名。作品保留 create/edit/from_tag/move/link/rename 六类操作、父子环与链接冲突检查。资料、本地链接及角色标记与其他人工命令共用最近一步撤销。角色标记的撤销快照见[角色写入契约](TERMS_CHARACTERS.md#3-切换之后)。身份创建、作品编辑沿用无历史语义。单独修改作品归属经资料接口并参与撤销。资料请求的 `work` 也可写成 `{"create": 名称}` 或 `{"tag": tag}`：新建或按 tag 取得作品与资料写入在同一事务，资料失败不留孤立作品；网页保存在第一次请求前即禁用按钮，新建作品时上级作品随 `create` 一并提交（2026-10-11，#35）。参数解析、核心写入、历史、名称与身份投影及事件入队共用 Immediate 事务。角色标记在写事务内取得本次撤销记录，成功提交后才送出响应。其他响应读取在提交及投影更新之后，不把提交后的响应或事件失败解释成回滚。

版权解析先查本地作品的 Danbooru/qualifier 链接；无匹配时按原 `related_tag.json` 查询及 `name_cache` 契约选择覆盖率至少 0.9 的最小作品，缺失则退至 0.5。Rust HTTPS 客户端保留 25 秒超时、2 MB 响应限制和 500 MB 流量预算，查询缓存与成功流量记账随命令提交；查询或后续写入失败回滚本次正式修改。`ILLUSTASH_COPYRIGHT_BASE` 仅允许本机带端口的 HTTP 合成查询替身。当前请求失败返回错误。

关联创建（含复用）、资料编辑成功后同事务写入 `backend_effects.kind = names:{"entity":角色ID,"refresh":false}`；本地链接写入相同结构而 entity 为 null，表示全范围。这是可恢复的名称更新请求，绑定事件所在包实例，不是完成通知。原生名称消费者按范围执行、重查来源指纹并更新正式名称/投影，成功后确认；失败、确认故障或中断保留 pending，重启幂等补跑。名称运行、网络授权与回退的现行边界统一见 [名字](TERMS_NAMES.md#9-显示名来源与后台更新)「原生运行与恢复」。

原生角色裁决入口为 `POST /api/change`、`/api/audit/answer`、`/api/ask/answer`、`/api/reassign` 与 `/api/sweep`。HTTP 参数、筛选/exclude/confirm 和未展开副本的选择集在同一 Rust 写事务解析，角色核心继续负责副本范围、父角色/子标签裁决、原归属核验、历史及投影。全部 change 动作、多角色确认、问答 yes/no/later 与 switch、可能认错的「都不是」（`kind:"ambiguous"` 加 `also_no:B`，对 A、B 各记一条否定，单独提交，提交时 A、B 必须仍是所选图的当前候选对，否则 400；一步撤销，撤销恢复这一步覆盖的旧问答记录，#786）、随机审核、单图和批量替换及扫除沿用现行语义；只解析接口消费的参数，未使用字段中的超大整数和非有限浮点不改变操作结果；写入仍要求当前 schema 和完整四项词条核心切换；later-only 返回 `undo:null`，不返回此前历史。裁决和 verdict 事件同事务提交，提交后无 learner 的事件处理及恢复由原生会话完成；角色与标签、画师、整理及身份命令共用最近一步撤销。生产回退使用相容产物及升级前备份。

原生会话同时接管 `POST /api/identity/decision`、`/api/identity/skip` 和 `/api/merge`。关系裁决复用核心的提议、证据指纹、状态决议与身份图重建；hybrid 合并保存 `duplicate_of` 决议，保留原始认领、资料和作品记录，通过身份图改变正式投影。未设置策略时沿现有合并链解析当前角色，再执行破坏式合并，搬移人工认领、候选、子标签、名称别名、置顶和审核归属，撤销恢复对应状态。已有身份未定记录继续参与投影和迁移；队列跳过仅保存排序后的身份键，不要求角色存在或 hybrid 策略。跳过本身不写事件，其最近一步撤销沿用既有 verdict 通知；重复跳过及撤销保留偏好 JSON 和 rowid 的现行写法。三个入口与既有人工命令共用会话切库隔离、统一历史与提交后事件恢复，不升级格式。

撤销沿用全库最近未撤销历史及明确 action id，不能只查标签历史。各现有历史类型的可逆操作、投影和持久化事件须按真实原路径验证，不能以 HTTP 返回成功替代正式恢复。全部人工接口由原生模块处理；未知接口保持不可用响应。异步副作用通过持久化事件恢复，worker 不得直接写人工裁决和核心投影。

验收在相同合成初始库的独立克隆上比较 HTTP 成功状态与原始响应、正式数据库全表 typed cells/rowid 和 DDL、历史、连续跨类型撤销及待处理事件。角色、画师、整理、折叠、置顶及身份关系、未定、跳过与合并的错误响应只比较状态码，失败仍须验证三库无部分提交。只为独立运行提供相同已批准输入，不放宽 golden 或归一化正式输出；注入失败须无部分提交。

未配置研究 worker 的基础模式中，`verdict` 事件按成功通知空操作确认；`done` 不表示学习完成，imported 和未知事件保留原状态。配置研究 worker 后，verdict/imported 由下文的原生研究协调器恢复和消费；正式投影始终通过核心发布。

标签目标解析、概念创建、裁决、历史和投影共用一个应用库 Immediate 事务，任一步失败回滚整个命令，不留下未使用的概念。命令提交后的事件确认失败时，人工操作已完整提交、事件 pending 可恢复，响应按上文 #40 的规则返回；这一阶段与事务内失败回滚分别验证。标签与撤销不升级数据库格式。

## 原生静态资源与图片服务

显式原生模式直接提供 `/`、`/app.js`、`/app.css`、`/theme.js`、`/favicon.svg`、`/splash.js` 的既有资源字节、Content-Type 和安全响应头，`web/static/core/`、`web/static/ui/` 里的页面模块按 `/core/<名>.js`、`/ui/<名>.js` 提供，`web/static/css/` 里的样式表按 `/css/<名>.css` 提供（`text/css`，名字只含小写字母、数字、`-`、`_`，见 [WEB_V2.md](WEB_V2.md#前端模块2026-10-18199)、#705），以及 `/image?id=&size=`；`/i18n.js` 是 `web/static/i18n.js` 前面写入当前语言设置、可用语言和译文表（见 [WEB_V2.md](WEB_V2.md#界面语言2026-10-18173)）。资产路径只从本次会话的应用库读取，不扫描来源，不启动 Python。尺寸上限沿用 thumb 400、medium 800、preview 1600、full 3000，不放大；原图定位失败和不可读返回 404，非法尺寸返回 400。full 同样输出有界 JPEG，并非原文件下载。

缓存键沿用 `sha256(ident:mtime_ns:size_bytes:size).jpg`；命中直接返回旧文件字节，绕过解码槽。未命中先遵循 EXIF 方向（缺失或非法值不旋转），再等比缩小、将透明区域压平到 `#f4f4f2`，输出质量 84 的 JPEG。JPEG 使用 vendored mozjpeg 的 libjpeg 快速配置和分数解码；其他格式使用 image 解码，SIMD 缩放保留 alpha 预乘与调色板及一位 PNG/BMP 的 nearest 规则。锁绑定包内缓存路径，同键在一个服务进程内只生成一次，最多四个解码任务并行；先独占创建随机临时文件，再原子替换，只清理本次临时文件。不宣称跨服务进程只生成一次或缓存写入的断电持久性。图片请求捕获旧会话的源路径与缓存目录后释放会话锁；解码、缓存和慢客户端不阻塞切库，也不会改用新包路径。

新 JPEG 与 PIL 不要求字节一致；合成图验收要求尺寸与方向一致、透明底色各通道误差不超过 2/255，新旧 JPEG 各通道平均绝对误差不超过 4/255，至少 99% 像素的单通道误差不超过 24/255。旧缓存和静态资源保持原始字节、响应头严格相同；错误响应按用户 2026-10-01 决定只比较状态码，既有 golden 不改。

原生 `POST /api/cache` 的 `action=clear`（2026-10-01）复用包内缩略图路径与 settings 统计；只删除直接子项中的 `*.jpg`（POSIX 区分大小写，Windows 沿用 glob 的不区分大小写；包含隐藏名称及符号链接本身），不递归，不删除临时文件、其他后缀或原图。缺失缓存目录返回 removed=0，不创建目录；成功返回先 removed 再原 settings 字段。文件已消失仍计入 removed，文件删除失败返回 500，保留旧入口的部分删除行为，不承诺回滚缓存。失败会释放协调锁与解码槽，修复缓存目录异常后可重试。

清理与图片缓存操作按包路径协调：图片持共享锁完成缓存字节读取或同键生成、临时文件原子发布；清理持独占锁，复用一个解码槽，删除后在锁内读取 settings。锁按规范化路径共享并随在途请求释放，包路径别名不会绕开协调；不同包不共用独占锁，配置 ID 相同的包也按各自路径区分。清理在请求格式及会话校验后捕获所属包并释放会话锁，等待图片、删除与 socket 交付不会阻塞切库，旧请求的清理与响应不会转向新包；过期或切库前未读完请求体的会话返回 403。清理成功后新图片请求可以重新生成缓存，不保证清理响应交付时目录永久为空。协调限于同一服务进程。

## 原生本地查询与宿主信息

原生模式接管 `GET /api/ask`、`/api/similar`、`/api/clusters`、`/api/sync`、`/api/names`（含 progress）、`/api/index`（含 progress）、`/api/about`、`/api/accounts`、`/api/provenance` 与 `POST /api/suggest`。ask 按 hint 优先、种子不足优先、否则 sample 的现行规则随机选择；保留按数量排序后的前 12 个候选、skip 最多 50 项、later 一周休息期、可见性/NSFW、单人约束、6 张上限与图片尺寸。候选随机抽取使用无模偏差的整数取样。随机输出只校验候选范围和规则；hint 图片按分数降序，seed/sample 图片沿 SQLite random 排序。回答由原生人工命令处理，已回答图片离开候选。

suggest 在只读事务内复用人工目标/filter/exclude 解析，明确不扩展复制品；按去重后的输入顺序截取前 2,000 图，保留机器归属及人工拒绝排除、频次/强弱/分数/名称排序与 9 个候选上限。名称显示、作品覆盖与筛选规则复用现有读取模块。只创建请求局部 TEMP 表，不写正式数据库、历史或事件。

similar/clusters 从持久化表读取，worker 缺席时 state/learner 为 off；sync 的来源盘点、识别编排和本地历史见「原生来源同步与应用刷新」。names 的原生后台与远程选择器见 [名字](TERMS_NAMES.md#9-显示名来源与后台更新)，读取提供进度、名称覆盖、旧配置兼容与 API key 后四位；覆盖统计复用名称读取的 Danbooru、WD、Photos、CCIP、CCIP-base 链接回退。账号 GET 只读取既有绑定元数据、钥匙串/文件是否非空、X 导入统计及本机桌面登录桥，不联网、不改变凭据；写分支见「原生账号与桌面登录桥」。index 的原生检查、补丁与恢复见「原生角色索引维护」，provenance 已有原生 Runner，任务/授权和 worker 缺失行为见 [PROVENANCE.md](PROVENANCE.md) 第 8 节，已有来源结果仍由作品详情与画师读取呈现。oc/groups 从未持久化，是实时按需特征计算；worker 缺席返回原 400，local-links 与 entities/works 的非空 remote 查询已接入原生名称文本来源，联网授权独立于凭据保存。

about 的应用名、版本和频道来自编译时现行应用元数据，schema 是应用 schema 36；build 是编译所用 Git 短提交，存在已跟踪改动时带 dirty；`git archive` 导出的源码没有 `.git`，改从 `native/terms/source-revision.txt`（export-subst 写入的导出提交）读取，不带 dirty（2026-10-12，#56）；其他非 checkout 构建可为空。runtime 为编译器版本；Unix platform 来自 uname，其他平台报 Rust 目标 OS/架构。python 为 null。libraries 只列识别要用的两项：「识别组件」（识别 worker 存在时为「可用」）和「ONNX Runtime」（worker 会加载的那个文件存在时为「已安装」：`ILLUSTASH_ORT_DYLIB`，否则会话资源根下的 `onnxruntime/`，与 worker 同一规则 `app_resources::ort_runtime`），缺失为 null、页面显示「未安装」；只判断文件在不在，宿主不加载 ONNX Runtime，也不报路径或版本（2026-10-15，#148）。页面使用 runtime 展示诊断。

## 原生账号与桌面登录桥

凭据平台边界为 `platform_credentials::{read,write,remove}`，按账号类型存取；共享 `support` 解析保留 `ILLUSTASH_APP_SUPPORT` 覆盖优先，账号存储接口不要求主目录存在；这不豁免整个宿主其他配置的主目录要求。`app_accounts` 保留类型校验（SauceNAO key 为 40 位十六进制）、账号锁、元数据及过期/网络授权业务；钥匙串 API、DPAPI 与私有文件权限只在平台模块内实现。钥匙串路径不求值文件目录，禁用开关仅在 macOS 切换到原文件存储。原读取空白裁剪、文件读取错误回退为空、解绑缺失忽略及错误文案保持不变；macOS 私有实现位于 `platform_credentials_macos`，与公共凭据模块使用同一 accounts 归属，通过 Security framework 的 `SecItemCopyMatching/Update/Add/Delete` 直接存取文件式 generic-password，服务名 `Illustash`、账号名及标签 `Illustash · <kind>` 不变；密钥只经进程内 CFData 传入，不启动 `security` 子进程。写入与删除先解析首个匹配项的 persistent reference，仅操作该项，避免对搜索列表内全部同名项批量修改；更新已有项保留原 ACL，旧 CLI 创建的条目可继续读取，不迁移到 Data Protection 钥匙串、不要求重新绑定。已有访问控制仍由系统执行。Rust 测试构建拒绝无显式句柄/搜索列表的钥匙串查询，搜索列表夹具也不能向默认钥匙串新增条目；该限制不改变发行构建。隔离合成钥匙串覆盖旧 API 建项、新 API 读写、旧 API 读回及删除，不能替代真实用户授权验收。程序回退仍使用原凭据存储方式与下述 Windows 重绑定规则。

原生宿主的 `GET /api/accounts` 与 `POST /api/accounts/*` 由 `app_accounts` 完成：Pixiv 粘贴绑定、登录窗口、连接、检查、解绑；SauceNAO 保存和解绑；X 归档导入。账号元数据与凭据仍属于整个应用，macOS 使用原 Illustash 钥匙串服务；Windows 使用当前用户的 DPAPI（非机器级、禁止交互提示），凭据密文放在应用数据目录 `accounts/<kind>.dpapi`，按账号类型绑定用途；macOS 禁用钥匙串或其他 Unix 平台使用权限 0600 的原 accounts 文件；响应只含绑定、身份、检查时间和失效事实。Windows 不自动读取或迁移旧明文凭据，需在设置页重新绑定；旧文件保持原样，新凭据加密成功后原子替换密文，解密失败明确报错且不回退。解绑删除密文；回退旧版本不会读取密文，应重新绑定，不能把密文改名为旧明文文件。窗口登录只记录 `via: app`，替换粘贴凭据，解绑清窗口数据；首次失效隔 5 秒复查，网络失败不记为登录过期。

`app_accounts::pixiv_binding()` 只读 Pixiv 元数据，不读钥匙串、SauceNAO 或桌面；`mark_pixiv(who,problem,detail)` 保留原身份/via，更新检查时间及失效事实，HTTP check 与溯源消费者复用同一合并逻辑。账号凭据及元数据操作共用进程内互斥和原子发布，避免并发覆盖其他账号；X 的包级 SQLite 导入不持账号锁。`app_accounts::secret` 供内部消费者读取凭据；`network_state(authorized)` 返回非敏感的授权/窗口/身份/失效事实，授权由调用方明确传入。`pixiv_get(path,authorized,session,in_app)` 未授权先拒绝；app 登录只走原桌面 token 与 127.0.0.1 桥，不回退到 cookie 或匿名登录。账号页面的显式校验只授权该次只读请求，保存 key 不授权名称、索引或溯源后台联网。名称模块与其他消费者通过 `model_credentials(&Connection)` 读取既有模型配置及 legacy 字段回退，敏感结构不实现 Debug/Serialize。每个直连请求上限 25 秒，桌面会话/登录 30 秒、窗口 fetch 120 秒；不跟随重定向。账号身份校验沿旧 Net 规则保持请求起点间隔至少 2 秒，短暂网络失败最多三次，拒绝登录及错误 JSON 不重试；后台消费者自行协调单次 pixiv_get 的节流、预算与任务取消。`ILLUSTASH_PIXIV_BASE` 仅接受带端口的 127.0.0.1 HTTP 合成替身。

X 只读取 ZIP 或解压目录的 `data/following.js`，限制列表为 64 MiB，校验数字账号 ID，按原 source/system/key 唯一键写 leads。复用 provenance v5 schema，版本比程序新则拒绝，旧 ref_sigs 补 colour；schema 与 leads 在同一事务提交，不写人工历史、核心投影或后处理事件。HTTP 请求固定 canonical 包路径与会话 generation，导入/账号校验为有界同步请求，切库和正常退出等待在途请求；旧 CSRF 请求不进入替代包，即使两包配置 ID 相同。归档选择复用原生窗口且释放会话锁，选择后重新验证 generation；取消不写库。无需启动研究 worker 或新增后台任务。

## 公共后台任务与会话结果边界

`app_tasks::TaskIdentity::new(&Library, generation, session_token)` 捕获打开时的 canonical 包路径、library_id、generation、会话令牌摘要及随机 task_id，可作为 serde 结果信封；相同 library_id 的包副本和重新打开的会话分别识别。`Library` 只把用户打开时的路径（含别名）用于显示和最近图库登记（`display_path()`）；数据库、源库、配置、特征、缩略图、备份与出处等包内文件一律经打开时固定的 canonical 路径（`instance_path()`）访问，HTTP 会话、设置、来源、导出、升级与后台任务都不跟随显示路径的符号链接重新定位。重新以该别名打开属于明确切库，换新 generation 与令牌（2026-10-11，#22）。文件夹 Syncer 使用这一身份和公共 Task，保留既有 GET/POST sync 契约；导出继续复用已经交付的取消/等待边界。

`Task::start(identity, timeout, work)` 启动合作式任务，提供 running/outcome、有限时长 wait 和 stop；wait 超时保留任务所有权，stop/Drop 取消并 join。任务持有 Library 快照，不持有 Host Arc；切库先验证并登记替代包，再停止旧任务，退出停止并等待消费者。`TaskContext::identity()` 返回只读身份；check 检查取消与截止时刻，cancellation 提供已有业务循环需要的取消标记。内存计算不能强制中断，业务循环须及时检查取消/超时，I/O 须设置自身超时。

`TaskContext::receive(identity, apply)` 在同一锁内比较完整信封并接收一次结果，取消或任务结束后拒绝迟到结果；apply 失败不占用接收名额。取消与已进入的结果提交串行，取消返回时没有在途 apply；apply 不可在自身接收锁内调用 cancel/stop。公共接收只负责会话与单次任务边界，持久幂等键必须由业务消费者与正式结果在同一事务保存。扫描继续用 sync_runs 将崩溃遗留 running 标记为 interrupted，源 journal 与应用摘要恢复两库提交边界；pending 后处理事件不被公共模块提前确认，各业务负责重试和恢复。

文件夹同步取消的持久状态取决于取消点（2026-10-03）：哈希/源事务提交前观察到取消时回滚 source；源事务已提交而应用刷新尚未提交时，保留源结果供后续刷新重试，不能承诺 source 不变。实际 run_with 失败且取消已置位才记录 interrupted；已成功结束的 done/needs_recognition 不因后续切库或正常退出改成中断。同步任务固定旧包实例，切库先取消并 join，再替换会话并发布新 generation；两库及最终 sync_runs 状态记录仍是各自的原提交边界。

取消回归用实际 run_with 的进度回调，在哈希完成且源事务提交前、源已提交且应用刷新前分别设置 ready/cancel/release/completion 闸门，验证应用服务切库、shutdown 和 Drop 的取消、等待、源提交前全表回滚及源提交后保留、人工历史及待处理事件保持、同 ID 的新物理包不变与旧 generation 结果拒收；完成路径先等待真实任务结束，再逐表核对正常收尾保留终态。HTTP 回归先观察持久终态，再核对切库/退出后的完整 typed cells、rowid 和重启重扫。不再用合成文件大小推断取消阶段；测试闸门仅在 cfg(test) 内，生产行为及 golden 不变。

后台任务的写连接（2026-10-21，#275）：同步及其文件夹、照片扫描，识别导入和名称获取的写连接遇到别的写入方时等 5 秒（`app_library::TASK_WRITE_WAIT`，与 `crate::configure` 给所有连接的忙等相同），原来只等 250 毫秒（名称 2 秒），等不过研究在写锁内算指纹的那一步。只有这些任务等它，页面的写请求用自己的连接。持写锁的一方现在都很短：研究接收检查不在写锁内算指纹、等锁时为源库写入方让路（见「原生学习、聚类与相似研究协调」），名称获取不在应用库写锁内读源库。停止在语句之间生效，桌面外壳给宿主 10 秒退出，一次等待在这之内。仍然忙就照旧失败，由下次自动同步或名称重试再来；同步失败时日志写明失败的步骤、它写的库和 SQLite 原始错误。

Unix `TaskContext::process(&mut Command)` 将可选 worker 放入独立进程组，轮询取消/超时，正常或异常退出、父进程先退出、任务取消及栈退出均杀死剩余进程组并 wait 直属子进程。stdin/stdout/stderr 默认关闭，结果通过调用方指定的文件交付，再经 receive 验证；worker 不得脱离进程组。Windows 使用私有 kill-on-close Job，挂起创建后归属并恢复，见 [独立计算包](BACKEND_TASKS.md#独立安装与可选计算包)。Unix 进程组收尾不宣称 SIGKILL 宿主时能执行析构；Windows Job 在宿主退出时由 OS 关闭持有句柄并清理后代。

### 导入任务

2026-10-12（#88）起，图库归档导入（`POST /api/libraries/import`）和识别结果导入（`POST /api/recognition/import`，含 `resume`）是 `app_imports` 后台任务。请求在会话锁内校验令牌、generation 和必需参数后启动任务，立即返回 `{"started":true,"task":<状态>}`；`GET /api/imports` 读状态，`POST /api/imports/cancel` 请求取消。应用同一时间只运行一个导入，两类共用，停止中的也算；其余启动返回 400「已有导入正在进行」。

状态的 `status` 为 running / stopping / done / error / cancelled，另含 `kind`（archive / recognition）、`id`（task_id）和 `file`（输入的文件或目录名）。运行中的 `stage` 是实际代码阶段：归档为 reading（读目录与配置、空间检查）、unpacking（`done/total` 为已写出字节与声明总字节）、cleaning（清除分享设置）、checking（按打开图库的校验）、publishing（发布与登记）；识别结果为 models（模型指纹）、reading（`done/total` 为已读文件数与 3）、verifying、writing（`done/total` 为已处理资产与总数）、committing（源事务提交）、projecting（应用索引与后处理通知），`resume` 只有 projecting。没有可数单位的阶段不给数字，也不估算百分比。结束后，done 带 `result`，内容与改动前同步响应的对象相同；error 带 `error` 和改动前的 HTTP 状态码 `code`（400/409/500/502）；cancelled 带 `reason`（「导入已取消」「图库已切换，导入已取消」「程序正在退出，导入已取消」），并保留停下时的阶段。状态属于应用而不属于会话：切库后的新页面仍能读到上一个导入为何停止，下一次导入开始时替换。

取消按钮、切库和退出都在检查点停止导入：归档每个解压块（LZMA 每次写出）和 SQLite 清理的进度回调；识别每 64 KiB 模型指纹、每个文件、每行标签、每个资产。发布点经 `TaskContext::receive` 与取消串行：识别是源事务提交，归档是保留新目录。取消先到就不提交，归档删除本次创建的目录且不登记；发布已经开始时，取消等它完成，结果如实记为 done，识别的后续投影被取消时按原规则记 pending 留待恢复。切库在会话锁内只发出取消，不等导入结束；退出先拒绝新导入，再给运行中的导入最多 5 秒清理，到时不再等待（例如阻塞在不返回的读取上）。识别导入运行期间，同一会话拒绝启动同步和索引 apply/drop。#77 的保证不变：不长持会话锁、输入大小与解压资源有上限、发布前校验。

设置页「图库」卡片显示当前导入的阶段和真实计数，运行中提供「取消导入」，结束后显示结果；轮询按序号和会话令牌丢弃迟到响应（#34）。识别结果导入没有入口按钮，经 HTTP 启动的识别导入同样显示在这里。回退到改动前的版本时，两个导入恢复为同步请求，没有阶段和取消入口；包格式与数据库不变。

## 原生来源同步与应用刷新

原生 `POST /api/sync` 对配置中的 Photos/文件夹混合来源按配置顺序启动后台盘点，重复启动返回 `started:false`；角色索引正在写入或 WD 扩展检查在跑时，请求和打开图库时的自动同步都排在后面，返回 `started:false, queued:true`，状态为 `waiting`，挡着的任务结束后由恢复检查开始（2026-11-18，#607）。`GET /api/sync` 提供进度、错误与最近八次记录。扫描跳过隐藏路径、`.pilib` 子目录和非图片文件，未变化文件不重新哈希；MD5 相同的移动/改名按原顺序保留资产身份，删除保留源记录并标记 trashed，恢复复用身份。同路径换图保留资产身份和人工裁决，清除源标签及 embeddings；损坏图片作为零尺寸资产盘点。无识别 worker 时仍导入文件夹基础信息，Photos 保持已有 rating 结果才进入应用库；已有结果正常浏览，待办明确显示 `needs_recognition`，不将未识别图片计作识别成功。macOS 没有给 App 照片图库的访问权限时（「不允许」，或更新后授权失效），打开 Photos.sqlite 被系统以 EPERM 拒绝（普通文件权限不足是 EACCES，仍按原来的数据库错误处理）；同步失败状态另带 `problem: "photos_permission"`，message 仍是原错误，不进同步记录、不改表。任务面板「等你确认」和首次引导的问题条据此说明到「系统设置 › 隐私与安全性 › 照片」打开 Illustash、退出重开后再同步，并给「打开系统设置」按钮；桌面外壳在窗口外只多放行这一个系统设置地址（`desktop/src/server.rs` 的 `PHOTOS_PRIVACY`）（2026-10-19，#202）。

盘点 CLI 是 `folder-scan` / `photos-scan --db <source.sqlite>`（标准输入为单来源 JSON）和 `library-scan --library <显式.pilib>`（盘点全部来源并刷新应用）；HTTP 使用同一模块。只有确认已不存在的文件才算移除：列不出的文件夹、读不到的目录项或元数据、算哈希时读取失败的文件记为暂时读不到，其中已有的资产原样保留，不标 trashed，其余区域的新增、移动与移除照常提交；扫描结果与同步终态带 `unreadable` 计数，界面提示已保留、恢复访问后再同步（2026-10-11，#43）。已移除的文件写回原路径时（内容相同或不同、大小或时间改变），本次扫描即恢复为存活，不需再同步一次（#44）。新资产/移除与内容变化复用角色 import 核心，路径更新、正式投影、内容及标签摘要和 `imported` 事件在应用事务中提交。扫描的 imported 事件持久化保留供研究消费者调度；正式识别导入的通知由结果包导入器确认，不代表研究算法完成。建库不扫描；服务启动和切库后按 startup.sync 自动启动。打开时 startup.sync=false 则本次会话关闭同步，重新启用偏好后须重新打开图库。`--no-sync` 关闭本会话同步，`--no-auto-sync` 仅关闭自动触发、保留手动同步；冻结/测试图库始终拒绝同步。有识别 worker 时总是在本机识别全部待识别图片；缺席 worker 显示 needs_recognition。原本机识别上限 `startup.local_limit`（超过即 needs_windows）已按用户决定删除（2026-10-12，#100）：已存的旧值读取时忽略，下次保存设置时去掉，请求再带它按未知设置拒绝；历史同步记录里的 needs_windows 原样保留。`--sync-limit N` 只限制每次识别数（0 为全部），不限制扫描。

Photos 适配只打开显式配置包内的 Photos.sqlite 只读连接，收录 App 与 preset 来自来源 apps。盘点包含来源统计、Photos UUID/链接、范围、删除/恢复及导入路径；优先本地 derivative，其次 original，均不存在时保留可重试路径。删除检查按 source_id 隔离，Photos 扫描不删除文件夹资产；不同来源碰撞同一资产标识时拒绝替换。Photos 不写应用库，正式刷新复用同一原生调用链；没有真实图库或平台权限验收。

可选识别 executable 由 `ILLUSTASH_RECOGNITION_WORKER` 显式配置，调用参数为 request.json 与 response.json 路径；请求 format 为 illustash-sync/2，包含完整 TaskIdentity、assets[{id,path,input_sha256}] 和独占 output 目录。输入内容绑定与预处理版本以 [v2 结果包](RESULT_PACKAGES.md) 为准；worker 只读请求资产并在 output 下生成结果包，响应为 identity、packages[目录路径]、failed[{uuid,reason}]，必须完整覆盖本次可读待办且无重复或越界资产；不可读原图由 Rust 记失败，后续同步补跑。输入字节与请求哈希不匹配时返回 `input_changed`，宿主报告同步冲突，留待下次同步重试；不把它记成损坏或 iCloud 未下载。其余逐张失败分两类（2026-10-13，#112），按出错的环节判定、不看错误文字：原生生产者先把文件整个读进内存，读取出错为 `unreadable`（权限、iCloud 占位、磁盘拔出等，下次同步重试）；读完后在内存里解码出错为 `undecodable`（截断、损坏、不是图片或格式不支持）。没有 `reason` 的旧 worker 输出按 `unreadable` 处理，行为与原来相同；未知取值按协议错误拒绝。`undecodable` 的图记进应用库 `prefs` 的 `sync_damaged`（研究输入不读这张表，不改 schema），每张记下当时文件的大小、修改时间与源库 `stable_hash`；之后的同步在选待识别图时先比对，三者不变就跳过、不占 `--sync-limit` 名额，任何一项变化（如换成正常图片，扫描按 #27 重算内容摘要）就重新识别，成功后去掉记录；已记损坏的图此刻读不到时按读不到计、记录保留；没有待识别图时清空记录。同步终态的 `failed` 与同步记录的 `failed` 只数读不到的图，`damaged` 另数已损坏而跳过的图，message 分别写「N 张暂时读不到（可能还在 iCloud），下次同步重试」和「M 张图片已损坏或格式不支持，留在图库里但不识别；文件改动后会重新识别」；`/api/sync` 有损坏记录时另带 `damaged{count,paths}`（按路径排序的前 50 个原图路径），任务面板在同步项下列出这些位置。回退到旧版本时 `sync_damaged` 被忽略，损坏图恢复为每次同步重试。响应上限 4 MiB，结果包由现行导入器验证；worker 不直接写两库。Unix 子进程复用公共进程组收尾。同步任务的期限按进展计算（2026-10-06，#3）：扫描进度、worker 识别进度、结果包预检和每批导入都算进展，连续 `ILLUSTASH_RECOGNITION_TIMEOUT_MS`（默认 600000，即 10 分钟）没有进展才判超时；整个任务另有 `ILLUSTASH_RECOGNITION_MAX_MS` 总上限（默认 24 小时）。不写进度文件的 worker 在识别期间没有进展信号，等同于原来的 10 分钟总期限。取消与超时分开报告：状态和同步记录的 message 为「任务已取消」「任务超时：N 没有进展」或「任务超时：超过 N 上限」，界面在「同步已中断」后显示原因。其他任务（研究、索引等）仍是从开始计算的固定期限，报错文字同样区分取消与超时。取消、超时或切库后旧信封拒收，同 library_id 的包副本仍分别固定实际包路径；成功结果调用 app_recognition::import_package，补跑先调用 resume。多结果包各自保留源/应用提交边界，中途失败不撤销已提交包，下次恢复/去重。done 表示本次受 per_run 限制的待办已处理，剩余待办留到下次；失败与 iCloud 提示不冒充成功识别。运行最终记录提交后才发布完成状态。Windows 进程树及包内识别生产者见 [独立计算包](BACKEND_TASKS.md#独立安装与可选计算包)。

**识别顺序**（2026-11-12，#360）：待识别的图按保存时间从新到旧送去识别（源库 `assets.added_at` 倒序，同一时间按资产 ID），与图库默认的浏览顺序（`a.added_at DESC,a.uuid`）相同。新图库第一次识别要很久（见[性能](TERMS_PERFORMANCE.md)第 6 节），这样首页从上往下最先认完，中途关掉应用时已经认完的也是最近保存的那部分；此前按资产 ID 排，资产 ID 是随机的，与保存时间无关。`added_at` 在资产第一次入库时写入，之后不改：Photos 来源取图片加入照片图库的时间（`ZADDEDDATE`），文件夹来源取文件的创建时间，文件系统不提供时取修改时间；都是本地时间 `YYYY-MM-DDTHH:MM:SS`。没有保存时间的排在最后：Photos 里缺这一项的图写入空字符串，旧源库里的 NULL 排在空字符串之后，彼此之间按资产 ID。`--sync-limit N` 在去掉已知损坏的图之后截取，留下的是最近保存的 N 张，其余等下一轮。顺序只决定先认哪些图、每 25 张一批各包含哪些图，不改变每张图写入的识别结果；[离线识别结果导入](#离线识别结果导入)按结果包自己的 manifest 处理，不经过这份列表。

**识别进度**（2026-10-06，#2）：宿主通过 `ILLUSTASH_RECOGNITION_PROGRESS` 把本任务 output 目录里的 `progress.json` 路径交给 worker，文件随 output 一起清理。worker 写临时文件再改名，整体替换内容 `{format:"illustash-recognition-progress/1",task_id,stage,total,processed,failed}`：`preparing` 表示正在加载模型与快照；`recognising` 时 `processed` 是已处理完的图片（含解码失败，`failed` 计在其中），不表示已导入。写入失败不影响识别。宿主等待 worker 时至多每 200 ms 读一次，只接受本任务 task_id、total 等于本次可读图片数、计数不越界也不倒退的值，其余忽略。同步状态的 `done`、`failed` 为 Rust 已记的不可读原图加上 worker 计数，并据此给出 `ms_per_image` 估时。worker 退出后以响应为准：状态转为 `importing`，`stage` 先是 `verifying`（结果包预检）再是 `importing`；导入进度单独记在 `import_done`/`import_total`，`done` 保持识别总数，不会倒退成导入数。不写这个文件的 worker 保持原行为，计数到导入时才变化。

**边算边导入**（2026-10-06，#3）：worker 每写完一批（结果包与角色 sidecar），最后写 `complete.json`（`format:"illustash-recognition-batch/1"`、task_id、该批资产及 manifest、sidecar 的 SHA-256）。宿主等待 worker 时至多每 500 ms 查看一次，只导入有完成标记、身份与摘要一致、资产属于本次待办且未出现过的批次；每批照常由 `prepare_worker_package`/`import_worker_package` 在自己的源库事务里校验与导入，经 `TaskContext::receive_part` 接收，取消、超时或切库后拒收。导入出错即终止 worker。worker 退出后只导入尚未导入的批次，再核对全部批次与失败项恰好覆盖本次待办。中断时已导入的批次保留，下一次同步只识别剩下的图；整轮不再是全进全不进（用户 2026-10-06 接受）。原生适配器路线的完成标记必须带 sidecar；不写完成标记的 worker 仍在退出后统一导入。同步进行期间研究任务不启动（`Research::recover` 收到同步忙碌），各批导入留下的 `imported` 事件在同步结束后由一次研究运行合并处理。

源库提交与应用刷新是两个恢复边界。源库提交前原子发布包内 `source.scan-pending.json`，记录同路径换图的资产和内容摘要；应用提交后删除。应用 `meta` 中的 `folder_hash:`、`folder_labels:` 摘要使提交后重试幂等；源已提交而应用未提交时，下次扫描仍能恢复内容变化。旧 WD 分级/题材证据用 `folder_stale_wd:<证据id>` 标记失效，正式投影与 WD pending 预检过滤该标记；证据行及人工历史保留。同结果的成功识别重交会重新激活对应旧证据。角色机器证据仍通过现有空覆盖批次失效。同一应用事务内，换图资产的相似机器缓存一并删除：它的 `image_sigs`、含它的 `relations` 与 `similar_retry`、`similar_paired` 标记，并按剩余关系重建 `duplicates`、`similar_groups` 与折叠；随后的 imported 事件让相似计算只重算这些图，内容未变的图沿用原结果。溯源缓存另记每张图的内容摘要（`content_versions`），每次溯源运行开始时与源库 `stable_hash` 比对，变化的图删除其 `lookups`、`image_sources`、`image_artists`、`marks` 后重算，运行后交付的快照替换旧内容算出的溯源证据；首次见到的图只记录。这些都是机器缓存，人工标签、裁决与历史不动（2026-10-11，#27）。签名、关系或像素哈希算法改变时，随该改动提供一次显式迁移清空对应缓存并全量重算；缓存里不另存算法版本，算法未变的升级不清缓存。切库与退出设置取消标记并等待旧任务结束；哈希分块与两库提交前检查取消，任务始终持有原包路径，未提交的扫描回滚。

### 原生角色索引维护

原生 GET/POST `/api/index` 提供 check/apply/drop/watch、进度、关注作品、候选与最近六次记录。watch 沿用 v2 作品 ID、pending_tags 与 20–5000 的 min_posts，按图库存在应用数据库 `prefs.index_watch`（#205）；没有记录时从共享 `char-patch/watch.json` 只读推出初值，旧 tag 配置用本库已确认的作品链接解析。检查保持共现 ≥0.5、最低帖子数、本体筛选、已有角色/元数据/拒绝记录排除，以及每作品候选数；本图库启用的数据包（现在是 WD 角色扩展，按图库保存的启用包摘要核对本机包，再读它的标签表；包不在本机时不算）能认出的角色也算已有（#561），每个关注作品的「数据包覆盖 · 自己补过 · 还缺」另存在 `coverage.json`，由 `GET /api/index/coverage` 返回，`/api/index` 的回复不变；空检查也记录 checked_at。桌面手动检查每次确认后以 `allow_network:true` 仅授权当前任务，不写入偏好或名称授权；后端拒绝缺少确认的手动 check。命令行进程明确传入 `--index-network` 时，启动及周期检查才读取 startup.index_check 并在七天到期重查；`--no-index-check` 关闭自动触发，手动检查保留，`--index-check` 可显式恢复本次自动触发。冻结图库不执行检查或恢复。

文本查询复用名称 Source 的主机节流、重试与 25 秒单次超时；名字和索引都不设流量预算（用户 2026-10-21 决定，#287），添加前按钮写明预计下载量。`--index-network` 只授予本次进程 Danbooru 索引查询和公开参考图下载，保存凭据不授予联网。添加角色的下载量确认只授权该次 apply，进程重启不延续请求授权。替身地址沿用 `ILLUSTASH_COPYRIGHT_BASE` 的带端口 loopback 限制；参考图只接受 Danbooru HTTPS 或本机合成 HTTP，禁重定向/URL 凭据，单图最多 2 MB（超过的跳过）；图片先写临时文件再改名。失败的单张参考图可跳过，取消与来源校验失败终止；可用参考不足仍由计算结果标为 rejected。每个角色的参考图列表（`<id>-refs.json`）写好就算下完，同一任务再次运行时跳过它，并复用中断角色已下载的图。重启或 60 秒自动重试没有联网授权、又还有角色没下完时，任务不报错，停在 `paused`（日志 `index-job.json` 记 `paused`，自动重试不再碰它），任务面板显示「添加角色已暂停：继续下载参考图需要你确认联网」：`{"action":"resume","allow_network":true}` 确认后接着下；暂停期间图库可能已同步，继续时重新做源库、特征和相册映射快照并更新 before 指纹，已下载的参考图保留；`{"action":"discard"}` 删除这个还没写入任何东西的任务。暂停时检查和别的添加都拒绝，先继续或放弃（#287）。没有实际账号、图库、Photos 或真实网络验收。

可选 executable 由 `ILLUSTASH_INDEX_WORKER` 显式配置，参数为 request.json、response.json；没有 worker 仍可检查、编辑关注列表和浏览，apply/drop 明确拒绝。请求 format=`illustash-index/1`，含完整 TaskIdentity、源输入摘要、action、rows、reason 和隔离 review。Rust 备份源库，准备角色参考、游戏名称、相册映射、图库特征和公开参考图；macOS/Windows 安装适配器 `illustash-index` 在隔离快照中调用 `index_compute`；计算不联网、不打开正式两库。响应回传 identity/before、characters 对象、aliases 三元组和 labels `[uuid,char_id,conf]`；上限 64 MB。进度文件也须匹配当前身份，只接受 fetch/clean/build/scan 及有界计数。Rust 校验版本、完整身份、源输入、角色/资产范围、重复项和置信度；抢救后仍有标签或向量、但来源资产行已经丢失的图，扫描保留原先的认领、置信度和写入来源；明确撤回角色时移除它的认领。宿主拒绝给孤立图新增或改写认领的结果。超时按「多久没有进展」算：每下完一张参考图、每写完一个角色的参考图列表、计算进度每变一次都算进展，默认 600 秒没有进展才超时，`ILLUSTASH_INDEX_TIMEOUT_MS` 可设毫秒值；整次运行另有 `ILLUSTASH_INDEX_MAX_MS`（默认 24 小时）上限（#287，原为从开始算的 600 秒总时长，几百个角色必然超时）。失败时状态的 message 只写原因，不带「不合法：」等内部类别，`kind` 写 check/add/drop，任务面板据此写「检查失败 / 添加角色失败 / 移除角色失败」。

正式写入由 Rust 完成：源角色、别名、char_ext 与 native_index_runs 收据同事务；char_ext 保留 char_index 来源和本次模型/运行标识。源提交后安装所选参考、原始特征、拒绝记录与重建锚点，再以现有 reindex 命令更新变化资产的角色证据/名称/投影。应用完成键、全范围 names 请求和 index_changed 事件同事务，名称由现行消费者接续；index_changed 交给现行研究消费者重载锚点并再聚类；未配置研究 worker 时保留 pending，基础宿主不将算法未运行写成完成；人工历史不改。最后更新候选与历史，删除 index-job.json 与任务目录后才报告完成，轮询看到完成时收尾已经结束（2026-10-13，#123）。完整状态和进度两个入口均在线程退出、共享锁释放后才公开 `checked`、`done`、`paused` 或 `error`；内部终态已写下但线程尚未退出时显示 `cleaning`，终态与下一次操作的再入边界一致（2026-11-15，#505）。共享索引文件（`char-anchors.npy/.txt` 与 `char-index/*`）先全部复制为同目录临时文件，复制失败或空间不足时正式文件不变；之后在同一发布代号内逐个原子改名替换。代号记在共享目录的 `index-generation`，替换期间为奇数、完成后为偶数；识别快照加载、识别提交前复核与研究输入指纹只接受前后同一偶数代号的读取，否则短暂重试，代号停在奇数（发布中途崩溃）时明确报「角色索引正在更新」，直到日志重放完成（2026-10-11，#47）。源提交与应用提交不宣称跨库原子；包内 index-job.json 记录可重放任务，源收据及应用 meta 完成键避免重复正式导入，独立历史收据避免完成记录重复。失败留待恢复，重启或 60 秒周期补跑；源输入已改变且尚未提交的结果作废，重新检查/申请。未完成任务只能重试同一选择，不能悄悄改做另一动作。

索引写任务与同步、显式识别导入互斥。索引写任务准备或运行时收到的同步请求排到任务结束。只读检查可与同步并行。共享索引使用 OS 文件锁，崩溃释放锁。备份分批检查取消，源输入指纹沿用原有的行顺序和编码逐行计算。备份推进和读行都计入任务进展。网络等待有单次超时。切库和退出取消并等待 Task 及进程树，固定 canonical 包实例；重新打开原包的恢复使用新身份，不接受旧会话结果。只保留最近五份原生源库/锚点备份，完成后清理隔离计算目录；原始参考特征与拒绝记录保留用于后续计算。Unix 进程树沿用公共 Task，Windows 使用私有 Job 与文件锁，见 [独立计算包](BACKEND_TASKS.md#独立安装与可选计算包)。未完成的原生任务由原生宿主恢复，不能用别的工具绕过恢复边界直接写正式源库。

索引计算模块 `index_compute` 在准备好的隔离 review 中执行 CCIP 参考特征、clean、角色/别名构建和 scan，不联网、不写正式数据库；使用既有 `CcipSession`，只加载 `model_feat.onnx` 和显式 CPU runtime，不加载 WD 或维护学习 Features 缓存。App Index 继续持有授权、任务身份/指纹、共享租约、校验、两库分阶段提交与恢复。原始补丁特征保留在 raw，拒收记录和人工 drop 理由保持；空角色集合重建有效 `(0,768)` 锚点并清除旧认领。

2026-10-04，索引分位数保留现行 macOS NumPy 2.5.3 标量 `np.percentile(dd,q)` 口径：f32 输入按 f32 插值、f64 输入按 f64 插值；与学习单独批准的 float64 p90 不混用。NPY 原始大小端、C/F 布局及 f32/f64 归约先保留，输出锚点/清洗特征才转 f32；矩阵运算复用 Platform 的系统 Accelerate。索引与识别别名矩阵均保留旧算法 1024 行分块，64 行会使尾部单行 GEMV 翻转 `.01` 边界判断；取消仍在归一化、逐别名行和 BLAS 前后检查，单次不可中断 BLAS 最多 1024 行乘全体锚点。scan 保留最近 9 项 argpartition 后排序、先去实体重再判断阈值、补丁 p75/上游 p90 和 4000 列分块；正式标签置信度仍取四位小数 CSV 距离，scan CSV 保留旧 `csv.writer` 的 CRLF 行结束字节。等频 core tags 的旧哈希集合没有跨进程固定顺序，现行计算以标签字典序打破同计数平局。其他平台 portable 数值尚未取得此项实际验收，不能据 macOS 证据宣布跨平台原生替代完成。macOS 安装 dispatcher 的 index 入口直接进入原生计算，无需 `ILLUSTASH_WORKER_PYTHON` 或可发现的 Python；模型目录与 tags 沿用统一资源解析，CPU runtime 使用 `ILLUSTASH_ORT_DYLIB` 或资源根下 `onnxruntime/libonnxruntime.dylib`，`--check` 实际加载 CCIP 会话。2026-10-05 起 Windows 安装 dispatcher 的 index 入口同样走原生计算（CPU runtime 默认资源根下 `onnxruntime/onnxruntime.dll`）。完整安装包的资源装配与平台验收范围以 [安装参考](BACKEND_TASKS.md#独立安装与可选计算包) 为准。

Windows 开发计算候选复用 #151 的固定 ILP64 OpenBLAS 加载器：显式绝对 `ILLUSTASH_RESEARCH_OPENBLAS_DLL`、登记分发指纹和模块文件身份检查保持；索引 f32/f64 的 DOT 与 C/F 原始布局 GEMV 均走该后端，不用顺序累加替代，不将 F 布局转成另一种矩阵形状。缺配置明确失败，不静默使用 portable。安装后的 worker 在启动器未提供 `ILLUSTASH_RESEARCH_OPENBLAS_DLL` 与 `ILLUSTASH_ROOT` 时，按安装位置解析 `resources/openblas/openblas.dll`，学习、索引、识别三类入口一致；`--check` 也会验证该后端身份。其他平台算法未改。2026-10-04 用户批准最近 9 项的并列距离统一使用既有 NumPy 1.26.4 标量 argintroselect/aquicksort 顺序；Windows NumPy 2.5.3 的 SIMD 并列项选择及顺序会改变，可能改变 scan 去实体重后的候选与声明。距离数值、阈值与非并列项顺序保持原合同。

**跨 CPU 重算（#483）**。计算沿用各机器的 BLAS 后端。Windows 的固定 OpenBLAS DLL 运行时按 CPU 选择内核，同一输入的锚点和溯源缓存分数等保存值可能出现末位差异，跨 CPU 不保证逐位一致。打开或导入图库时沿用已保存的机器认领和人工裁决。新识别即使读取同一组已保存的锚点，也会在本机重算别名，`0.01` 边界附近的样本可能归入不同角色。主动执行索引 apply/drop 会重新扫描全库，以新结果替换 `char_ext` 机器认领并刷新变化资产，继续保留人工裁决。计算沿用既有阈值。识别结果导入按内容指纹去重，指纹不同时拒绝覆盖仍有效的结果。本机宿主复算认领并严格校验原生 worker 的角色 sidecar。见[识别结果包契约](RESULT_PACKAGES.md)。

显式模型验收入口为 `python -m unittest tests.platform_index_models -v -b`，不进入普通 discover。开发计算候选的 `tests.test_native_index_compute.NativeIndexActualCcipTest` 与安装适配器的 `tests.test_native_index_production.InstalledIndexAppTest.test_actual_ccip_download_to_installed_adapter_without_python` 同样只按名字显式运行，普通 discover 不收集；显式运行时缺模型、runtime 或独立旧 raw 直接失败。必须提供当轮 `ILLUSTASH_TERMS_BIN`、`ILLUSTASH_INDEX_ADAPTER_BIN`、`ILLUSTASH_RECOGNITION_COMPUTE_BIN`、`ILLUSTASH_NATIVE_ROOT`，以及授权的 `ILLUSTASH_INDEX_INSTALLED_MODEL`、`ILLUSTASH_INDEX_INSTALLED_RUNTIME`、独立旧算法的 `ILLUSTASH_INDEX_REFERENCE_RAW`；缺输入直接失败。该入口只安装索引所需 CCIP 资源，用八张坐标生成 RGB 图片检查原生参考推理、正式发布、后续识别认领、缓存重算与 drop；子进程空 PATH，无 Python workers、WD 或外部模型/runtime 路径覆盖。补丁识别阈值按旧 `claim_thresholds` 取 p75。缓存重算保留角色元数据和 raw，仅删除派生的清洗矩阵；公共 apply 仍只接受最近检查里的缺失角色，不新增强制重算接口。该窄安装布局不代表签名 App 或全功能安装包，其他平台继续按各自门槛验收。

### 离线识别结果导入

原生 `POST /api/recognition/import` 与 `recognition-import` CLI 共用 `app_recognition`：校验 v1 manifest、目标图库、模型/标签指纹、资产范围、摘要、NPY float32 维度与有限非零向量、合法标签后，单源事务写机器标签/特征和去重账本。源提交、WD 核心证据提交、应用正式导入、后处理通知各保留实际边界。应用恢复标记和 imported 事件同事务提交，resume 不重复正式投影或人工历史。基础宿主无研究 worker 时通知成功 no-op；研究 dispatcher 失败留 pending 和错误，不将算法未运行写成已完成。契约、原生接口和回退细节统一见 [RESULT_PACKAGES.md](RESULT_PACKAGES.md)。显式 HTTP 导入在会话锁内校验 generation 与 CSRF、捕获当前包后作为后台任务运行（见[导入任务](#导入任务)），校验与写入不阻塞读取、切库和退出；运行期间拒绝启动同步和索引写任务。取消、切库或退出在源事务提交前停止导入，不写入；已提交的阶段照常留待恢复（2026-10-12，#26、#88）。导入接口复用 canonical 包固定路径，异步研究接收者仍须先验证捕获的包实例和会话身份。

## 内部 worker 协议与任务状态

`native/terms/src/worker_protocol.rs` 定义索引、同步识别、溯源、研究的 request/response，以及索引 `index-job.json`、识别 manifest、签名文件和溯源网络请求的 serde 类型；外部文件在入口解析，结构错误带实际文件和嵌套字段路径。索引恢复不再把无效 journal 当成新任务。协议版本、request 字节、HTTP 字段顺序与数据库内容保持现行契约；身份、版本、资产范围及提交前指纹的语义校验仍由原协调模块执行。索引结果直接按类型校验，避免再建立整份动态 JSON 副本。索引 journal 与 `native_index_runs.result` 回执通过有序 JSON 值保存原键序；源库回执沿用不包含计算结果正文的格式。

`native/terms/src/task_state.rs` 持有状态枚举和有序展示快照，包含现行 `needs_recognition` 等状态（`needs_windows` 只为解析旧记录保留）；外来研究 progress 和已存溯源状态先解析，坏溯源状态以含文件/字段的失败信息呈现，基础浏览与人工操作继续可用。crate 根对非测试 `unwrap` 启用 Clippy warn，`app_index`、`app_sync`、`app_provenance`、`app_research` 使用模块 deny；锁中毒用带说明的 expect，worker/job 外部字段使用可返回错误的解析。其余存量内部 unwrap 已改为有说明的 expect，保留既有不变量失败行为，不把它们宣称为可恢复错误。显式 `test-read-host` feature 的测试读宿主不属于生产代码，生产模块 deny 不受影响。

`app_http::handle` 保留本机/跨站、会话 cookie、generation、CSRF、body 格式检查与分派；`http_host_get.rs` 和 `http_host_post.rs` 承接会话业务路由，沿用旧快照、锁释放和切库后拒收边界。

## 原生学习、聚类与相似研究协调

显式设置 `ILLUSTASH_RESEARCH_WORKER` 为可执行文件后，Rust 宿主通过 `app_research::Research` 管理学习、聚类、相似扫描和按需 relate/OC 分组。可执行文件接收 `request.json response.json` 两个参数；安装研究包提供原生 `illustash-worker`（计算在 `native/terms/src/research_compute/`）。基础版不设置该变量，研究状态为 off，缺失依赖/崩溃显示 error，已有特征、缓存、身份 pairs、浏览和人工操作仍走 Rust。启动和切库均调度当前包；相似自动扫描服从包的 startup.similar，学习与人工通知独立。

请求协议为 `illustash-research/1`，携带 #45 的完整 TaskIdentity、版本、只供计算的 SQLite 快照、特征/补充锚点目录和输入参数。worker 的学习、聚类、相似扫描、关系比较与 OC 分组只能修改临时快照。补充锚点使用显式共享资源目录。Rust 校验回传身份、输入指纹、类型、范围和引用后，通过学习核心发布原型、弱证据和正式投影，重建相似组、重复与折叠投影。学习结果、研究缓存和本批事件回执在同一应用库事务内提交。

宿主在每个数据库的一致读事务内为当次任务取数（#29、#457）。源快照只取 `embeddings` 里的 CCIP 行，学习另取 `characters`，relate 的源快照为空。应用库的取数范围如下。

- 学习（events/regroup）在副本中调用核心学习发布并重算投影，混合任务还会分组和扫描相似图。副本保留普通应用表结构、索引、视图和触发器，按学习核心、分组及相似计算取数，术语和身份表按组保留。纯学习批次省去已有的术语投影行，带分组的批次会复制这些行以读取主题标签。学习指纹保守覆盖应用表。不复制的数据和指纹排除项分别由 `app_research.rs` 的 `SnapshotDb::copies_rows` 与 `UNREAD`、`UNREAD_META` 定义。名称表、显示投影和对应版本见[核心词条存储](TERMS_SCHEMA.md#14-第-8-版2026-10-19-起201)。画廊和相似组已读对应的操作历史也跳过。核心学习戳仍独立核验。另核对特征和锚点文件内容，以及全部原图的大小、修改时间。
- similar 取 `assets`、`duplicates`、`excluded`、`image_sigs`、`relations`、`similar_paired`、`similar_retry`，核对特征文件和全部原图。OC 取 `memberships`、`duplicates`、`excluded` 和 `general_hits` 的可见行，核对特征文件。relate 只取请求 UUID 的原图路径，并核对相应原图。

接收时，宿主用取快照时的同一对连接开启新读事务，逐库比较 `PRAGMA data_version`。版本相同就复用该库的行指纹，有其他连接提交就重算该库在本任务范围内的指纹。共享文件和原图在锁外重新核对。发布方取到应用库写锁后再比较应用库版本与共享文件戳（大小、修改时间、索引代号），有变化就放锁重试，最多三轮（#275）。源库读事务一直保持到应用库事务提交。等写锁每 20 毫秒看一次源库写入方，出现就让路（见下文）；一轮 5 秒内拿不到写锁按「有变化」算。三轮都有提交时不发布，任务内状态显示 `waiting`「图库正在写入，研究稍后继续」，按 2、4、8、16、32 秒退避后用同一份结果重新检查，仍不行就结束本次运行，状态 `waiting`「图库一直在写入，研究稍后继续」，不记错误、pending 原样保留、不计入下面输入变化的 5 次，恢复 60 秒后重新开始。OC/relate 只读、不取写锁。原图只在锁外核对。范围内的人工操作、输入更新和旧会话结果仍导致拒收。 在 50,000 张合成图库的 Windows x64 release 对照中，单张确认到学习生效的三轮中位数由 16.465 秒降至 4.606 秒，观测到的两个临时数据库主文件合计由 522.0 MB 降至 224.9 MB（#457）。

相似缓存的回传与发布（2026-10-12，#31）：App 在请求中写 `delta_tables:true`，worker 把 `image_sigs`、`relations`、`similar_paired`、`similar_retry` 按快照回传变更表 `{delete, append}`：`delete` 是快照中要去掉的行的主键，`append` 是保留的行之后依次追加的行。App 在指纹确认后照此写入，每个主键必须命中一行。没有该字段的请求（旧 App）仍收到整表；旧 worker 回传的整表和 `clusters`/`cluster_info` 由 App 与现存行比较：按主键匹配、先后一致的前缀原地保留（取值不同则原地改写），其余删除后依次追加。两条路径发布后各表的行和 rowid 先后都与原来的整表删除重插相同，只是 rowid 不再从 1 重新编号；结果无变化时不写缓存行。关系与签名只检查本次写入的行，未写的行就是库中已有的行；写入的行若指向已不在图库的图，只能与被替换的旧行完全相同。`duplicates`、`similar_groups` 与折叠投影仍按现状每次重建。研究快照目录在任务结束（成功、失败或取消）时删除；被强杀的进程留下的 `illustash-research-<任务>` 目录，在下次启动研究任务时若已超过一天且超过两倍任务时限即回收。合成 5 万张时，无变化的回传由约 32.6 MB 降到约 200 字节。整表回传仍受支持，新旧 App 与 worker 可以混用。

verdict/imported/index_changed 的 pending 事件按实际包实例恢复，完成本批结果发布后才确认；途中新增事件留给下一批。识别导入的 `dispatch` 先持久写入 `research_runs` 调度意图，再允许 #46 确认通知，算法完成由研究状态与意图的 done 表示；通知以事件 ID 保存稳定调度键，确认失败后的重放不会新增意图；重复导入不重复通知。手动 similar/regroup 也保存可重放意图。失败保留 pending/error，60 秒退避或手动/重启重试；发布前输入指纹不符（「研究输入已改变」）或学习批次过期（「学习输入已改变」）不按失败处理（2026-10-05，#194）：同一任务内状态显示 `waiting`「输入有变化，稍后自动重试（n/5）」，按 2、4、8、16、32 秒退避后用新快照重算，最多重试 5 次，过期结果始终不发布、一致性检查不放宽；用完仍冲突才以 error 报告并暂停本会话自动恢复，待手动「重新检查」/「重新分组」或重开。基准间隔可用 `ILLUSTASH_RESEARCH_RETRY_MS` 调整，重试计入任务总时限；OC 分组与 relate 查询不重试。研究读源库时为写入方让路（#226、#275、#457）。`source.sqlite` 使用回滚日志。它的共享读锁保证从源指纹复核到应用库发布之间，写方无法提交更新的源输入。WAL 允许写方在旧读快照仍存活时提交，采用 WAL 需要另行协调这两个提交，并让损坏抢救路径处理 WAL 文件。大图库读取 CCIP 向量期间，同步或识别导入可能在等锁。宿主先算应用库指纹、复制应用输入，再读源库；复制、复核和等待应用写锁期间按 20 毫秒间隔检查写方的 `-journal`，见到写方就中断、放锁。学习与相似任务随即结束，状态显示 `waiting`「图库正在写入，研究稍后继续」，不记错误、pending 原样保留，也不计入上面的 5 次；恢复在没有同步运行、距让路 2 秒后重新开始。OC 分组与 relate 在任务内等写入方离开 2 秒后重问，最多 5 次，仍在写入就报错。`POST /api/similar` 或 `/api/regroup` 的 `action:cancel` 暂停本会话恢复，待下一次显式启动或重开。GET similar/clusters 显示实际状态/进度；OC 分组和 relate 在释放会话锁后等待受管理任务，切库后拒绝旧响应。默认总时限 30 分钟，可用 `ILLUSTASH_RESEARCH_TIMEOUT_MS` 显式调整。 设置 `ILLUSTASH_RESEARCH_TIMING` 后，宿主向 stderr 输出两库取指纹、复制、核心学习戳、worker 及发布各阶段的毫秒数。

学习落定的信号（2026-11-17，#404）：只读的 `GET /api/learning[?upto=N]` 回答编号不超过 `upto` 的 verdict 事件是否都已处理完，回复 `{upto, state, learner}`。不带 `upto` 时取当前最大的 verdict 编号（没有为 0）：写入提交以后才回复，所以回复之后第一次读到的编号不小于那次写入的编号；`upto` 不是非负整数或大于当前最大编号时回 400。只看 verdict，imported 和 index_changed 没配 worker 时没人确认。`state` 按顺序取第一条成立的：`done`（没有这类 pending，结果已发布或不需要学习）、`sync`（同步在运行，结束后研究才启动）、`error`（learner 出错，约 60 秒后重试）、`stopped`（本会话里不会自己继续：取消过、自动重试用完，或者只读的验收图库）、`pending`（其余，会自己继续）；`learner` 与 `/api/clusters` 的同一个对象。接口立即回复，从不在服务端等，轮询和时限在前端（`core/meta.js`）。会话锁下只取标志，放锁后用只读连接查一条 `EXISTS`。回答 `pending` 而研究空闲（没在运行、不在重试等待、没有停下）时，主循环在约 100 ms 内提前跑一次恢复检查，不等下一个整秒。写入路由、它们的回复和 `backend_effects` 都没有改。

切库与退出同步 stop/join，使用 #45 的进程组收尾，正常父 worker 先退出也清理子进程；同 library_id 的不同包分别识别。Windows worker 使用受验的 kill-on-close Job，生命周期见 [独立计算包](BACKEND_TASKS.md#独立安装与可选计算包)；Unix SIGKILL 宿主不承诺析构清理。关闭变量回到基础模式，缓存继续可读。

macOS/Windows 图像关系复用 `research_compute` 的特征加载、只读快照和结果发布协议（relate 只做像素比较，不加载 CCIP 特征）；安装 dispatcher 接管 similar、relate 及含 similar 的 mixed events，不分拆学习批次。App 继续拥有取消/进程树、输入指纹、人工历史与原子发布。`relation_image` 的 `signature` / `signature_bytes` 返回原 Pillow 解码入口对应方向的 16×16 灰度字节与宽高比，原 bands 含 A 或 mode P 时以 `#f4f4f2` 压平；RGB/L 的 PNG tRNS 保留原有不铺底条件；`reference_colour` / `reference_colour_bytes` 保留溯源 `_colour` 的 draft96、丢弃 alpha、24×24 RGB BOX 语义，不额外应用 EXIF（TIFF 解码器本身依旧按 Pillow 转置）。文件名和 bytes 入口分别保留 TIFF raw 单 strip、mmap 安全模式下的旧方向/尺寸合同，不假定两者相等。8-bit CMYK TIFF 通过既有锁定 tiff 解码器保留四通道，与 CMYK JPEG 共用先 thumbnail/reduce 后整数 RGB 转换的路径；不使用 image 适配器提前转 RGB 的 float 截断。GIF 局部首帧的画布使用透明索引（无透明时索引 0）的调色板颜色（含本帧局部调色板），不把解码器空白 RGBA 的透明黑当作旧背景。错误使用应用已有 Result，relate 仅将原解码/IO 失败归入 missing；I;16 reducing_gap 和 raw planar associated-alpha 的旧 ValueError 传播为任务失败，调用方按原业务处理，接口不创建缓存或访问资产库。

图像解码/缩放合同按原输入模式区分；独立参考和失败边界见 `tests/test_native_relations_compute.py`，实际安装路由、非 no-op 新缓存的取消/旧快照拒绝、切库/退出/超时/重放及 mixed events ack 回滚见 `tests/test_native_research_production.py`。Pillow 12.3 对当前 height > width×100 且高度缩小的图像先纵向再横向缩放，判断在 reducing_gap 之后；modeF、byte 和整数路径均保留该严格条件及逐轴存储舍入。byte reducing_gap 保留原 float32 倒数乘子、UINT32 乘积右移24及末行/末列的实际样本数；不能替换为精确整数除法（3×3 总和5时原结果是0）。下表描述当前代码合同，不代替 PR 中指定提交的实际验收。

| 输入域 | 解码后及缩放前的规则 | 缩放后转换及现有限制 |
|---|---|---|
| JPEG RGB/L、CMYK/YCCK | 按原 draft bound 解码、EXIF；CMYK 保留四通道 | 原 byte thumbnail/reduce 后 CMYK 整数转 RGB；alpha 不适用 |
| PNG 灰度 1/2/4/8、RGB8、P、LA8/RGBA8 | 原 bands 的 A/P 条件；RGB/L tRNS 不增加 A；1/P 用 nearest | RGBA 在 byte thumbnail 前 premultiply、后还原并铺底；signature/colour 不先做 thumbnail |
| PNG RGB16/LA16/RGBA16；TIFF RGB16/RGBA16 | 原 Pillow 在解码时取高字节，先于 thumbnail/alpha；不使用除 257 的四舍五入 | 与原 8-bit 模式共用后续路径；PNM maxval 缩放是另一合同 |
| PNG I;16；TIFF unsigned I;16/I;16B | 16-bit 值保留至 thumbnail，filename TIFF mmap 和 bytes 原有方向分别处理 | 逐 axis 16-bit resample 后 RGB clamp255；I;16B 保留旧 native-storage 字节顺序；reducing_gap 对此模式的原错误保留 |
| TIFF BlackIsZero I32/F32 | 复用共享 `image_samples::tiff_scalar` 的 typed scalar_samples，保留原 I/F 值、方向和布局；display 不应用 ICC。little-endian 主机上的 MM Deflate8 普通 strips 在消费前交换 32-bit word，保留旧 Pillow libtiff rawmode 行为，pure samples 不改 | 已验证交换域要求 planar1、predictor1、rows>0 和正确 strip 数；其它旧可读 MM32 编码/布局由 display typed 路径保持相同 word 交换，LZW/PackBits 不借用未经验证的 owner 缩放。交换先于 thumbnail/reduce；I/F full reduce 保留原同型 2×2 加法（I32 wrap、F32 每步舍入）后进 double，末行/列是逐项 double；逐 axis I32/F32 存储后才 RGB clamp；I32 cubic 正溢出保留平台原 C 转换：Mac arm64 饱和，Windows x64 存 INT_MIN |
| TIFF L8/RGB8/RGBA8/RGBX、CMYK8 | 先 TIFF 自身转置；RGBX padding 不算 A；CMYK 保留 raw 四通道 | raw 单 strip mmap 安全模式下，原 filename reshape/transpose 与 bytes 普通 decode 不合并；tiled/raw 与压缩 FillOrder2 由 display typed 路径处理 |
| GIF 首帧 P | 首帧局部/global palette，画布用 transparent 索引（无透明时 0）；nearest | 保留透明像素 RGB 给丢弃 alpha 的 colour；signature/load 按原 matte 条件 |
| PNM P1/P4、P2/P3/P5/P6 | 使用共享 `image_samples::pnm` 的原样本读取（与 #153 像素哈希共用）；P1/P4 保留 mode1 nearest；其余按原 value/maxval*out_max 的 ties-even，RGB/L8 输出 byte，gray>8 保留 modeI | modeI 先有符号逐 axis resample/reduce 再 RGB clamp，不能提前夹255，也不使用 PNG 高字节规则；P7 原拒绝边界保留 |
| BMP L/1/P/RGB、WEBP RGB/RGBA、ICO/TGA | BMP identity 灰调色板是 L，其余 P/1 用 nearest；ICO 保留原最大面积、最低深度、稳定并列的选帧顺序，PNG payload 保留原 bands；32-bit BMP 保留 alpha、不用 AND mask | TGA 原 P/1 nearest、文件/bytes 检测、水平/垂直方向；16-bit RGB/palette、LA descriptor 与 raw1 适配已有 decoder，不建设新图像平台。ICO/TGA 手写头与 Pillow 导出样本独立对照 |
| TIFF P/1/LA、8/16-bit planar/associated alpha | display 另用锁定 tiff 0.10.3（MSRV 1.74）读取 packed/LA 和各 plane；0.9.1 typed scalar 所有者与现有 RGB/CMYK 路径不变。palette 只在内存将第一 IFD 的 photometric 改为 BlackIsZero 解压索引，保留 offsets、compression 和原 palette；P/1 thumbnail 用 NEAREST | 原始方向/路径 mmap 与 bytes 分开对照；零 alpha 解预乘清零 RGB。planar tile 以本 plane 局部坐标裁剪 decoder 的底/右 padding。raw planar associated-alpha 保留旧 Pillow ValueError；raw16 planar 保留原 byte-band stride。FillOrder2 压缩在解码前对编码区间 union 翻转一次，允许共享 strip offsets |

TIFF 额外 I/F 模式保留 signed16、unsigned32、WhiteIsZero 的原样本和先 thumbnail 后 RGB clamp；packed12 按 tile 局部 bit 边界及原右缘 floor stride 解包，不全图拼接半字节。TIFF JPEG 的 abbreviated strip/tile 复用现有 mozjpeg 后端与 JPEG tables，保持原 IDCT 和 CMYK 原通道至缩放后的整数转换；不使用另一 decoder 的近似 IDCT。未知/损坏编码沿原错误处理，不声明泛用 Pillow 格式全覆盖。

关系 NCC 的第三方 FFT header 原样保存在 `native/terms/vendor/pocketfft/`，完整 BSD 许可证见同目录 `LICENSE.md`。正式来源为 NumPy `v2.5.3` 的 `numpy/fft/pocketfft` 子模块，固定指向官方 `mreineck/pocketfft` 提交 `33ae5dc94c9cdc7f1c78346504a85de87cadaa12`；header SHA-256 为 `50f4ddbe60432325f560c706b6f1c49f9e0949fc7f20ae6499f2299e37d9790c`，许可证 SHA-256 为 `a85ca13fdf90160b64a0698215868c13b74d835ad0a4e2ba44713b8c5058a056`。窄 C++ wrapper 对照该版本 `_pocketfft_umath.cpp`，保留补零时的 scalar plan、axis 顺序和逐 axis normalization。NumPy 2.5.3 前向默认整数因子 `1` 使 gufunc 选择 double 内核，每个 axis 后转回 complex64；乘积为 complex64，逆向因子与内核为 float32；Mac ARM 乘积按 NumPy NEON 的顺序，先舍入虚部乘积，再对实部乘积与该值执行 FMA。Windows x64 在 CPU 同时支持 AVX2/FMA 时沿用同样的 NumPy 复数乘积舍入；基础 x86 路径保留分离乘加。Windows FFT 的逐阶段与六组 valid correlation 固定参考独立于 Mac golden，不能跨平台复用逆向位级结果。Windows 灰度 RGB[h,w,3] @ weights[3] 通过 #151 的固定 ILP64 OpenBLAS 后端逐图像行调用 (w,3) GEMV，w=1 保留 DOT；不扁平化图像高度，不手写另一套 BLAS 尾块舍入。后端缺失、指纹或加载身份错误沿现有 Result 返回，不回退为另一套数学。候选保留这个实际类型解析及中间舍入，不能把整链改为一种精度。Clang 浮点参数遵循该版本根 `meson.build` 的 `-ftrapping-math`。不新增计划缓存或调度器。
