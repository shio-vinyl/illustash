# 本地全量验收

现行参考，2026-11-15。本文补充 [AGENTS.md](../AGENTS.md) 的本地验收约定；CI 覆盖及允许跳过仍以 `.github/workflows/ci.yml`、`.github/workflows/windows-full.yml`、`tools/check_all.sh` 为准。

## 开发检查与完整入口

开发 worktree 先跑新改测试和相关定向检查。`tools/check_all.sh` 先验证测试文件语法及验收工具合成测试，再执行 crates/core、crates/platform 与 crates/relation-fft 的独立 fmt/clippy/test（前两项另做 locked build）、宿主 fmt/clippy、JS、桌面 clippy/test，随后执行原有 Rust 与 crates/iqdb 的 fmt/clippy/test、release 构建、屏蔽退役参考目录导入的 Python discover 和空 PATH 契约重放。Python 全套由 `tests.run_fixed_suite` 执行，调度和参数见下文；覆盖、失败即退出、跳过白名单不变。导入源位于 `tests/reference/`、`web/` 或 #53 已退役的旧索引工具文件时立即拒绝，任何被捕获的导入尝试也使检查失败。构建前只运行固定的测试语法、验收工具合成测试等纯轻量项。任意指定的定向测试使用 `CHECK_ALL_TARGETED_TESTS='tests.test_native_http.TestCase'`（使用完整模块路径，在仓库根执行，以实际类名为准），在本轮候选 release 核心、测试读宿主及 IQDB 准备完成并设置二进制路径之后、Python discover 之前运行；输出候选 head，不提前读取旧 target。旧变量 `CHECK_ALL_PREFLIGHT_TESTS` 非空时明确退出 2，不静默改变其含义。额外定向结果不能代替全量，也不缩小 discover 的覆盖。

当前候选的 `read-http`、`recognition-compute`、`index-compute`、`research-compute`、`relation-probe`、`provenance-compute` examples 在本轮 release 构建中显式准备，再导出各自 `ILLUSTASH_*_BIN`；研究/索引安装适配器同样显式指向本轮 `illustash-worker`。本地与 CI 使用相同接线，缺产物不能靠测试默认跳过形成通过。模型的专项授权及平台证据仍单列。

脚本依赖 Python 3.12（`CHECK_ALL_PYTHON` 可指定解释器）、Node 22、Rust stable（含 rustfmt/clippy），Python 依赖与历史构建缓存放在被忽略的 `.venv/check-all/`。

直接执行 `tools/check_all.sh` 仍要求已提交的干净候选，运行期间 head/已跟踪文件保持不变。验收工具合成测试的前置与全量都使用 `unittest -b` 捕获模式，文本捕获器不提供二进制输出流时仍保留原始完整日志及其摘要。脚本独立执行不会生成可复用签名记录。缓存速度没有固定承诺。

2026-10-03 用户批准的 PR #131 / #136 / #141 是限定合批例外：各项保留独立 review 和受影响平台专项，整合保留原提交，最终整合 head 一次完整入口通过后整批 merge；原 PR 的历史或定向证据单列，不冒称逐项新 head 全量。其余任务仍按 AGENTS.md 原合并门槛执行，不能因复用或合批省略实际 GUI / 安装验收。

**前端结构检查**（#320）：`tools/check_all.sh` 在 `check_architecture.py` 之后运行 `tools/check_frontend.py --strict-stale --base-ref <与 origin/main 的共同祖先>`，`ci.yml` 里对应的步骤带 `--strict-stale`（不带 `--base-ref`：「Rust 核心」job 的检出只有一层历史，取不到与 main 的共同祖先；这一项由集成会话的本机全量覆盖）。`--strict-stale`：基线里已不再出现的项算失败，改好违规的 PR 要同时跑 `python tools/check_frontend.py --shrink`；`--base-ref`：基线比共同祖先上的大就失败，新增违规不能靠往 `tools/frontend_baseline.json` 里加通过。取不到 `origin/main` 和 `main` 时脚本会明说并跳过 `--base-ref` 这一项，先 `git fetch origin` 即可。规则本身见 [AGENTS.md](../AGENTS.md#前端结构约定)。集成会话组批时，先合入的 PR 若收紧了基线，后面改到同一批文件的 PR 合并后要再跑一次 `--shrink`。

新加一条带基线的规则时（如 #349 的规则 8），在加规则的同一个 PR 里运行一次 `python tools/check_frontend.py --init-rule <规则名>`：它只给基线里还没有的这条规则按现状登记存量，别的规则不动；已登记的规则、没有基线的硬规则会被拒绝（`--init` 在基线已存在时也拒绝）。`--base-ref` 把「基准上的基线和 `tools/check_frontend.py` 里都没有」的规则认作首次登记，输出里注明，不算基线变大；登记的数量多于现状仍会失败。基准脚本里已有、基线里却缺了的规则照常算变大。之后这条规则和其他规则一样只能 `--shrink`。

## 并行运行与耗时口径

`tests/run_fixed_suite.py` 默认以 CPU 核数为进程上限，按测试类分组动态领取，同一类在一个子进程内运行；`--jobs N` 指定正整数上限，`--jobs 1` 使用原串行 discover。并行模式仅接受 `-v`、`-b`，筛选等其他 unittest 参数需配合 `--jobs 1`。每组输出整段回放，父进程检查发现的测试 ID 是否漏跑或出现额外项，子进程崩溃、测试失败及退役导入尝试均使入口失败。子进程以 `-c` 启动，先把仓库根插到 `sys.path` 最前再运行 `tests.run_fixed_suite`，并沿用父进程的 `-I`、`-X utf8`、`-W` 等解释器选项，因此不依赖解释器把 cwd 放进 `sys.path`；带 `._pth` 的嵌入式 Python（Windows 验收机）与 `-I` 一样不加 cwd、忽略 `PYTHONPATH`，父进程须同样自行插入仓库根后用 `runpy` 运行，`tests/test_run_fixed_suite.py` 以 `-I` 覆盖这一情形。单独调用运行器前仍须准备对应候选二进制和环境；改变进程数不构成完整验收。

`SERIAL` 是运行器内的测试类名单：并行池结束后，名单里的类逐个单独运行，每个用例仍只执行一次，不自动重试。2026-10-13，[PR #134](https://github.com/shio-vinyl/illustash/pull/134) 修复找回画师终态与忙状态不一致、PID 文件未写完整及研究调用计数混入其他任务的问题后，名单为空（`SERIAL=frozenset()`），全部正常发现的测试类进入并行池。临时加入名单时，在对应 Issue 记录失败现象和移除条件；修复后验证并行运行并移除，不把串行结果当作竞态已修复。

2026-10-13 的合成数据实测：Apple M2 8 核、macOS 26.6.2 arm64，Python 3.12.12、Node 22.23.2、Rust/Cargo 1.94.1。最终已测 head `5ad38c9`（包含 #131 全部优化）单次完整检查为 **569 秒（9 分 29 秒）**，其中 Python 全套 8 进程为 122 秒（638 项、611 通过、27 白名单跳过），Rust 重放为 220 秒；依赖缓存已存在。此前 `0748ced` 单次完整检查为 903 秒，包含 dev 依赖优化级别变化后的约 190 秒一次性重编。验收详情分别见 [PR #134](https://github.com/shio-vinyl/illustash/pull/134) 和 [PR #131](https://github.com/shio-vinyl/illustash/pull/131)；这两轮未覆盖 Windows，不能把耗时外推到其他机器、平台或冷缓存，也不表示后续 main 已重新完整运行。

全量耗时包含依赖准备、fmt/clippy、测试编译、Rust/JS/Python 执行、release 构建和空 PATH 重放；Python 汇总及 Rust 重放各只是其中一部分。比较时写明已测 head、平台、进程数、数据口径和缓存是否已存在，分别列出编译与执行时间。当前 Rust 重放缓存每个场景的路径替换规则并预筛无目标字符串的单元格；`native/terms/Cargo.toml` 的 dev profile 优化第三方依赖，自有 core/platform 和修改过的 tiny_http 保持未优化。依赖重编的单次总时长、测试执行时间与固定验收缓存复用是不同口径，不能合成一个提速数字。

## dev 构建的调试信息（#362）

2026-11-14 起，5 个 Cargo 项目（`crates/core`、`crates/platform`、`crates/relation-fft`、`crates/iqdb`、`native/terms`、`desktop`）的 `Cargo.toml` 各自写了同一套 dev profile：自己的 crate 用 `debug = "line-tables-only"`（只带行号表），第三方依赖用 `debug = false`。`cargo test` 用的 test profile 继承 dev。release 的设置没动，安装包内容不变。

- **范围**：自己的 crate 指项目本身和不在本项目 workspace 里的 path 依赖（`illustash-core`、`illustash-platform`，desktop 里还有 `illustash-terms`）。后者会落进 `[profile.dev.package."*"]`，所以在每个项目里单列一条，否则它们也拿不到行号表。vendored 的 `tiny_http` 当第三方处理。
- **回溯**：panic 和测试失败的回溯里，自己的代码仍有 `文件:行:列`，标准库的栈帧也有；第三方依赖的栈帧没有文件名和行号，只有函数名。要用调试器或需要变量信息时，临时把对应 `Cargo.toml` 里的 `debug` 几行删掉（用完 `git checkout -- <Cargo.toml>`）。Cargo 输出的 `Finished `test` profile [unoptimized + debuginfo]` 是固定的 profile 名称，不反映 `debug` 的取值。
- **将来合成 workspace（#311）**：profile 只能写在根 `Cargo.toml`，到时把这几段并到根上。
- **三种设置**：现状（默认，完整调试信息）、甲（上面这套，采用）、乙（全部 `line-tables-only`）。选甲的理由：冷编译比乙更快、体积更小，增量编译在两个平台上都不比乙慢；乙多出来的只是第三方依赖栈帧里的文件名和行号，开发会话用不到。

测量口径（构建的是 `cargo test --locked --no-run`，每个设置用一个新的临时 `CARGO_TARGET_DIR`，量完删除；基线 main `0b41d543`，三种设置在同一提交上只改 `Cargo.toml`；设置是否生效用 `cargo -v` 里每个 rustc 的 `-C debuginfo=` 核对过；没有用 sccache）：

| | 现状 | 甲 | 乙 |
|---|---|---|---|
| **macOS** 冷编译 `native/terms` | 97.7 秒 | 81.1 秒（−17%） | 84.5 秒（−13%） |
| macOS 冷编译 `desktop` | 69.5 秒 | 59.3 秒（−15%） | 64.9 秒（−7%） |
| macOS 增量（改 terms 一个文件，含链接） | 7.7 秒 | 7.2 秒 | 7.3 秒 |
| macOS `target` 大小 terms | 2.08 GiB | 1.49 GiB（−28%） | 1.66 GiB（−21%） |
| macOS `target` 大小 desktop | 2.04 GiB | 1.43 GiB（−30%） | 1.68 GiB（−17%） |
| **Windows** 冷编译 `native/terms` | 239.1 秒 | 194.3 秒（−19%） | 206.5 秒（−14%） |
| Windows 冷编译 `desktop` | 124.7 秒 | 106.5 秒（−15%） | 110.0 秒（−12%） |
| Windows 增量（改 terms 一个文件，含链接） | 20.1 秒 | 16.2 秒（−19%） | 16.8 秒（−16%） |
| Windows `target` 大小 terms | 4.61 GiB（其中 PDB 1.91） | 2.20 GiB（PDB 0.57，−52%） | 2.49 GiB（PDB 0.78，−46%） |
| Windows `target` 大小 desktop | 3.45 GiB（其中 PDB 0.64） | 2.17 GiB（PDB 0.33，−37%） | 2.32 GiB（PDB 0.38，−33%） |

- macOS：Apple M2 8 核、16 GB，Rust 1.94.1，`CARGO_BUILD_JOBS` 取默认（8），经 `collab_run.py timing` 开跑，开跑时整机 CPU 最近 30 秒平均占用 7%～20%（门槛 20%）。冷编译各量 3 轮，每轮换一次三种设置的顺序，表里是中位数（范围：terms 现状 91.8～103.2、甲 79.3～91.6、乙 82.5～120.9 秒，乙的最大值是一次离群）。增量是在冷编译之后，往 `native/terms/src/app_names.rs` 末尾各追加一个函数、连续三次，三轮共 9 次的中位数，范围 6.8～8.6 秒，三种设置分不开。macOS 默认把调试信息留在 `.o` 里不合并（`split-debuginfo = unpacked`），链接本来就轻，所以增量没有差别，收益在 rustc 的生成时间和磁盘上。体积是冷编译后量的，增量三次之后再多约 0.36 GiB（各设置相近）。
- Windows：验收机 Intel i7-11800H 8 核 16 线程、32 GB，MSVC，Rust 1.99.0，`CARGO_BUILD_JOBS=4`（与验收入口一致），经 `collab_run.py run --slot windows` 在常驻检出的同一份 git 对象上另开一次性检出构建，不用 sccache，每个设置只量 1 次。同一台机器在更早的 main（`c36dfbb`）上量过一次现状和甲，冷编译 terms 为 231.1 和 187.0 秒、增量 22.8 和 17.1 秒，冷编译与本次相差约 4%，趋势相同。MSVC 链接要生成 PDB，完整调试信息下 PDB 占 terms 的 `target` 约 40%，所以体积和增量的差别比 macOS 大。
- 单次的冷编译数字受机器状态影响，别的会话在编译时会拉高；差别在 10% 以内的不要当结论。

## 本机资源：排队、计时与后台优先级

规则见 [AGENTS.md](../AGENTS.md#约定)「本机资源」一条，这里写做法。工具是 agent-collab skill 的 `scripts/collab_run.py`（路径按所在 harness 的 skill 安装位置），槽位是本机文件锁，没有常驻进程。

| 用途 | 命令 | 行为 |
|---|---|---|
| 全量、`local_acceptance.py run`、单独的 release 构建 | `collab_run.py run -- <命令>` | 占 heavy 槽，被占时排队 |
| 计时、性能测量 | `collab_run.py timing -- <命令>` | 等待期间不占槽：先等 heavy 和 vm 都空出、且没有普通 `run` 在排队（有就让它们先走），再等整机 CPU 最近 30 秒平均占用不超过 20%，然后同时占两个槽开始；已开跑的照常跑完。槽位空着、CPU 却累计 15 分钟都没闲下来，就退出 75 并列出最占 CPU 的进程。门槛可在 `run.json` 的 `timing` 一节或用 `--idle-cpu`、`--idle-for`、`--idle-timeout` 调。开跑时 stderr 有一行 `CPU idle: …% busy on average over the last 30s (max …; limit …)`，照抄进 PR 的测量口径 |
| 虚拟机 | `collab_run.py run --slot vm -- tart run …` | 见下文「macOS 实机测试」 |
| Windows 远端构建与验收 | `collab_run.py run --slot windows -- …` | 与本机槽位互不阻塞。容量 2（本机 `run.json` 的 `slots.windows.capacity`），对应验收机的两份常驻检出，见下文「Windows 验收」；加 `--exclusive` 时等两个位置都空出、一起占下，用于要独占验收机的计时和验收 |
| 开发期的编译和定向测试 | `collab_run.py bg -- <命令>` | 不排队，直接以后台优先级运行 |

`bg` 在 macOS 上用 `taskpolicy -b`：进程只跑能效核，磁盘 I/O 被节流，cargo、rustc、测试进程和 Python 子进程都继承（`ps -o pri=` 显示 4，普通进程是 31）。其他系统退回 `nice -n 10`，没设 `CARGO_BUILD_JOBS` 时设为核数的一半。在 `run` / `timing` 里面调用 `bg` 时保持正常优先级，所以全量和计时不会被降级。对时间敏感的定向测试在后台优先级下失败时，先在 `run` 里重跑排除负载，再下结论（#219）。

**检查放哪台机器**（2026-11-16 用户定，#587）：多个会话并行时，本机的负载主要来自各工作树以后台优先级跑的编译和定向测试，它们不占槽，排队管不住；Windows 验收机核数更多，平时大多闲着。所以开发期的定向检查按下表分到两台机器：

| 检查 | 放哪 | 怎么跑 |
|---|---|---|
| 定向 Rust 测试、clippy，不碰 macOS 专用代码 | Windows 验收机（优先） | `collab_run.py run --slot windows -- win_run.sh --steps terms-clippy,terms-test <head>`，步骤名见 `win_run.sh` 开头；另加定向 unittest 用 `--python "tests.x"` |
| `cargo fmt --check` | Mac | 验收机没装 rustfmt；fmt 不编译，直接跑 |
| 碰到 macOS 专用代码（`cfg(target_os = "macos")` 的分支）的编译、clippy 和测试 | Mac | `collab_run.py bg -- cargo …` |
| 虚拟机走查要用的 macOS 构建 | Mac | release 构建，`collab_run.py run -- …` 排 heavy 槽 |
| 界面级的 `tests.test_frontend_architecture`、`tests.test_styleguide_sync` | PR 上的 CI | `ci.yml` 的「Frontend checker tests」一步；CI 只在非草稿 PR 上跑，草稿阶段要看结果就在本机跑 |
| 合入门槛 | Mac | 集成会话在候选上跑 `tools/check_all.sh`，不变 |

验收机上的结果只代表 Windows：碰到平台分支的改动，两边各跑一次。clippy 在 Windows 上看不到 `cfg(target_os = "macos")` 的代码，反过来也一样。

**拦截**：Claude Code 和 Codex 都接了 `PreToolUse` 钩子，按本机的 `~/.config/agent-collab/run.json` 拦下没按规定运行的命令，并给出改写后的完整命令。2026-10-19 起的范围：

- heavy：`tools/check_all.sh`、`tools/local_acceptance.py run`、`cargo build --release` / `-r`、`cargo test --release`、`cargo tauri build`、`python -m tests.run_fixed_suite`
- 计时：`tools/bench_recognition.py`（新加的计时脚本也应加进来）
- windows：`tools/windows_acceptance.py`、`tools/windows_research_acceptance.py`
- 后台：`cargo build`、`cargo test`、`cargo clippy`、`cargo check`、`cargo nextest`、`python -m unittest`；直接写 `taskpolicy -b …` 也放行，经 SSH 在别的机器上跑的不管

钩子只看代理直接发起的命令，脚本内部调用的（例如 `check_all.sh` 里的 cargo）不受影响。`cargo run` 不在范围内：常驻的宿主进程不应长期占着槽位或被降级。

`status` 区分三种状态：`heavy #0 running`（占着槽在跑）、`waiting for an idle CPU, holds no slot`（在等空闲，不占槽）、`waiting for the slot`（在排队）。2026-10-21 之前启动的计时进程用旧逻辑，等空闲时仍占着槽，显示为 `held, not started`，等它自己超时或开跑。

**运行历史**：每次 `run`、`timing`、`bg` 结束（包括排不上、等不到空闲而放弃的）都追加一行到状态目录的 `history.jsonl`：模式、槽位、会话、仓库、命令、排队、等空闲和执行的秒数、退出码、开始时的负载，以及当时别的槽位是否被占。`collab_run.py history` 按槽位汇总最近 7 天（次数、失败、放弃、排队和执行时长的中位数与最大值），`--raw N` 列出最近 N 次。讨论排队代价、全量耗时时以这里的数据为准，引用时写明时间范围。

**回退**：钩子环境里设 `COLLAB_RUN_GUARD=0` 关闭全部拦截；只想取消某一类，从 `run.json` 删掉对应规则。`bg` 不改变任何持久状态，不用就是回退。

**编译缓存（sccache）**：2026-11-12 起，本机 illustash 的开发编译共用一份 sccache 缓存；全量、计时、`local_acceptance`、release 构建和 `cargo run` 不走缓存，编译行为不变。用户级的 `~/.cargo/config.toml` 用 `[build] rustc-wrapper` 指向包装脚本 `~/.local/bin/illustash-rustc-wrapper`。这个脚本只在四个条件同时满足时调 sccache：输出目录在 illustash 的检出或 worktree 里，调用的是 rustc 本身，不是 incremental 编译，进程是后台优先级。不满足时直接执行 rustc，所以本机其他 Rust 项目不受影响。这样分是因为 sccache 的编译由常驻服务进程执行，服务的优先级取自启动它的那个进程，`taskpolicy -b` 不会随每次请求传过去。服务如果由普通进程启动，`bg` 编译在未命中时会跑到性能核上；反过来，全量又会被降到能效核。官方文档规定同一份本地缓存只能由一个服务使用，所以只让后台编译用，服务也总在后台优先级下启动。仓库内的 `.cargo/config.toml` 不加 wrapper，因为 Windows 验收机和 CI 上没有 sccache。效果和测量口径见 [#240 的评论](https://github.com/shio-vinyl/illustash/issues/240#issuecomment-6806900035)。

用法：`illustash-rustc-wrapper --stats` 查看命中率和缓存占用；单条命令前加 `ILLUSTASH_SCCACHE=0` 临时不用缓存；回退就删掉 `~/.cargo/config.toml` 里的 `rustc-wrapper` 一行。缓存放在 `~/Library/Caches/sccache-illustash`，上限 10 GB（`SCCACHE_CACHE_SIZE`）。加上或去掉 wrapper 都不会让已有的 target 重新编译。`local_acceptance.py` 会记录 `~/.cargo/config.toml` 的摘要，所以这个文件一改，已有的验收记录下次复用时就会以「环境改变」被拒一次，重跑一次 `local_acceptance.py run` 就能生成新记录。

**#587 复测和自家 crate 的限制**（2026-11-17）：同一改动（main `49703ec4` 上往 `native/terms/src/app_names.rs` 追加一个函数）在三个新建的工作树里先后跑 `cargo test --locked --no-run --all-features --manifest-path native/terms/Cargo.toml`，都从空 target 开始，用正式的包装脚本、另用一套端口和缓存目录，`taskpolicy -b`（同 `bg`），经 `collab_run.py timing` 开跑（开跑前 30 秒平均 19.6%，门槛 20%；跑的过程中别的会话的后台编译没停，1 分钟负载 8～12），每项单次，Apple M2 8 核、macOS 26.7.1、Rust 1.94.1：

| 情况 | 墙钟 | Rust 命中 |
|---|---|---|
| 第一个工作树，sccache 空缓存 | 913 秒 | 0 / 153 |
| 第二个工作树，缓存已热 | 506 秒 | 117 / 153（76%） |
| 第三个工作树，不开 sccache（`ILLUSTASH_SCCACHE=0`） | 863 秒 | — |

第二个工作树比不开 sccache 快约四成，空缓存时多花约 6%，缓存共 144 MiB。没命中的是 illustash 自己的 crate 和带 build script 的依赖（实测 `crates/core` 时，第二个工作树统计里 6 个未命中，服务日志里报未命中的是 `illustash_core`、`proc_macro2`、`serde`、`serde_core`、`serde_json`、`libsqlite3_sys`、`rusqlite`）。sccache 0.19 把 `CARGO_MANIFEST_DIR`、`OUT_DIR` 这类带工作树绝对路径的环境变量算进缓存键，换一个工作树就对不上；关掉 incremental（`CARGO_INCREMENTAL=0`）、再设 `SCCACHE_BASEDIRS` 指向各工作树根目录，未命中数一样，所以 wrapper 维持上面的接法，不为自家 crate 关 incremental。自家 crate 要省编译，只能靠把定向检查挪到验收机（见上文「检查放哪台机器」）。

## 修改测试的导航

业务契约和固定期望的比较规则见 [架构参考](ARCHITECTURE_DIRECTION.md)；本节说明测试分工、依赖和修改方法，测试入口与合并门槛仍按上文及 AGENTS.md。

| 要验证的行为 | 优先阅读或修改的位置 | 依赖与范围 |
|---|---|---|
| 事务、证据、人工历史、投影和业务规则 | `crates/core/src/` 的测试模块、`crates/core/tests/` | 直接调用核心，多数用内存 SQLite；适合规则的边界组合 |
| 文件、凭据、进程、系统数学库等平台能力 | `crates/platform/src/` 的测试模块、`crates/platform/tests/` | 以真实平台边界为准；其他平台的证据单列 |
| 宿主任务状态、取消、切库与提交顺序 | `native/terms/src/` 的测试模块；`tests/test_native_lifecycle.py`、`test_native_sync.py`、`test_native_research.py`、`test_native_provenance.py` 等 | 模块内测试可控制实际任务阶段；HTTP 测试验证调用方看到的状态和结果 |
| 旧行为与旧图库兼容 | `native/terms/tests/baseline.rs`、`contracts.rs`、`it/v8_upgrade.rs`，比较规则在 `tests/common/baseline.rs`；`tests/fixed_expectations.py`、`test_native_library_upgrade.py` | 使用提交的录制或历史合成包；期望独立于当前候选输出；对照基准见下文 |
| 数值、图像及计算发布 | `tests/test_native_*_compute.py`、`test_native_*_production.py` | 显式指定候选 examples/worker；固定数值由 `*_expectations.py` 读取，Windows 差异另有覆盖文件 |
| 界面、资源装配与检查工具 | `tests/*.test.cjs`、`test_pixiv_poll.js`、`test_packaging*.py`、`test_local_acceptance.py`、`test_architecture.py` | JS 与合成工具测试不等于 GUI 或完整安装验收 |

真实模型专项为 `tests/platform_*_models.py`，独立于默认 `test_*.py` discover；模型、runtime 和候选必须显式提供。不能把合成计算或资源装配通过写成真实模型、桌面 GUI 或另一平台通过。

### 新增与收拢覆盖

新增用例先写清可触发的条件和可观察的错误结果，再选择能覆盖它的最低一层。规则的边界组合优先直接调用核心；HTTP、CLI 或 worker 层保留请求接线、授权、发布及生命周期的代表性场景。同一规则跨层出现时，应能说明各层独有的失败方式。收拢前对照输入、断言和状态边界；相似名称、相同路由或同样出现 SQLite 不足以证明重复。

普通新用例优先检查受影响的返回值、行和业务不变量；拒绝写入、事务回滚、切库隔离等本身要求其他状态不变的场景，才做相应范围的完整状态比较。已有固定录制的比较范围按既定契约维护，不能借性能优化直接删减。

### 固定期望与共享夹具

`tests/fixtures/reference_recordings/` 按测试 ID 和调用顺序取值，`native/terms/tests/fixtures/baseline/` 按模块重放核心调用；改测试模块、类名、方法名或取值顺序之前先查读取方。`tests/image_expectations.py`、`index_expectations.py` 等是冻结答案的读取器，旧模块名出现在这些文件里不表示旧业务实现仍在执行。比较失败先区分输入、路径/时钟归一化、平台差异和产品行为变化；原生输出不得刷新旧答案。产品决定确需改变期望时，按架构参考的规则在对应 PR 说明。

**对照基准（#201）**：Python 时代的三类旧录制（24 组核心调用重放、4 组 HTTP 固定契约、Python 固定期望里的整库摘要）比较全部表、schema 和隐式 rowid，核心 v8 改表结构后都会失效。2026-10-19 用当时的 main（核心第 7 版）在同样的场景上另录了一份新基准，放在 `native/terms/tests/fixtures/baseline/` 和 `tests/fixtures/baseline_states/`：接口回复按字节比较；每次调用之后比较除退役旧表以外的全部表内容，不比 schema 和隐式 rowid；只服务旧表的调用（`sync-*`、`verify-*`、`cutover-*` 和未切换完的库上的调用）不进新基准，它们留下的库作为检查点，在下一个保留的调用之前载入，处于录制时核心版本的检查点先升级。比较规则写在 `native/terms/tests/common/baseline.rs` 开头，两边共用；Python 侧照同样的规则，在录下的状态与实时状态比较时一并比对。比较规则由用户 2026-10-19 确认（#201）。核心第 8 版起旧的核心重放和契约录制已删除，新基准是唯一对照；Python 固定期望的整库摘要在录下的状态与实时状态比较时改用新基准。录制当时在旧录制照常核对、一致时进行（PR #239 的 `record_baseline`，随旧录制一起删除，留在 git 历史里）；新基准不重录，被测输出不得用来刷新它。v8 新加的列在比较器里逐项登记排除（`SKIP_COLUMNS`，写明原因）。`baseline.rs` 的测试进程关掉了 SQLite 的内存统计（`SQLITE_CONFIG_MEMSTATUS`）：每次读状态都重新打开库、解析 schema，各模块的重放线程原先大半时间在抢这把进程级的锁；关掉后 debug 下整个重放从约 110 s 降到约 35 s（2026-10-21，空闲时三次中位数），比较的内容不变。在这之后改用 release 构建只能再省约 7 s，线程数 4 也比 2 或 8 快，所以都没有改。

词条第 9 版的新增状态及物理表／有效视图适配，由 Rust/Python 共用 [精确清单](../tests/baseline_additions.json) 与 v7 历史参考分开验收（#441）：表、meta 键和映射以该清单为唯一明细，每项登记引入版本和具名的严格语义测试。适配只用于实时应用库对历史参考的投影；冻结侧存在清单要分离的新增表、meta 键或物理表时直接失败。物理表与有效视图的完整列及带类型的行（含重数）须一致，才应用映射；历史结构属性表达只在该投影中重建。不使用通配或期望交集，旧表旧列、类型、行重数、响应和名称增量校验保持。Python `baseline_state()`、Rust `state()`/`exact()`、当前库前后比较、原样备份和失败回滚仍保留物理表、原始属性及全部新状态。

自动人工备份会在开库后写入保护状态、摘要缓存和 WAL。HTTP 共享夹具先等待真实首份快照与摘要补算结束（补齐摘要后马上再出的那一份也要写完，`refill` 为假，#516），再采集操作前的完整状态；需要比较切库目标前后状态的夹具先打开并完成目标的首备份。settings/startup/cache 的 `package.library_bytes`、`library.bytes` 及 libraries 的当前包 `info.library_bytes` 按当前主库加 WAL 的独立文件大小精确核验，其余响应字节和头继续对照冻结记录。仅 v5/v6 迁移及撤销等价用例用可释放的 `ILLUSTASH_TEST_HUMAN_BACKUP_GATE` 文件闸门保留原完整状态比较，随后释放并验收真实首备份；等待不持数据库或发布锁，切库及退出照常取消，产品不设置该变量（#441）。

新基准和 Python 固定期望都录在导入按 tag 后缀建作品（#296）之前。重放时设上只供测试的环境变量 `ILLUSTASH_TEST_NO_IMPORT_WORKS`（`baseline.rs` 在启动核心时设，`tests/fixed_expectations.py` 加载时设），导入照旧规则走，录下的期望不改（用户 2026-11-12 定）；产品从不设置它。所以对照测试不覆盖导入建作品，这一段由 `crates/core` 的 `character_import` 单元测试（「不是作品」清单）和 `native/terms` 的 `app_create`（新图库从索引导入角色时建作品）、`app_recognition`（WD 结果按 tag 后缀归作品）测试负责。

冻结的核心重放和 `NativeHumanCharactersTest` 中比较 `summer` 造型投影的用例用 `ILLUSTASH_TEST_LEGACY_SKIN_NAMES` 沿用 #847 扩充前的造型选名规则。现行规则由 `names_view` 的显示与搜索测试和 `app_names` 的模型缓存测试覆盖。

HTTP 测试共用 `tests/test_read_http.py` 的端口、等待和进程清理工具；多种原生测试借用 `tests/test_native_lifecycle.py` 的建包与宿主方法，worker 启动与进程存在性工具在 `tests/native_worker_fixture.py`。测试对宿主的 HTTP 请求统一经 `tools/verify_read_http.py` 的 `fetch`：每个请求一条短连接，读完响应后以 RST 关闭，本机不留 TIME_WAIT。逐个正常关闭时，并行全套约 1.5 万个连接会在 Windows 上耗尽默认的 16,384 个临时端口（#140）。新增大量请求或轮询时沿用 `fetch`，不要另起逐次关闭的客户端；只有验证连接本身行为（半包、卡住、慢读）的测试才自建 socket。修改这些共享方法须检查实际使用它们的测试类，别只看定义文件。提取 helper 时避免把带 `test_*` 方法的 `TestCase` 作为模块级别名留给 discover；`__module__` 还可能参与固定录制的定位，移动前须核对。

### 慢测试与时序失败

排查时分别计入构建、建库/复制、启动进程、执行被测操作、扫描与比较期望、等待/清理；同时记录调用次数、进程次数和参与比较的数据量。录制重放的一个 Rust 测试函数包含多次调用，每次还会扫描数据库；测试项数不能代表工作量。先消除重复执行和比较器内部的重复计算，再根据实测决定并行粒度。共享 Cargo 产物的任务可能争锁，录制模块内部也已有并行线程；增加外层并行前须核对这些依赖及机器负载。

等待状态应以完整结果为条件：文件存在不保证 PID 已写完，终态可见也需符合下一次操作的接纳契约。超时用例单独设置短超时；其他用例的等待上限应容纳正常负载。四类时限怎么分见 [AGENTS.md](../AGENTS.md#约定)「等待上限与性能断言」（#508）：Python 等待统一用 `tools/wait_limit.py` 的 `WAIT`（默认 60 秒），整机很慢时设 `ILLUSTASH_TEST_WAIT_SECONDS=180` 一类整体放大，排查挂死时设小一点让它早点失败；它只改变真挂死时多久报错，不改变通过时的耗时。取消或提交边界优先用已存在的阶段闸门、进度回调或显式握手定位；自动重试和一律加长 sleep 会掩盖竞态。失败信息应带最后状态、子进程日志或不一致的表/字段，避免只剩一个通用等待超时。

调整 `tools/check_all.sh` 的结构时检查 `tests/test_local_acceptance.py` 与 `test_packaging_resources.py`：它们目前还按脚本文本定位构建、环境导出和 discover 顺序。维护时保留候选产物先准备再消费、漏跑失败及跳过白名单等行为；新增检查优先验证行为，避免继续绑定命令的排版与拼写。全量检查前置的 `test_local_acceptance.py` 也会被后续 discover 再执行；任何去重须同时保留快速失败入口及最终覆盖。

### 不稳定测试的隔离名单（#501）

2026-11-15 用户定。`tests/quarantine.json` 列出已知会偶发失败、正由 Issue 跟踪的测试。规则见 [AGENTS.md](../AGENTS.md#约定)「隔离名单」一条，这里写做法。

- **条目**：`kind`（`python` 或 `rust`）、`id`、`platforms`（`macos`、`windows`）、`issue`、`added`、`expires`；Rust 条目另写 `crate`（Cargo 项目目录，如 `native/terms`）。Python 的 `id` 是 discover 下的测试 ID（`test_native_research.NativeResearchTest.test_x`），Rust 的 `id` 是 libtest 名称（`app_sync::tests::test_x`，集成测试不带二进制名）。到期日默认加入后两周。
- **效果**：名单里的测试照常运行；在条目列出的平台上，它们的失败（含 subTest 和 setUp 的失败）不计入结果，在输出和 job 摘要里单列「隔离名单中的失败（不计入结果）」。别的平台照常计入。
- **校验**：名单格式错误、条目到期（运行当天晚于 `expires`）、名单里的测试找不到时，检查失败。Python 条目由 `tests.run_fixed_suite` 在 discover 后核对，各平台都核对；Rust 条目由 `cargo-test` 入口用 `cargo test -- --list` 核对，只在条目列出的平台上核对，因为 Rust 测试常按平台条件编译（如桌面壳的进程树测试只在 Unix 上有）。`python tools/quarantine.py check` 只查格式和到期，`tools/check_all.sh` 一开头就跑它。
- **Rust 的做法**：`python tools/quarantine.py cargo-test --manifest-path <项目>/Cargo.toml` 代替 `cargo test`：先 `--exact --skip` 掉本平台的条目跑其余测试，由它决定退出码；再用 `--exact` 逐个单独运行这些条目，只汇报。本机全量、`macos-full.yml`、`windows-full.yml` 都经这个入口。
- **用到它的入口**：本机全量、Windows 快检、两份手动运行的完整回归，以及发布前在 Windows 验收机上的完整回归，都用同一份名单。定向复跑（`windows-full.yml` 的 `sync-boundary`、`research-worker`）直接跑选中的用例，不按名单放过失败，用来复现。

## 固定验收窗口

固定路径为 Git 公共目录所在仓库的 `.venv/local-acceptance/worktree`，由 `tools/local_acceptance.py` 建立 detached worktree。它与主工作区及开发 worktree 的源码、target 分开；只在固定路径现场构建，不复制其他目录的 Tauri target。开发 worktree 的缓存可按原规则清理。固定验收 worktree 本身和外层记录、密钥不随普通任务收尾删除；它的 target 等构建缓存可以删除（2026-10-19 用户定，#240），下次 `run` 时在固定路径冷编译，不影响已有记录的校验。

开发方先提交候选、做定向检查，在 PR 交接完整 SHA 和所需资源。父会话安排窗口后，从本仓库任一 worktree 执行：

```bash
python3.12 tools/local_acceptance.py run <已提交候选SHA>
python3.12 tools/local_acceptance.py result <候选SHA>
```

工具不 fetch、不 push、不合并。它自动创建或移动自己登记的固定 worktree；现有未知目录、分支模式、未提交或未跟踪内容均拒绝，不覆盖。两个命令都以非阻塞进程锁串行操作，第二个占用者直接失败。运行子进程继承锁描述符，即使控制器退出，仍在运行的自有检查进程保有锁；正常结束即释放，无持久占用或手工解锁指令。中断不产生成功记录，不杀其他会话。不得绕开此入口直接在固定路径切 head 或写 target。固定区只能串行验收一个已提交候选，没有排队、合批或调度服务。

`result` 会在持锁期间选择该候选并验证已有结果；不运行构建或测试。无匹配结果时退出非零。`run` 先尝试复用，无匹配则从头执行唯一完整命令 `bash tools/check_all.sh`，输出同步写入外层 `check-all.log`。完整命令退出 0、输入和 head 保持不变、工具链和环境校验通过之后才写 `result.json`。失败或中断移除旧成功记录，保留失败日志供调查，不能称为通过。

## 复用口径与失效

默认指纹包含所有已跟踪文件的路径、Git blob 和模式：源码、测试、脚本、锁文件、配置、所有现行文档和生成资源都不例外。只允许 `docs/acceptance/local-summary.md` 这一个普通非可执行文件用作不被产品、构建或测试读取的验收叙述；不提交原始运行日志。该文件不是测试输入，新增读取、include、打包或按通配符消费它的代码必须同时取消豁免，并重新全量。其他文档的说明收尾仍须重跑。工具/规则/指纹策略变更也失效，不能简单排除全部 Markdown。

记录保留原实测 commit、tree、完整命令、exit 0、开始/结束时间、完整日志摘要及有效输入。候选必须是已测提交的线性后代（不含 merge），每个提交只能改变上述叙述文件；即使其他提交改回相同代码，也不在复用范围。候选 tree 和已测 tree 分别展示。原生构建嵌入 Git SHA：复用的是原提交完整运行的验收事实，原产物保留原 SHA；不声称当前候选重新构建或其产物字节相同，不把当前 SHA 标到原产物上。需要候选 SHA 的实际发行构建另行构建并验收。

环境指纹包含同一固定路径、机器名、OS/架构、全部传入环境变量的摘要、Python/Node/Bash/Rust 工具链版本和文件摘要、实际 rustup 工具链、Cargo/Git 配置、macOS SDK/实际 clang 和明确区分的 Command Line Tools 安装身份或完整 Xcode 版本（探针失败不忽略），以及已安装 Python 依赖文件和模式。环境变量明文不会写入记录；`PWD` 固定化，`_`/`OLDPWD`/`SHLVL` 等壳层状态和被完整脚本明确 unset 的 `CARGO_TARGET_DIR` 移除后传给检查。首次安装 Python 依赖按完整脚本要求执行，记录安装后的实际依赖指纹；其余工具链/环境在运行前后必须一致。

被忽略的 `packaging/desktop-resources` 内容纳入指纹。桌面构建输出仅窄识别 `desktop/build.rs` 复制的 `desktop/dist/splash.js`、`desktop/dist/i18n.js` 及 `desktop/gen/schemas` 下的 `acl-manifests.json`、`capabilities.json`、`desktop-schema.json` 和 `macOS-schema.json`。这两份复制必须与已跟踪的普通文件 `web/static/splash.js`、`web/static/i18n.js` 逐字节一致；Tauri 文件必须为普通 JSON object 文件，首次由完整检查生成后记录各自内容和模式，签名记录同时绑定已跟踪的 build.rs、capabilities/permissions、Cargo.lock 等源输入及工具链。复用时这些已测输出也须逐项匹配，删除/篡改使复用失效；完整命令之后缺少要求输出不得记录成功。输入指纹不因这些具体输出从无到有而变动，产物证据与输入分别校验。旧候选的复制不一致时拒绝复用，但可由新的完整检查重新复制源并校验后登记；不泛化豁免整个 dist 或 gen，未知文件/符号链接仍拒绝。ONNX Runtime 1.30 导入会生成根目录 `:memory:.ses`，只将这一普通文件的时间戳/UUID 固定格式纳入同样的签名生成输出证据，不豁免任意 `:memory:*`；未知名称或格式拒绝，文件变化使复用失效。记录格式为 v3，旧 v1/v2 记录失效并需完整执行。未知忽略输入（例如外部 models/reference）直接拒绝，不读取内容；各已登记 crate（包括 crates/platform）的 target、工具 `.venv` 缓存、Python 字节码和 `.DS_Store` 属于可再生输出。固定区不得额外放置外部输入；若引入新的被忽略构建输入，应先修改策略和合成测试。旧图库升级输入使用 `tests/fixtures/library_upgrade` 中提交的历史 v5/v6 合成包；每项测试复制后升级，不在全量检查期间构建历史版本。

本机私有权限密钥对记录作 HMAC 校验，完整日志也校验摘要；缺字段、失败、定向命令、签名/日志篡改、输入或环境不匹配均拒绝。该记录用于防止误改/手工修改记录后的复用；拥有本机密钥和工具源码写权限的人不是安全隔离边界。不能把记录移到另一机器或平台冒称覆盖。密钥丢失则旧记录无效，从头完整运行。

PR 应明确区分“当前 head 上完整执行退出 0”和“当前候选仅验收叙述变化，复用已测 commit 的全量事实”，列出双方 commit/tree、固定路径、机器/平台/工具链、结果和跳过理由；其他平台与尚未做的 UI 验收单列。缓存收益只引用同一口径的实测时间；没有实测就不写提速数字。

## 合并队列（集成会话）

2026-10-18 起，合入 main 由一个集成会话统一做（用户决定见 [AGENTS.md](../AGENTS.md#约定)「合并队列」一条）。起因：各开发会话分别在自己的 PR head 上跑全量，先合进一个，main 就变了，后面改到同一批文件的 PR 都要 rebase 再跑；并行的全量还互相抢机器。

**开发会话**：做完定向检查和需要的平台验收（动了 Windows 专用代码的在验收机上编译和测试，其余以 CI 的 Windows x64 快检为准；界面改动在浏览器里看，WebKit 专有的问题才进虚拟机），在 PR 描述里写明结果和检查级别（界面 / 普通代码 / 数据层 / 只改文档 / 只改测试和工具），转为 Ready 并写「待集成」。不跑本机全量，不合并。推送照 [AGENTS.md](../AGENTS.md#约定)「推送频率」一条：一组相关提交攒成一次推送，被拒或遇到限流时等一会儿再试。

改了 Rust 代码的 PR，转 Ready 之前先对改过的 Cargo 项目跑格式和 clippy（2026-10-19 用户定，#240；起因是第 11 批全量因为一个 PR 没过 `cargo fmt --check` 白跑一次）。参数与 `tools/check_all.sh` 相同：`crates/core`、`crates/platform`、`crates/relation-fft`、`crates/iqdb`、`desktop`、`native/terms` 跑 `cargo fmt --manifest-path <项目>/Cargo.toml --check`；`crates/core`、`crates/platform`、`crates/relation-fft`、`crates/iqdb`、`desktop` 跑 `cargo clippy --locked --manifest-path <项目>/Cargo.toml --all-targets -- -D warnings`，`native/terms` 在 `--all-targets` 后面再加 `--all-features`。clippy 经 `collab_run.py bg` 运行。之后合了最新 main 或又改了代码，要重跑。PR 描述写明在哪个 head 上跑的、结果如何。前后叠着的 PR（例如同一件事拆成几块）在描述里写明顺序。

**集成会话**，每一批：

1. 从 `origin/main` 开候选分支 `integrate/<日期>-<序号>`，按顺序把这批 PR 的 head 逐个 `git merge --no-ff` 进来；合不上的退回原会话，写明冲突文件，这一批不带它。同一批里有前后叠着的 PR，按叠放顺序合。合并提交的信息照 GitHub 的格式写，并带上 PR 关联的 Issue：第 4 步整批推送时，Issue 靠合并提交里的 `Closes` 关闭（#612）。关联的 Issue 用 `gh pr view <PR> --json closingIssuesReferences` 查，没有就省掉最后一个 `-m`：

   ```bash
   git merge --no-ff <PR head> -m "Merge pull request #<PR> from shio-vinyl/<分支>" -m "<PR 标题>" -m "Closes #<Issue>"
   ```

   例外是迁移清单的指纹冲突（2026-10-18 用户批准）。`tests/migration_inventory.json` 每个 `sources` 指纹单独一行、指纹之间空一行（规范格式由 `python -m tools.migration_inventory fmt` 写出，`check` 和单元测试拦住不规范的格式），两个 PR 改不同文件的指纹时 git 会自动合并，只有两边改了同一个文件的指纹时才冲突。如果冲突只在这个文件，并且正在合入的 PR 相对共同基点只改了 `sources`，集成会话自己解决：取候选里的版本，再跑 `python -m tools.migration_inventory sync-sources`，它只把 `sources` 改成当前源码的指纹并按规范格式写回，不动边界和归属；然后确认 `python -m tools.migration_inventory report` 的 `warnings` 和 `errors` 都为空，提交合并。正在合入的 PR 如果还改了这个文件里的边界、归属或别的键，照旧退回原会话。第 4 步整批推送时，这个合并提交随候选一起进 main，PR 原来的 head 是它的祖先，GitHub 照常把 PR 标为已合并，不用再推 PR 的分支。只有退回逐个合入时，前一个 PR 合入后这个 PR 在 GitHub 上仍会冲突，重算的结果才要放到它的 head 上：候选里合入它的那个合并提交是它原 head 的后代，把这个提交快进推到它的分支，不用强推，测过的提交就是 PR head。推的是别的会话的分支，每次推之前先请用户同意，推完通知原会话合入前别再推送，并在 PR 里留评论说明。
2. 候选里只要有一个 PR 需要全量（普通代码或数据层），就在候选 head 上经 `collab_run.py` 跑一次 `tools/check_all.sh`；含数据层改动的，升级测试照常包含。整批都是界面级、只改文档或只改测试和工具时，按各自门槛检查（界面级跑 `node --test`，两条 Python 测试以 CI 为准；只改文档的查相对链接；只改测试和工具的跑受影响的测试、以 CI 为准），不跑全量。
3. 全量失败时，先排除负载等环境原因（#219），再按 PR 二分定位，把失败的 PR 退回原会话，其余重新组批。
4. 通过后集成会话推候选、开临时 PR，把候选 head 和全量结果报给 Lead；推送和核对（下面第 2～4 小步）由 Lead 做，用户 2026-11-17 长期授权，不再逐批请用户批准。整批一次推到 main（#612）：N 个 PR 只有一次推送、main 上一次 CI，合入的就是测过的那些提交。2026-11-18 第十四批实测走通（#629）。
   1. 把候选分支推到 origin，开一个临时 PR：候选分支 → main，非草稿，标题写「集成候选：第 N 批（不要单独审）」，正文列出同批 PR 和本机全量结果（机器、系统、架构、工具链、通过与跳过），写明要交叉审查的 PR 已审过。main 的分支保护要求推上去的提交带着通过的「全部检查」，管理员也不例外；各 PR 上跑过的检查挂在 PR 自己的合并提交上，不算候选的。临时 PR 上由 `pull_request` 事件跑出的「全部检查」挂在候选 head 上，分支保护认它。不要用 `gh workflow run ci.yml --ref <候选分支>` 手动触发：`workflow_dispatch` 跑出的「全部检查」即使通过，直接推送时也不被认（第十三批实测被拒，报 `GH006 … Required status check "全部检查" is expected`）。GitHub 文档也只计入由 `push`、`pull_request` 等事件触发的 workflow job 检查（[Troubleshooting required status checks](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/collaborating-on-repositories-with-code-quality-features/troubleshooting-required-status-checks) 的「Checks from some workflow jobs are not evaluated」）。等「全部检查」通过再推：托管 macOS runner 排不上被取消的，用 `gh run rerun <run> --failed` 重跑一次；真失败照下文「和合并队列的关系」处理。Windows x64 快检不计入分支保护，结果照常查看。
   2. 推之前逐个核对：每个 PR 现在的 head（`gh pr view <PR> --json headRefOid`）就是候选里合入它的那个合并提交的第二个父提交。对不上说明测过之后又有人推了，这个 PR 退出这一批，重新组批。这一步代替 `--match-head-commit`。
   3. `git push origin <候选 head>:refs/heads/main`，不加 `--force`：main 没被动过才能快进。合入 main 只走这个队列，所以正常不会被拒。
   4. 核对 `origin/main` 就是候选 head，`origin/main^{tree}` 与候选 head 的 tree 相同，每个 PR 和临时 PR 都显示为已合并（`gh pr view <PR> --json state` 为 `MERGED`），关联的 Issue 都已关闭。不符就停下报告。

   推送被拒时先看原因：不是快进，说明 main 被动过，停下报告；候选的「全部检查」还没跑完就等它，没过照失败处理。只有别的原因（比如分支保护改了）才退回逐个合入：先关掉临时 PR（不合它），再按候选里的顺序用 `gh pr merge --merge --match-head-commit <PR head>`，每两个之间隔一分钟以上（AGENTS.md「推送频率」），全部合完后核对 `origin/main^{tree}` 与候选 head 的 tree 相同，不同就停下报告。
5. 全量结果已经写在临时 PR 里，各 PR 不再逐个留评论；只有退回逐个合入时，在临时 PR 里补一句原因。删除已合并的远端分支，以及本地和远端的候选分支。
6. 清理本机 worktree（2026-10-19 用户定）：每个 worktree 带一份 Rust 构建产物，约 7～13 GB，不清理会把磁盘占满。这一批合入后，对 `git worktree list` 里分支已合入 `origin/main`、没有开着的 PR 以它为头或目标、`git status --porcelain` 为空（没有未提交和未跟踪的文件）的 worktree，执行 `git worktree remove` 并删除对应的本地分支；任何一条不满足就跳过，并在报告里列出。不清理固定验收区 `.venv/local-acceptance/worktree` 和集成会话自己的工作区；虚拟机母版和副本不在此列。报告里写明删了几个、腾出多少空间（`df -h /` 前后对比）。

平台专用改动（如 `cfg(windows)`）的定向验收由开发会话在自己的 head 上完成；组批时如果冲突解决碰到了平台专用代码，退回原会话补测，不由集成会话代做。

## GitHub Actions（PR 自动检查）

2026-11-12 用户定（#4），仓库公开后启用。规则见 [AGENTS.md](../AGENTS.md#约定)「GitHub Actions」一条，这里写做法。

**定位**（2026-11-15 用户定，#501）：
- PR 和 push 上只做快速冒烟：格式、clippy、编译、前端 JS、原生固定契约和 Windows 快检。
- 合入门槛只看集成会话在候选上跑的本机全量；各 Rust 项目的 `cargo test` 和 macOS 的 Python 全量由它覆盖，不在 CI 上自动跑。
- Windows 的完整测试在 Windows 验收机上跑：发布提交定下后跑一次（[开发说明](DEVELOPMENT.md#发布)第 3 步），改了 `cfg(windows)` 代码的 PR 照旧在验收机上做定向检查。
- `macos-full.yml`、`windows-full.yml` 都只能手动运行，用来对指定提交补测，或在验收机不能用时备用。

挪走的依据：2026-11-14、15 两天 `ci.yml` 跑了 182 次、失败 23 次，一半以上是时限类偶发失败，集中在 macOS Python 全量和 Windows；macOS 的 Python 全量与本机全量完全重复。留下的几项这两天没出过偶发失败，单项都在几分钟内跑完。

**触发**：`ci.yml` 在目标为 main 的 PR 打开、推送新提交、重新打开、转 Ready 时运行；草稿 PR 和来自 fork 的 PR 不跑。push 到 main 时也跑一次，作用是写编译缓存：GitHub 的缓存按分支隔离，PR 写的缓存只有同一个 PR 读得到，main 上写的所有 PR 都能读。也可以在 Actions 页面对任意分支手动触发；集成会话合入前在候选分支上手动触发一次，让候选 head 带上「全部检查」（见[合并队列](#合并队列集成会话)第 4 步）。仓库私有时（`github.event.repository.private`）自动触发的运行一律跳过，只有手动触发的会跑，所以这个 PR 在公开前合入也不会耗分钟数；私有期间也不手动触发 macOS 运行（私有库按 10 倍计分钟）。同一 PR 有新提交时取消旧运行。push 到 main 的运行各自成组、互不取消；重型 job 另有只在 push 时共用的并发组，后一次代码 push 取消前一次还没跑完的同名 job，集成会话整批一次推送时 main 上只有一次运行；退回逐个合入时，main 上仍只有最后一次跑完。纯文档 push 跳过这些 job，不进组，不会取消前一轮还在写缓存的代码运行（#501）。这些组按 ref 区分，别的分支的 push 运行不会取消 main 的 job。

**job**：「改动范围」先判断改了什么：PR 比较它与 main 的合并提交和当前 main，push 比较推送前后的两棵树（多提交的 push 合起来算），都按 `--no-renames` 把改名拆成删除和新增，移进 `docs/` 的代码仍算代码。只改 `docs/` 和根目录的 Markdown 时跳过构建与测试；`docs/THIRD_PARTY.md` 和 `docs/BACKEND_TASKS.md` 随安装包分发，仍须检查。新建分支、取不到推送前的提交（如强推后）和手动触发一律按代码改动处理。只改前端静态资源、前端 JS 测试、结构检查或样式清单时（可同时改普通文档），只跑 Ubuntu 上的前端 job。桌面构建读取的 `web/static/splash.js`、`i18n.js` 和 `locales/` 下的文件，及宿主编译测试读取的 `web/static/core/jobs.js`，仍按代码改动处理。代码改动有四个必需 job：「Rust 格式、clippy 与编译」（依赖方向，core、platform、relation-fft、terms、iqdb 的 fmt 和 `clippy --all-targets`，core、platform 的 build）、前端 JS（Ubuntu，同时检查前端结构和样式清单）、桌面壳 fmt/clippy、原生固定契约空 PATH 重放。除 JS 外都跑在 Apple 芯片的 `macos-latest` 上。公开库另有 Windows x64 快检（见下节）。分支保护要求「全部检查」汇总结果。代码改动要求四个必需 job 全部成功，前端改动要求前端 job 成功且其余跳过，文档改动要求所有构建与测试跳过；分流失败、缺失输出和 fork PR 没跑检查均判失败。Windows 不在「全部检查」的 `needs` 里，汇总不等它，Windows 快检作为单独的检查显示实际成败。

**手动运行的完整回归**：`macos-full.yml` 和 `windows-full.yml` 只在 Actions 页面手动运行（`workflow_dispatch`），`ref` 填标签、分支或提交哈希，留空为所选分支的最新提交。`macos-full.yml` 有两个 job：「Rust 测试 (macOS)」经 `tools/quarantine.py cargo-test` 跑 core、platform、relation-fft、terms、iqdb、desktop 的 `cargo test`（一个项目失败不妨碍后面的项目继续跑）；「Python 全量 (macOS)」是原先 PR 上的 Python job：release 构建后跑 `tests.run_fixed_suite`，跳过白名单与本机全量相同。只有在 main 最新提交上手动运行时保存缓存，其余只还原。Windows 那份见下节。

**改检查项时**：同步检查 `ci.yml`、`macos-full.yml`、`windows-full.yml` 和 `tools/check_all.sh`。`tests/test_packaging_resources.py` 核对候选二进制接线（本机全量、macOS 和 Windows 完整回归、Windows 快检）、PR 上不再跑 `cargo test` 和 Python 全量、macOS 两处跳过白名单的一致性和 Windows 白名单的匹配规则，并检查触发条件和各 workflow 的权限；`tests/test_ci_workflows.py` 直接运行分流和汇总脚本，并检查并发组和快检选中的测试类。`native-upgrade.yml` 是升级测试的单独手动入口，要求整类零跳过；macOS 的 Python 全套和 Windows 完整回归已经包含这一类。

**冻结输入**：索引参考的 SQLite 输入哈希若因 SQL 导出的小数位格式变化而不匹配，只有整库 schema、类型、值（浮点逐位）、rowid 和版本标记与冻结数据库完全一致时，才使用冻结 SQL 的原始哈希重查。数组、资产 ID、参数和结果仍精确匹配；不刷新录制，也不把真实数据差异当作格式差异。

**缓存与权限**：Rust 编译缓存用 `Swatinem/rust-cache`，每个 job 一份，覆盖该 job 构建的 Cargo 项目；key 含各项目 Cargo.lock 的哈希，lock 变了时按前缀回退到最近一份，再增量编译。只在 main 上保存，PR 只读，免得 PR 的缓存挤掉 main 的（每个仓库合计 10 GB，超了按最久未用淘汰）。rustc 每 6 周升级一次 stable，升级后第一次是冷编译。workflow 只有 `contents: read`，checkout 不留凭据，不用 secret。每个 job 设了超时，卡住时不会耗满默认的 6 小时。

**和合并队列的关系**：集成会话组批时查看各 PR 的「全部检查」及 Windows x64 快检结果。失败的先退回原会话查明原因，runner 故障或排队取消可重跑一次；隔离名单里的测试失败不退回（见「不稳定测试的隔离名单」）。手动运行的完整回归不是合入条件。PR CI 以该 PR 和触发时 main 的合并提交为输入，集成会话仍在合批候选上跑本机全量（见上一节）。

**耗时**：各 job 的耗时以实际运行记录为准；Windows x64 的冷、热缓存实测见下节。2026-11-15 收缩后的一次手动运行（#501，[CI 43731628509](https://github.com/shio-vinyl/illustash/actions/runs/43731628509)）里，四个必需 job 最长约 2 分钟，「全部检查」在触发后 2 分 16 秒出结果；收缩前它要等 macOS Python 全量，约 13–15 分钟。仓库私有期间没有触发过；2026-10-01 开源前的旧版本在 `macos-latest` 上的一次运行里，Python 全量 job 约 25 分钟，其余各约 2 分钟，当时的代码量和 job 组成与现在不同，只能作量级参考。

### Windows x64 CI

公开库使用 `windows-latest` 和 MSVC x64，分三种运行（#501）。Cargo 沿用仓库的静态 CRT 配置，关闭 CI 增量编译以控制磁盘占用；`TEMP` 和 `TMP` 指到系统盘的短目录，Python 日志统一使用 UTF-8。

**快检**（`ci.yml` 的「Windows x64 快检」，代码 PR 和代码 push 自动运行）：release 构建 terms（宿主和 worker）、带 `test-read-host` 的 `read-http` 测试读宿主，`cargo check` 桌面壳的全部目标，再用 `tests.windows_fixed_suite --select` 跑 job 里 `QUICK_TESTS` 列的三个测试类：原生启动和生命周期（`NativeLifecycleTest`）、新建图库（`NativeLibraryCreateTest`）、核心读路由的 HTTP 契约（`CoreReadHTTPTest`），都用合成图库。跳过白名单与全量相同。Rust 缓存覆盖 terms 和桌面壳两个 Cargo 项目，只在 main 上保存，测试失败也保存，PR 从 main 还原。快检不跑 clippy、Rust 测试、其余 example 和数值对照，结论单独显示，不进「全部检查」的等待链。2026-11-15 试过把工作目录、临时目录和 Rust 工具链加进 Defender 排除项（#501）：runner 上的实时保护和按访问扫描本来就关着，构建耗时也在不加排除项的范围内，没有保留。

**完整回归**（`windows-full.yml`，范围 `full`）：内容是原先 PR 上的完整 Windows job。运行 core、platform、relation-fft、terms、desktop 的 clippy（参数与本机检查相同）、这五个项目及 iqdb 的 `cargo test`、terms 与 IQDB 的 release 构建和六个测试 example，再用这些 `.exe` 跑 Python 全套，包括升级测试和 #347 的严格数值对照。只能手动运行：发布前的完整回归平时在 Windows 验收机上跑（同样的内容，见 [开发说明](DEVELOPMENT.md#发布)第 3 步），这份在验收机不能用、或真机上数值参考对不上时备用。只有在 main 最新提交上手动运行时保存缓存。Rust 测试经隔离名单的 `cargo-test` 入口运行。

**定向复跑**（`windows-full.yml`，范围 `sync-boundary` 或 `research-worker`）：对指定提交只跑一个用例组，`repeat` 可重复 1–10 次，每次的结果分别记进 job 摘要；运行名、job 名和摘要都写明范围与实际提交，定向结果不代替完整回归。`sync-boundary` 跑 `app_sync::tests::scan_cancellation_and_completed_lifecycle_have_explicit_boundaries`（#497），先用 `--list` 列出实际选中的测试，失败报告写明夹具最后进入的阶段，并附上隔离子进程的 stdout 和 stderr。`research-worker` 跑 `test_native_research.NativeResearchTest`（#498），数值环境与完整回归相同；失败报告附上 worker 已启动的次数和各次 kind、最后状态，以及 `worker.stderr.log`（每次尝试的编号、kind、模式、异常和计算程序 stderr）和 Windows 启动器追加写的 `research-worker.stderr`。断言和时限与全量相同。研究定向复跑需要包含 #501 的提交（`tests.run_fixed_suite --select`），更早的提交会在构建前明确报错；同步边界没有这个限制，但更早的提交没有阶段证据。失败时 `regression-logs/` 下每次的日志作为工件保留 7 天。

`tests.run_fixed_suite --select 前缀` 也可以在本机用：前缀按测试 ID 匹配（如 `test_native_research.NativeResearchTest`，按「.」边界），可重复；导入失败的测试文件仍会报出，某个前缀没选中任何测试时判失败。

CI 使用标准 Python 3.12，与 macOS job 一致；常驻验收机使用嵌入式 3.13，`tests/test_run_fixed_suite.py` 的隔离入口测试覆盖其模块查找行为。完整回归和研究定向复跑从 PyPI 下载并缓存固定的 x64 NumPy 2.5.3 wheel，配合固定的 Pillow 12.3.0 给 Python 对照用，再用打包脚本和 `crates/platform/src/research_openblas.json` 验证 DLL 的 SHA-256 并设置 `ILLUSTASH_RESEARCH_OPENBLAS_DLL`。3.12 与 3.13 对应的 wheel 含相同的已登记 DLL。快检只装 `tests/requirements.txt`，不做数值对照。

**快检实测**（2026-11-15，#501 验收，合成数据，各一次）：Windows Server 2025 x64、Python 3.12.10，runner 的 CPU 型号未记录。冷缓存是没有可还原的 Rust 缓存的首轮；热缓存是还原同一配置的完整缓存（压缩后约 587 MiB）的一轮。拆分前 main 上的 Windows 缓存由当时的完整 job 保存，配置不同，快检还原不到，所以合入后第一次代码 push 是冷编译，之后的 PR 才从 main 还原热缓存。

| 步骤 | 冷缓存 | 热缓存 |
|---|---:|---:|
| 工具链、Rust 缓存、Python 依赖 | 41 秒 | 58 秒 |
| 构建被测核心与测试读宿主（release） | 8 分 07 秒 | 5 分 12 秒 |
| 桌面壳编译检查 | 2 分 33 秒 | 28 秒 |
| 原生启动、SQLite 与 HTTP 契约（38 项，8 项白名单跳过） | 49 秒 | 1 分 13 秒 |
| 整个 job（含初始化、缓存保存等） | 12 分 30 秒 | 8 分 08 秒 |

同一天另两轮冷缓存的整个 job 为 12 分 54 秒和 10 分 38 秒。拆分前的完整 Windows job 冷缓存 48 分 54 秒、热缓存 34 分 55 秒（下表），[CI #151](https://github.com/shio-vinyl/illustash/actions/runs/43703770545) 热缓存 29 分 29 秒；快检冷缓存短约 36 分钟，热缓存比 #151 短约 21 分钟。热缓存的构建仍要 5 分钟左右：rust-cache 只缓存依赖，terms、core、platform 这些自有 crate 每次都重新编译。不同 runner 的 CPU 不同，这些是作业耗时记录，不是稳定的加速比。

**拆分前的 Windows 缓存实测**（2026-11-15，同一 main `925798c1`，各一次）：当时 PR 上的完整 Windows job，也就是现在的完整回归。Windows Server 2025 x64、Rust 1.100.0、Python 3.12.10。冷轮未命中 Rust 和 wheel 缓存；热轮命中两者。冷轮 CPU 为 EPYC 7763，热轮为 EPYC 9V74，且热轮有一项测试失败，因此下列是作业耗时记录，不能当作稳定加速比。

| 步骤 | 冷缓存 | 热缓存 |
|---|---:|---:|
| 各 Rust clippy/test 步骤合计（含构建） | 25 分 32 秒 | 17 分 10 秒 |
| 被测核心、examples 与 IQDB release 构建 | 9 分 23 秒 | 3 分 45 秒 |
| Python 全套步骤 | 12 分 05 秒 | 12 分 21 秒 |
| 整个 Windows job（含初始化、缓存等） | 48 分 54 秒 | 34 分 55 秒 |

[冷轮](https://github.com/shio-vinyl/illustash/actions/runs/43681348320/job/132712622628) 全部通过；Python 共 780 项，727 通过、53 项按既有白名单跳过。[热轮](https://github.com/shio-vinyl/illustash/actions/runs/43681348320/job/132723570413) 的 Rust 与 release 步骤通过，Python 全套执行完毕：726 通过、53 跳过、1 失败（研究测试的受控 worker 异常退出，数值比较未失败）。热轮保留失败结论，不因缓存命中或汇总检查通过而视为验收通过。

冷轮保存的 Rust 缓存压缩后为 1,273,862,781 字节（约 1.19 GiB），NumPy wheel 为 12,456,384 字节（约 11.88 MiB）；当时仓库缓存共 16 份、5,739,708,417 字节（约 5.35 GiB），尚未达到 10 GiB 上限，暂不额外裁剪。这里只统计缓存 API 返回的压缩体积，不是本机 target 目录的大小。

固定 OpenBLAS DLL 仍会按 CPU 选择内核，浮点末位可能不同。灰度单元测试先按 DLL 摘要和实际内核选择独立 NumPy 固定参考，再严格比较五种宽度的全部输出位。NumPy 2.5.3 对应 DLL 的 SkylakeX 保留原期望，Haswell/Sandybridge 使用编译候选之前独立采集的参考，来源及摘要写在测试旁。未登记组合会明确报告缺少独立参考并使测试、Windows job 和 workflow 失败，不跳过、不按实际输出任选期望；这份参考清单不限制产品支持的 DLL 或 CPU。「全部检查」不含 Windows，不能据其绿色状态认定 Windows 已通过。

**九个数值方法的参考口径（#347，2026-11-15 批准）**：`tests/numeric_reference_mode.py` 列出的九个裁剪定位、溯源匹配和索引计算方法，默认继续严格冻结回放。Windows CI 显式设置 `ILLUSTASH_TEST_NUMERIC_REFERENCE=numpy` 后，每次从相同合成输入执行测试专用 NumPy/Pillow 完整算法，并与原生结果精确比较；未列出的方法仍使用原有参考。仅在 `tests/numpy_reference_environment.py` 登记的 DLL 摘要、NumPy 进程实际 OpenBLAS 配置/内核/线程数、NumPy SIMD 条件、平台和依赖版本全部匹配时，再严格比较现有冻结参考。任一比较失败都会使测试失败，不尝试另一种参考来消除失败；未知模式、缺候选或缺依赖同样失败，缺候选硬失败仅作用于实际选中的九个方法。登记证明参考适用于已验证环境，历史录制 CPU/core 身份仍未知；单独的 SkylakeX 字符串不能触发冻结比较。显式窗口拒绝非空 `NPY_DISABLE_CPU_FEATURES` 覆盖，避免 NumPy 与原生 CPU 分派不一致；登记另核对 AVX2、FMA3、AVX512F、X86_V3、X86_V4 的实际启用状态。

独立参考只保留历史 Python 的纯计算及缓存路径，不导入退役应用、不加载模型、不读原生输出构造期望、不刷新固定值。并列选择仍用已批准的标量 NumPy argintroselect/aquicksort 顺序，八组手写 `tied_nearest` 不变。比较涵盖业务返回、完整缓存表的确定字段及保留行、索引 ID 顺序、清洗/拒绝 JSON、CSV 字节和 NPY 的 dtype、shape、存储顺序和数据位；`char-anchors.npy` 头部必须声明 C 顺序，数据比较区分正负零，不锁定头部 padding；源库字节、人工标注和篡改拒绝断言保留。溯源的旧行时间戳须原样保留，新行的生成时间须落在实际调用的 UTC 秒窗口内；跨历史记录比较时，仅归一化 `artists.created_at`、`image_sources.fetched_at`、`image_artists.updated_at`、`lookups.last_at` 四列。`next_due_at`、`cursor` 及其他确定字段不归一化。

完整回归和研究定向复跑在编译前输出 CPU、NumPy/Pillow、DLL SHA-256、NumPy 进程实际 OpenBLAS 配置/内核/线程数及 CPU 分派覆盖。身份探针在 Python 进程加载 NumPy wheel 的 DLL；这里的线程数仅为 NumPy 参考进程实测，不表示已测候选子进程的线程数，日志将候选线程数标为未测。每次实际进入的数值比较输出 `NUMERIC_REFERENCE` 记录，`windows_fixed_suite` 在 job 摘要中分别统计 NumPy 和冻结比较的通过/失败数；这份记录与 unittest 的总体成败一起阅读。其他平台的默认验收入口不设置该变量。

`python -m tests.windows_fixed_suite --log windows-unittest.log` 是候选二进制和 OpenBLAS 接线后的全套入口；带 `--select` 和 `--scope` 时只跑选中的测试，日志和摘要标明范围。它列出全部 unittest 跳过，白名单逐条匹配平台限制，缺被测二进制、DLL 或升级输入均判失败。默认 discover 会排除显式点名的真实模型用例，打包资源测试使用合成文件，原生 ORT 通过运行时动态加载，故完整回归只需要固定 OpenBLAS。自建 ONNX Runtime 的实际加载及真实模型、DirectML、CRT 依赖检查、安装／覆盖升级／卸载、GUI 和首次流程、真机性能仍在 x64 验收机上做。试运行期间 Windows 快检和完整回归保留真实成败，分支保护的汇总暂不以它们决定代码 PR 是否通过。

## 界面走查（浏览器，#453）

2026-11-15 起。`tools/ui_walk/` 在隔离宿主 + 合成图库上用无头 Chromium 走一组跨页面、跨浮层的使用流程，每一步记快照（焦点、打开的层、选中、光标、详情栏、大图、地址、滚动、提示条）、截图并检查控制台错误。现有的 Node 测试用假 DOM，拦不住「功能 A 对、功能 B 对，连起来不对」的问题（#386）；这套走查补的是这一层。它是 Chromium 上的初筛，不代替 WKWebView（虚拟机）和 WebView2（Windows）里的实测，输入法组字也不在里面（见 #388 的做法）。

**跑一次**（在仓库根）：

```sh
python3 -m tools.ui_walk run                       # 当前检出的 web/static，中英文界面各跑一遍
python3 -m tools.ui_walk run --ref d5bf1212        # 某个提交的 web/static（git archive），后端仍是当前构建的宿主
python3 -m tools.ui_walk run --static 某处/web/static --flow viewer-detail --lang zh
python3 -m tools.ui_walk compare .walk/<A> .walk/<B>   # 逐步比较两次运行
python3 -m tools.ui_walk list                      # 流程和它们拦的 #386 条目
```

结果在 `.walk/<时间>/`（已被 git 忽略）：`report.md` 列出每条流程的检查、控制台错误和截图链接，`report.json` 是全部步骤的快照，截图在 `zh/`、`en/` 下。有检查没通过、控制台报错（含 5xx 回应）或流程没跑完时退出 1。`compare` 写 `compare.md` 和左右并排截图的 `compare.html`，有差异时退出 1；用法是新旧两份前端各跑一遍（#469 迁一页之前和之后），差异应只剩有意的改动。快照里的图一律写文件名，不写 uuid。

**依赖**：Node 22（自带 WebSocket，驱动经 Chrome DevTools Protocol 发真实键鼠事件，没有 npm 依赖、不加构建步骤）、Python 3、当前检出构建的 `illustash-terms` 和 `illustash-worker`（`cargo build --bin illustash-terms --bin illustash-worker`，经 `collab_run.py bg`；没有 worker 时分组、相似页是空的，报告开头会写明），以及 chrome-headless-shell：默认取 Playwright 缓存里最新的一份（`npx playwright install chromium-headless-shell` 只下载浏览器，不往仓库加依赖），或用 `ILLUSTASH_WALK_CHROME` 指定。不用 Chrome 的 `--headless=new`：它在本机主进程会长期占满一个核，挡住别的会话的计时，页面里的定时器和 requestAnimationFrame 也会卡住。走查不经 `collab_run.py` 排队：它不编译，CPU 占用和定向测试相当。

**隔离**：每条流程、每种语言开始前把合成图库恢复成刚建好的样子，各起一个宿主；宿主的 HOME、应用数据、共享资源都在 `.walk/fixture/` 下，和 `tools/dev_preview.py` 一样不碰真实图库、钥匙串和网络（关掉自动同步、来源查询和索引检查）。不点「打开其他图库」「导入」「添加文件夹」这类会在本机弹系统选择框的入口。宿主用一次性码地址开页，界面语言由浏览器的 `Accept-Language` 决定，时区固定为东八区，视口 1440×900。

**合成图库**（`tools/ui_walk/fixture.py`）：约 110 张生成的图，内容只经产品自己的入口写入：`library-create`、`library-scan`，一个合成的识别结果包（WD 评级与画面标签、WD 认出的角色和「也可能是」、按方向造的 WD/CCIP 向量；模型文件和标签表是假的，宿主只核对 SHA-256），一个合成的画师证据包（机器认定和「也许是」），再起宿主让研究 worker 分组、找相似，经 HTTP 接口建角色、确认归属、跳过和排除。结果有人工确认的角色 3 个、WD 认出的 1 个、未识别分组、相似组、字节相同的副本、R18 图和画师；引导标成跳过、只问一次的提示卡记成问过。随机抽查和身份整理的题在新建图库里没有写入方，造不出来。夹具按「生成脚本 + 宿主和 worker 程序」的哈希缓存在 `.walk/fixture/<键>/`，第一次建约一两分钟，之后直接用；新旧两份前端比较时用的是同一份图库。`python3 tools/dev_preview.py --fixture walk` 用同一份图库起交互预览（代理会话起它之后用 `preview_start {url}` 打开跳转地址）。加 `--wd-extension`、`--ccip-longtail` 时，脚本另起只听 127.0.0.1 的下载服务，提供合成的 WD 扩展包和 CCIP 长尾参考包（长尾包还在隔离的共享目录写一个只有一个锚点的基础角色索引），宿主经测试清单 `ILLUSTASH_TEST_WD_EXTENSION`、`ILLUSTASH_TEST_CCIP_LONGTAIL` 认它们的身份；`.claude/launch.json` 的 `web-packs` 就是这样启动的（#559）。

**流程**（`tools/ui_walk/flows.mjs`）按使用流程组织，不按单页：主要页面逐个打开；详情栏 → 大图翻页 → 关大图后三者指向同一张；同一张图在详情栏、大图、角色页按 C 都是「换成」，和按钮一致；未识别里多选 → 大图里设角色 → 关大图 → 撤销；按组整理整组设为 → 撤销后等学习收尾；浮层叠浮层时 Esc 只关栈顶、点外面只关外面的；搜索后 Esc；筛选、滚动、开详情栏 → 去设置 → 后退；设置里切原比例；新图库的导览接管焦点、上手清单不盖详情栏按钮。检查项只写人看得见的对错（哪张图、哪一层、焦点在哪、张数），不写实现细节，所以换一种实现后同一套检查仍然成立。新增流程照文件开头的说明加一项。

**失败处理**：启动、动作、快照和控制台错误均进入报告。每条流程只跑一次，超时保留已完成步骤并失败退出，不自动刷新或重跑。原宿主连接线程池竞态（#477）已修复。

同一个检出里一次只跑一个走查（共用夹具的 `live/` 目录，后来的会等前一个结束）；不同 worktree 互不影响。只支持 macOS 和 Linux。

**旧版复现**（2026-11-15，本机 macOS arm64，chrome-headless-shell 148）：用 #386 那批修复之前的 `d5bf1212` 跑，走查拦下了已修的 1-1、1-3、1-4、1-5、1-6、1-8、1-9。1-2（整组设为后的撤销竞态）未复现：合成图库的学习在撤销回应前已完成；现有 group-undo 只检查最终状态，不代表已覆盖该竞态窗口。逐条验收结果见 #453 的 PR。

## 界面性能测量（#683）

2026-11-19 起。`tools/ui_perf/` 在隔离宿主 + 合成大图库上，用 Chromium 和 WebKit 跑同一组场景，记录帧、长任务、渲染各阶段和滚动几屏以后的图形内存。0.4 界面整体升级（#703）改前、改后各量一次，用同一套脚本、同一台机器、同一个宿主程序，只换前端；新视图要守的预算写在 #683。

**跑一次**（在仓库根）：

```sh
python3 collab_run.py bg -- python3 -m tools.ui_perf prepare            # 建夹具、编译 WebKit 小程序（第一次约十分钟）
python3 collab_run.py timing -- python3 -m tools.ui_perf run --root 某个检出   # 计时；默认两个内核、全部场景，预热 1 次、记 3 次
python3 -m tools.ui_perf run --scenario home --engine chromium --runs 1 --warmup 0   # 改脚本时试跑，数字不作数
python3 -m tools.ui_perf report .perf.noindex/runs/<时间>                 # 按保存的结果重写报告
```

（`collab_run.py` 指 agent-collab skill 的 `scripts/collab_run.py`。）宿主程序取 `ILLUSTASH_TERMS_BIN`，没设时取当前检出构建的那个；测量要用 release 构建（`cargo build --release --bin illustash-terms`，经 `collab_run.py run`），debug 版生成缩略图、回读接口都慢得多。`--root` 是宿主的 `--root`，从它的 `web/static` 读前端：改前的数字用 `git worktree add --detach` 取 main 的那个提交，把它当 `--root`。结果在 `.perf.noindex/runs/<时间>/`（已被 git 忽略）：`report.md` 是中位数和口径（机器、宿主、前端的提交、夹具规模、次数），`runs/` 是每次运行的全部步骤。一次性的数字写进 PR 和 Issue，不进仓库。

**合成大图库**（`tools/ui_perf/fixture.py`）：默认 3 万张、45 个加入日期（长尾，最多的一天两千多张）、300 个角色（人工确认的归属，约一半的图有角色）、30 部作品，另备 3000 张同步时才放进来的新图；`--images`、`--days` 等参数可调。图是 800 张画面池里的一张加上各自编号的文字块，每个文件字节不同，所以都是单独的图；长边 1000–1600，缩略图和中图都要真的缩小。内容只经 `library-create`、`library-scan` 和 HTTP 接口写入，和走查夹具一样；没有识别结果（没有评级、标签和向量）。加入日期取文件的创建时间，生成时按天设好，时区固定为东八区。缓存在仓库根的 `.perf.noindex/`（目录名以 `.noindex` 结尾，Spotlight 不扫）：图只取决于生成参数，换宿主程序要重建的只有图库（几分钟）；每次运行从 `pristine/` 恢复，缩略图缓存保留。宿主的 HOME、应用数据、共享资源都在夹具目录里，关掉自动同步、来源查询和索引检查，不碰真实图库和钥匙串。

**场景**（`__main__.py` 的 `SCENARIOS`）：首页滚动（空闲）、首页同步导入中（先停在最上面 24 秒看新图一批批进来，再往下滚）、图库网格滚动、大图翻页、资料库长列表。滚动是页面里每帧按时间推进 `scrollTop`（2400 像素/秒），两个内核一样。同步导入：测量脚本每 4 秒往来源文件夹放 150 张新图（复制出来的文件创建时间是当时，落在「今天」），页面每 4 秒经自己的同步入口（`startSync` + `pollSync`，和上手指引里的按钮一样）扫描导入；没有在产品里加测试钩子。每个场景每次新起宿主和浏览器。

**指标**：页内探针（`probe.js`，两个内核注入同一份）记帧间隔（`requestAnimationFrame`，长帧指超过 50ms）、长任务（只有 Chromium 有这个 API）、测量窗口里删掉的已显示图片及其中删的时候在视野里的数（同步导入时「可见的那一天整块重建」就是后者不为零）、元素和 `<img>` 数。Chromium 另录性能记录（`chromium.mjs`）：主线程的任务、样式重算、布局、绘制、脚本、GC，所有线程上的图片解码和光栅化。内存检查点读 `footprint`：Chromium 的 GPU 进程和渲染进程、WebKit 的 WebContent 和 GPU 进程，graphics 是名字带「(graphics)」的各类之和（`footprint.py`）。

**两个内核**：Chromium 是 chrome-headless-shell（查找同界面走查），默认 `--chromium-mode gpu`（`--enable-gpu --use-angle=metal`，本机实测 GPU 合成和光栅化都开着，图形内存才有意义）；`software` 是 `--disable-gpu`，图形内存不作数。WebKit 是 `webkit.swift` 编译成的小程序，系统 WKWebView 打开同一个地址，网页数据不落盘（用户 2026-11-19 定，#703 第 4 问；虚拟机里的性能数字不作数）。它会在屏幕左上角开一个不抢焦点的窗口，测量期间别挡住它，被挡住时 WebKit 会停掉绘制。两边视口 1440×900、dpr 2。

**改版后**：依赖页面结构的部分（入口地址、就绪标志、滚动容器、怎么开大图和翻页、怎么触发同步）只在 `probe.js` 开头的 `PAGES` 里，界面改了只改那几行；场景和指标不动，改前改后才能比。离线部分的检查是 `tests/test_ui_perf.py`。

## macOS 实机测试（虚拟机）

2026-10-15 起，在 Mac 上启动 App 的实机测试都在 [Tart](https://tart.run) 虚拟机里做（用户决定见 [AGENTS.md](../AGENTS.md#约定)「Mac 实机测试」一条）。虚拟机有自己的用户目录、应用数据和钥匙串，碰不到本机正式版的数据；本机只被占用磁盘，以及运行时的 CPU 和内存。

**排队**（2026-11-22 用户定，见 [AGENTS.md](../AGENTS.md#约定)「Mac 实机测试」一条）：不设专门的虚拟机验收会话，谁要测谁开，经 `collab_run.py run --slot vm` 排队，同一时间只开一台，测完关机、删掉副本。界面改动默认在隔离宿主的浏览器里看，只有 WebKit 才看得出的问题才进虚拟机。Windows 只在 x64 验收机（`--slot windows`）上验收，不再做 Windows ARM 虚拟机走查。

**母版与副本**：本机保留一台母版 `illustash-master`，来自 `ghcr.io/cirruslabs/macos-tahoe-base`（macOS 26），约 30 GB。镜像默认关闭 SIP，母版已在恢复模式里用 `csrutil enable` 打开 SIP；Gatekeeper 保持开启，母版里从没装过 Illustash。每次测试用 `tart clone illustash-master <名字>` 复制一份（APFS 写时复制，几乎不占额外空间），测完 `tart stop` 后 `tart delete` 删掉副本。母版只在用户明确同意时才修改或删除。镜像下载约 27 GB，大文件不经本机代理下载。

**启动与操作**：
- 启动用 `tart run --no-graphics --vnc-experimental [--dir=名字:路径:ro] <副本>`。不加 `--no-graphics` 时，tart 会在本机打开「屏幕共享」并占住 VNC 连接。VNC 只在测试期间开着，监听所有网卡，用的是随机口令。
- 同一时间只开一台虚拟机（2026-10-18，见 [AGENTS.md](../AGENTS.md#约定)「本机资源」一条）：把上面的命令包在 `collab_run.py run --slot vm -- tart run …` 里放后台运行，虚拟机运行期间占着 vm 槽，`tart stop` 后释放；别的会话要开虚拟机时会排队，`collab_run.py status` 能看到是谁开着。不能前台运行的虚拟机（例如 UTM 的 `utmctl start` 会立即返回），用 `collab_run.py run --slot vm --` 包住一个等它关机的命令。不测的时候就关掉，不要让虚拟机空跑。
- 截图、点击、输入走 VNC，不需要虚拟机里的系统授权。命令走 `tart exec`：它用的是虚拟化通道，不依赖网络，以当前登录的用户身份运行。`tart exec` 第一次访问「下载」或管理用户时，虚拟机里会弹出授权框，要用 VNC 点允许。
- VNC 下滚轮和 Cmd 组合键不可靠：滚动靠拖滚动条，退出等操作走菜单栏；在原生文件选择器里直接输入 `/`，就能打开「前往文件夹」。

**验收用法**：
- 新用户首次安装：用 `sysadminctl -addUser` 建一个标准用户，再用 `sysadminctl -autologin set` 设为自动登录；重启后走完系统设置向导，之后的步骤都在这个账户里做。
- Gatekeeper：要让 App 像从网上下载的一样，就在虚拟机里用 `python3 -m http.server --bind 127.0.0.1` 提供 zip，再用 Safari 下载。这样文件会带上 Safari 的隔离标记；直接拷进虚拟机的文件没有这个标记。
- 断网：在虚拟机里用 `networksetup -setnetworkserviceenabled Ethernet off` 关掉唯一的网络服务（需要管理员权限）。这时 `scutil --nwi` 显示 Not Reachable，DNS 解析失败，等同拔掉网线；恢复时把 `off` 改成 `on`。
- 验收结果按平台写在 Issue 或 PR 里，注明这是虚拟机，以及构建的提交和安装包的哈希。

**做不了的**：iCloud 照片（虚拟机没登录 Apple 账户）；性能和识别速度（虚拟机里的数字没有参考价值）；Windows（Apple 芯片上只能跑 ARM 版 Windows，和用户的 x64 环境不同，Windows 仍在 x64 验收机上做）。

## Windows 验收（常驻检出）

2026-11-12 起（#240），Windows 验收机上的定向检查和完整 Windows 检查都在常驻检出里增量编译，不再每次新建目录冷编译、跑完删掉。2026-11-17 起（#587）有两份常驻检出，同一时间能跑两个作业。验收机经本机的 SSH 别名访问，主机名、IP 和 SSH 配置不写进仓库。

**布局**：验收机上的 `D:\illustash-build\persistent`。`src` 和 `src2` 是两份互相独立的 git 检出（`src2` 从 `src` 本地克隆，各自的本地分支 `win-acceptance`）；`sccache` 是两份共用的编译缓存；`runs\<run-id>` 放每次的步骤日志、`status.txt` 和 sccache 统计；`tools` 放入口脚本（`run.cmd`、`run.ps1`、`pyrun.py`、`clean.ps1`）和 sccache。入口脚本和本机这一侧的驱动脚本 `win_run.sh` 都不进仓库，因为里面有 SSH 别名。

**两份检出**（#587）：
- `collab_run.py` 经环境变量 `COLLAB_RUN_SLOT_INDEX` 告诉 `win_run.sh` 拿到的是 `windows` 槽的第几个位置，0 用 `src`，1 用 `src2`；`win_run.sh --checkout src|src2` 可以手动指定。同一时间两个作业不会落到同一份检出上；验收机上每份检出另有一把锁（`src` 沿用 `lock.txt`，`src2` 是 `lock-src2.txt`），同一份已有作业在跑时报 `busy` 退出。
- 按检出分开的：锁、toolchain 记录、临时目录（`tmp\<检出>`）、送过去的 bundle 和 target。两份共用的：sccache 缓存和服务。服务不再每次结束时停掉，空闲 30 分钟后自己退出；统计不清零，每次运行记下开跑前后两份统计，`win_summary.py` 相减得出本次的命中数，另一份检出同时在跑时会混进它的数字（`status.txt` 里有 `concurrent` 一行）。设了 `SCCACHE_IGNORE_SERVER_IO_ERROR=1`，服务中途被停掉时编译退回直接用 rustc，不会失败。
- 每次运行先把 `tools\run.cmd` 复制成 `runs\<run-id>.cmd` 再执行，`run.ps1` 也把 `pyrun.py` 复制进运行目录，所以 `win_run.sh --deploy` 重新部署脚本不会打断正在跑的作业。
- `--jobs N` 设本次的 `CARGO_BUILD_JOBS`，默认值见下文「耗时」。

**一次运行**：
- 仓库是私有的，验收机没有 GitHub 凭据。本机打一个以验收机当前 HEAD 为基的增量 git bundle，经 scp 送过去。验收机 `git fetch` 以后 checkout 到 `win-acceptance`，再 `git clean -ffdx`，只保留 5 个 Cargo 项目的 `target`。各项目用仓库内默认的 target，不设 `CARGO_TARGET_DIR`。不用 detached HEAD，原因见 #327。
- 环境只在 `run.cmd` 里设置：vcvars、`RUSTC_WRAPPER`（sccache）、`ILLUSTASH_*`。不改验收机的全局配置。`.cargo/config.toml` 的 crt-static 照常生效。
- 完整检查的 14 步：core、platform、terms、desktop 的 clippy，core、platform、desktop、terms、iqdb 的 test，js，terms、examples、iqdb 的 release 构建，Python 全套。也可以只跑其中几步，或者另加定向 unittest。
- 长任务经 WMI 启动，SSH 断开或本机休眠都不会中断。本机用 `collab_run.py run --slot windows -- win_run.sh …` 包住，轮询 `status.txt`，直到出现 `done`。

**清理规则**（2026-11-12 定，2026-11-17 扩展到两份检出）：入口脚本每次同步之后检查本次这份检出，命中下面任一条就清掉它 6 个项目的 target（检出保留，另一份不动）：

- `rustc -V` 和这份检出上次记录的不同；
- 这份检出的 target 合计超过 40 GB（一次完整构建约 14.5 GB）；
- D 盘剩余空间低于 30 GB。

两份检出的 target 最多约 80 GB，加上 sccache 上限 20 GB，2026-11-17 时 D 盘剩余约 145 GB，够用。Cargo.lock 变了不清，交给增量编译处理。sccache 上限 20 GB，超了自动淘汰最旧的。`win_run.sh --clean all` 只清本次这份检出的 target；`--clean cache` 再清共用的 sccache 缓存，另一份检出有作业在跑时跳过这一项（`status.txt` 写 `clean-sccache-skipped`）。手动全清在验收机上运行 `powershell -NoProfile -ExecutionPolicy Bypass -File D:\illustash-build\persistent\tools\clean.ps1`：默认清两份检出的 target 和 sccache 缓存，`-Checkout src|src2` 只清一份，`-TargetsOnly` 只清 target；有作业在跑的检出拒绝清，sccache 缓存只在两份都空闲时才删。需要隔离的特殊验收（打包、安装）仍可以临时建一个一次性目录，用完删掉。

`tools` 里还有一些会话按 #587 之前的 `run.ps1` 复制的一次性入口脚本，它们只认 `src` 和 `lock.txt`，开跑和结束时会停掉 sccache 服务。新的入口照 `run.ps1` 写：按检出取锁，不停共用的服务。

**耗时**（2026-11-12，head `687bb63`，完整 14 步，`CARGO_BUILD_JOBS=4`，单次）：

| 情况 | 墙钟 |
|---|---|
| 冷编译（target、sccache 都是空的） | 15 分 46 秒 |
| 同一 head 重跑 | 5 分 59 秒 |
| 换到改了 native/terms 一处的 head | 7 分 34 秒 |
| target 被清掉、sccache 是热的（命中 89%） | 10 分 18 秒 |

重跑的下限主要是测试执行：terms 测试约 2 分 10 秒，Python 全套约 2 分 20 秒。#327 修好之前，普通检出里每次 cargo 调用都会重编 terms，重跑和小改动各因此多约 1 分钟。各步的详细数字见 #240。

**并发数和两个作业同时跑**（2026-11-17，#587）：`CARGO_BUILD_JOBS` 默认 8。测的是开发期最常见的定向检查 `terms-clippy,terms-test`，head `49703ec4`，每次先 `--clean all` 清掉这份检出的 target、sccache 是热的（相当于一份检出第一次编这个 head），每项单次，经 `collab_run.py run --slot windows --exclusive` 独占验收机：

| 情况 | 每个作业的墙钟 | 两个作业都跑完 |
|---|---|---|
| 单作业，`CARGO_BUILD_JOBS=4` | 4 分 56 秒 | — |
| 单作业，8 | 4 分 42 秒 | 依次跑约 9 分 24 秒 |
| 单作业，16 | 4 分 44 秒 | — |
| 两个作业同时跑，各 8 | 6 分 40 秒 / 6 分 46 秒 | 6 分 46 秒 |
| 两个作业同时跑，各 4 | 6 分 22 秒 / 6 分 29 秒 | 6 分 29 秒 |

单作业时并发数 4、8、16 分不开：这项检查的大头是 terms 自己的编译和测试执行，依赖都从 sccache 取（Rust 命中 88.7%）。两个作业同时跑，每个慢约四成，但两个一起跑完比依次跑快约 30%。各 4 和各 8 只差约 4%，都在单次测量的误差以内。默认取 8 的理由：只有一个作业时，依赖没命中的宽构建（换工具链、Cargo.lock 大改、release）能用满 16 个线程；两个作业同时跑时合计 16，正好等于线程数。要改就用 `win_run.sh --jobs N`。

**注意**：
- cmd 和 Windows PowerShell 5.1 按系统代码页（GBK）读没有 BOM 的脚本，UTF-8 中文注释可能吞掉换行：`.cmd` 只写 ASCII，`.ps1` 要带 UTF-8 BOM。
- PowerShell 变量名不分大小写，循环变量会覆盖同名的大写变量。WMI 启动的进程，工作目录在系统目录下，脚本里一律用绝对路径。
- 回传的输出里有 GBK 字节，用 `grep -a` 判断。验收机没装 rustfmt，fmt 在本机跑。
