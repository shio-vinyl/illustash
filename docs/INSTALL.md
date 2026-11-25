# 装不上、打不开怎么办

现行参考，2026-11-18。面向使用者，说明安装、首次启动、下载识别模型和更新时常见的拦截与报错。macOS 部分按 2026-10-15、10-16、10-18、11-14 在 macOS 26.6（arm64）虚拟机里的实测写成；Windows 部分标明了哪些已经实测。开发者从源码构建见 [开发说明](DEVELOPMENT.md)。

安装包在 [GitHub Releases](https://github.com/shio-vinyl/illustash/releases) 下载，0.1.0 起正式发布。安装包没有经过 Apple 公证或 Windows 代码签名，所以系统第一次打开时会拦截，按下面的步骤放行一次即可。

## macOS

只支持 Apple 芯片（M 系列）的 Mac，需要 macOS 14 或更新版本；Intel 芯片的 Mac 打不开。

### 「无法打开"Illustash"」

第一次双击时，系统会提示无法验证 Illustash 是否包含恶意软件，只有「移到废纸篓」和「完成」两个按钮。这是因为安装包没有经过 Apple 公证，不代表文件有问题。

1. 点「完成」（不要点「移到废纸篓」）。
2. 打开「系统设置 › 隐私与安全性」，往下翻到「安全性」，会看到「已阻止"Illustash"以保护 Mac」，点旁边的「仍要打开」。
3. 再弹一次确认框时，选「仍要打开」。标准（非管理员）账户这时还要输入一位管理员的用户名和密码。

放行一次之后，以后双击就能直接打开。2026-10-18 复测时，管理员账户在第 3 步也被要求输入密码。

### 更新到新版本之后

有新版本时，可以在应用里点「下载并重启」，也可以自己下载新版本覆盖安装。两种办法的图库、设置、识别模型和其他图库文件夹都不受影响。

**在应用里「下载并重启」**（2026-11-14 在 macOS 26.6 虚拟机里用合成的新旧两个版本实测，#279）：

- 不用再放行：应用自己下载的新版本没有「来自网络」的隔离标记，打开时不会出现「无法打开"Illustash"」。
- iCloud 照片不用重新允许：更新后第一次同步照常读取照片，「系统设置 › 隐私与安全性 › 照片」里仍是完全访问。
- 只有 Illustash 放在「应用程序」里、从那里打开时才能这样更新；从「下载」文件夹直接打开时，设置里会提示先把它拖进「应用程序」。
- 用标准（非管理员）账户、Illustash 又是管理员装进「应用程序」的话，替换时系统可能要求输入管理员的用户名和密码。**这一条还没有实测。**

**自己下载新版本覆盖安装**：安装包用的是临时签名，每个版本的签名都不一样，macOS 会把新下载的版本当成另一个 App。2026-10-18 在虚拟机里用两个不同的构建实测，从网上下载新版本、覆盖安装之后：

- **要重新放行一次**：第一次打开新版本时，又会出现上面的「无法打开"Illustash"」，按同样的三步放行。
- **iCloud 照片要重新允许**：用 iCloud 照片作来源的图库，更新后第一次同步时，会再次询问是否允许 Illustash 访问照片图库，选「允许完全访问」。
- 如果选了「不允许」，任务面板会提示「没有访问照片图库的权限」（新图库的上手引导期间首页顶上也会提示），已经加入的图都在。点提示旁的「打开系统设置」（直接打开「系统设置 › 隐私与安全性 › 照片」），打开 Illustash 的开关，在系统的提示里选「退出并重新打开」，重开后同步就能读到照片（默认启动时会自动同步）。这一步不需要输入密码（2026-10-19 虚拟机实测，#202）。

### 拖进「应用程序」时要求输入管理员密码

用标准（非管理员）账户时，把 Illustash 拖进「应用程序」需要输入一位管理员的用户名和密码。这是 macOS 对标准账户的正常要求。

### 「完整安装包组件缺失：…；请重新安装 macOS 完整版」

安装包里的某个文件不见了或损坏了（例如只拷了一部分）。重新下载完整安装包，覆盖安装后**退出并重新打开** Illustash。只点「重试」不会生效，界面上也会这样提示。

### 「识别组件没能启动」

图片仍会加入图库，可以手动整理，但不会自动认出角色。先在提示里点「重新检查」；还不行的话，按下面「报告问题」附上诊断信息。

## 识别模型

安装包里没有带识别模型。第一次使用时，应用会提示「识别要用的模型还没下载」，原始模型约 618 MB，另有约 26 MB 的基础角色索引，Apple 芯片的 macOS 再加约 467 MB 的 WD 加速文件（合计约 1.1 GB）；下载前可以先建库、浏览和手动整理。原始模型已就位时，首次准备结束后自动开始识别。模型放在用户目录里，所有图库共用，以后更新 Illustash 不用再下：

- macOS：`~/Library/Application Support/Illustash/Shared/models/`
- Windows：`%LOCALAPPDATA%\Illustash\Shared\models\`

每个文件都按应用里写好的大小和 SHA-256 核对，对不上就丢弃，所以从哪里下载都一样安全。下载中断、取消或者中途关掉应用，下好的部分都会保留，下次接着下。自动重试时，核对已下载的部分期间保留进度，暂时不显示速度。来源不支持续传或断点失效时，应用从头下载该文件。「设置 › 数据 › 识别模型」里可以删掉未完成的部分；不再需要时也可以在那里删除已下载的模型，之后要重新下载或导入才能识别新加入的图片，已经识别的结果都保留。卸载 Illustash 不会删除下载的模型。

### 下载很慢或下载失败

原始模型先从 Hugging Face 的镜像（默认 hf-mirror.com）下载，不行再换 ModelScope；基础角色索引和 macOS 的 WD 加速文件从项目 GitHub Release 下载，不经过镜像。下载走系统设置里的 HTTP 代理；不支持自动代理配置（PAC）。

- **换镜像**：在「设置 › 数据 › 识别模型」里把「Hugging Face 镜像」换成其他兼容 Hugging Face 的地址；填 `https://huggingface.co` 就是直连 Hugging Face。
- **自己下载再导入**：用浏览器或下载工具把下表 4 个文件下载下来，在应用里点「从文件导入…」选中它们（可以一次多选，也可以选包含它们的 ZIP）。文件名和所在文件夹都不重要，应用按大小和 SHA-256 认出每个文件，放到正确的位置；对不上的文件会被拒绝。

| 文件 | 大小（字节） | SHA-256 | Hugging Face | ModelScope |
|---|---|---|---|---|
| WD SwinV2 Tagger v3 | 467,461,052 | `ae25f5f12b2d5891728fb7768fa63d65bf13a83d6d4823e3a1bf7b42875c74ff` | [model.onnx](https://huggingface.co/deepghs/wd14_tagger_with_embeddings/resolve/4ec31f2aeabf66353f3ee3ccd20d2f07e716851b/SmilingWolf/wd-swinv2-tagger-v3/model.onnx) | [model.onnx](https://modelscope.cn/models/deepghs/wd14_tagger_with_embeddings/resolve/master/SmilingWolf/wd-swinv2-tagger-v3/model.onnx) |
| CCIP 特征模型 | 150,248,245 | `4ea118d16496274f4f6e08d3afc768cc592389e8f7f32f8732ce2215c228ac5f` | [model_feat.onnx](https://huggingface.co/deepghs/ccip_onnx/resolve/eb2acdd29af1703388d3d0c04221add322bc9110/ccip-caformer-24-randaug-pruned/model_feat.onnx) | [model_feat.onnx](https://modelscope.cn/models/deepghs/ccip_onnx/resolve/master/ccip-caformer-24-randaug-pruned/model_feat.onnx) |
| CCIP 比对模型 | 1,649 | `7e4646fdfbe369ad485b227ec47ac4092c98b8deef3af44e19fdbd8ba7bc25c1` | [model_metrics.onnx](https://huggingface.co/deepghs/ccip_onnx/resolve/eb2acdd29af1703388d3d0c04221add322bc9110/ccip-caformer-24-randaug-pruned/model_metrics.onnx) | [model_metrics.onnx](https://modelscope.cn/models/deepghs/ccip_onnx/resolve/master/ccip-caformer-24-randaug-pruned/model_metrics.onnx) |
| CCIP 参数 | 147 | `b5535577288e22940b853192cd1661e45c7615b7d89b49a8745c2c81685552d4` | [metrics.json](https://huggingface.co/deepghs/ccip_onnx/resolve/eb2acdd29af1703388d3d0c04221add322bc9110/ccip-caformer-24-randaug-pruned/metrics.json) | [metrics.json](https://modelscope.cn/models/deepghs/ccip_onnx/resolve/master/ccip-caformer-24-randaug-pruned/metrics.json) |

Hugging Face 的地址固定在上游的某个提交上，上游以后改动也不影响；镜像站把 `https://huggingface.co` 换成镜像地址即可。下载后可以自己核对：macOS 用 `shasum -a 256 文件`，Windows 用 `certutil -hashfile 文件 SHA256`。模型的许可见 [第三方组件与许可](THIRD_PARTY.md#模型与数据)。

### 基础角色索引

基础角色索引让识别认得约 7,400 个角色（只有原始模型时只认得 WD 词表里的约 2,700 个），和识别模型一起下载，所有图库共用。首次准备会等当前下载任务结束。索引下好时，打开的图库先启用它再自动同步。索引没下成或取消下载时，已经就位的原始模型可继续识别，「设置 › 数据 › 识别模型」会提示补下载。以后补下载索引期间，已有模型可以照常用于识别。打开的图库会在空闲时自动启用索引。在同一处可以撤回或再启用，撤回对这台电脑上的所有图库生效，你确认过和移出的结论都保留。手动下载 [基础角色索引 v1](https://github.com/shio-vinyl/illustash/releases/download/character-index-v1/character-index-v1.bin) 后也可从设置导入：文件为 25,787,626 字节，SHA-256 为 `ce814cbcd9619babde566f46d80f7d9b0ce9f1c4dff20d8c679ac2bb0fa1fcb0`。

### 识别加速（macOS）

在 Apple 芯片的 Mac 上，识别默认通过 Core ML 加速标签与角色特征计算。WD 加速文件由项目模型 Release 提供，首次下载模型时一并下载；已有安装可在「设置 › 数据 › 识别模型」补下载。手动下载 [WD 加速文件 v1](https://github.com/shio-vinyl/illustash/releases/download/wd-coreml-v1/wd-swinv2-v3-v1.onnx) 后也可从设置导入：文件为 467,387,780 字节，SHA-256 为 `57344d061ecaa0b8addb9cc33ae1a133a5bb18525590fdc61edb2aacdad2e444`。它需要与原始 WD 模型共同保留，不能用它替换原始文件。

首次识别会准备编译缓存，WD 和 CCIP 分别保存在 `~/Library/Application Support/Illustash/Shared/coreml-cache/` 下。模型、运行库或 macOS 版本变化时，相应模型的缓存会重新生成。加速文件缺失、损坏，或编译、运行失败时，识别自动回到原始模型 CPU，设置页显示原因；已有图库和模型身份不受影响。想只用 CPU，关闭「使用 GPU 加速识别」，下次识别时生效。

**隐私**（#381）：macOS 随包的 ONNX Runtime 是微软官方发布的版本，默认开启遥测，会把使用事件发给微软（`mobile.events.data.microsoft.com`），并在 `~/Library/Application Support/Microsoft/DeveloperTools/.onnxruntime/` 写一个设备标识。Illustash 启动每个识别、索引进程时都设 `ORT_DISABLE_TELEMETRY=1`，并在初始化时调用关闭遥测的接口，所以它不联网，也不写那个目录。如果你在 Illustash 之外用过别的 ONNX Runtime 程序，那个目录可能是它们留下的。

<!-- 旧锚点：0.2 及更早的应用里 WD 角色扩展卡片的「手动下载说明」链到这里 -->
<a id="可选的-wd-角色扩展包"></a>

### 角色资料包

角色资料包让识别多认一批 WD 词表和基础角色索引里没有的角色，在「设置 › 角色资料」的「角色资料包」卡片上管理：一个包、一个开关，默认启用，所有图库共用同一份文件，每个图库分别记启用与否。包由两个组成部分合成（#602）：WD 扩展头和 CCIP 长尾锚点，只在卡片的折叠区里分开列。原来分开发布的 WD 角色扩展 v1 和 CCIP 长尾参考包 v1 合起来算角色资料包 0.1（测试版），随应用内置。版本号后面写「（测试版）」的版本，行为和正式版一样，也默认启用。

启用时检查本图库已有的识别结果，之后同步交付的新识别结果在后台检查。卡片上的「重新检查」重扫整库；「撤回…」去掉本图库里由它给出的候选和它补上的机器归属，人工结论保留，之后可以再启用。「删除角色资料包」只删本机的文件，已启用的图库保留现有的候选，新识别的图不再检查。

- **下载**：首次准备时点下载识别模型，角色资料包会一起下载：先读一次 GitHub 上公开的角色资料包目录，下载其中最新的版本，读不到时下载内置的 0.1；也可以在卡片上点「下载角色资料包」，或「从文件导入…」。下载失败或取消后会保留可续传的数据，空的临时文件会清除。
- **默认启用**：新图库（还没有任何识别结果）第一次同步时就启用，之后识别的图照常检查。第一次同步时角色资料包还没下载好（比如下载失败，之后点了「重试」），下完以后自动启用，同时检查已经识别过的图。从旧版本升级上来、已经有识别结果的图库不自动启用，卡片上提示一次：点「启用…」才检查，点「不用」就不再提示。撤回以后同样不再自动启用。
- **新版本**：新版本不随应用发布，列在本项目一份签过名的包目录里。应用只在检查更新时（打开了自动检查，或手动点「检查更新」）顺带读这份目录；关掉检查更新、也没有手动检查，就不读。目录里有更新的版本时，卡片上多一行「有新版本：N 个角色」。点「下载新版本」才下载；下好以后，已经启用的图库点「换成新版本…」才用新版本重新检查，换掉旧版本给出的候选和由它补上的归属，人工结论不变。换了以后照样可以撤回；没有启用的图库，卡片上直接是本机已有的最新版本。
- **只给候选的角色**：分级的新版本里有只给候选的角色，它们只出现在「也可能是」里，不补主归属，也不改已有的归属。
- **签名**：签名对不上的目录整份不用；应用只认内置的 0.1 和最近一份验过签的目录里列出、这个版本能用的版本。
- **离线**：从 [Releases](https://github.com/shio-vinyl/illustash/releases) 里最新的 `character-pack-<版本>`（如 `character-pack-0.5`）下载 `character-pack-<版本>-offline.zip`，在卡片上「从文件导入…」选它，不用解压。离线包里有两个组成部分的文件、签过名的包目录，以及许可和模型说明；应用先核对目录的签名，再按目录认包文件，许可和说明不导入，也不列在导入结果里。只有包文件、没有目录时，只能导入内置的 0.1：它的两个文件在 [`wd-extension-v1`](https://github.com/shio-vinyl/illustash/releases/tag/wd-extension-v1) 和 [`ccip-longtail-v1`](https://github.com/shio-vinyl/illustash/releases/tag/ccip-longtail-v1) 这两个 Release 里，大小和 SHA-256 见[第三方组件与许可](THIRD_PARTY.md#模型与数据)。

「从文件导入…」也支持带有签名清单的角色词表 ZIP。导入成功后，重启应用即可启用新词表。

也可以离线使用命令行启用 WD 部分。先在桌面应用里完成图库升级并退出，再从 ZIP 中解出模型文件。

macOS 在终端用下面的命令启用。把路径换成自己的图库和解出的文件。应用装在其他目录时，相应修改程序路径。

```sh
cli="/Applications/Illustash.app/Contents/Resources/compute/bin/illustash-terms"
"$cli" wd-extension --library "/path/to/图库.pilib" --package "/path/to/wd-extension-v1.bin"
```

Windows 在 PowerShell 中使用安装目录内的控制台程序。

```powershell
$cli = '<安装目录>\compute\bin\illustash-terms.exe'
& $cli wd-extension --library 'C:\path\to\图库.pilib' --package 'C:\path\to\wd-extension-v1.bin'
```

要通过命令行撤回，将末尾的 `--package` 和文件路径换成 `--withdraw`。命令行指定的文件只供本次启用使用，按文件的 SHA-256 认它是哪个角色资料包版本里的 WD 部分：内置的 0.1，或本机最近一份验过签的包目录里列出的版本（先在桌面应用里检查一次更新或导入离线包）。要让桌面应用在同步中给每批新识别的图片自动检查，仍需在卡片上下载或导入角色资料包。已完成交付的批次会立即显示 WD 扩展候选，中断后仍保留。

### 可选的 CCIP 长尾角色参考包

对当前图库运行 `ccip-longtail --library <图库.pilib> --root <应用资源目录> --package <参考包>` 启用并重扫已保存的 768 维 CCIP 向量。命令报告缺少向量和向量不兼容的张数。启用后的新图沿用识别时生成的向量。已有基础归属和 WD 扩展归属优先，包只为没有主归属的图提供机器归属。基础角色索引更新后，重新运行命令。用 `ccip-longtail --library <图库.pilib> --withdraw` 撤回。

冻结包为 `A-longtail-p75.zip`（1,273,692 B，54 个角色），SHA-256 为 `d36eeab8768e30cc36e2ba5b2788f00a5f608ecad0df264a2830a44c14407fb9`。下载地址见 [模型 Release](https://github.com/shio-vinyl/illustash/releases/tag/ccip-longtail-v1)。安装后所有图库共用这个文件，每个图库分别启用。它是角色资料包的 CCIP 部分，下载、启用、换包和离线导入见[角色资料包](#角色资料包)；命令行的 `--package` 同样按 SHA-256 认内置的或目录里的版本。启用需要兼容的基础角色索引和 CCIP 模型。来源与许可见[第三方组件与许可](THIRD_PARTY.md#模型与数据)。维护者用 `python3.12 tools/build_ccip_longtail.py <冻结成员目录> <输出目录>` 复封并校验同一 ZIP。

## Windows

- 安装包装在当前用户下，不需要管理员权限。
- 安装包没有代码签名，下载和第一次运行时可能各被拦一次（2026-11-15 用 v0.1.0 在没装过 Illustash 的 Windows 11 虚拟机上实测，#421）：
  1. **用 Edge 下载时**：下载完成后，Edge 的下载列表里可能提示「通常不会下载 Illustash_…_x64-setup.exe。请在打开前确保信任 …」。把鼠标移到这一项上，点右边的「…」，选「保留」；在弹出的「打开前请确保信任 …」里点「显示详细信息」，再点「仍然保留」。其他浏览器可能有类似的提示，选保留即可。
  2. **第一次运行时**：Windows SmartScreen 提示「Windows 已保护你的电脑」，只有一个「不运行」按钮。点文字下方的「更多信息」，再点新出现的「仍要运行」。之后从开始菜单或桌面启动不会再被拦。

  遇到这里没写的拦截或报错，请按下面「报告问题」反馈。
- 装好后从开始菜单或桌面上的「Illustash」快捷方式启动。桌面快捷方式由安装最后一页的「创建桌面快捷方式」决定，默认勾选。
- **在应用里「下载并重启」**：同样不需要管理员权限。新版本的安装包在后台运行，只显示一个进度条，装好后 Illustash 自己重新打开，回到原来的图库。应用自己下载的安装包没有「来自网络」的标记，不会出现 SmartScreen 提示（2026-11-14 在 Windows 验收机上用合成的新旧两个版本实测，#279）。
- **识别默认用显卡加速**（DirectML，2026-10-21 起，#271）：需要支持 DirectX 12 的显卡，有独立显卡的笔记本会用独立显卡。不想用时，在「设置 › 数据 › 识别模型」里关掉「使用 GPU 加速识别」，之后全部用 CPU，下次识别时生效。没有合适的显卡、驱动出错，或者显卡在识别中途出错时，会自动改用 CPU 接着识别，设置里会显示上次没能用上加速的原因。用不用显卡，分级、标签和角色判定都一样，只有置信度的最后一位小数偶尔不同。**只有核显的电脑还没有测过**，提升可能很小；如果觉得反而变慢，关掉这个开关即可。
- **隐私**：随包的 ONNX Runtime 由本项目从源码构建，不带微软的遥测配置，它的诊断事件不进 Windows 的遥测通道；Illustash 还在初始化时调用关闭遥测的接口（#381）。GPU 加速用的是微软的 DirectML 组件，随安装包提供。它登记在 Windows 的遥测通道里（2026-11-13 核对随包文件，#381），按它的许可，可能依照 Windows 的「诊断数据」设置向微软发送使用信息；实际上传了什么，本项目没有抓包核实。不想用的话，在「设置 › 数据 › 识别模型」里关掉「使用 GPU 加速识别」，识别就只用 CPU，不再加载 DirectML。

## 报告问题

用仓库的 Issue「报告问题」模板，附上界面显示的错误编号及「设置 › 关于 › 复制诊断信息」的内容；剪贴板不可用时可点「导出诊断信息」。本地服务退出或图库打不开时，启动页也能复制或查看诊断，重启后记录仍在。诊断只包含版本、构建、系统、错误编号、操作阶段、原因分类/系统错误码和提交/重试状态，不含凭据、cookie、图片、真实文件名或路径，也不会自动上传。记录最多 128 条、每条不超过 2 KiB、保留 14 天；启动、写入或查阅时清理过期记录。

遇到本地服务异常退出、识别自检失败或其他后台任务失败时，可在桌面版的「设置 › 关于」点「打开日志所在文件夹」，按问题发生时间选取 `runtime.log` 或相应历史文件附在反馈里。日志默认保存在本机，最多 3 份、每份 2 MiB，包含脱敏后的阶段、原因、可用的错误码和已核验的程序源码位置，不会自动上传。

若提示「已经提交」「可能已有部分提交」或「提交状态需核对」，先查看任务或操作结果再重试。诊断目录不可写或正忙时会显示「诊断记录未保存」，原始业务结果不受影响。请勿另行粘贴真实图片、图片文件名、本机路径、账号信息或原始终端输出。
