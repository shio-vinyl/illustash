// 设置 › 画师（#714）：找回画师（docs/PROVENANCE.md 第 8 节，运行的轮询在 core/jobs.js）和绑定的账号（Pixiv、X、
// SauceNAO；Pixiv 和 SauceNAO 所有图库共用，密钥存在钥匙串里，X 的关注按图库导入）。
import {t, tb} from '../core/i18n.js';
import {isMac, localTime, number, pct} from '../core/format.js';
import {state} from '../core/state.js';
import {PROV_ACTIVE, loadProvenance} from '../core/jobs.js';
import {saveStartup, loadAccounts, accountRequest, waitPixivLogin, stopPixivLogin} from '../core/settings.js';
import {toast} from './dom.js';
import {devMode, groups, tiles} from './settings_group.js';
import {openTasks} from './tasks.js';

// ---------- 找回画师 ----------
const prov = s => s.provenance;
const working = s => PROV_ACTIVE.includes(prov(s)?.state?.status);
const cov = s => prov(s)?.coverage;
function coverageTiles(s) {
  const c = cov(s); if (!c) return [];
  const tile = (key, label) => c.pictures?.[key] ? [{key, label, big: pct(c.with_artist?.[key] || 0, c.pictures[key]),
    small: t('{done}/{total} 张', {done: number(c.with_artist?.[key] || 0), total: number(c.pictures[key])})}] : [];
  const total = Object.values(c.pictures || {}).reduce((a, b) => a + b, 0);
  return [...tile('x', t('X 的图有画师')), ...tile('pixiv', t('Pixiv 的图有画师')), ...tile('other', t('其他来源有画师')),
    {key: 'post', label: t('能对到原帖'), big: pct(c.with_original_post || 0, total), small: t('{n} 张', {n: number(c.with_original_post || 0)})}];
}
function provLine(s) {
  const st = prov(s).state || {}, run = prov(s).runs?.[0];
  if (working(s)) return t('正在查找…');
  if (st.status === 'failed' && st.reason) return t('上次出错：{reason}', {reason: tb(st.reason)});
  return run ? t('上次 {time} · 新找到 {n}', {time: localTime(run.started_at), n: number(run.found || 0)}) : t('还没有查过');
}
const provReady = s => !!prov(s)?.available;

// ---------- 账号 ----------
async function account(action, body = {}) {
  try { return await accountRequest(action, body); } catch (err) { toast(err.message); return null; }
}
const draft = {pixiv: '', x: ''};
const pixiv = s => s.accounts?.pixiv;
const desk = s => !!s.accounts?.desktop;
const inApp = s => pixiv(s)?.via === 'app';
async function pixivAct(act) {
  if (act === 'login') { if (await account('pixiv/login')) waitPixivLogin(); return; }
  if (act === 'check') {
    const r = await account('pixiv/check');
    if (r) { toast(r.ok ? t('Pixiv 登录有效：{name}', {name: r.name || r.user_id}) : t('Pixiv 登录已过期，请重新登录')); loadAccounts(); }
    return;
  }
  if (act === 'unbind') { stopPixivLogin(); if (await account('pixiv/unbind')) { toast(t('已解绑')); loadAccounts(); } return; }
  if (act === 'bind') {
    const r = await account('pixiv', {value: draft.pixiv});
    if (r) { draft.pixiv = ''; toast(t('已绑定 Pixiv：{name}', {name: r.name || r.user_id})); loadAccounts(); }
  }
}
// 登录按钮：正在等用户在 Pixiv 窗口里登录时不给，状态写在说明里。
const login = (s, label) => s.pixivWaiting ? [] : [{act: 'login', label}];
const waiting = s => s.pixivWaiting ? t('正在等你在 Pixiv 窗口里登录…') : '';
const why = () => t('找回画师会暂停查 Pixiv，免得在没登录的状态下漏掉 NSFW 作品。');
// Pixiv 已绑定但登录过期时的说明：桌面 App 里重新登录；登录在 App 窗口里而现在不在 App 里；网页版（开发用）重新粘贴。
function expiredNote(s) {
  const p = pixiv(s);
  if (!p?.bound || !p.expired) return '';
  if (desk(s)) return t('Pixiv 不再接受这个登录（退出过，或者过期了）。') + why() + t('在 Pixiv 窗口里重新登录一次就能恢复。')
    + (p.window_logged_in ? t('窗口里已经重新登录了，点「重新检查」确认。') : '');
  if (inApp(s)) return t('Pixiv 登录在桌面 app 的 Pixiv 窗口里。') + why() + t('请从 Illustash 桌面 app 打开，再重新登录。');
  if (!devMode) return t('Pixiv 不再接受这个登录状态。') + why() + t('请从 Illustash 桌面 app 重新登录。');
  return t('Pixiv 不再接受这个登录状态（在浏览器里退出过，或者过期了）。') + why() + t('重新复制一次 {cookie} 粘贴到下面就能恢复。', {cookie: 'PHPSESSID'});
}
// 粘贴 PHPSESSID 是普通网页版的开发入口：开发模式下、不在桌面 App 里时才有（没绑定，或绑定的不是 App 窗口且已过期）。
const pasteShown = s => devMode && !desk(s) && (!pixiv(s)?.bound || (pixiv(s).expired && !inApp(s)));

export function build(main, scope) {
  return groups(main, [
    {key: 'provenance', title: t('找回画师'), scope: 'library',
     note: t('逐张查图的画师：用像素哈希和 IQDB 签名（只发哈希文本，不发图片）在 Danbooru 查原帖，并比对你关注的画师。结果在详情面板的「画师」里。'), parts: [
       {kind: 'info', key: 'provReading', label: t('正在读取…'), hidden: s => !!prov(s)},
       {kind: 'info', key: 'provNone', label: t('只有图库包能找回画师。'), hidden: s => !prov(s) || provReady(s)},
       {kind: 'block', key: 'provTiles', hidden: s => !provReady(s) || !cov(s), mount: tiles(coverageTiles)},
       {kind: 'action', key: 'provStatus', hidden: s => !provReady(s), label: provLine,
        dot: s => working(s) ? 'busy' : prov(s).state?.status === 'failed' ? 'warn' : '',
        buttons: s => working(s) ? [{act: 'tasks', label: t('在任务面板查看')}] : [{act: 'all', label: t('找回全库画师…')}],
        run: (s, act) => openTasks({armProvenance: act === 'all'})},
       {kind: 'switch', key: 'provAuto', hidden: s => !provReady(s), label: t('同步后自动查新图'),
        hint: s => (prov(s).auto_network ? t('新加入的图做签名识别、查像素哈希、比对新找到的画师；每次最多 {mb} MB', {mb: number(prov(s).budgets?.new || 50)})
          : t('新加入的图只在本机做签名识别；要联网查，在任务面板里每次授权')) + (prov(s).frozen ? ' · ' + t('测试图库不同步，这个开关不起作用') : ''),
        get: s => !!prov(s).auto,
        set: async on => {
          try { await saveStartup({provenance: on}); toast(on ? t('同步后会自动查新图的画师') : t('同步后不再自动查画师')); } catch (err) { toast(err.message); }
          loadProvenance();
        }},
       {kind: 'action', key: 'audit', hidden: s => !provReady(s) || !cov(s), label: t('画师抽查'), hint: t('50 张随机图，逐张核对找到的画师'),
        href: '#/artist-audit', buttons: () => [], value: '›'},
       {kind: 'note', text: t('登录 Pixiv 后才能比对 NSFW 作品（见下方「账号」）。'), hidden: s => !provReady(s) || !s.accounts || !!pixiv(s)?.bound},
     ]},
    {key: 'pixiv', title: 'Pixiv', scope: 'shared', hidden: s => !s.accounts,
     note: t('找回画师会读你的关注列表，把关注的人记为候选画师；在详情面板点「找 TA 的其他图」时，才拉这位画师公开作品的小图，在本机逐张比对。只读，不点赞、不关注、不发任何内容。'), parts: [
       {kind: 'note', hidden: s => pixiv(s)?.bound || !desk(s),
        text: t('打开 Pixiv 官方登录页，由你亲手登录（出现人机验证请自己完成）；登录状态只留在 app 的 Pixiv 窗口里。登录后才能看到 NSFW 作品。Pixiv 没有给第三方开放接口，这里用的是网页自己的接口，低频只读。Illustash 不是 pixiv 开发或分发的应用。')},
       {kind: 'note', hidden: s => pixiv(s)?.bound || desk(s),
        text: () => devMode ? t('网页版（开发用）：在浏览器里登录 pixiv.net，打开开发者工具（{keys}）→ Application（Safari 为「存储空间」）→ Cookie → pixiv.net，复制 {cookie} 的值粘贴到这里。',
          {keys: isMac ? '⌥⌘I' : 'Ctrl+Shift+I', cookie: 'PHPSESSID'}) : t('请从 Illustash 桌面 app 打开设置，在它的 Pixiv 窗口里登录。')},
       {kind: 'action', key: 'pixivLogin', label: t('尚未登录'), hint: waiting, hidden: s => pixiv(s)?.bound || !desk(s),
        buttons: s => login(s, t('登录 Pixiv…')), run: (s, act) => pixivAct(act)},
       {kind: 'action', key: 'pixivBound', hidden: s => !pixiv(s)?.bound,
        label: s => pixiv(s).expired ? t('登录已过期') : t('已绑定'), dot: s => pixiv(s).expired ? 'warn' : null,
        hint: s => {
          const p = pixiv(s);
          return [p.name || '', p.user_id ? `ID ${p.user_id}` : '', p.following != null && !p.expired ? t('关注 {n} 位', {n: number(p.following)}) : '',
            p.checked_at ? t('检查于 {time}', {time: p.checked_at}) : '', waiting(s)].filter(Boolean).join(' · ');
        },
        buttons: s => [...(desk(s) && !pixiv(s).expired ? login(s, inApp(s) ? t('打开 Pixiv 窗口') : t('改用窗口登录…')) : []),
          {act: 'check', label: t('重新检查')}, {act: 'unbind', label: t('解绑')}],
        run: (s, act) => pixivAct(act)},
       {kind: 'note', text: expiredNote, hidden: s => !expiredNote(s)},
       {kind: 'action', key: 'pixivRelogin', label: t('重新登录'), hint: waiting, hidden: s => !pixiv(s)?.bound || !pixiv(s).expired || !desk(s),
        buttons: s => login(s, t('重新登录 Pixiv…')), run: (s, act) => pixivAct(act)},
       {kind: 'field', key: 'pixivPaste', label: 'PHPSESSID', hint: t('开发用入口'), type: 'password', placeholder: '12345678_…', hidden: s => !pasteShown(s),
        get: () => draft.pixiv, set: v => { draft.pixiv = v; }},
       {kind: 'action', key: 'pixivBind', label: '', hidden: s => !pasteShown(s),
        buttons: s => [{act: 'bind', label: pixiv(s)?.bound ? t('重新绑定') : t('绑定')}], run: (s, act) => pixivAct(act)},
     ]},
    {key: 'x', title: 'X', scope: 'library', hidden: s => !s.accounts,
     note: t('用 X 的官方数据归档导入关注列表：X 设置 → 你的账号 → 下载你的数据，收到邮件后下载 zip 导入。只读取其中的关注（数字账号 ID），私信、推文都不会打开。'), parts: [
       {kind: 'action', key: 'xImport', label: s => s.accounts.x.imported ? t('已导入 {n} 个关注', {n: number(s.accounts.x.imported)}) : t('尚未导入'),
        hint: s => {
          const x = s.accounts.x; if (!x.imported) return '';
          return t('已对应到画师 {resolved} 位 · 待查 {pending} 个', {resolved: number(x.resolved || 0), pending: number(x.pending || 0)})
            + (x.imported_at ? ' · ' + t('导入于 {time}', {time: String(x.imported_at).slice(0, 16)}) : '');
        },
        buttons: () => [{act: 'import', label: t('导入归档…')}],
        run: async () => {
          const path = draft.x.trim(), r = await account('x/import', path ? {path} : {});
          if (r && !r.cancelled) { draft.x = ''; toast(t('新导入 {n} 个关注', {n: number(r.new)})); loadAccounts(); }
        }},
       {kind: 'field', key: 'xPath', label: t('归档路径'), placeholder: t('或粘贴 zip / 解压后文件夹的路径'), wide: true, plain: true,
        get: () => draft.x, set: v => { draft.x = v; }},
     ]},
    {key: 'saucenao', title: t('SauceNAO（以图搜图）'), scope: 'shared', hidden: s => !s.accounts?.saucenao?.bound, parts: [
      {kind: 'action', key: 'sauce', label: t('已保存 API key'), hint: t('以图搜图还没有上线，现在不会使用'), buttons: () => [{act: 'remove', label: t('移除')}],
       run: async () => { if (await account('saucenao/unbind')) { toast(t('已解绑')); loadAccounts(); } }},
    ]},
  ], {scope});
}
// 进这一节：找回画师的状态和账号各读一次（先立即显示缓存的状态）。
export function loadSection() { loadProvenance(); loadAccounts(); }
