# 第三方组件与许可

状态：现行参考，2026-11-16。本项目代码以 GPL-3.0-or-later 发布（见仓库根目录 `LICENSE`），另附一项 GPLv3 第 7 条附加许可（见下文[附加许可](#附加许可gplv3-第-7-条)）；本文列出随代码、安装包或模型包分发的第三方组件、应用第一次使用时下载的模型，以及它们的许可。模型与数据不受本项目代码许可约束，按各自许可使用。Illustash 名称与图标不随代码授权。

## 附加许可（GPLv3 第 7 条）

2026-10-22 用户定（#271）。`LICENSE` 保持 GPL 原文不动；Illustash 的版权人按 GPLv3 第 7 条另外给出下面这项许可，允许本程序与 DirectML 这类 GPU 推理运行库组合并一起分发、运行。它只覆盖这类运行库，不放宽对其他库的要求，也不改变这些运行库自己的许可。本文随每个分发产物放进 `resources/licenses/THIRD_PARTY.md`。正文以英文为准：

> **Additional permission under GNU GPL version 3 section 7**
>
> If you modify this Program, or any covered work, by linking or combining it with a GPU inference runtime library, that is, a library distributed by its vendor in binary form that the Program loads at run time only to run machine-learning models on graphics or other accelerator hardware (such as Microsoft DirectML, and Windows ML or a hardware vendor's execution provider for ONNX Runtime), or with a modified version of such a library, containing parts covered by the terms of that library's license, the copyright holders of Illustash grant you additional permission to convey the resulting work. Corresponding Source for a non-source form of such a combination shall not include the source code for the parts of the library that are not covered by the GNU GPL.

## Rust 依赖

完整依赖及版本由 `native/terms/Cargo.lock`、`desktop/Cargo.lock`、`crates/iqdb/Cargo.lock` 固定。2026-10-16 清点：三个 workspace 共 518 个第三方 crate（含 vendored 的 `tiny_http`），几乎全部为 MIT、Apache-2.0、BSD、ISC、Zlib、Unicode-3.0 等宽松许可或其多选一组合；`desktop` 另有 5 个 MPL-2.0 crate（Tauri 生态，按文件级开源义务使用）。标注含 LGPL 的 crate 均为与 MIT/Apache-2.0 多选一，按后者使用。上述许可均与 GPL-3.0 兼容。

**随包的许可正文**（2026-10-16，#161）：`packaging/rust-licenses/rust-dependencies.txt` 收录每个第三方 crate 的许可正文，随每个分发产物放进 `resources/licenses/rust-dependencies.txt`，并在 manifest 里登记。文件开头按 crate 列出名称、版本、声明的许可表达式和适用正文的编号，后面是去重后的正文；多选一的 crate 只收录选用的那一份，优先顺序见 `about.toml` 的 `accepted`。正文取自各 crate 发布包里的许可文件（含子目录里嵌入的上游代码许可，如 mozjpeg-sys 带的 libjpeg-turbo）；没带许可文件的 crate 用 SPDX 标准文本。

- 生成：`tools/gen_rust_licenses.py` 调用固定版本的 [cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9.2（MIT OR Apache-2.0），离线读本机 cargo 注册表里的 crate 源码，按全部 feature、全部目标平台解析三个 workspace，构建期和测试用 crate 也收录。结果和 `crates.json` 一起提交；`crates.json` 登记覆盖的 crate、正文的 SHA-256，以及锁文件里列出但任何 feature 组合都不会编译的可选依赖（`not_built`，不附正文）。打包脚本和检查都不需要 cargo-about，也不下载任何东西。
- 检查：打包前和 `tests/test_packaging_resources.py` 都比对 `crates.json` 与三份 `Cargo.lock`。附了正文的 crate 加上 `not_built` 必须恰好等于锁文件里的全部第三方包（按名称加版本），正文哈希必须与登记一致；不一致时测试失败、打包在产出任何文件之前拒绝执行。我们自己的 crate（`illustash-*`，GPL-3.0-or-later）不列入。
- 更新：改了任何 `Cargo.lock` 或依赖的 feature 之后都要重新生成。只改 feature、没改锁文件却启用了某个 `not_built` 包时，上面的检查发现不了，必须手动重跑。新依赖用了 `accepted` 以外的许可时，生成会失败，先确认与 GPL-3.0 兼容再加进 `about.toml`。

```bash
cargo install --locked --features cli cargo-about@0.9.2
```

```bash
python3 tools/gen_rust_licenses.py
```

生成要求三个 workspace 的全部 crate 源码已在本机 cargo 注册表里（包括其他平台才用到的 crate）；缺的话先对每个 workspace 运行 `cargo fetch --locked --manifest-path <workspace>/Cargo.toml`。

## 随源码分发

| 组件 | 位置 | 许可 |
|---|---|---|
| PocketFFT | `native/terms/vendor/pocketfft/` | BSD-3-Clause，原文 `native/terms/vendor/pocketfft/LICENSE.md` |
| LittleCMS 2.19 及 lcms2 封装层 | Rust 依赖 | MIT，原文 `native/terms/licenses/LCMS.txt` |
| Unicode 字符数据 | `crates/core` 的 casefold 与 HTTP 字符表，由 `tools/gen_casefold.py`、`tools/gen_http_unicode.py` 从 Python `unicodedata`（Unicode 15.0）生成 | [Unicode License v3](https://www.unicode.org/license.txt) |
| 标签表 | `tags/tags_v3.csv` | 取自 WD tagger 模型仓库的 `selected_tags.csv`（Apache-2.0）；标签名与计数源自 [Danbooru](https://danbooru.donmai.us) |
| 画面标签对照表 | `tags/general_names.json` | 简体中文译名和社区别名取自 danbooru-tags-data-zh（MIT），日文名取自 CharaDB 的标签数据（CC0-1.0）。固定来源见下节 |

每个分发产物（便携包、安装包）的 `resources/licenses/` 里有本项目 `LICENSE`、本文（`THIRD_PARTY.md`）、PocketFFT、LittleCMS 和画面标签对照表的许可原文，以及 Rust 依赖的许可正文 `rust-dependencies.txt`（见上节），见 [安装与平台](BACKEND_TASKS.md#独立安装与可选计算包)。

### 画面标签对照表

- 中文译名及社区别名使用 danbooru-tags-data-zh 的 [`tags/general.csv`](https://github.com/amenorira/danbooru-tags-data-zh/blob/8e73f3b1c15b30f873c11a8c65d2d8264cdf0224/tags/general.csv)。该版本的 README 将项目声明为 MIT。README 末尾另有社区交流与学习的用途说明，本项目按同一版本 [MIT 许可原文](https://github.com/amenorira/danbooru-tags-data-zh/blob/8e73f3b1c15b30f873c11a8c65d2d8264cdf0224/LICENSE) 给出的再分发条款使用，并保留 amenorira 的版权声明和许可原文。
- 日文名使用 CharaDB 的 [`data/items_public.json`](https://github.com/taki-ttt/charadb/blob/7fe2e4b009ad6d85496101e929ac8528cb21d357/data/items_public.json)。其 [许可原文第 2 节](https://github.com/taki-ttt/charadb/blob/7fe2e4b009ad6d85496101e929ac8528cb21d357/LICENSE) 明确把该文件的标签数据按 CC0-1.0 发布。只取能直接对应单个 WD 标签的日文名。保留两份上游许可，见 [中文数据](../tags/general_names_zh_LICENSE.txt) 和 [日文数据](../tags/general_names_ja_LICENSE.txt)。

| 输入或产物 | SHA-256 |
|---|---|
| `tags/tags_v3.csv` | `298633d94d0031d2081c0893f29c82eab7f0df00b08483ba8f29d1e979441217` |
| danbooru-tags-data-zh 固定版 `tags/general.csv` | `4df46d24d61ee32b96d9aa79ae390b0c35bdd99ce15d591b617dc949e92c4240` |
| CharaDB 固定版 `data/items_public.json` | `7d3afccb974dbec08b03b33cd8ba922849903af89ce51eb6428d63f0158f5e9e` |
| `tags/general_names.json` | `958e30a1d99f37f6497d9ecf2dbc7ed1ef173b80637f6937d94a2ec4c0ec1ab6` |

在仓库根目录运行 [`python3 tools/build_general_tag_names.py`](../tools/build_general_tag_names.py) 下载固定版本的两个数据文件、核对三个输入的 SHA-256 并重建。已有核对过的输入时，传 `--zh general.csv --ja items_public.json` 离线重建。生成脚本只取 WD v3 的 8,106 个 `category=0` 标签，以原 tag 为键，提取译名和别名字段。生成文件为 459,155 字节，共 7,773 个条目，其中 7,750 个含中文名、56 个含单列日文名。该文件编入原生宿主，安装和使用均可离线。

## 打包时加入的运行库

构建脚本不下载任何文件，以下组件由打包者显式提供，许可原文按原字节放入安装包：

| 组件 | 平台 | 许可 |
|---|---|---|
| ONNX Runtime | macOS、Windows | MIT；随包附 `LICENSE` 与 `ThirdPartyNotices.txt`。上游官方构建默认开启遥测，本项目在每个加载它的进程里关掉（#381），见 [装不上、打不开怎么办](INSTALL.md#识别加速macos) |
| OpenBLAS（取自固定版本 NumPy wheel） | Windows | BSD-3-Clause；随包附 wheel 的完整许可目录 |
| DirectML 1.15.4（NuGet `Microsoft.AI.DirectML`，ONNX Runtime 构建时取用的同一个包，#271） | Windows | 「Microsoft Software License Terms」专有许可，允许随用到它的 Windows 应用分发（第 1 条）；头文件等示例代码为 MIT（`LICENSE-CODE.txt`）。随包附 `LICENSE.txt`、`LICENSE-CODE.txt` 与 `ThirdPartyNotices.txt` 原文。它与本项目的组合由下文[附加许可](#附加许可gplv3-第-7-条)允许。该许可第 2 条写明软件可能收集使用信息并发送给微软，见 [装不上、打不开怎么办](INSTALL.md#windows) |

## 模型与数据

仓库不含模型。2026-10-16 用户决定（#168）：首个内测版起发布的安装包也不带模型，应用第一次使用时从上游下载；按白名单带上模型的完整安装包只用于开发和测试。下载与随包用的是同一组文件，见下表；逐文件的大小、SHA-256 和下载地址写在应用内置的清单 `native/terms/models.json` 里，使用者手动下载的说明见 [装不上、打不开怎么办](INSTALL.md#识别模型)。

| 名称 | 文件的来源 | 用途 | 许可 |
|---|---|---|---|
| WD SwinV2 Tagger v3 | [deepghs/wd14_tagger_with_embeddings](https://huggingface.co/deepghs/wd14_tagger_with_embeddings) 的 `SmilingWolf/wd-swinv2-tagger-v3/model.onnx`（DeepGHS 加了向量输出的重导出），原模型 [SmilingWolf/wd-swinv2-tagger-v3](https://huggingface.co/SmilingWolf/wd-swinv2-tagger-v3) | 图片标签与角色识别 | Apache-2.0 |
| CCIP | [deepghs/ccip_onnx](https://huggingface.co/deepghs/ccip_onnx) 的 `ccip-caformer-24-randaug-pruned/`，原模型 [deepghs/ccip](https://huggingface.co/deepghs/ccip) | 角色特征比对 | 上游声明 OpenRAIL；随包按 BigScience Open RAIL-M 分发 |

**下载来源**（2026-10-19，#168）：应用依次尝试两个来源：Hugging Face 上游仓库固定提交（WD `4ec31f2`、CCIP `eb2acdd`）的地址，经可配置的镜像（默认 hf-mirror.com）；ModelScope 上同名仓库 `deepghs/wd14_tagger_with_embeddings`、`deepghs/ccip_onnx` 的副本。ModelScope 的两个仓库不是 DeepGHS 官方维护的（平台账号搬运，最后更新于 2024-12 / 2025-02），2026-10-16 核对时文件与上游逐字节相同。应用只认清单里的 SHA-256，所以来源不影响拿到的文件。原始 WD/CCIP 模型由用户从上述来源取得；许可原文仍随每个计算包附上（见下一段），关于页照常写明 CCIP 的使用限制。

2026-11-12 用户批准（#375）：macOS 的 WD Core ML 执行变体由本项目 GitHub 模型 Release 分发，按 Apache-2.0 附许可原文、原始来源及变换说明。它基于上述固定 WD 模型，固定 batch 为 1，降低六维形状、删除等价广播、分解 ReduceL2 并预转置常量矩阵；不量化、不训练，不改原始模型文件。构建入口 `tools/build_wd_coreml.py`，固定版本与 SHA-256 见 `native/terms/models.json`，随包声明见 `packaging/model-licenses/NOTICE.txt`。原始模型仍是 CPU 回退和结果身份的唯一依据。

WD 角色扩展 v1（#534）从 Illustash 0.3 起支持，通过项目的 GitHub 模型 Release `wd-extension-v1` 单独分发 `wd-extension-v1.bin`（应用下载）、`wd-extension-v1.zip`（手动下载）和 `SHA256SUMS.txt`。二者中的同一冻结模型文件为 940,701 字节，SHA-256 为 `9da0365457e479eff189150926289c6cf8325cd9bb98c0bd2504c26d7dcae471`，含 228 个分类头和固定阈值 `0.8008725047111511`。随安装包提供模型公告、Apache-2.0 许可和 Character Index 声明的 MIT 许可文本。复封入口见 [构建脚本](../tools/build_wd_extension.py)，启用与撤回见 [安装说明](INSTALL.md#角色资料包)。

角色词表只分发 tag、公开关系的制作端判断、外部编号和取名位置。Danbooru wiki 正文、作为比对材料的 zh-tags 译文和 Bangumi 名称均留在制作端；应用通过 API 获取 Bangumi 名字并保留来源。Wikidata 结构化数据按 CC0 发布。来源见 [Bangumi 公开归档](https://github.com/bangumi/Archive)、[Bangumi 版权和开发者协议](https://bgm.tv/about/copyright) 与 [Wikidata 许可](https://www.wikidata.org/wiki/Wikidata:Licensing)。

2026-11-17 起（#602），WD 角色扩展和下面的 CCIP 长尾参考包合成一个「角色资料包」：上面的 v1 和长尾参考包 v1 合起来是内置的角色资料包 0.1（测试版）；版本号见 [开发说明](DEVELOPMENT.md#发布角色资料包)。以后的版本不随应用发布，列在签名的包目录里（见 [开发说明](DEVELOPMENT.md#发布角色资料包)），每个版本有自己的 Release：包文件、离线包和其中附带的许可文本。目录条目给整个包和每个组成部分各写一个许可（WD 部分 Apache-2.0，CCIP 部分 MIT），每个版本的数据来源、评测口径和变化写在那个版本的 Release 正文和对应的研究 Issue 里；图片、逐图结果和角色明细不公开。

扩展文件按 Apache-2.0 分发。分类头用公开 Danbooru 样本在冻结的 WD 向量上独立拟合，底座为 SmilingWolf 的 WD SwinV2 Tagger v3 经 DeepGHS 重导出的向量模型（固定提交 `4ec31f2aeabf66353f3ee3ccd20d2f07e716851b`）。DeepGHS Character Index（MIT，固定提交 `5ee6075ff09f82c27e23f33044b9f9cabaf1e8c3`）用于筛选原索引未覆盖的角色。扩展模型文件包含词表、参数、阈值与底座指纹。ZIP 附 Apache-2.0 原文、两份 WD 模型卡、Character Index 的归属和 MIT 许可、`NOTICE.txt` 与 `provenance.json`。原始图片的版权归各原作者。[实验输入和评测口径](https://github.com/shio-vinyl/illustash/issues/534#issuecomment-6907588551)

**基础角色索引**（2026-11-17 用户定，#607）：从 Illustash 0.3 起属于识别模型的下载清单，和 WD 加速文件一样由项目的 GitHub 模型 Release `character-index-v1` 分发 `character-index-v1.bin`，固定大小和 SHA-256 写在 `native/terms/models.json` 的 `character_index` 里；首次准备时和 WD、CCIP 一起默认下载，不随安装包带，缺它不挡识别。包由 [制包脚本](../tools/build_character_index.py) 从上述固定提交的角色元数据和 CCIP 参考特征算出：每个角色一个锚点（参考向量归一化后的等权中心乘回平均模长）和 p50、p75、p90 三档距离阈值，共 7,367 个角色；不含原始参考向量和图片。包是不压缩的 ZIP，附 `provenance.json`（上游提交、元数据和文件树的 SHA-256、兼容的 CCIP 模型 SHA-256、成员 SHA-256、制包环境）、Character Index 的 MIT 许可与归属及 `NOTICE.txt`。锚点由 CCIP 模型的特征算出，CCIP 部分照上文的 Open RAIL-M 说明办理。包为 25,787,626 字节，SHA-256 为 `ce814cbcd9619babde566f46d80f7d9b0ce9f1c4dff20d8c679ac2bb0fa1fcb0`。启用与撤回见 [图库](LIBRARIES.md#基础角色索引)。

**随包的许可原文**（2026-10-16，#8；2026-10-19 起不论带不带模型，#168）：原文提交在 `packaging/model-licenses/`，是打包的唯一来源。`sources.json` 登记每个文件的上游地址和 SHA-256。每个计算包（`--research`）都把这个目录整份按原字节放进 `resources/licenses/models/`；文件缺失、多出、内容与登记不符，打包都会报错，不会产出半成品。Windows 和 macOS 安装包的暂存步骤也把这些文件列为必需组件。

- WD：Apache-2.0 原文（apache.org 的 `LICENSE-2.0.txt`），加上两个上游仓库在固定 revision 下的模型卡。两个仓库都没有 NOTICE 文件。
- CCIP：两个上游模型卡只写了 `license: openrail`。Hugging Face 对这个标识的定义是「OpenRAIL 许可家族」，没有对应的条款文本。2026-10-16 用户决定：按最通用的 BigScience Open RAIL-M（2022-08-18 版，BigScience 发布页链接的 PDF 原件）随包，同时附两份模型卡。该许可第 5 条及附件 A 的使用限制对使用者生效，再分发时必须一并传递。同日查过几个使用 CCIP 的开源项目：多数写「OpenRAIL，以模型卡为准」，不附条款原文，且运行时才下载、不随包；没有一家附了确定的条款文本。
- `NOTICE.txt`（英文）列出每个模型文件的上游仓库、revision、SHA-256 和对应的许可文件。CCIP 一节就是该模型的分发条款：写明随包的 CCIP 按 Open RAIL-M 许可给使用者，附件 A 的使用限制是条款中可执行的部分，再分发者须把限制写进自己的分发协议、告知接收者并附上许可。这满足该许可第 4(a)(b) 条。
- 2026-10-16 用户决定（#163）：不另设需要点击接受的协议，安装向导和首次启动都不加确认；应用「关于」页的「致谢与第三方组件」和 README「许可」一节写明 CCIP 受 Open RAIL-M 第 5 条约束及原文位置。不向上游 DeepGHS 征询具体条款文本；上游日后指明别的版本时，替换原文并改 `NOTICE.txt` 的版本说明。
- 更新上游许可或模型时，替换原文、改 `sources.json` 的哈希，并同步 `NOTICE.txt`。
| Character Index | [deepghs/character_index](https://huggingface.co/datasets/deepghs/character_index) 固定提交 `5ee6075ff09f82c27e23f33044b9f9cabaf1e8c3` | 基础角色索引的来源数据（见下文）；「检查新角色」补的角色另从 Danbooru 公开图片取参考（每次运行前确认） | MIT |

应用「关于」页另列出致谢的上游项目与数据源（Danbooru、Bangumi 等）；联网获取的数据按各站点条款使用，见 [找回画师](PROVENANCE.md) 与 [名字](TERMS_NAMES.md#9-显示名来源与后台更新)。

长尾参考包使用上述 CCIP 特征模型，从公开 Danbooru 缩图生成参考和锚点。原图版权归原作者；包携带特征、阈值和来源清单。冻结的 Character Index 基础索引用于筛选和评估，版权和 MIT 归属按上游保留。包版本、样本范围和复跑口径见 [#557](https://github.com/shio-vinyl/illustash/issues/557#issuecomment-6915312907)。
