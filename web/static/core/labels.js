// 页面给后端各种类型起的名字（#199）：图为什么被排除、两张图是什么关系。不碰 DOM。
import {t} from './i18n.js';
import {workName} from './catalog.js';
import {state} from './state.js';
// 角色的圆头像：角色目录里的封面 uuid（core/meta.js 读的摘要行，/api/characters 的 cover）；没有是 ''。
export const avatarOf = id => state.charMap.get(id)?.cover || '';
export const REASONS = {photo: t('真人/照片'), screenshot: t('截图'), text: t('文字图'), manual: t('手动排除')};
// 两张相似的图是什么关系；其中哪些折叠成一张是图库设置。
export const RELATION_NAMES = {identical: t('完全相同'), duplicate: t('重复'), subtle: t('细微差分'), variant: t('差分'), crop: t('局部重复'), edit: t('局部编辑')};
export const FOLD_OPTIONS = [['identical', RELATION_NAMES.identical, t('像素一样的不同文件')], ['duplicate', RELATION_NAMES.duplicate, t('同一张图缩放或重新压缩过')], ['subtle', RELATION_NAMES.subtle, t('只差表情、签名、小物件（< 2%）')],
  ['edit', RELATION_NAMES.edit, t('加字水印、调色、色块或马赛克')], ['crop', RELATION_NAMES.crop, t('一张是另一张的裁剪')], ['variant', RELATION_NAMES.variant, t('同一张画改了局部或重新上色')]];
// 画师抽查里画师是怎么找回来的（docs/PROVENANCE.md 第 8 节）：方法 → [名字, 说明]；先后就是统计里的先后。
export const AUDIT_METHODS = {pixel_hash: [t('像素哈希'), t('和 Danbooru 上的帖子是同一个文件，画师取自帖子的画师标签')],
  gallery_match: [t('本地比对'), t('和 TA 公开作品的小图逐张比对一致')], ocr: [t('签名识别'), t('图上的签名或水印')]};
export const RELATION_KINDS = [['', t('全部')], ...['duplicate', 'subtle', 'variant', 'crop', 'edit'].map(k => [k, RELATION_NAMES[k]])];
// 同作品重名时，接口在原名旁给出当前语言的形态短词，手动名称和普通角色没有这个字段。
export function roleName(role) {
  const qualifier = role?.name_disambiguation?.qualifier;
  return qualifier ? t('{name}（{qualifier}）', {name: role.name, qualifier}) : role?.name || '';
}
// 显示名来自服务端（中文 → 日文 → 可读的英文）；原始 tag 只在悬停时显示。
export const nameTip = c => !c ? '' : c.tag && c.tag !== c.name ? c.tag + (c.name_source === 'model' ? ' · ' + t('机器翻译') : c.name_source === 'wiki' ? ' · ' + t('来自 Danbooru 别名') : '') : c.name;
// 角色名字旁边的作品（#293）：原创角色写「原创」，否则写它直接所属的作品，否则什么都不写。c 是角色，或在
// state.charMap 里查到的某张图的归属（{id, name}）。
export const catalogRole = c => c && (state.charMap.get(c.id) || c);
export const workLabel = c => { const x = catalogRole(c); return !x ? '' : x.kind === 'oc' ? t('原创') : x.work_id ? x.work_name || workName(x.work || '') : ''; };
// 只放得下名字的地方，悬停时显示「名字（作品）」。
export const nameWithWork = c => { const w = workLabel(c); return w ? t('{name}（{work}）', {name: roleName(c), work: w}) : roleName(c); };
