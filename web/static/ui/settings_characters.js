// 设置 › 角色资料（#714）：角色资料包（状态和按钮的数据在 ui/packs.js）、检查新角色（native/terms/src/app_index.rs）、
// 角色名称和名称翻译。补角色以角色资料包为主，检查新角色只补两次发包之间新出现的角色（#606）：卡片排在资料包后面；上次
// 检查时所有关注作品都不缺角色的，正文收进折叠区，只留一行说明。不知道缺不缺（还没检查过、检查早于 #561、没有关注作品）
// 和还缺的时候照常展开，还缺几个另外写在资料包卡片底部。
import {t, tb} from '../core/i18n.js';
import {number, stamp} from '../core/format.js';
import {state} from '../core/state.js';
import {workName} from '../core/catalog.js';
import {charHash} from '../core/route.js';
import {loadPack} from '../core/packs.js';
import {readError} from '../core/session.js';
import {INDEX_ACTIVE, NAMES_ACTIVE, indexLine, namesLine, indexAction, checkIndex, onJob, pollIndex, pollNamesSoon} from '../core/jobs.js';
import {loadIndex, loadNames, loadCoverage, indexCoverage, saveModelApi, testModelApi} from '../core/settings.js';
import {confirmAction} from './dialog.js';
import {toast} from './dom.js';
import {packButtons, packDeleteButtons, packDot, packHasTask, packHintText, packInfoItems, packLine, packManual, packName, packNote,
  packOfferInfo, packProgress, packResultItems, runPack} from './packs.js';
import {groups, refreshSection, startupRow, taskRow} from './settings_group.js';
import {openTasks} from './tasks.js';

// ---------- 检查新角色 ----------
// 上次检查时有结果的关注作品合计还缺几个角色；不知道时（还没检查过、检查早于这一版、Danbooru 不认识全部作品）为 null。
export function coverageMissing(c) {
  const list = Object.values(c?.coverage || {}).filter(Boolean);
  return list.length ? list.reduce((n, v) => n + (v.missing || 0), 0) : null;
}
// 检查新角色收进折叠区：有关注的作品、上次检查都不缺，并且没有在跑、没有暂停或出错要处理。
export function indexFolded(x, c) {
  const s = x?.state?.status;
  return !!x?.watch?.works.length && coverageMissing(c) === 0 && !INDEX_ACTIVE.includes(s) && !['error', 'paused'].includes(s);
}
// 每个作品的覆盖：[{tag, name, packs, own, missing}]；还没检查过、或检查早于这一版（没有 coverage.json）时为空。
export function coverageItems(x, c) {
  const name = tag => x?.works?.find(w => w.tag === tag)?.name || workName(tag);
  return Object.entries(c?.coverage || {}).filter(([, v]) => v).map(([tag, v]) => ({tag, name: name(tag), ...v}));
}
export function coverageSummary(items) {
  const sum = key => number(items.reduce((n, v) => n + (v[key] || 0), 0));
  return t('数据包覆盖 {packs} 个 · 自己补过 {own} 个 · 还缺 {missing} 个', {packs: sum('packs'), own: sum('own'), missing: sum('missing')});
}
const indexBusy = s => INDEX_ACTIVE.includes(s.index?.state?.status);
// 检查新角色的设置改了以后：读回状态和覆盖情况（检查时间或最近一条历史变了才真的读）。
async function act(body) {
  try { await indexAction(body); pollIndex(); if (await loadCoverage(state.index)) refreshSection(); }
  catch (e) { toast(e.message); }
}
const historyItems = s => (s.index?.history || []).flatMap(h => h.characters.map(c => ({...c, at: h.at})));
const indexDot = s => { const st = s.index?.state?.status; return indexBusy(s) ? 'busy' : st === 'error' ? 'warn' : st === 'off' ? 'off' : ''; };
const itemCoverage = x => t('数据包覆盖 {packs} 个 · 自己补过 {own} 个 · 还缺 {missing} 个', {packs: number(x.packs), own: number(x.own), missing: number(x.missing)});
// 检查新角色的几行；收起时整段放进折叠区，展开时直接列在组里（同一份配置用两次，key 带前缀）。inst 是这一节的实例。
const indexParts = (p, inst) => [
  taskRow(p + 'status', s => indexLine(s.index.state, s.index.check), indexDot),
  // 关注哪些作品在「资料库 · 作品」里逐个作品决定；这里只管检查怎么运行。整行是链接。
  {kind: 'info', key: p + 'works', label: t('关注的作品'), hint: t('在资料库的「作品」里逐部开关'), href: '#/characters/works',
   value: s => (s.index.watch.works.length ? t('{n} 部', {n: number(s.index.watch.works.length)}) : t('还没有')) + ' ›'},
  {kind: 'field', key: p + 'min', type: 'number', min: 20, max: 5000, step: 10, unit: t('张'), label: t('只提示图片较多的角色'),
   hint: t('Danbooru 上图片少于这个数的角色不提示'), get: s => s.index.watch.min_posts,
   set: v => act({action: 'watch', works: state.index.watch.works, min_posts: Number(v)})},
  {kind: 'note', text: t('每次检查前确认联网，只允许本次运行。')},
  {kind: 'fold', key: 'indexCoverage', hidden: s => !coverageItems(s.index, indexCoverage()).length,
   summary: s => coverageSummary(coverageItems(s.index, indexCoverage())), parts: [
     {kind: 'list', key: p + 'coverage', label: '', items: s => coverageItems(s.index, indexCoverage()), itemKey: x => x.tag, itemText: x => x.name,
      itemHint: itemCoverage},
   ]},
  {kind: 'fold', key: 'indexHistory', hidden: s => !historyItems(s).length,
   summary: s => t('最近添加的角色（{n} 个）', {n: number(historyItems(s).length)}), parts: [
     {kind: 'list', key: p + 'history', label: '', items: historyItems, itemKey: c => c.at + '\n' + (c.short_tag || c.tag),
      itemText: c => c.entity && c.status === 'added' ? c.entity.name : c.tag || c.short_tag,
      itemHref: c => c.entity && c.status === 'added' ? charHash(c.entity.id) : null,
      itemHint: c => stamp(c.at) + ' · ' + (c.status === 'added' ? t('匹配 {n} 张', {n: number(c.entity?.count ?? c.claims)})
        : c.status === 'dropped' ? t('已移除') : t('未添加：{reason}', {reason: tb(c.reason || '')})),
      actions: [{label: c => inst.armed === c.short_tag ? t('再点一次确认移除') : t('移除'), hidden: c => !(c.status === 'added' && c.entity),
        disabled: (c, s) => indexBusy(s), title: () => t('从索引中移除这个角色：重新扫描后它匹配的图恢复原状，以后检查不再提示'), run: c => inst.drop(c)}]},
   ]},
];
// 资料包卡片底部（#606）：上次检查时还缺几个角色。上次检查找到的还没添加时，整行链到资料库的「作品」，在那里逐个作品查看
// 并添加；否则带一个再检查一次的按钮（联网前照常确认）。不缺或不知道时不显示。
const missing = s => s.pack && s.index?.watch ? coverageMissing(indexCoverage()) : null;
const toAdd = s => (s.index?.check?.missing || []).length > 0;
const checkBlocked = s => { const st = s.index?.state?.status; return INDEX_ACTIVE.includes(st) || st === 'off' || st === 'paused' || !s.index?.watch?.works.length; };
const missingLabel = s => t('关注的作品还缺 {n} 个角色', {n: number(missing(s) || 0)});

// ---------- 名称翻译（可选的 OpenAI 兼容模型，只用来翻译 Bangumi 没有的名字） ----------
const api = () => state.names?.model_api;
// 三个框一起保存：打的字先记在这一节的草稿里（离开输入框时），点「保存」或在框里按回车才发出去；轮询刷新不会把草稿冲掉，
// 换节或离开设置页时草稿（包括没保存的 key）随这一节释放。保存时记下发出去的值，回复回来时只清掉那以后没再改过的框。
// 保存成功、发出去的 key 被采用以后，密码框里如果还是这个 key（焦点还在框里，轮询不会改它的值），也要清掉：不然离开
// 输入框时它又进了草稿，下一次只改地址的保存会把旧 key 带到新地址（宿主把带 key 的保存当成给新地址填的凭据）。
// draft.keySent 记下要清的值，密码框那一行的 force 照它判断（见 ui/rows.js 的 field.force）。
function modelAct(draft, act, snap = draft) {
  const m = api();
  const done = (body, sent, note) => saveModelApi(body).then(() => {
    toast(typeof note === 'function' ? note() : note);
    for (const [k, v] of Object.entries(sent)) if (draft[k] === v) draft[k] = k === 'key' ? '' : null;
    if (sent.key) draft.keySent = sent.key;
  }, e => toast(e.message)).then(refreshSection);
  if (act === 'clear') return done({key: null}, {}, t('API key 已清除'));
  if (act === 'test') return testModelApi().then(r => toast(r.ok === false ? t('连接失败：{reason}', {reason: tb(r.error) || t('未知错误')}) : t('连接成功')),
    e => toast(t('连接失败：{reason}', {reason: e.message})));
  const sent = {base: snap.base, model: snap.model, key: snap.key};
  const body = {base: (snap.base ?? m.base ?? '').trim() || 'https://api.deepseek.com', model: (snap.model ?? m.model ?? '').trim() || 'deepseek-chat'};
  if (snap.key.trim()) body.key = snap.key.trim();
  // 后端只为输入 key 时的那个地址保留 key：换了新地址而不带 key 会把它清掉。
  return done(body, sent, () => m.configured && !state.names?.model_api?.configured ? t('接口地址已更改，原来的 API key 已清除，请重新填写') : t('设置已保存'));
}

const namesDot = s => { const st = s.names?.state?.status; return NAMES_ACTIVE.includes(st) ? 'busy' : st === 'error' ? 'warn' : ['off', 'unauthorized'].includes(st) ? 'off' : ''; };
const counts = s => s.names?.counts;
const countItems = s => [[t('角色'), 'characters'], [t('作品'), 'works'], [t('造型'), 'skins']].filter(([, k]) => counts(s)?.[k])
  .map(([label, k]) => ({k, label, ...counts(s)[k]}));

export function build(main, scope) {
  // 这一节自己的状态：名称翻译的草稿、待确认移除的角色、资料包读取失败的原因。都随这一节的作用域释放。
  const draft = {base: null, model: null, key: '', keySent: null};
  const inst = {armed: null, packError: null};
  // 移除要点两次（4 秒内）；第二次点返回提交的 promise，按钮等结果回来再开放。
  inst.drop = c => {
    if (inst.armed !== c.short_tag) {
      inst.armed = c.short_tag; refreshSection();
      scope.timeout(() => { if (inst.armed === c.short_tag) { inst.armed = null; refreshSection(); } }, 4000);
      return undefined;
    }
    inst.armed = null;
    return act({action: 'drop', char: c.short_tag, reason: '在设置中手动移除'});
  };
  // 回车和按钮共用一条队列：一次保存落定再发下一次，每次发的是按下那一刻框里的值，连续回车时回复不会乱序把值退回去。
  let saving = Promise.resolve();
  const queue = act => {
    const snap = {base: draft.base, model: draft.model, key: draft.key};
    saving = saving.then(() => modelAct(draft, act, snap));
    return saving;
  };
  const save = () => queue('save');
  const view = groups(main, [
    {key: 'pack', title: packName(), scope: 'library', note: s => s.pack ? packNote(s.pack) : '', parts: [
      {kind: 'note', text: () => inst.packError ? readError(inst.packError) : t('正在读取…'), hidden: s => !!s.pack},
      {kind: 'action', key: 'packStatus', hidden: s => !s.pack, label: s => packLine(s.pack), hint: s => packHintText(s.pack), link: s => packManual(s.pack),
       dot: s => packDot(s.pack) || '', progress: s => packProgress(s.pack),
       buttons: s => [...packButtons(s.pack), ...(packHasTask(s.pack) ? [{act: 'tasks', label: t('在任务面板查看')}] : [])],
       run: (s, a) => a === 'tasks' ? openTasks() : runPack(a)},
      {kind: 'list', key: 'packResults', label: '', hidden: s => !packResultItems(s.pack).length, items: s => packResultItems(s.pack),
       itemKey: x => x.key, itemText: x => x.text, itemHint: x => x.hint || ''},
      {kind: 'action', key: 'packOffer', hidden: s => !packOfferInfo(s.pack), label: s => packOfferInfo(s.pack).what, hint: s => packOfferInfo(s.pack).hint,
       buttons: s => packOfferInfo(s.pack).buttons, run: (s, a) => runPack(a)},
      {kind: 'info', key: 'packMissingLink', hidden: s => !missing(s) || !toAdd(s), label: missingLabel,
       hint: t('上次检查新角色时，角色资料包和本地都还没有'), href: '#/characters/works', value: t('查看并添加') + ' ›'},
      {kind: 'action', key: 'packMissing', hidden: s => !missing(s) || toAdd(s), label: missingLabel,
       hint: t('上次检查新角色时，角色资料包和本地都还没有'),
       buttons: s => [{act: 'check', label: t('检查新角色…'), disabled: checkBlocked(s)}],
       run: async () => { try { if (await checkIndex(confirmAction)) pollIndex(); } catch (e) { toast(e.message); } }},
      {kind: 'fold', key: 'pack', summary: t('文件、版本与许可'), hidden: s => !packInfoItems(s.pack).length, parts: [
        {kind: 'list', key: 'packFiles', label: '', items: s => packInfoItems(s.pack), itemKey: x => x.key, itemText: x => x.text, itemHint: x => x.hint,
         itemTitle: x => x.title},
        {kind: 'action', key: 'packDelete', label: '', hidden: s => !packDeleteButtons(s.pack).length, buttons: s => packDeleteButtons(s.pack), run: (s, a) => runPack(a)},
      ]},
    ]},
    {key: 'index', title: t('检查新角色'), scope: 'library', hidden: s => !s.index?.watch,
     note: s => indexFolded(s.index, indexCoverage())
       ? t('上次检查（{time}）关注的作品都不缺角色。角色资料包发布以后才出现的角色，在这里检查。', {time: stamp(indexCoverage().checked_at)})
       : t('在 Danbooru 上查关注的作品有没有角色资料包和本地都还没有的角色，只发送作品名。'),
     more: s => t('添加角色时每个角色下载约 20 张公开参考图（约 {mb} MB），在本机算好特征后重新扫描图库。', {mb: s.index?.mb_per_character}),
     parts: [
       {kind: 'fold', key: 'indexMore', summary: t('检查设置与记录'), hidden: s => !indexFolded(s.index, indexCoverage()), parts: indexParts('f-', inst)},
       ...indexParts('', inst).map(part => ({...part, hidden: s => indexFolded(s.index, indexCoverage()) || Boolean(part.hidden?.(s))})),
     ]},
    {key: 'names', title: t('角色名称'), scope: 'library', hidden: s => !s.names,
     note: t('从 Bangumi 取角色的中文名，只发送 tag 和作品名。没有中文名时显示日文名，再没有就显示英文名；原始 tag 在悬停提示里。'), parts: [
       taskRow('namesStatus', s => namesLine(s.names.state || {}) + (s.names.state?.error ? t('：{reason}', {reason: tb(s.names.state.error)}) : ''), namesDot),
       startupRow('names', t('启动时获取'), t('只查新增的角色')),
       {kind: 'fold', key: 'namesStats', hidden: s => !counts(s),
        summary: s => {
          const cc = counts(s)?.characters, active = NAMES_ACTIVE.includes(s.names?.state?.status);
          return t('覆盖率') + (active ? t('（完成后更新）') : '') + (cc ? t('：{p}% 的角色有中文名', {p: Math.round(cc.zh / Math.max(1, cc.total) * 100)}) : '');
        }, parts: [
          {kind: 'list', key: 'namesCounts', label: '', items: countItems, itemKey: x => x.k, itemText: x => x.label,
           itemHint: x => t('共 {total} · 中文名 {zh} · 日文名 {ja} · 英文名 {en}', {total: number(x.total), zh: number(x.zh || 0), ja: number(x.ja || 0), en: number(x.readable || 0)})},
        ]},
     ]},
    {key: 'model', title: t('名称翻译（可选）'), scope: 'library', hidden: () => !api(),
     note: t('Bangumi 没有中文名时，用兼容 OpenAI 接口的大模型翻译，只发送 tag、日文名和作品名。译名可能不准，悬停提示里会标「机器翻译」。API key 只存在这个图库的数据库里。'), parts: [
       {kind: 'field', key: 'modelBase', label: t('接口地址'), placeholder: 'https://api.deepseek.com', wide: true, plain: true,
        get: () => draft.base ?? api()?.base ?? '', set: v => { draft.base = v; }, enter: save},
       {kind: 'field', key: 'modelName', label: t('模型'), placeholder: 'deepseek-chat', wide: true, plain: true,
        get: () => draft.model ?? api()?.model ?? '', set: v => { draft.model = v; }, enter: save},
       {kind: 'field', key: 'modelKey', label: 'API key', type: 'password', wide: true, get: () => draft.key, set: v => { draft.key = v; }, enter: save,
        force: (s, current) => { const hit = !!draft.keySent && current === draft.keySent; draft.keySent = null; return hit; },
        placeholder: () => api()?.configured ? t('已配置（尾号 {hint}），留空则不修改', {hint: api().hint || ''}) : t('未配置')},
       {kind: 'action', key: 'modelSave', label: () => api()?.configured ? t('已配置') : t('未配置，不会使用'),
        buttons: () => [...(api()?.configured ? [{act: 'clear', label: t('清除 key')}, {act: 'test', label: t('测试连接')}] : []), {act: 'save', label: t('保存'), primary: true}],
        run: (s, a) => queue(a)},
     ]},
  ], {scope});
  // 检查新角色结束（检查时间或最近一条历史可能变了）：重读每个作品的覆盖情况。
  scope.add(onJob((job, change) => { if (job === 'index' && change.ended) loadCoverage(state.index).then(changed => { if (changed && scope.alive) refreshSection(); }); }));
  // 进这一节：资料包、检查新角色、角色名称和覆盖情况各读一次（还没读到的先显示「正在读取…」）。离开以后回来的回复不弹提示。
  function load(fresh) {
    const route = state.route, here = () => scope.alive && state.route === route;
    const fail = e => { if (here()) toast(e.message); };
    inst.packError = null;
    const reads = [loadPack().catch(e => { if (here()) { inst.packError = e; refreshSection(); } })];
    if (fresh || !state.index) reads.push(loadIndex().then(() => here() && refreshSection(), fail));
    if (fresh || !state.names) reads.push(loadNames().then(() => {
      if (!here()) return;
      refreshSection();
      if (NAMES_ACTIVE.includes(state.names?.state?.status)) pollNamesSoon();
    }, fail));
    reads.push(Promise.all(reads.slice(1)).then(() => loadCoverage(state.index, fresh)).then(changed => { if (changed && here()) refreshSection(); }));
    return Promise.all(reads);
  }
  return {update: view.update, load};
}
