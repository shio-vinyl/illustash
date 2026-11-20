// 资料库（#199）：按作品分组的角色和各自最近进了多少张、搜索、作品树和作品的样图（#711 作品卡），以及外部索引的关注列表。
// 不碰 DOM；三个页签在 ui/catalog_page.js 和它导入的几个文件，作品页在 ui/work_page.js，对话框在 ui/catalog.js。tagged、workTree、localLinks 由这里写（字段声明在 core/state.js）；index 由 core/jobs.js 写，
// 这里经 setIndex 换上新的回应。
import {t} from './i18n.js';
import {work} from './format.js';
import {api, post} from './session.js';
import {state} from './state.js';
import {setIndex} from './jobs.js';
import {pageOf} from './pages.js';
import {parseHash} from './route.js';
// 每个角色的最近动态（{recent, last}），每种筛选、每个数据版本读一次。
export const catalog = {activity: new Map(), activityKey: ''};
export const recentOf = c => catalog.activity.get(c.id)?.recent || 0;
// 是否需要并且做了一次新的读取。
export async function loadActivity() {
  const key = state.nsfw + '|' + state.readEpoch; if (catalog.activityKey === key) return false;
  const act = await api('/api/character/activity?nsfw=' + state.nsfw);
  if (!act) return false;
  catalog.activity = new Map(Object.entries(act)); catalog.activityKey = key;
  return true;
}
// 下一次 loadActivity() 重新读取（换了筛选，显示的是别的图）。
export function forgetActivity() { catalog.activityKey = ''; }
export const searchCharacters = q => api('/api/characters?min=1&q=' + encodeURIComponent(q) + '&nsfw=' + state.nsfw);
export async function loadTagged() { state.tagged = await api('/api/tagged?' + new URLSearchParams({nsfw: state.nsfw})); }
// 作品树带每部作品最多四张最近加入的样图和它们的主色（#717 的 marks=color），作品卡和作品页的切片、配色用它；样图跟着 NSFW 设置。
export async function loadWorkTree() { state.workTree = await api('/api/works?' + new URLSearchParams({marks: 'color', ...(state.nsfw ? {nsfw: state.nsfw} : {})})); }
// 一部作品的样图 [{uuid, color}]（作品树还没读到、或这部作品没有可见的图时是空的）。
export const workCovers = id => (state.workTree || []).find(w => w.id === id)?.covers || [];
// 作品的配色：样图里第一个有主色的；都没有时是 null（界面退回中性色）。
export const workColor = covers => covers.find(x => x?.color)?.color || null;
// 名字是 Danbooru tag 的自有角色（只拿这些名字去问 Danbooru；没有就不发请求）。返回是否拿到了回应。
export async function loadLocalLinks() {
  const found = await api('/api/local-links').catch(() => null);
  if (found === null) return false;
  state.localLinks = found;
  return true;
}
// 自建角色的 Danbooru 关联变了：忘掉上一次读到的清单，下次用到时重读。
export function forgetLocalLinks() { state.localLinks = null; }

// ---------- 海报和作品页的图 ----------
// 海报每次打开资料库显示随机抽到的一张这个角色的图（#293，先抽只有它一个人的图），连同同一次抽到的最多 12 张给海报的小查看器。
// 读失败时只有封面。
export function posterPictures(c) {
  const offset = Math.floor(Math.random() * Math.max(1, c.count - 11));
  return api('/api/assets?' + new URLSearchParams({entity: c.id, order: 'newest', offset, limit: 12, count: 0, ...(state.nsfw ? {nsfw: state.nsfw} : {})}))
    .then(r => {
      const items = r.items || [], alone = items.filter(x => x.memberships?.length === 1), pool = alone.length ? alone : items;
      if (!pool.length) return [c.cover];
      const pick = pool[Math.floor(Math.random() * pool.length)].uuid;
      return [pick, ...items.map(x => x.uuid).filter(u => u !== pick)];
    }, () => [c.cover]);
}
// 作品页的「最近加入」和横幅的切片：这部作品最近加入的图（带主色）。原创角色、未归作品不是图库的筛选条件，按各个角色读不划算，
// 用角色的封面顶上。
export async function workImages(key, signal) {
  if (key === 'oc' || key === 'other') return castWorks().find(w => w.key === key)?.chars.filter(c => c.cover).slice(0, 24).map(c => ({uuid: c.cover, color: c.color || null})) || [];
  const r = await api('/api/assets?' + new URLSearchParams({work: key, order: 'newest', limit: 24, count: 0, marks: 'color', ...(state.nsfw ? {nsfw: state.nsfw} : {})}), {signal});
  return r?.items || [];
}

// ---------- 返回按来路走（#711） ----------
// 作品页横幅左上的返回写「回到哪里」：上一条地址是作品页签写「作品」，资料库写「资料库」，另一部作品、某个角色写它的名字，
// 别的页写活页条上那个入口的名字；没有上一条（刚打开）时去作品页签。
const ENTRY_NAMES = {library: () => t('图库'), catalog: () => t('资料库'), galleries: () => t('画廊'), tidy: () => t('整理'), settings: () => t('设置')};
export function backTo(hash) {
  if (!hash) return {label: t('作品'), href: '#/characters/works', step: false};
  const r = parseHash(hash);
  const label = r.page === 'work' ? castWorks().find(w => String(w.key) === r.id)?.name || t('作品')
    : r.char ? state.charMap.get(r.char)?.name || t('角色')
    : r.page === 'characters' ? (r.section === 'works' ? t('作品') : r.section === 'artists' ? t('画师') : t('资料库'))
    : ENTRY_NAMES[pageOf(r).entry]?.() || t('返回');
  return {label, href: hash, step: true};
}

// ---------- 按作品分的角色 ----------
export const workKey = c => c.kind === 'oc' ? 'oc' : c.work_id || 'other';
// 不是作品的组（原创角色、未归作品）带 pseudo，排在作品之后（#293）。
export function castWorks() {
  const names = new Map((state.workTree || []).map(w => [w.id, w.name])), groups = new Map();
  for (const c of state.chars) {
    const k = workKey(c);
    if (!groups.has(k)) groups.set(k, {key: k, name: k === 'oc' ? t('原创角色') : k === 'other' ? t('未归作品') : c.work_name || names.get(k) || t('未命名作品'), pseudo: k === 'oc' || k === 'other', chars: [], images: 0, recent: 0});
    const g = groups.get(k); g.chars.push(c); g.images += c.count; g.recent += recentOf(c);
  }
  for (const g of groups.values()) g.chars.sort((a, b) => b.count - a.count);
  return [...groups.values()].sort((a, b) => a.pseudo - b.pseudo || b.images - a.images);
}
export const PSEUDO_NOTES = {oc: t('画师的原创角色，不属于任何作品'), other: t('还没填所属作品的角色')};
// 列表的张数档位：[key, label, test]。
export const BUCKETS = [['all', t('全部'), () => true], ['1', t('1 张'), c => c.count === 1], ['2', t('2–4 张'), c => c.count >= 2 && c.count < 5], ['5', t('5–19 张'), c => c.count >= 5 && c.count < 20], ['20', t('20–99 张'), c => c.count >= 20 && c.count < 100], ['100', t('100 张以上'), c => c.count >= 100]];
// 列表的行：base（全部角色，或一次搜索）中落在某一档的，按某一列排序（dir 为 1 或 -1）。
export function castRows(base, {bucket, sort, dir}) {
  const test = BUCKETS.find(b => b[0] === bucket)?.[2] || (() => true);
  const key = {name: c => c.name, work: c => c.work_name || '', count: c => c.count, recent: recentOf, last: c => catalog.activity.get(c.id)?.last || '', skins: c => (c.variants || []).filter(v => v.count).length}[sort];
  return base.filter(test).sort((a, b) => { const x = key(a), y = key(b); return (x > y ? 1 : x < y ? -1 : 0) * dir; });
}

// ---------- 作品与关注列表（#198） ----------
// 某些 Danbooru tag 指的作品，供本地作品出现之前的关注列表使用。
export const WORK_NAMES = {blue_archive: t('蔚蓝档案'), 'arknights:_endfield': t('明日方舟：终末地'), wuthering_waves: t('鸣潮'), 'meitantei_precure!': t('名侦探光之美少女'),
  genshin_impact: t('原神'), 'honkai:_star_rail': t('崩坏：星穹铁道'), honkai_impact_3rd: t('崩坏 3'), zenless_zone_zero: t('绝区零'), arknights: t('明日方舟')};
export const workName = w => state.workNames?.get(w) || WORK_NAMES[w] || work(w);
// 名字、别名或 tag 与 q 匹配的本地作品（q 为空时是全部）。
export const searchWorks = q => api('/api/works?' + new URLSearchParams({q}));
export async function loadIndex() { const x = await api('/api/index'); if (x) setIndex(x); return x; }
// 显示用的作品树：在每个父作品下深度优先，大的作品在前；父作品已不在的子作品显示在顶层。filter 为 all、watched 或
// pending；q 匹配名字或 tag。
export function shelfRows(x, {q = '', filter = 'all'}) {
  const watched = new Set(x.watch.works), ids = new Set(x.works.map(w => w.id)), kids = new Map();
  for (const w of x.works) { const p = ids.has(w.parent_id) ? w.parent_id : ''; if (!kids.has(p)) kids.set(p, []); kids.get(p).push(w); }
  for (const list of kids.values()) list.sort((a, b) => (b.count || 0) - (a.count || 0) || a.name.localeCompare(b.name, 'zh'));
  const rows = [], walk = (p, depth) => { for (const w of kids.get(p) || []) { rows.push([w, depth]); walk(w.id, depth + 1); } };
  walk('', 0);
  q = q.trim().toLowerCase();
  const tests = {all: () => true, watched: w => watched.has(w.id), pending: w => w.status === 'pending'};
  return {rows: rows.filter(([w]) => tests[filter](w) && (!q || w.name.toLowerCase().includes(q) || (w.tag || '').toLowerCase().includes(q))),
    watched, pending: x.works.filter(tests.pending).length};
}
export async function saveWatch(works) { setIndex(await post('/api/index', {action: 'watch', works, min_posts: state.index.watch.min_posts})); }
// 为搜索到的 Danbooru 作品选定要关注的本地作品：带这个 tag 的那个，否则由用户挑（choose(title, works) 解析为 id，
// 新作品为 ''，取消为 null），用这个 tag 关联或新建。解析为它的 id，取消时为 null。
export async function watchTarget(r, choose) {
  const x = state.index;
  let id = r.id || x.works.find(w => w.tag === r.tag)?.id;
  if (!id) {
    const chosen = await choose(t('Danbooru 的 {tag} 对应哪个本地作品？', {tag: r.tag}), x.works);
    if (chosen === null) return null;
    id = (await post('/api/works', chosen ? {action: 'link', id: chosen, tag: r.tag} : {action: 'create', name: r.name || r.tag, tag: r.tag})).id;
    setIndex(await api('/api/index'));
  }
  return id;
}
// 新作品（id 为 null）连同父作品一次请求建好，或编辑一个作品；之后重读索引。解析为它的 id。
export async function saveWork(id, {name, parent, tag}) {
  if (!id) id = (await post('/api/works', {action: 'create', name, ...(tag ? {tag} : {}), ...(parent ? {parent} : {})})).id;
  else await post('/api/works', {action: 'edit', id, name, parent, tag});
  setIndex(await api('/api/index'));
  return id;
}
