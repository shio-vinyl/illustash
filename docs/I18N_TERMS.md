# 中英术语对照

现行参考，2026-10-18（#182）；2026-11-16 加[语气](#语气)一节（#528），中英文界面文案都照它写；同日按它把界面文案（`web/static`、桌面启动页和菜单、后端原样显示的提示）全量审过一遍。界面译文的用词以 `web/static/locales/en.json` 现有译法为准，这里只列反复出现、容易译得前后不一的词；翻译方式、译文表格式和检查见 [WEB_V2.md](WEB_V2.md#界面语言2026-10-18173)。加其他语言时，先按这张表定下本语言的对应词，再译整表。改了某个词的英文时，同一个 PR 里把 `en.json` 里所有用到它的条目和这张表一起改。

## 名词

| 中文 | English | 说明 |
|---|---|---|
| 图库 | Library | 一个图库包。导出图库 Library export |
| 资料库 | Catalog | 角色、作品资料所在的页面；「还没收录」译为 not in the catalog |
| 作品 | series | 角色所属的作品。单复数同形：1 series、3 series |
| 角色 | character | |
| 原创角色 | original character | 画师自己的角色；「原创角色（未细分）」Original character (unsplit) |
| 自建角色 | characters you created | 用户自己添加、还没关联 Danbooru 的角色 |
| 造型 | form | 角色下的变体：服装、异格、变身等，数据里是角色的 `variants`。2026-11-14 用户定（#423）界面上统一叫造型，不再分皮肤、子标签、形态。英文不用 variant（相似图片的「差分」是 Variant）和 look（长得像、外观是 look）；「文件形态」File form 说的是文件，不是角色 |
| 标签 | tag | 我的标签 My tags；只当标签 Tag only |
| 画师 | artist | |
| 张（量词）、图片、图 | picture | 不用 image；「{n} 张」写成复数 `{n} picture` / `{n} pictures` |
| 原图 | original | 「读不到原图」originals could not be read |
| 副本 | copy | 重复的同一张图；副本组 copy group |
| 来源 | source | 图库读图的文件夹、iCloud 照片等 |
| 出处 | origin | 一张图在 X、Pixiv 上的原帖 |
| 识别 | recognition / identify | 名词用 recognition（识别结果 recognition results），动词与状态用 identify（已自动识别 identified automatically） |
| 角色索引 | character index | |
| 人工整理（记录） | your organizing | 用户手工做的判断，备份和撤销都围绕它 |
| 任务面板 | task panel | |
| 大图 | Large view | |
| 收藏 | favorite | 人工标记的收藏（#447）：画廊里的「收藏」Favorites，按钮「收藏」Favorite、「取消收藏」Unfavorite。存进图库的图不叫收藏，叫「加入」 |
| 加入 | saved / added | 图进图库：共 {n} 张 {n} saved；每月加入张数 Collection chart；最后加入 Last saved |

## 页面与分组

| 中文 | English |
|---|---|
| 首页 | Home |
| 未识别 | Unidentified |
| 已跳过 | Skipped |
| 已排除 | Excluded |
| 待确认 | To confirm |
| 重复 | Duplicates |
| 相似图片 | Similar pictures |
| 相似组 N | Similar group N（未识别的分组「第 N 组」是 Group N） |
| 身份整理 | Identities |
| 随机抽查 | Spot check |
| 画师抽查 | Artist spot check |
| 找回画师 | Find artists |
| 名称翻译 | Name translation |
| 设置 › 通用 | Settings › General |
| 网络与隐私 | Network and privacy |
| 排除规则 | Exclusion rules |
| 视图选项 | View options |
| 新手引导 | Introduction |

## 动作

| 中文 | English |
|---|---|
| 同步 | Sync |
| 跳过 | Skip |
| 撤销 | Undo |
| 关联 Danbooru | Link to Danbooru |
| 合并到另一个角色 | Merge into another character |
| 按相似细分 | Split by similarity |
| 判断关系 | Compare |
| 跟随系统 | System |
| 可疑优先 | Doubtful first |

## 写法

- 标题、按钮、菜单用句首大写（View options，不写 View Options）；句子照常加句号。
- 引用界面上的名字用弯引号：the “Duplicates” page。省略号用一个字符 `…`。
- 计数放在名词前，按复数规则写（`{n} pictures`）；不方便写复数的句子改写成不依赖单复数的说法。
- `NSFW`、`Danbooru`、`Pixiv`、`X`、`iCloud`、`Photos` 等专名不译。内容筛选在中英文界面上都叫 NSFW，不写 R18；说的是 Pixiv 上登录才能看的作品时也这样叫（用户 2026-11-19 定，#706）。

## 语气

2026-11-16 用户定（#528），中英文都适用：照常见桌面和网页软件的界面文案写，平实、简短，用正常语气，不客套。文案写成标签和事实，说清这一页有什么、这个按钮做什么、现在是什么状态。不当解说员：不替系统讲原理，没人问不安抚，不把几句说明串成一行，不旁白用户在做的事。

### 要去掉的说法

下面的「改前」都是 #528 全量改写以前界面上的原文（`ui/shell.js` 各页的 `heading`、`core/pages.js` 的 `empty`、按钮的 `title`），「改后」是照这一节写的样子。

1. **替系统讲解它在做什么、为什么这么做。** 用户要的是结果，不是机制。「系统会」「自动」「学会」「用来衡量」这类话多半可以删。

   | 改前 | 改后 |
   |---|---|
   | 长得像的图分成一组，一组一组过 · 每确认一组，系统就学会认 TA 的一种样子<br>Look-alike pictures grouped together, one group at a time · each confirmed group teaches one more look of that character | 按长相分组的未识别图<br>Unidentified pictures grouped by look |
   | 角色不一致 {n} 组 · 对齐角色后自动折叠<br>{n} groups with disagreeing characters · they fold once the characters are aligned | 角色不一致 {n} 组<br>{n} groups with disagreeing characters |

2. **没人问就安抚、作保证。** 能撤销、只在本机处理、不改原图是这个应用的常态，常态不用说。要说的是例外：不能撤销、会联网发出什么、会动原图文件，而且写在那个动作旁边。

   | 改前 | 改后 |
   |---|---|
   | {n} 张 · 只是标记，不改动原图<br>{n} pictures · only marked, the original files are not touched | {n} 张<br>{n} pictures |
   | 画的是同一张、但不是同一个文件 · 只在本机比较<br>The same artwork in different files · compared on this computer only | 同一幅画的不同文件<br>The same artwork in different files |
   | {n} 对长得像的角色 · 按涉及图片数排序 · 每一步都可撤销<br>{n} look-alike character pairs · by number of pictures involved · every step is undoable | {n} 对长得像的角色<br>{n} look-alike character pairs |
   | 和另一个角色是同一个人：把图和人工结论都并过去（可撤销）<br>Same person as another character: move the pictures and your decisions over (undoable) | 把这个角色的图和人工结论并到另一个角色<br>Move this character’s pictures and your decisions to another character |

3. **用「·」把几句说明串成一行。** 标题下的副标题只放数量和当前状态；「·」只分隔并列的短数据（`{groups} 组 · {n} 张`），不分隔句子。操作方法和快捷键不常驻在副标题和页面上，放进说明（放在哪见 #527）；「修改会同步到整组」这类点下去会影响什么的事实，挪到那个动作的提示或确认里（见[可以保留的说明](#可以保留的说明)）。

   | 改前 | 改后 |
   |---|---|
   | 还剩 {left} / {n} 张 · 鼠标移到角色 tag 上点 ✓ / ✗，或选中多张按 Y / N；打开大图也会问「是 TA 吗？」<br>{left} / {n} left · hover a character tag and click ✓ / ✗, or select several and press Y / N; the large view asks “Is this them?” too | 还剩 {left} / {n} 张<br>{left} / {n} left |
   | {groups} 组 · {n} 张 · 修改任一份会同步到整组<br>{groups} groups · {n} pictures · changing any copy changes the whole group | {groups} 组 · {n} 张<br>{groups} groups · {n} pictures |
   | 同色角标为同一组 · 修改会同步到整组 · 角色不一致的组对齐后自动折叠<br>Badges of one color are one group · changes apply to the whole group · groups fold once their characters agree | 同色角标是同一组<br>Badges of one color are one group |

4. **用旁白口吻描述用户自己正在做的事。** 用户知道自己在干什么，按钮已经写了能做什么，副标题不再复述一遍。

   | 改前 | 改后 |
   |---|---|
   | 验收「找回画师」：固定的一组随机图，逐张判断机器找到的画师对不对<br>Check “Find artists”: a fixed random set of pictures; judge one by one whether the artist found is right | {n} 张随机图<br>{n} random pictures |
   | {n} 张 · 选中后设为角色，或跳过<br>{n} pictures · select them to assign a character, or skip | {n} 张<br>{n} pictures |
   | 已跳过 {n} 张 · 可以恢复到未识别<br>{n} skipped · can be returned to unidentified | 已跳过 {n} 张<br>{n} skipped |
   | 待确认 {n} 张 · 像截图但有人物，排除或保留<br>{n} to confirm · look like screenshots but have people; exclude or keep | 待确认 {n} 张：像截图，但有人物<br>{n} to confirm: look like screenshots but have people |
   | 整组都是同一个原创角色：新建一个原创角色，把这组图都设为它<br>The whole group is one original character: create one and assign the group to it | 新建一个原创角色，把这组图都设为它<br>Create an original character and assign this group to it |

5. **客套、道谢、寒暄和表情。** 一概不写：谢谢、恭喜、辛苦了、表情符号、感叹号都不用。做完了就写做完了。

   | 改前 | 改后 |
   |---|---|
   | 抽查做完了，谢谢！结果会用来衡量认人准不准。<br>Spot check done, thanks! The results measure how accurate recognition is. | 抽查做完了。<br>Spot check done. |
   | 全部认出来了 🎉<br>Everything is identified 🎉 | 没有未识别的图。<br>No unidentified pictures. |

英文照同样的规矩：不写 *don’t worry*、*simply*、*just*、*Note that*，不写 *thanks*、*great*、*all set*、*oops* 这类客套和感叹，不用感叹号，不用 *we* 指代应用，不写 *the app will automatically…* 这类讲解。

### 可以保留的说明

- **用户做决定前需要知道的事实**：会删掉、覆盖或改动什么，能不能撤销，代价是什么（要重新下载、要从头再来），会不会联网发出什么。写在确认框和那个按钮的提示里，只在要做这个决定时出现，不写成每页常驻的旁白。例子，都保留原样：
  - 确认框：删除已下载的识别模型（约 {size}）？删除后要重新下载或从文件导入，才能识别新加入的图片；已经识别的结果都保留。
  - 设置里图库列表「移除」按钮的提示：只从列表里移除，不删除图库。用户可能以为会删文件，这句就是决定前要知道的事实，和第 2 类的常态保证不同。
  - 「标为已检查」按钮的提示：把剩下的机器识别都记为已检查；以后新识别的图会重新计入未检查。后半句是点下去以后会发生的事。
- **按钮和菜单的提示**写它做什么，用动词开头，一句话；前面不加「：」引出的情境说明（见第 4 类最后一例）。
- **副标题和设置分区的说明**写成内容清单或状态：「当前图库、来源、同步与排除」「最近一次同步新加入 {n} 张」。
- **空状态**写现状，一句话：「这一组都整理完了。」

### 自查

写完逐句问：删掉这句，用户会做错什么或找不到什么吗？不会就删。句子里出现「系统」「自动」「只是」「只在」「可撤销」「放心」「谢谢」、感叹号、表情符号，或一行里有两个以上的「·」时，再看一眼。
