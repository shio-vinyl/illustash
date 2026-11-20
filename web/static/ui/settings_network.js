// 设置 › 网络与隐私（#200，#676 建议 1 改成权限列表，#714）：每个会联网的功能一行，标题链到它所在的那一节，说明写
// 「服务：发送的内容」，右边是现在的状态和控件：能开关的放开关（名称联网查询、自动检查更新），按次授权的写「每次询问」，
// 只有点按钮才联网的写「手动」。
// 找回画师那一行只写状态、不放开关：同步后联不联网由启动时的授权决定（native/terms/src/app_provenance.rs 的
// auto_network），「设置 › 画师」里「同步后自动查新图」只管本机的签名识别，放在这里会让人以为关掉它就不联网。
import {t} from '../core/i18n.js';
import {isMac} from '../core/format.js';
import {loadProvenance} from '../core/jobs.js';
import {allowNames, loadAccounts, updatesPost, updatesTick} from '../core/settings.js';
import {confirmAction} from './dialog.js';
import {toast} from './dom.js';
import {drawGuide} from './guide.js';
import {groups, refreshSection} from './settings_group.js';

// ---------- 名称：这个图库联网查询名称的授权 ----------
// 名称的文字查询（只有 tag 和作品名）需要这个图库授权：授权之前，后端以「显示名联网尚未授权」拒绝。其他失败都是真正的
// 网络或服务错误。搜索结果下面的「允许查询…」（ui/settings.js）也走这里。
const NAMES_NOTE = t('允许这个图库联网查询名称吗？\n\n会把你输入的角色名、作品名，以及图库里角色的 Danbooru tag 和作品名发给 Danbooru 和 Bangumi，用来搜索角色、作品和补全中文名。不会发送图片。\n\n允许后对这个图库一直有效，可以在「设置 › 网络与隐私」里随时撤销。');
export async function setNamesNetwork(allow) {
  if (allow && !await confirmAction(NAMES_NOTE, t('允许联网查询名称'))) return false;
  try {
    await allowNames(allow);
  } catch (err) { toast(err.message); return false; }
  toast(allow ? t('已允许这个图库联网查询名称') : t('已撤销名称联网查询'));
  drawGuide();
  return true;
}

// Windows 上随包的 DirectML 登记在系统的遥测通道里（#381）；macOS 不带它。
const DIRECTML_NOTE = t('GPU 加速用的微软 DirectML 组件登记在 Windows 的遥测通道里，可能依照系统的「诊断数据」设置向微软发送使用信息；不想用可在「设置 › 数据 › 识别模型」里关掉「使用 GPU 加速识别」。');
const ASK = () => t('每次询问'), MANUAL = () => t('手动');
const via = (service, what) => t('{service}：{what}', {service, what});
const net = s => s.names?.network;
const modelHost = m => { try { return new URL(m?.base || 'https://api.deepseek.com').host; } catch { return '—'; } };
// 一行：标题链到 #/settings/<section>，右边是状态文字。
const line = (key, section, label, hint, value, extra = {}) => ({kind: 'info', key, label, href: `#/settings/${section}`, hint, value, ...extra});

export function build(main, scope) {
  return groups(main, [
    {key: 'network', title: t('会访问网络的功能'), note: t('原图不发出去。找回画师和查找画师会发送由图片算出的哈希和签名文本。'),
     foot: () => t('同步、图片识别、相似图片检测、学习与分组都只在本机进行。X 的关注通过导入官方数据归档获得，不联网。') + ' '
       + t('识别用的 ONNX Runtime 已关掉微软的遥测，不联网。') + (isMac ? '' : ' ' + DIRECTML_NOTE),
     parts: [
       line('index', 'characters', t('检查新角色'), via('Danbooru', t('关注作品的名称；添加角色时下载约 20 张公开参考图')), ASK),
       // 角色名称和 Danbooru 搜索共用这一项授权：打开时先确认，关掉直接撤销。
       {kind: 'switch', key: 'names', label: t('名称联网查询'), href: '#/settings/characters', hidden: s => !net(s),
        hint: s => via(t('Bangumi、Danbooru'), t('tag、作品名、日文名')) + ' · ' + t('角色名称和 Danbooru 搜索共用这一项授权，只发送文字')
          + (s.settings?.startup?.names ? ' · ' + t('启动时自动（只查新角色）') : ''),
        warn: s => !net(s)?.granted_at && net(s)?.this_run ? t('本次启动已允许（命令行）') : '',
        get: s => !!net(s)?.granted_at,
        set: async on => { await setNamesNetwork(on); refreshSection(); }},
       line('model', 'characters', t('名称翻译'), s => via(s.names?.model_api?.configured ? modelHost(s.names.model_api) : t('你配置的模型接口'), t('tag、日文名、作品名')),
         s => s.names?.model_api?.configured ? t('已配置') : t('未配置，不使用')),
       line('provenance', 'artists', t('找回画师'), via(t('Danbooru（含 IQDB）'), t('像素哈希和 IQDB 签名（一串文本，不是图片）、画师名和账号')),
         s => s.provenance?.auto_network ? t('同步后自动，每次有流量上限') : ASK()),
       line('pixiv', 'artists', t('读取 Pixiv 关注'), via('Pixiv', t('用你在 app 里登录的状态读取关注列表（只读）')),
         s => s.accounts?.pixiv?.bound ? t('已登录') : t('未登录')),
       line('lookup', 'artists', t('查找画师、找 TA 的其他图'), via(t('Danbooru、画师主页'), t('所选图片的哈希文本；下载画师公开作品的小图，只在本机比对')), MANUAL),
       line('search', 'characters', t('在 Danbooru 上搜角色和作品'),
         via('Danbooru', t('你输入的文字；自建角色的名字（核对能否对上 Danbooru）；「设为…」里 Danbooru 候选的 tag（取一张全年龄小图）')),
         s => net(s) && !net(s).authorized ? t('未允许，不联网') : MANUAL()),
       line('saucenao', 'artists', t('以图搜图'), via('SauceNAO', t('已保存 key，还没有功能使用它')), MANUAL, {hidden: s => !s.accounts?.saucenao?.bound}),
       line('pack', 'characters', t('下载角色资料包'), s => via('GitHub', s.models?.on_demand
         ? t('一次普通的 HTTPS 下载，取回角色资料包，首次准备时先读一次公开的角色资料包目录；不发送图库里的任何信息')
         : t('一次普通的 HTTPS 下载，取回角色资料包；不发送图库里的任何信息')), MANUAL),
       line('models', 'data', t('下载识别模型'), s => via((s.models?.sources || []).join(t('、')), t('一次普通的 HTTPS 下载，取回模型文件；不发送图库里的任何信息')),
         MANUAL, {hidden: s => !s.models?.on_demand}),
       {kind: 'switch', key: 'updates', label: t('自动检查更新'), href: '#/settings/about', hidden: s => !s.updates,
        hint: via('GitHub', t('一次普通的 HTTPS 请求，读取公开的版本列表和角色资料包目录；不发送图库里的任何信息')) + ' · ' + t('启动时和之后每天查一次 GitHub 上有没有新版本，有新版本时提示你，不会自己下载或安装'),
        get: s => !!s.updates?.auto,
        set: async on => {
          try { await updatesPost({action: 'auto', allow: on}); toast(on ? t('已打开自动检查更新') : t('已关闭自动检查更新')); } catch (err) { toast(err.message); }
          updatesTick(); refreshSection();
        }},
       line('install', 'about', t('下载更新'), via('GitHub', t('下载新版本的安装包和签名，校验后安装；不发送图库里的任何信息')), MANUAL,
         {hidden: s => !s.updates?.install}),
     ]},
  ], {scope});
}
// 进这一节：找回画师和账号的状态各读一次（名称、模型、更新由外壳和启动时读）。
export function load() { loadProvenance(); loadAccounts(); }
