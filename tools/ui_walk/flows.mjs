// 走查流程（#453）。按跨页面、跨浮层的使用流程组织，不按单页组织：#386 找到的问题多半是「功能 A 对、功能 B 对，
// 连起来不对」。每条流程从一份新复制的合成图库开始（tools/ui_walk/fixture.py），只用真实键鼠事件操作界面；
// 准备数据（如重开引导）才直接调接口，并在步骤名里写明。
// 检查项只写「人看得见的对错」，不写实现细节，这样换一种实现（#469）后同一套检查仍然成立。
// covers 写这条流程能拦住的 #386 条目，report.md 里会列出。
//
// 新增流程：在 FLOWS 里加一项 {name, title, covers, run(w, page)}，w 是 walker.mjs 的 Walker。

import {sleep} from './cdp.mjs';

// ---------- 小工具 ----------

const uuidOf = (w, name) => Object.entries(w.config.names).find(([, n]) => n === name)?.[0];
const cardSel = (w, name) => `#grid .card[data-id="${uuidOf(w, name)}"]`;
const cardAt = i => `#grid .card[data-index="${i}"]`;

// 换页：点活页条上的链接（一级入口、二级项、设置，和人一样）；活页条上没有的地址改 location.hash（等同点页面里的链接）。
async function goto(page, hash) {
  const link = await page.eval(`(() => { const a = [...document.querySelectorAll('#rail a[href]')].find(a => a.getAttribute('href') === ${JSON.stringify(hash)}); return a ? true : false; })()`);
  if (link) await page.click(`#rail a[href="${hash}"]`);
  else await page.eval(`location.hash = ${JSON.stringify(hash)}`);
  await page.waitFor(`decodeURIComponent(location.hash).startsWith(${JSON.stringify(hash)})`, 4000);
  await gridReady(page);
}

// 等网格（或面板页）画好：网格页等第一张卡片出现，面板页等主区有内容。
async function gridReady(page) {
  await page.settle({quiet: 300});
  await page.waitFor(`document.querySelector('#grid .card[data-id]') || !document.getElementById('grid').offsetParent
    || !document.getElementById('empty').hidden`, 6000);
  await page.settle({quiet: 200});
}

// 把一张图滚到网格的可见范围里（虚拟网格只画看得见的卡片）。和人往下滚找图一样改的是滚动位置。
async function reveal(w, page, name) {
  const sel = cardSel(w, name);
  const found = await page.eval(`(async () => {
    const v = document.getElementById('viewport');
    for (let i = 0; i < 60; i++) {
      const el = document.querySelector(${JSON.stringify(sel)});
      if (el) { el.scrollIntoView({block: 'center'}); await new Promise(r => setTimeout(r, 200)); return true; }
      if (v.scrollTop + v.clientHeight >= v.scrollHeight - 2 && i > 0) v.scrollTop = 0; else v.scrollTop += v.clientHeight * 0.8;
      await new Promise(r => setTimeout(r, 150));
    }
    return false; })()`);
  if (!found) throw new Error('网格里找不到 ' + name);
  await page.settle({quiet: 200});
  return sel;
}
const openCard = async (w, page, name, opts) => page.click(await reveal(w, page, name), opts);

const inPage = (page, code) => page.eval(`(async () => { ${code} })()`);
// 读当前状态里的一个值（只读，不改）。
const stateOf = (page, expr) => inPage(page, `const S = (await import('/core/state.js')).state; return ${expr};`);
// 这张图现在挂着哪些角色（读接口，不看界面文案，中英文一样）。
const rolesOf = (w, page, name) => inPage(page, `const r = await fetch('/api/detail?id=' + encodeURIComponent(${JSON.stringify(uuidOf(w, name))}));
  if (!r.ok) return null; const d = await r.json(); return (d.memberships || []).map(m => m.name).sort();`);
// 页面里的写入接口（只在准备数据时用）。
const post = (page, path, body) => inPage(page, `const {api} = await import('/core/session.js');
  return api(${JSON.stringify(path)}, {method: 'POST', body: JSON.stringify(${JSON.stringify(body)})});`);

// 这张图（按 uuid）上的人工结论：[{id, name, status}]，按角色 id 排（读 /api/detail 的 assertions，中英文一样）。
const assertionsOf = (page, uuid) => inPage(page, `const r = await fetch('/api/detail?id=' + encodeURIComponent(${JSON.stringify(uuid)}));
  if (!r.ok) return null; const d = await r.json();
  return (d.assertions || []).map(a => ({id: a.id, name: a.name, status: a.status})).sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0);`);
// 命令面板里搜一个角色名，等结果出来；回车前底栏那句话说明会做什么。
async function pickerFind(page, name) {
  // 面板没开时不打字：网格里的字母都是快捷键。
  if (!await page.waitFor(`document.getElementById('picker').open`, 3000)) throw new Error('命令面板没有打开');
  await page.type(name);
  await page.waitFor(`[...document.querySelectorAll('#pickerResults [role=option]')].some(r => r.textContent.includes(${JSON.stringify(name)}))`, 4000);
  await page.settle({quiet: 200});
}

// 更改和撤销会延后刷新角色统计。等可见反馈完成，再记录稳定状态，避免下一场景跨过提示消失的时刻。
async function feedbackEnd(page) {
  if (!await page.waitFor(`!document.getElementById('toast').hidden`, 6000)) throw new Error('操作没有显示反馈');
  if (!await page.waitFor(`document.getElementById('toast').hidden`, 12000)) throw new Error('操作反馈没有结束');
  await page.settle({quiet: 400});
}

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

// ---------- 流程 ----------

export const FLOWS = [
  {
    name: 'catalog',
    title: '资料库：全部作品弹层 → 作品页 → 返回回到资料库（#711）',
    async run(w, page) {
      await w.step('打开资料库', () => goto(page, '#/characters'));
      await w.step('点「全部作品」', () => page.click('#castAll'));
      const rows = await page.waitFor(`document.querySelectorAll('#castPop .cast-wrow').length > 0`, 4000);
      w.check('全部作品：弹层里有作品', !!rows, '');
      const width = await page.eval(`document.getElementById('castPop')?.getBoundingClientRect().width || 0`);
      w.check('全部作品：弹层够宽（三列放得下）', width >= 320, `width=${width}`);
      await w.step('点一部作品', () => page.click('#castPop .cast-wrow'));
      const opened = await page.waitFor(`location.hash.startsWith('#/w/') && !!document.querySelector('.cat-hero-title')?.innerText`, 4000);
      w.check('作品页：横幅上有作品名', !!opened, await page.eval('location.hash'));
      const posters = await page.eval(`document.querySelectorAll('.cat-workpage .cat-poster').length`);
      w.check('作品页：有角色海报', posters > 0, `posters=${posters}`);
      const back = await page.eval(`document.querySelector('.cat-hero-top > a.btn')?.innerText.trim() || ''`);
      w.check('作品页：返回写回到资料库', back === '资料库' || back === 'Catalog', back);
      await w.step('点返回', () => page.click('.cat-hero-top > a.btn'));
      const returned = await page.waitFor(`location.hash === '#/characters' && document.querySelectorAll('#charIndex .cat-poster').length > 0`, 4000);
      w.check('返回：回到资料库的角色页签，海报还在', !!returned, await page.eval('location.hash'));
    },
  },
  {
    name: 'character',
    title: '角色页的横幅（#712）：名字、返回按来路（从日历、资料库进来）、未检查开关、页头小标题',
    async run(w, page) {
      const f = w.config.fixture, c = f.chars[0], hash = `#/library?char=${encodeURIComponent(f.charIds[c.name])}`;
      const banner = `document.querySelector('#gridHead .char-hero')`;
      const backText = `(document.querySelector('.char-hero-bar > a.btn')?.innerText || '').trim()`;
      await goto(page, '#/calendar');
      await w.step('从日历进角色页', () => goto(page, hash), {shot: true});
      const nameText = `(${banner}?.querySelector('.char-hero-name')?.innerText || '').trim()`;
      await page.waitFor(nameText, 4000);
      const name = await page.eval(nameText);
      w.check('横幅：写着角色名', name === c.name, name);
      w.check('页头：横幅的名字看得见时小标题藏着', await page.eval(`!!document.querySelector('#heading .heading-main.is-tucked')`), '');
      const fromCalendar = await page.eval(backText);
      w.check('返回：从日历进来写「图库」', fromCalendar === '图库' || fromCalendar === 'Library', fromCalendar);
      // 未检查开关：合成图库的角色可能没有未检查的图，有开关时才按。按过以后多一条历史，返回仍回到日历。
      if (await page.eval(`!!document.querySelector('.char-hero-check') && !document.querySelector('.char-hero-check').closest('[hidden]')`)) {
        await w.step('按未检查', () => page.click('.char-hero-check'));
        const pressed = await page.waitFor(`document.querySelector('.char-hero-check')?.getAttribute('aria-pressed') === 'true' && location.hash.includes('check=1')`, 4000);
        w.check('未检查：按下后只看未检查', !!pressed, await page.eval('location.hash'));
      }
      await w.step('点返回', () => page.click('.char-hero-bar > a.btn'));
      const back = await page.waitFor(`location.hash === '#/calendar'`, 4000);
      w.check('返回：回到日历', !!back, await page.eval('location.hash'));
      w.check('离开以后网格头部藏起来', await page.eval(`document.getElementById('gridHead')?.hidden !== false`), '');
      await goto(page, '#/characters');
      await goto(page, hash);
      await page.waitFor(backText, 4000);
      const fromCatalog = await page.eval(backText);
      w.check('返回：从资料库进来写「资料库」', fromCatalog === '资料库' || fromCatalog === 'Catalog', fromCatalog);
    },
  },
  {
    name: 'pages',
    title: '主要页面逐个打开，截图并检查控制台和空页',
    async run(w, page) {
      const f = w.config.fixture;
      const pages = [
        ['图库 · 按天', '#/days', 'home'], ['图库 · 网格', '#/library', 'grid'], ['图库 · 日历', '#/calendar', 'panel'], ['资料库', '#/characters', 'panel'],
        ['角色页', `#/library?char=${encodeURIComponent(f.charIds[f.chars[0].name])}`, 'grid'],
        ['资料库 · 作品', '#/characters/works', 'panel'], ['资料库 · 画师', '#/characters/artists', 'panel'],
        ['作品页（未归作品）', '#/w/other', 'work'], ['画廊', '#/galleries', 'panel'],
        ['未识别 · 按组整理', '#/groups', 'panel'], ['未识别 · 第 1 组', `#/g/${f.groups[0]?.id}`, 'grid'],
        ['全部未识别', '#/unmatched', 'grid'], ['已跳过', '#/skipped', 'grid'], ['重复', '#/duplicates', 'grid'],
        ['画廊', '#/galleries', 'panel'],
        ['相似的图', '#/similar', 'panel'], ['相似的图 · 第 1 组', `#/s/${f.similar[0]?.id}`, 'grid'], ['已排除', '#/excluded', 'grid'],
        ['身份整理', '#/identity', 'empty'], ['画师抽查', '#/artist-audit', 'panel'],
        ['设置 · 通用', '#/settings', 'panel'], ['设置 · 图库', '#/settings/library', 'panel'],
        ['设置 · 角色资料', '#/settings/characters', 'panel'], ['设置 · 画师', '#/settings/artists', 'panel'],
        ['设置 · 网络与隐私', '#/settings/network', 'panel'], ['设置 · 数据', '#/settings/data', 'panel'], ['设置 · 关于', '#/settings/about', 'panel'],
      ];
      for (const [label, hash, kind] of pages) {
        const snap = await w.step(`打开${label}`, () => goto(page, hash), {shot: true});
        // 作品页的作品名写在横幅上，页头标题按设计是空的（#711）。
        const title = kind === 'work' ? await page.eval(`document.querySelector('.cat-hero-title')?.innerText || ''`) : snap.heading;
        w.check(`${label}：标题不空`, !!title, title);
        if (kind === 'grid') w.check(`${label}：网格有图`, snap.total > 0, `total=${snap.total}`);
        // 合成图库里没有长得像的角色对：身份整理是空状态，看它在、写了事实（中文那句不到 20 个字，不用下面的字数判断）。
        if (kind === 'empty') {
          const fact = await page.waitFor(`(document.querySelector('#groupsPage .empty-state:not([hidden]) .empty-fact')?.textContent || '').trim()`, 3000);
          w.check(`${label}：空状态写了事实`, fact, '');
          continue;
        }
        if (kind !== 'grid') {
          const filled = await page.eval(`(() => { const v = document.getElementById('viewport'); return [...v.children].some(el => !el.hidden && el.id !== 'grid' && el.id !== 'marquee' && el.id !== 'empty' && (el.innerText || '').trim().length > 20); })()`);
          w.check(`${label}：页面有内容`, filled, '');
        }
      }
    },
  },

  {
    name: 'days',
    title: '图库按天视图：两张卡、各天和到底的说明；认一认点掉一张再「点掉的是…」；点图开详情；深色；at= 滚到那一天；去网格看这一天',
    covers: ['#707', '#675'],
    async run(w, page) {
      const ready = () => page.waitFor(`document.querySelector('#homePage .day-tile img')`, 8000);
      let snap = await w.step('打开图库（按天）', async p => { await goto(p, '#/days'); await ready(); }, {shot: true});
      w.check('标题是图库', /图库|Library/.test(snap.heading), snap.heading);
      const days = await page.eval(`[...document.querySelectorAll('#homePage .day')].map(d => d.dataset.day).sort().reverse()`);
      w.check('合成图库的几天都画出来了', days.length >= 3, days);
      await w.step('滚到底（接着读更早的几天，读完写到底的说明）', async p => {
        await p.eval(`(async () => { const v = document.getElementById('viewport'); for (let i = 0; i < 20; i++) { v.scrollTop = v.scrollHeight;
          await new Promise(r => setTimeout(r, 250)); const e = document.querySelector('#homePage .day-end'); if (e && !e.hidden) return; } })()`);
      }, {shot: true});
      w.check('读到底：写着最早的一张和总张数', await page.eval(`(() => { const e = document.querySelector('#homePage .day-end'); return !!e && !e.hidden && /\\d/.test(e.textContent); })()`), '');
      await w.step('回到顶上', p => p.eval(`document.getElementById('viewport').scrollTop = 0`));
      w.check('同步卡在', await page.eval(`!document.querySelector('#homePage .day-sync').hidden`), '');
      const asking = await page.eval(`!document.querySelector('#homePage .day-ask').hidden`);
      w.check('认一认卡在（合成图库有可问的）', asking, '');
      if (asking) {
        await w.step('认一认：点「不全是」展开', p => p.click('#homePage .day-ask button[data-ref="open"]'));
        snap = await w.step('点掉第一张', p => p.click('#homePage .day-ask .judge-tile'), {shot: true});
        w.check('第一张标成「不是 TA」', await page.eval(`document.querySelector('#homePage .day-ask .judge-tile').getAttribute('aria-pressed') === 'true'`), '');
        w.check('有了「点掉的是…」', await page.eval(`!document.querySelector('#homePage .day-ask button[data-ref="other"]').hidden`), '');
        // #675 第二步：选另一个角色，点掉的那张一步记成「不是原角色、是选中的角色」（两条人工结论），其余几张留在卡上；撤销一步全撤。
        const tiles = `#homePage .day-ask .judge-tile:not([inert])`;
        const offId = await page.eval(`document.querySelector('#homePage .day-ask .judge-tile').dataset.peek`);
        const askedId = await page.eval(`new URLSearchParams((document.querySelector('#homePage .day-ask [id] a')?.getAttribute('href') || '').split('?')[1] || '').get('char') || ''`);
        const asked = await page.eval(`document.querySelector('#homePage .day-ask [id] a')?.textContent || ''`);
        const left = await page.eval(`[...document.querySelectorAll(${JSON.stringify(tiles)})].map(t => t.dataset.peek).filter(u => u !== ${JSON.stringify(offId)})`);
        const pick = w.config.fixture.chars.map(c => c.name).find(n => n !== asked);
        const before = await assertionsOf(page, offId);
        await w.step('点「点掉的是…」', p => p.click('#homePage .day-ask button[data-ref="other"]'));
        w.check('角色选择器打开了', await page.waitFor(`document.getElementById('picker').open`, 3000), '');
        await w.step(`选 ${pick}，回车`, async p => { await pickerFind(p, pick); await p.key('Enter'); await feedbackEnd(p); }, {shot: true});
        const after = await assertionsOf(page, offId);
        w.check('点掉的那张：原角色记成 rejected、选中的角色记成 confirmed',
          !!askedId && after.some(a => a.id === askedId && a.status === 'rejected') && after.some(a => a.name === pick && a.status === 'confirmed'), {before, after, askedId, pick});
        w.check('其余几张原样留在卡上（按 UUID 比）', same(await page.eval(`[...document.querySelectorAll(${JSON.stringify(tiles)})].map(t => t.dataset.peek)`), left), {left});
        await w.step('撤销', async p => { await p.click('#undoButton'); await feedbackEnd(p); });
        const undone = await assertionsOf(page, offId);
        w.check('撤销一步，两条人工结论都还原（和提交前逐项相同）', same(undone, before), {before, undone});
      }
      snap = await w.step('点第一天的大图（开详情）', p => p.click('#homePage .day .day-tile'), {shot: true});
      w.check('详情打开在点的那张', !!snap.focused, snap.focused);
      const focusedId = await stateOf(page, 'S.focused');
      w.check('那张图标成当前（只有它一张）', await page.eval(`(() => { const on = [...document.querySelectorAll('#homePage .day-tile[aria-current="true"]')];
        return on.length === 1 && on[0].dataset.id === ${JSON.stringify(focusedId || '')}; })()`), focusedId);
      await w.step('Esc 关详情', p => p.key('Escape'));
      await w.step('深色', p => p.eval(`document.documentElement.dataset.theme = 'dark'`), {shot: true});
      await w.step('换回浅色', p => p.eval(`document.documentElement.dataset.theme = 'light'`));
      const last = days.at(-1);
      await w.step('地址带 at= 最早那天', async p => { await p.eval(`location.hash = '#/days?at=${last}'`); await p.settle({quiet: 500}); });
      w.check('滚到了最早那天', await page.eval(`(() => { const v = document.getElementById('viewport'), d = document.querySelector('#homePage .day[data-day="${last}"]');
        if (!d) return false; const top = d.getBoundingClientRect().top - v.getBoundingClientRect().top; return v.scrollTop > 0 && top > -40 && top < v.clientHeight / 2; })()`), '');
      snap = await w.step('点「在网格里看」', async p => { await p.click('#homePage .day .day-grid'); await gridReady(p); }, {shot: true});
      w.check('进了网格，按那一天筛选', /#\/library\?.*day=\d{4}-\d{2}-\d{2}/.test(decodeURIComponent(snap.hash || '')) && snap.total > 0, {hash: snap.hash, total: snap.total});
    },
  },

  {
    name: 'viewer-detail',
    title: '图库点图开详情栏 → Space 进大图翻几张 → 关大图：详情栏、光标和 Space 指向同一张',
    covers: ['#386 1-4'],
    async run(w, page) {
      await w.step('打开图库', () => goto(page, '#/library'));
      const before = await w.step('点第 6 张（开详情栏）', p => p.click(cardAt(5)));
      w.check('详情栏打开在点的那张', before.focused && before.focused === before.cursor, before);
      await w.step('Space 进大图', p => p.key('Space'));
      let snap;
      for (let i = 1; i <= 3; i++) snap = await w.step(`大图里按 → 第 ${i} 次`, p => p.key('ArrowRight'));
      const seen = snap.viewer;
      snap = await w.step('Esc 关大图', p => p.key('Escape'), {shot: true});
      w.check('大图关了', !snap.overlays.includes('viewer'), snap.overlays);
      w.check('详情栏跟到大图里最后看的那张', snap.focused === seen, {focused: snap.focused, viewer: seen});
      w.check('光标和详情栏是同一张', snap.cursor === snap.focused, {cursor: snap.cursor, focused: snap.focused});
      w.check('网格上只有一张画着「当前」', same(snap.marked.focused, snap.marked.cursor) && snap.marked.cursor.length <= 1, snap.marked);
      snap = await w.step('再按 Space', p => p.key('Space'));
      w.check('Space 打开的就是详情栏那张', snap.viewer === seen, {viewer: snap.viewer, expected: seen});
      await w.step('Esc 关大图', p => p.key('Escape'));
      snap = await w.step('Esc 关详情栏', p => p.key('Escape'));
      w.check('详情栏关了', !snap.overlays.includes('inspector'), snap.overlays);
    },
  },

  {
    name: 'viewer-info',
    title: '大图里翻几张后按 I 换成详情卡片 → 详情里翻页：详情是大图里最后看的那张，翻页时卡片不重建（#709）',
    covers: ['#424 2-7', '#709'],
    async run(w, page) {
      await w.step('打开图库', () => goto(page, '#/library'));
      await w.step('点第 3 张（开详情卡片）', p => p.click(cardAt(2)));
      await w.step('Space 进大图', p => p.key('Space'));
      let snap;
      for (let i = 1; i <= 2; i++) snap = await w.step(`大图里按 → 第 ${i} 次`, p => p.key('ArrowRight'));
      const seen = snap.viewer;
      snap = await w.step('按 I 换成详情', p => p.key('i'), {shot: true});
      w.check('大图关了', !snap.overlays.includes('viewer'), snap.overlays);
      w.check('详情卡片开着，是大图里最后看的那张', snap.overlays.includes('inspector') && snap.focused === seen, {focused: snap.focused, viewer: seen});
      // 翻页只换变了的部分：卡片和图片节点还是原来那些（#709 完成标准）。
      await page.eval(`window.__card = document.querySelector('#inspector .dc'); window.__img = document.getElementById('detailImg')`);
      snap = await w.step('详情里按 ←', p => p.key('ArrowLeft'));
      w.check('翻到上一张，光标跟着', snap.focused !== seen && snap.cursor === snap.focused, {focused: snap.focused, cursor: snap.cursor});
      const kept = await page.eval(`document.querySelector('#inspector .dc') === window.__card && document.getElementById('detailImg') === window.__img`);
      w.check('翻页没有重建卡片', kept, kept);
      snap = await w.step('Esc 关详情', p => p.key('Escape'));
      w.check('详情卡片关了', !snap.overlays.includes('inspector'), snap.overlays);
    },
  },

  {
    name: 'panel-peek',
    title: '按组整理里长按一张看大图 → 翻一张 → Esc：大图里只翻这一组、没有「信息」，关掉后分组和详情都不动（#424 的 2-2）',
    covers: ['#424 2-2', '#709'],
    async run(w, page) {
      await w.step('打开未识别 › 按组整理', () => goto(page, '#/groups'));
      const before = await inPage(page, `return document.querySelectorAll('#groupsPage .tidy-group[data-cid]').length;`);
      const at = await page.center('#groupsPage .tidy-group [data-peek]');
      w.check('组里有能看大图的图', !!at, at);
      if (!at) return;
      let snap = await w.step('长按第一张', async p => {
        await p.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: at.x, y: at.y});
        await p.send('Input.dispatchMouseEvent', {type: 'mousePressed', x: at.x, y: at.y, button: 'left', clickCount: 1});
        await sleep(600);
        await p.send('Input.dispatchMouseEvent', {type: 'mouseReleased', x: at.x, y: at.y, button: 'left', clickCount: 1});
      }, {shot: true});
      w.check('大图开了', snap.overlays.includes('viewer'), snap.overlays);
      const info = await page.eval(`!!document.querySelector('#viewerBar [data-act="info"]')`);
      w.check('借来的列表开的大图没有「信息」', !info, info);
      const first = snap.viewer;
      snap = await w.step('大图里按 →', p => p.key('ArrowRight'));
      w.check('翻到这一组的下一张', snap.viewer && snap.viewer !== first, {first, now: snap.viewer});
      snap = await w.step('Esc 关大图', p => p.key('Escape'));
      w.check('大图关了', !snap.overlays.includes('viewer'), snap.overlays);
      w.check('没有开详情卡片', !snap.overlays.includes('inspector'), snap.overlays);
      const after = await inPage(page, `return document.querySelectorAll('#groupsPage .tidy-group[data-cid]').length;`);
      w.check('分组原样', after === before, {before, after});
    },
  },

  {
    name: 'swap-key',
    title: '同一张已有角色的图，在详情栏、大图、角色页里按 C：都是「换成」，标的键和按钮做同一件事',
    covers: ['#386 1-3', 'SHORTCUTS 的 C'],
    async run(w, page) {
      const f = w.config.fixture, [a, b] = f.chars, target = a.images[0];
      // 每个场景按 C、选另一个角色、回车，再看这张图上的角色；然后撤销，下一个场景从同一状态开始。
      const scenes = [
        ['图库的详情栏', '#/library', async p => { await openCard(w, p, target); }],
        ['图库的大图', '#/library', async p => { await openCard(w, p, target); await p.key('Space'); }],
        // 固定时间排序，避免异步学习改变「可疑优先」顺序；本流程核对 C 的操作语义。
        ['角色页的光标', `#/library?char=${encodeURIComponent(f.charIds[a.name])}&sort=newest`, async p => { await openCard(w, p, target); await p.key('Escape'); }],
      ];
      for (const [where, hash, open] of scenes) {
        await w.step(`${where}：打开`, async p => { await goto(p, hash); await open(p); });
        let snap = await w.step(`${where}：按 C`, p => p.key('c'));
        w.check(`${where}：C 打开了命令面板`, snap.dialogs.includes('picker'), snap.dialogs);
        snap = await w.step(`${where}：搜「${b.name}」`, p => pickerFind(p, b.name), {shot: true});
        await w.step(`${where}：回车，等操作反馈结束`, async p => { await p.key('Enter'); await feedbackEnd(p); });
        await page.settle({quiet: 400});
        const roles = await rolesOf(w, page, target);
        w.check(`${where}：C 是换成（${a.name} 换成 ${b.name}）`, same(roles, [b.name]), {roles});
        snap = await w.step(`${where}：⌘Z 撤销，等操作反馈结束`, async p => {
          await p.key('Escape'); await p.key('Escape'); await p.key('z', {modifiers: ['Meta']}); await feedbackEnd(p);
        });
        await page.settle({quiet: 400});
        w.check(`${where}：撤销后回到 ${a.name}`, same(await rolesOf(w, page, target), [a.name]), await rolesOf(w, page, target));
      }
      // 详情栏上标着 C 的按钮做的也是同一件事。
      // 详情卡片角色身份块里的「换一个… C」（#709；原来是底部的「更正角色 C」，#748）。等卡片读完这张再点，和人看到内容再点一样。
      await w.step('点详情卡片的「换一个… C」', async p => { await goto(p, '#/library'); await openCard(w, p, target); await p.click('#inspector .dc[aria-busy="false"] [data-act="swap"]'); });
      await w.step(`搜「${b.name}」并回车`, async p => { await pickerFind(p, b.name); await p.key('Enter'); });
      await page.settle({quiet: 400});
      w.check('按钮也是换成', same(await rolesOf(w, page, target), [b.name]), await rolesOf(w, page, target));
    },
  },

  {
    name: 'select-assign-undo',
    title: '未识别里多选 → 大图里给一张设角色 → 关大图 → 撤销 → 回到网格：选择、焦点、详情栏、列表都回到原样',
    covers: ['#386 2-6 类', 'WEB_UI_MAP §6 大图规则'],
    async run(w, page) {
      const f = w.config.fixture, c = f.chars[1];
      let snap = await w.step('打开全部未识别', () => goto(page, '#/unmatched'));
      const total0 = snap.total, first = snap.items.slice(0, 3);
      await w.step('点第 1 张', p => p.click(cardAt(0)));
      snap = await w.step('⇧ 点第 3 张（连选 3 张）', p => p.click(cardAt(2), {modifiers: ['Shift']}), {shot: true});
      w.check('选中了 3 张', snap.selected.length === 3, snap.selected);
      snap = await w.step('Space 开大图', p => p.key('Space'));
      const shown = snap.viewer;
      w.check('大图开的是光标那张', shown === snap.cursor, {viewer: shown, cursor: snap.cursor});
      snap = await w.step('大图里按 C', p => p.key('c'));
      w.check('C 只作用于大图那张（面板说的是这一张）', snap.dialogs.includes('picker'), snap.picker);
      await w.step(`搜「${c.name}」`, p => pickerFind(p, c.name), {shot: true});
      snap = await w.step('回车设为', p => p.key('Enter'));
      await page.settle({quiet: 500});
      snap = await w.step('（等列表重读）', null);
      w.check('命令面板关了、大图还开着', !snap.dialogs.length && snap.overlays.includes('viewer'), {dialogs: snap.dialogs, overlays: snap.overlays});
      w.check('在大图里改了一张之后选择清空', snap.selected.length === 0, snap.selected);
      w.check('设好的那张有了角色', same(await rolesOf(w, page, shown), [c.name]), await rolesOf(w, page, shown));
      snap = await w.step('Esc 关大图', p => p.key('Escape'), {shot: true});
      w.check('焦点不在被藏起来的大图里', !/#viewer/.test(snap.active), snap.active);
      w.check('列表少了一张', snap.total === total0 - 1, {before: total0, after: snap.total});
      snap = await w.step('⌘Z 撤销', p => p.key('z', {modifiers: ['Meta']}), {shot: true});
      await page.settle({quiet: 600});
      snap = await w.step('（等撤销后的重读）', null);
      w.check('撤销后那张没有角色了', same(await rolesOf(w, page, shown), []), await rolesOf(w, page, shown));
      w.check('撤销后张数回到原来', snap.total === total0, {before: total0, after: snap.total});
      w.check('撤销后列表开头和原来一样', same(snap.items.slice(0, 3), first), {before: first, after: snap.items.slice(0, 3)});
      w.check('光标在列表里', snap.cursor === null || snap.items.includes(snap.cursor) || snap.total > 0, snap.cursor);
      snap = await w.step('再按 → 移动光标', p => p.key('ArrowRight'));
      w.check('撤销后方向键还能用', snap.cursor !== null, snap.cursor);
    },
  },

  {
    name: 'group-undo',
    title: '未识别 › 按组整理里把一整组设为角色 → 撤销：那一组回来，活页条、页签上的张数回到原样（等后台学习收尾）',
    covers: ['#386 1-2'],
    async run(w, page) {
      const f = w.config.fixture, who = f.chars[2].name, group = f.groups[0];
      const counts = () => inPage(page, `const S = (await import('/core/state.js')).state;
        return {unmatched: S.pools?.unmatched ?? null, sections: document.querySelectorAll('#groupsPage .tidy-group[data-cid]').length,
          first: document.querySelector('#groupsPage .tidy-group[data-cid]')?.dataset.cid ?? null};`);
      await w.step('打开未识别 › 按组整理', () => goto(page, '#/groups'), {shot: true});
      const before = await counts();
      w.check('有分组', before.sections > 0, before);
      await w.step(`第 ${group.id} 组点「设为…」`, p => p.click(`#groupsPage .tidy-group[data-cid="${group.id}"] [data-ref="assign"]`));
      await w.step(`搜「${who}」`, p => pickerFind(p, who));
      await w.step('回车：整组设为', p => p.key('Enter'), {settle: {quiet: 600}});
      const after = await counts();
      w.check('那一组离开了列表', after.sections === before.sections - 1, {before, after});
      await w.step('⌘Z 撤销', p => p.key('z', {modifiers: ['Meta']}), {settle: {quiet: 600}});
      // 撤销之后后台还在学习，学完计数和分组才定下来（#400）：最多等 8 秒，期间不再操作，和人停下来看一样。
      let now = await counts();
      for (const end = Date.now() + 8000; Date.now() < end && (now.sections !== before.sections || now.unmatched !== before.unmatched);) {
        await sleep(400); now = await counts();
      }
      await w.step('（等撤销后的学习收尾）', null, {shot: true});
      w.check('撤销后那一组回到列表', now.sections === before.sections, {before, now});
      w.check('撤销后未识别张数回到原样', now.unmatched === before.unmatched, {before: before.unmatched, now: now.unmatched});
      // 活页条整理的二级层里「未识别」那一项的数字（#706）。
      const side = await page.eval(`(document.querySelector('#railItems .rail-item[data-key="unmatched"] .rail-count')?.textContent || '').trim()`);
      w.check('活页条的未识别数字和页面一致', Number(side.replace(/\D/g, '') || 0) === now.unmatched, {side, unmatched: now.unmatched});
    },
  },

  {
    name: 'group-other',
    title: '未识别 › 按组整理：点掉组里的一张 →「点掉的是…」设为另一个角色：这组留着、少了那张 → ⌘Z 撤销：那张回到组里（#787）',
    covers: ['#787'],
    async run(w, page) {
      const f = w.config.fixture, who = f.chars[1].name, group = f.groups[0];
      const sec = `#groupsPage .tidy-group[data-cid="${group.id}"]`;
      const members = () => page.eval(`[...document.querySelectorAll(${JSON.stringify(sec + ' .judge-tile[data-peek]:not([inert])')})].map(t => t.dataset.peek)`);
      await w.step('打开未识别 › 按组整理', () => goto(page, '#/groups'));
      const before = await members();
      w.check(`第 ${group.id} 组有图`, before.length > 1, before);
      const offId = before[0], was = await assertionsOf(page, offId);
      await w.step('点掉这组的第一张', p => p.click(`${sec} .judge-tile[data-peek="${offId}"]`), {shot: true});
      w.check('有了「点掉的是…」', await page.eval(`!document.querySelector(${JSON.stringify(sec + ' [data-ref="other"]')}).hidden`), '');
      await w.step('点「点掉的是…」', p => p.click(`${sec} [data-ref="other"]`));
      w.check('角色选择器打开了', await page.waitFor(`document.getElementById('picker').open`, 3000), '');
      await w.step(`选 ${who}，回车`, async p => { await pickerFind(p, who); await p.key('Enter'); await feedbackEnd(p); }, {shot: true});
      const after = await assertionsOf(page, offId);
      w.check(`点掉的那张记成 ${who}（人工确认）`, after.some(a => a.name === who && a.status === 'confirmed'), {after});
      w.check('这组还在，少了那一张（其余按 UUID 比）', same(await members(), before.slice(1)), {before});
      await w.step('撤销', async p => { await p.click('#undoButton'); await feedbackEnd(p); });
      let now = await members();
      for (const end = Date.now() + 8000; Date.now() < end && !now.includes(offId);) { await sleep(400); now = await members(); }
      w.check('撤销后那张回到组里', now.includes(offId), {now});
      w.check('撤销后那张的人工结论还原', same(await assertionsOf(page, offId), was), {was});
    },
  },

  {
    name: 'doubts',
    title: '整理 › 可能认错：是 A、是 B、都不是、「点掉的是…」各答一次再撤销；减少动态效果时卡的收起动画是 0；「以后再说」之后这批不再出现（#787）',
    covers: ['#787'],
    async run(w, page) {
      // 合成图库里有一对分不清的角色（WD 把 dahlia 的几张图同样认成了 ember，tools/ui_walk/fixture.py 的 ALSO），是一张卡。每种回答
      // 答完都撤销，卡回来再答下一种；「以后再说」不进撤销，放在最后。
      const first = '#groupsPage .doubt-card[data-key]:not([inert])';
      const cards = () => page.eval(`[...document.querySelectorAll(${JSON.stringify(first)})].map(c => c.dataset.key)`);
      const ids = () => page.eval(`(() => { const c = document.querySelector(${JSON.stringify(first)});
        return c ? [...c.querySelectorAll('.judge-tile[data-peek]:not([inert])')].map(t => t.dataset.peek) : []; })()`);
      const pool = () => stateOf(page, 'S.pools?.ambiguous ?? null');
      const rail = () => page.eval(`Number((document.querySelector('#railItems .rail-item[data-key="doubts"] .rail-count')?.textContent || '').replace(/\\D/g, '') || 0)`);
      const settled = async (want, test) => { let now = await want(); for (const end = Date.now() + 8000; Date.now() < end && !test(now);) { await sleep(400); now = await want(); } return now; };
      // 每张图上某个角色的人工结论（confirmed / rejected / ''），按图的次序。
      const statusOf = async (pics, id) => (await Promise.all(pics.map(u => assertionsOf(page, u)))).map(list => (list || []).find(a => a.id === id)?.status || '');
      const all = (list, status) => list.length > 0 && list.every(x => x === status);
      // 点一个按钮，记下卡片收起时在它自己身上起的动画请求的时长，等卡片离开页面再读（不靠计时：减少动态效果时应是 0，平时是
      // --dur-1）。时长为 0 的动画在两帧之间就播完了，逐帧读 getAnimations() 看不到，所以在这张卡上接住 animate() 的调用。
      const collapseMs = async (p, act) => {
        await p.eval(`(() => { const card = document.querySelector(${JSON.stringify(first)}), seen = []; window.__collapse = null;
          card.animate = (...args) => { seen.push(Number(args[1]?.duration) || 0); return Element.prototype.animate.apply(card, args); };
          const watch = () => { if (card.isConnected) setTimeout(watch, 10); else window.__collapse = seen; };
          watch(); })()`);
        await p.click(`${first} [data-ref="${act}"]`);
        await p.waitFor(`window.__collapse !== null`, 5000);
        return p.eval(`window.__collapse.length ? Math.max(...window.__collapse) : -1`);
      };
      let snap = await w.step('打开整理 › 可能认错', async p => { await p.eval(`location.hash = '#/doubts'`); await p.settle({quiet: 600}); }, {shot: true});
      w.check('页头的整理页签里选中「可能认错」', await page.eval(`[...document.querySelectorAll('#heading .seg a')].some(a => a.getAttribute('href') === '#/doubts' && a.getAttribute('aria-current') === 'page')`), snap.heading);
      const start = await cards(), pics = await ids(), total = await pool();
      w.check('合成图库有一张可能认错的卡', start.length === 1 && pics.length > 1, {start, pics});
      if (!start.length) return;
      w.check('活页条「可能认错」的数字等于卡上的张数', await rail() === pics.length && total === pics.length, {rail: await rail(), total, pics: pics.length});
      const key = start[0], was = await Promise.all(pics.map(u => assertionsOf(page, u)));
      // 问句里的两个名字链到角色页（#/library?char=…）：A 在前、B 在后。
      const [A, B] = await page.eval(`[...document.querySelector(${JSON.stringify(first)}).querySelectorAll('.tidy-q a')].map(a => ({id: new URLSearchParams((a.getAttribute('href') || '').split('?')[1] || '').get('char'), name: a.textContent}))`);
      w.check('卡上的两个角色读得到', !!A?.id && !!B?.id, {A, B});
      const expect = {
        a: async () => all(await statusOf(pics, A.id), 'confirmed'),
        b: async () => all(await statusOf(pics, A.id), 'rejected') && all(await statusOf(pics, B.id), 'confirmed'),
        none: async () => all(await statusOf(pics, A.id), 'rejected') && all(await statusOf(pics, B.id), 'rejected'),
      };
      const said = {a: `每张 ${A.name} 都是 confirmed`, b: `每张 ${A.name} 是 rejected、${B.name} 是 confirmed`, none: `每张 ${A.name}、${B.name} 都是 rejected`};
      for (const [act, label] of [['a', '是 A'], ['b', '是 B'], ['none', '都不是']]) {
        await w.step(`点「${label}」`, async p => { await p.click(`${first} [data-ref="${act}"]`); await feedbackEnd(p); }, {shot: act === 'a'});
        w.check(`「${label}」以后卡收起`, !(await cards()).includes(key), {key});
        w.check(`「${label}」以后：${said[act]}`, await expect[act](), {A: await statusOf(pics, A.id), B: await statusOf(pics, B.id)});
        w.check(`「${label}」以后活页条的数字是 0`, await settled(rail, n => n === 0) === 0, {rail: await rail()});
        await w.step('撤销', async p => { await p.click('#undoButton'); await feedbackEnd(p); });
        w.check(`撤销「${label}」以后卡回来`, (await settled(cards, c => c.includes(key))).includes(key), {key});
        w.check(`撤销「${label}」以后人工结论还原（逐张逐项相同）`, same(await Promise.all(pics.map(u => assertionsOf(page, u))), was), {pics});
      }
      // 「点掉的是…」：改成第三个角色（不是 A 也不是 B）
      const offId = pics[0], third = w.config.fixture.chars.find(c => c.name !== A.name && c.name !== B.name);
      await w.step('点掉第一张', p => p.click(`${first} .judge-tile[data-peek="${offId}"]`));
      w.check('主按钮写「其余 N 张是 A」', /\d/.test(await page.eval(`document.querySelector(${JSON.stringify(first + ' [data-ref="a"]')}).textContent`)), '');
      await w.step('点「点掉的是…」', p => p.click(`${first} [data-ref="other"]`));
      w.check('角色选择器打开了', await page.waitFor(`document.getElementById('picker').open`, 3000), '');
      await w.step(`选 ${third.name}，回车`, async p => { await pickerFind(p, third.name); await p.key('Enter'); await feedbackEnd(p); }, {shot: true});
      const after = await assertionsOf(page, offId);
      w.check(`点掉的那张：${A.name} 记成 rejected、${third.name} 记成 confirmed`,
        after.some(a => a.id === A.id && a.status === 'rejected') && after.some(a => a.name === third.name && a.status === 'confirmed'), {after});
      w.check('其余几张没动', same(await Promise.all(pics.slice(1).map(u => assertionsOf(page, u))), was.slice(1)), '');
      w.check('卡还在，少了那一张（其余按 UUID 比）', (await cards()).includes(key) && same(await ids(), pics.slice(1)), {pics});
      await w.step('撤销', async p => { await p.click('#undoButton'); await feedbackEnd(p); });
      w.check('撤销以后那张回到卡上，人工结论还原', same(await settled(ids, x => x.length === pics.length), pics) && same(await assertionsOf(page, offId), was[0]), {pics});
      // 收起动画：平时是 --dur-1（约 140ms，作对照），开了「减少动态效果」是 0（卡直接消失）
      let ms = -1;
      await w.step('点「是 A」，记下卡片收起的动画时长（对照）', async p => { ms = await collapseMs(p, 'a'); await feedbackEnd(p); });
      w.check('平时卡片收起有动画（时长 > 0，--dur-1）', ms > 0, {ms});
      await w.step('撤销', async p => { await p.click('#undoButton'); await feedbackEnd(p); });
      w.check('撤销以后卡回来', (await settled(cards, c => c.includes(key))).includes(key), {key});
      await w.step('打开「减少动态效果」（模拟系统设置）', p => p.send('Emulation.setEmulatedMedia', {features: [{name: 'prefers-reduced-motion', value: 'reduce'}]}));
      await w.step('点「是 A」，记下卡片收起的动画时长', async p => { ms = await collapseMs(p, 'a'); await feedbackEnd(p); });
      w.check('开了「减少动态效果」：卡片收起的动画时长是 0（直接消失）', ms === 0, {ms});
      await w.step('撤销', async p => { await p.click('#undoButton'); await feedbackEnd(p); });
      await w.step('关掉「减少动态效果」', p => p.send('Emulation.setEmulatedMedia', {features: [{name: 'prefers-reduced-motion', value: ''}]}));
      w.check('撤销以后卡回来', (await settled(cards, c => c.includes(key))).includes(key), {key});
      // 「以后再说」：这批不再出现（重新进这一页也不在），也不进撤销
      await w.step('点「以后再说」', async p => { await p.click(`${first} [data-ref="later"]`); await p.settle({quiet: 800}); });
      w.check('「以后再说」以后卡收起', !(await cards()).includes(key), {key});
      await w.step('去按组整理再回来', async p => { await p.eval(`location.hash = '#/groups'`); await p.settle({quiet: 600}); await p.eval(`location.hash = '#/doubts'`); await p.settle({quiet: 800}); }, {shot: true});
      w.check('重新进来，「以后再说」的那批不在，写着空状态', !(await cards()).length && await page.eval(`!!document.querySelector('#groupsPage .empty-state')`), '');
      w.check('页头页签和活页条上都不再写可能认错的张数', await page.eval(`!/\\d/.test([...document.querySelectorAll('#heading .seg a')].find(a => a.getAttribute('href') === '#/doubts')?.textContent || '')`) && await rail() === 0, '');
      w.check('「以后再说」没有改人工结论', same(await Promise.all(pics.map(u => assertionsOf(page, u))), was), {pics});
    },
  },

  {
    name: 'similar-seen',
    title: '重复 › 相似的图：进一组 → 页头点「看过了」→ 回到列表，这组不在了 → ⌘Z 撤销：这组回到列表（#427、#713）',
    covers: ['#427', '#713'],
    async run(w, page) {
      const gid = String(w.config.fixture.similar[0]?.id ?? '');
      w.check('合成图库里有相似组', !!gid, gid);
      if (!gid) return;
      const listed = () => inPage(page, `return [...document.querySelectorAll('#groupsPage .tidy-card')].map(a => a.getAttribute('href'));`);
      await w.step('打开重复 › 相似的图', () => goto(page, '#/similar'), {shot: true});
      const before = await listed();
      w.check('列表里有这一组', before.includes('#/s/' + gid), before);
      await w.step(`点进相似组 ${gid}`, p => p.click(`#groupsPage .tidy-card[href="#/s/${gid}"]`), {settle: {quiet: 600}});
      const button = await page.eval(`(document.getElementById('sgroupSeen')?.textContent || '').trim()`);
      w.check('页头有「看过了」', button === '看过了' || button === 'Mark as seen', button);
      await w.step('点「看过了」', p => p.click('#sgroupSeen'), {settle: {quiet: 600}, shot: true});
      const hash = await page.eval('location.hash');
      w.check('回到相似的图', hash.startsWith('#/similar'), hash);
      const after = await listed();
      w.check('这一组不在列表里了', !after.includes('#/s/' + gid), after);
      await w.step('⌘Z 撤销', p => p.key('z', {modifiers: ['Meta']}), {settle: {quiet: 600}});
      let now = await listed();
      for (const end = Date.now() + 5000; Date.now() < end && !now.includes('#/s/' + gid);) { await sleep(300); now = await listed(); }
      w.check('撤销后这一组回到列表', now.includes('#/s/' + gid), now);
    },
  },

  {
    name: 'esc-stack',
    title: '浮层叠浮层时 Esc 只关最上面那个：详情栏 → 大图 → 命令面板；视图选项 → 命令面板；任务面板 → 命令面板；点外面只关外面的',
    covers: ['WEB_UI_MAP §3.3'],
    async run(w, page) {
      await w.step('打开图库', () => goto(page, '#/library'));
      await w.step('点一张开详情栏', p => p.click(cardAt(2)));
      await w.step('Space 进大图', p => p.key('Space'));
      let snap = await w.step('大图里按 C 开命令面板', p => p.key('c'), {shot: true});
      w.check('三层都在', snap.dialogs.includes('picker') && snap.overlays.includes('viewer') && snap.overlays.includes('inspector'), snap);
      snap = await w.step('Esc', p => p.key('Escape'));
      w.check('第 1 下只关命令面板', !snap.dialogs.length && snap.overlays.includes('viewer'), snap);
      snap = await w.step('Esc', p => p.key('Escape'));
      w.check('第 2 下只关大图', !snap.overlays.includes('viewer') && snap.overlays.includes('inspector'), snap.overlays);
      snap = await w.step('Esc', p => p.key('Escape'));
      w.check('第 3 下关详情栏', !snap.overlays.includes('inspector'), snap.overlays);

      snap = await w.step('点页头的视图选项', p => p.click('#viewButton'));
      w.check('视图选项打开', snap.overlays.includes('viewPopover'), snap.overlays);
      snap = await w.step('⌘K 开跳到角色', p => p.key('k', {modifiers: ['Meta']}));
      w.check('命令面板叠在视图选项上', snap.dialogs.includes('picker'), snap.dialogs);
      snap = await w.step('Esc', p => p.key('Escape'));
      w.check('只关命令面板，视图选项还在', !snap.dialogs.length && snap.overlays.includes('viewPopover'), snap);
      snap = await w.step('Esc', p => p.key('Escape'));
      w.check('再关视图选项', !snap.overlays.includes('viewPopover'), snap.overlays);
      w.check('焦点回到视图选项按钮', /#viewButton/.test(snap.active), snap.active);

      snap = await w.step('点活页条的同步圆环（任务）', p => p.click('#taskButton'));
      w.check('任务面板打开', snap.overlays.includes('taskPanel'), snap.overlays);
      snap = await w.step('⌘K 开跳到角色', p => p.key('k', {modifiers: ['Meta']}), {shot: true});
      w.check('命令面板叠在任务面板上', snap.dialogs.includes('picker') && snap.overlays.includes('taskPanel'), snap);
      snap = await w.step('Esc', p => p.key('Escape'));
      w.check('只关命令面板，任务面板还在', !snap.dialogs.length && snap.overlays.includes('taskPanel'), snap);
      snap = await w.step('Esc', p => p.key('Escape'));
      w.check('再关任务面板', !snap.overlays.includes('taskPanel'), snap.overlays);
      snap = await w.step('点视图选项，再点活页条的同步圆环（任务）', async p => { await p.click('#viewButton'); await p.click('#taskButton'); });
      w.check('点在视图选项外面：它关掉，任务面板打开', !snap.overlays.includes('viewPopover') && snap.overlays.includes('taskPanel'), snap.overlays);
      snap = await w.step('Esc', p => p.key('Escape'));
      w.check('Esc 关任务面板', !snap.overlays.includes('taskPanel'), snap.overlays);
    },
  },

  {
    name: 'search-esc',
    title: '图库搜索后按 Esc：搜索框和筛选一起清掉，再按一次离开输入框',
    covers: ['#386 1-6'],
    async run(w, page) {
      const f = w.config.fixture, name = f.chars[0].name;
      let snap = await w.step('打开图库', () => goto(page, '#/library'));
      const all = snap.total;
      await w.step('按 / 聚焦搜索框', p => p.key('/'));
      snap = await w.step(`输入「${name}」`, async p => { await p.type(name); await sleep(800); }, {shot: true});
      w.check('搜索后结果变少', snap.total > 0 && snap.total < all, {all, now: snap.total});
      snap = await w.step('Esc', p => p.key('Escape'), {shot: true, settle: {quiet: 500}});
      w.check('搜索框清空了', snap.search === '', snap.search);
      w.check('搜索框空了，筛选也跟着清掉', snap.search !== '' || snap.total === all, {search: snap.search, total: snap.total, all});
      snap = await w.step('再按 Esc', p => p.key('Escape'));
      w.check('离开了输入框', !/#search/.test(snap.active), snap.active);
    },
  },

  {
    name: 'place-roundtrip',
    title: '图库筛选、往下滚、开详情栏 → 去设置 → 后退：筛选、滚动位置、详情栏都还在；换到别的页再点回来同样',
    covers: ['#386 1-8'],
    async run(w, page) {
      let snap = await w.step('打开图库', () => goto(page, '#/library'));
      await w.step('NSFW 只看全年龄（页头按钮，等提示消失）', async p => {
        await p.key('h');
        if (!await p.waitFor(`document.getElementById('toast').hidden`, 6000)) throw new Error('筛选提示没有消失');
      });
      snap = await w.step('往下滚', async p => { for (let i = 0; i < 4; i++) { await p.scroll('#viewport', 600); await sleep(120); } await sleep(300); });
      const scrolled = snap.scroll;
      w.check('滚下去了', scrolled > 300, scrolled);
      const visible = await page.eval(`(() => { const v = document.getElementById('viewport').getBoundingClientRect(); return [...document.querySelectorAll('#grid .card[data-id]')].map(c => [c, c.getBoundingClientRect()]).filter(([, r]) => r.top > v.top + 40 && r.bottom < v.bottom - 40).map(([c]) => c.dataset.index)[0]; })()`);
      snap = await w.step('点一张看得见的图', p => p.click(cardAt(visible)), {shot: true});
      const before = {focused: snap.focused, scroll: snap.scroll, total: snap.total, hash: snap.hash};
      snap = await w.step('点活页条的设置', p => p.click('#settingsLink'));
      w.check('到了设置页', snap.hash.startsWith('#/settings'), snap.hash);
      snap = await w.step('浏览器后退', async p => { await p.eval('history.back()'); await gridReady(p); await sleep(400); }, {shot: true});
      w.check('后退回到图库，地址一样', snap.hash === before.hash, {before: before.hash, now: snap.hash});
      w.check('张数一样（NSFW 筛选还在）', snap.total === before.total, {before: before.total, now: snap.total});
      w.check('滚动位置回来了', Math.abs(snap.scroll - before.scroll) < 60, {before: before.scroll, now: snap.scroll});
      w.check('详情栏还是那张', snap.focused === before.focused, {before: before.focused, now: snap.focused});
      await w.step('点活页条的资料库', p => goto(p, '#/characters'));
      snap = await w.step('再点活页条的图库', p => goto(p, '#/library'));
      w.check('从活页条回来也回到原位置', Math.abs(snap.scroll - before.scroll) < 60, {before: before.scroll, now: snap.scroll});
    },
  },

  {
    name: 'card-chips',
    title: '卡片上的芯片有样式：随机抽查的角色标签（✓ / ✗ 悬停才出现，正在抽查的着色）、未识别图上的「+ 名字」（#708 审核第 1 条）',
    async run(w, page) {
      // 走查图库里没有抽查和「可能是」的数据：在网格里放一张和 ui/grid_card.js 同样结构的样例卡，读真实浏览器算出来的样式，
      // 查的是 css/ 里这些类名有规则（审核第 1 条：样式还挂在旧的 .chips 下，新卡片上全丢了）。
      await w.step('打开网格', () => goto(page, '#/library'));
      const looks = await page.eval(`(() => {
        const card = document.createElement('div'); card.className = 'card'; card.style.cssText = 'transform:translate(0,0);width:240px;height:240px';
        card.innerHTML = '<div class="card-stage"><span class="card-chips"><span class="atag checked open"><b>A</b><button type="button" data-tagact="yes">✓</button></span>'
          + '<span class="atag checked no"><b>B</b></span><button class="hint-add" type="button">+ C</button></span></div>';
        document.getElementById('grid').append(card);
        const s = sel => { const st = getComputedStyle(card.querySelector(sel)); return {display: st.display, bg: st.backgroundColor, radius: st.borderTopLeftRadius,
          border: st.borderTopStyle, deco: st.textDecorationLine}; };
        const out = {open: s('.atag.open'), button: s('.atag.open button'), no: s('.atag.no b'), hint: s('.hint-add')};
        card.remove();
        return out;
      })()`);
      const filled = x => x.bg !== 'rgba(0, 0, 0, 0)' && x.radius !== '0px';
      w.check('在抽查的标签有底色和圆角', filled(looks.open), looks.open);
      w.check('标签上的 ✓ / ✗ 不悬停时藏着', looks.button.display === 'none', looks.button);
      w.check('答了「不是」的标签划掉名字', looks.no.deco.includes('line-through'), looks.no);
      w.check('「+ 名字」有自己的底色、没有默认边框', filled(looks.hint) && looks.hint.border === 'none', looks.hint);
    },
  },

  {
    name: 'calendar',
    title: '图库的日历：每天的格子按张数拼图，翻月，点一天去按天视图的那一天；三种视图之间切换，搜索词跟着走（#708）',
    async run(w, page) {
      let snap = await w.step('打开日历', () => goto(page, '#/calendar'), {shot: true});
      const month = () => page.eval(`document.querySelector('#calendarPage .cal-title > :first-child')?.textContent || ''`);
      const days = () => page.eval(`[...document.querySelectorAll('#calendarPage .cal-day[href]')].map(a => ({href: a.getAttribute('href'),
        tiles: [...a.querySelectorAll('img')].filter(i => !i.hidden).length, n: Number((a.querySelector('.cal-count')?.textContent || '0').replace(/\\D/g, ''))}))`);
      w.check('月份标题不空', !!await month(), await month());
      // 这个月没有图时往前翻到有图的月份（‹ 去筛选后最近有图的月份）
      for (let i = 0; i < 3 && !(await days()).length; i++) {
        if (await page.eval(`document.querySelector('#calendarPage .cal-nav > button')?.disabled`)) break;
        snap = await w.step('上一个有图的月份', p => p.click('#calendarPage .cal-nav > button'), {settle: {quiet: 400}});
      }
      const cells = await days();
      w.check('有图的日子是链接', cells.length > 0, cells.length);
      w.check('格子按张数拼 1、2 或 4 张', cells.every(c => c.tiles === (c.n >= 4 ? 4 : c.n >= 2 ? 2 : 1)), cells.slice(0, 6));
      const shown = await month(), day = cells[0]?.href || '';
      snap = await w.step('点有图的第一天', p => p.click(`#calendarPage .cal-day[href="${day}"]`), {shot: true, settle: {quiet: 500}});
      w.check('到了按天视图的那一天', snap.hash.startsWith('#/days') && /[?&]at=\d{4}-\d{2}-\d{2}/.test(snap.hash), snap.hash);
      snap = await w.step('浏览器后退', async p => { await p.eval('history.back()'); await gridReady(p); await sleep(400); });
      w.check('回到日历的同一个月', snap.hash.startsWith('#/calendar') && await month() === shown, {hash: snap.hash, month: await month(), shown});
      // 视图切换：在网格里搜一个角色，切到日历和按天，搜索词都在
      const name = w.config.fixture.chars[0].name;
      await w.step('打开网格', () => goto(page, '#/library'));
      await w.step('按 / 聚焦搜索框', p => p.key('/'));
      await w.step(`输入「${name}」`, async p => { await p.type(name); await sleep(800); });
      snap = await w.step('页头的「日历」', async p => { await p.click('#heading .seg a[href^="#/calendar"]'); await gridReady(p); }, {shot: true});
      const q = hash => new URLSearchParams(hash.split('?')[1] || '').get('q');
      w.check('切到日历，搜索词还在', snap.hash.startsWith('#/calendar') && q(snap.hash) === name, snap.hash);
      snap = await w.step('页头的「网格」', async p => { await p.click('#heading .seg a[href^="#/library"]'); await gridReady(p); });
      w.check('切回网格，搜索词还在、网格有图', q(snap.hash) === name && snap.total > 0, {hash: snap.hash, total: snap.total});
    },
  },

  {
    name: 'settings-fit',
    title: '设置 › 通用里切「原比例」→ 回图库：网格换成原比例，没有报错',
    covers: ['#386 1-9'],
    async run(w, page) {
      await w.step('打开设置', () => goto(page, '#/settings'));
      let snap = await w.step('点「原比例」', p => p.click('[data-row="fit"] button[data-value="1"]'), {shot: true});
      const fit = await stateOf(page, 'S.fit');
      w.check('切换生效', fit === true, {fit});
      w.check('点的时候没有报错', !w.steps.at(-1).console.length, w.steps.at(-1).console);
      snap = await w.step('回图库', () => goto(page, '#/library'));
      // 原比例里卡片按图的宽高比排，不全是方的（卡片下面没有标题条，#708）。
      const cards = await page.eval(`[...document.querySelectorAll('#grid .card[data-id]')].filter(c => Math.abs(c.offsetWidth - c.offsetHeight) > 2).length`);
      w.check('网格是原比例', cards > 0, {cards});
      await w.step('按 R 切回方格', p => p.key('r'));
      w.check('R 切回方格', (await stateOf(page, 'S.fit')) === false, '');
    },
  },

  {
    name: 'galleries',
    title: '画廊：收藏在最前、两种画廊分得出；换展示方式记得住；瀑布按列排；旧的收藏地址能打开；把图从展示页和网格拖进手选画廊再撤销；「更多像这张的」页上没有「保存为画廊」',
    covers: ['#710', '#646'],
    async run(w, page) {
      const ids = Object.keys(w.config.names).slice(0, 12);
      await w.step('（准备）收藏 6 张，建一个手选画廊（放进 4 张）、一个空的手选画廊和一个按条件的画廊', async p => {
        await post(p, '/api/favorite', {ids: ids.slice(0, 6)});
        await post(p, '/api/galleries', {action: 'create', kind: 'manual', name: 'Summer', ids: ids.slice(6, 10)});
        await post(p, '/api/galleries', {action: 'create', kind: 'manual', name: 'Empty'});
        await post(p, '/api/galleries', {action: 'create', name: 'Faved', params: {has: '["fav"]'}});
        await p.send('Page.reload');   // 接口直接写的，页面不知道：重新载入，和用户换了个时候打开一样
        await p.waitFor(`document.getElementById('rail') && !document.getElementById('splash')`, 15000);
      });
      let snap = await w.step('点活页条的画廊', () => goto(page, '#/galleries'), {shot: true});
      const cards = () => page.eval(`[...document.querySelectorAll('.gcard')].map(c => c.dataset.kind || (c.classList.contains('gcard-new') ? 'new' : '?'))`);
      const kinds = await cards();
      w.check('收藏在最前，最后一张是「新建」', kinds[0] === 'fav' && kinds.at(-1) === 'new', kinds);
      w.check('手选画廊的卡片标成手选', kinds.filter(k => k === 'manual').length === 2, kinds);
      w.check('画廊列表有标题', !!snap.heading, snap.heading);

      snap = await w.step('打开旧的收藏地址 #/library?has=["fav"]', async p => {
        await p.eval(`location.hash = '#/library?has=' + encodeURIComponent('["fav"]')`); await gridReady(p);
      }, {shot: true});
      w.check('旧地址改写到收藏画廊', /^#\/(gallery|showcase)\/fav/.test(snap.hash), snap.hash);
      w.check('收藏画廊里是收藏的 6 张', snap.total === 6, snap.total);

      // 展示方式：点页头的分段控件；离开再回来还是它。
      const showBtn = id => `.gallery-shows [data-show="${id}"]`;
      for (const id of ['collage', 'magazine', 'hall', 'color', 'masonry']) {
        snap = await w.step(`展示方式换成 ${id}`, async p => { await p.click(showBtn(id)); await sleep(500); await gridReady(p); }, {shot: true});
        const pressed = await page.eval(`document.querySelector(${JSON.stringify(showBtn(id))})?.getAttribute('aria-pressed')`);
        w.check(`${id}：分段控件按下的是它`, pressed === 'true', pressed);
        const pieces = await page.eval(`document.querySelectorAll('.stage .piece').length`);
        if (['collage', 'magazine', 'hall'].includes(id)) w.check(`${id}：展示页放了图`, pieces > 0, pieces);
        else w.check(`${id}：网格有图`, snap.total === 6, snap.total);
        if (id === 'color') w.check('色彩：按颜色排', await page.eval(`document.getElementById('sort')?.value`) === 'color', '');
      }
      // 瀑布：等宽的几列、高随图的比例；「方格 / 原始比例」开关藏起来；↓ 走到同一列的下一张。
      const geo = await page.eval(`(() => { const cs = [...document.querySelectorAll('#grid .card')].map(c => [Math.round(parseFloat(c.style.width)), Math.round(parseFloat(c.style.height))]);
        return {widths: new Set(cs.map(c => c[0])).size, heights: new Set(cs.map(c => c[1])).size, fitHidden: document.getElementById('fitToggle')?.hidden}; })()`);
      w.check('瀑布：列宽一样、高随图，没有「方格 / 原始比例」开关', geo.widths === 1 && geo.heights > 1 && geo.fitHidden === true, geo);
      // 从下面还有图的那一列的第一张开始（收藏只有 6 张，有的列只有一张）：→ 走过去，再 ↓。
      const start = await inPage(page, `const R = await import('/ui/grid_rows.js'), S = (await import('/core/state.js')).state;
        for (let i = 0; i < S.total; i++) if (R.cardRect(i).y === R.cardRect(0).y && R.cardRect(R.verticalStep(i, 1)).x === R.cardRect(i).x && R.verticalStep(i, 1) !== i) return i;
        return -1;`);
      await w.step('↓ 到第一张，→ 走到下面还有图的那一列，再 ↓', async p => {
        await p.key('ArrowDown'); for (let k = 0; k < start; k++) await p.key('ArrowRight'); await p.key('ArrowDown');
      });
      const steps = await inPage(page, `const R = await import('/ui/grid_rows.js'), S = (await import('/core/state.js')).state;
        return {start: ${start}, cursor: S.cursor, x: [R.cardRect(${start}).x, R.cardRect(S.cursor).x], y: [R.cardRect(${start}).y, R.cardRect(S.cursor).y]};`);
      w.check('瀑布：↓ 走到同一列的下一张', start >= 0 && steps.x[0] === steps.x[1] && steps.y[1] > steps.y[0], steps);
      await w.step('Esc', p => p.key('Escape'));

      await w.step('展示方式换成拼贴，再去别处', async p => { await p.click(showBtn('collage')); await sleep(500); await goto(p, '#/days'); });
      snap = await w.step('从活页条回到收藏', async p => { await goto(p, '#/galleries'); await p.click('.gcard[data-kind="fav"]'); await sleep(600); await gridReady(p); });
      w.check('记得住：回来还是拼贴', snap.hash.startsWith('#/showcase/fav')
        && await page.eval(`document.querySelector(${JSON.stringify(showBtn('collage'))})?.getAttribute('aria-pressed') === 'true'`), snap.hash);

      // 拖进手选画廊：真鼠标按住一张图拖到活页条「Summer」上。活页条拿起之后才换成画廊封面，所以先拿起、再量目标。
      const count = name => inPage(page, `const g = (await (await fetch('/api/galleries')).json()).find(x => x.name === ${JSON.stringify(name)});
        const r = await fetch('/api/assets/count?has=' + encodeURIComponent(JSON.stringify(['gallery:' + g.id]))); return (await r.json()).total;`);
      const dragOnto = async (p, selector, label) => {
        const from = await p.center(selector);
        await p.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: from.x, y: from.y});
        await p.send('Input.dispatchMouseEvent', {type: 'mousePressed', x: from.x, y: from.y, button: 'left', clickCount: 1});
        for (let k = 1; k <= 4; k++) { await p.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: from.x - k * 6, y: from.y, button: 'left', buttons: 1}); await sleep(16); }
        await p.waitFor(`!!document.querySelector('.lift') && [...document.querySelectorAll('#railItems .rail-item')].some(x => x.offsetParent && x.dataset.key === 'new')`, 3000);
        await sleep(600);
        const to = await p.eval(`(() => { const el = [...document.querySelectorAll('#railItems .rail-item')].find(x => x.offsetParent && x.railItem?.label === ${JSON.stringify(label)});
          const r = el.getBoundingClientRect(); return {x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2)}; })()`);
        for (let k = 1; k <= 8; k++) {
          await p.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: Math.round(from.x + (to.x - from.x) * k / 8), y: Math.round(from.y + (to.y - from.y) * k / 8), button: 'left', buttons: 1});
          await sleep(16);
        }
        offs = await p.eval(`[...document.querySelectorAll('#railItems .rail-item.is-off')].map(x => x.offsetParent ? x.railItem?.label : null).filter(Boolean)`);
        await p.send('Input.dispatchMouseEvent', {type: 'mouseReleased', x: to.x, y: to.y, button: 'left', clickCount: 1});
        await sleep(900);
      };
      let offs = [];   // 上一次拖动时活页条上变淡（不接收）的项
      let before = await count('Summer');
      snap = await w.step('按住展示页的一张图拖到活页条上的 Summer', p => dragOnto(p, '.stage .piece:nth-child(3) img', 'Summer'), {shot: true});
      w.check('放进去了', await count('Summer') === before + 1, {before, after: await count('Summer')});
      const condName = await inPage(page, `return (await (await fetch('/api/galleries')).json()).find(x => !x.kind)?.name || '';`);
      w.check('拖动时按条件的画廊变淡、不接收', !!condName && offs.includes(condName), {condName, offs});
      w.check('有提示和撤销', !!(await page.eval(`!document.getElementById('toast').hidden && !document.getElementById('toastUndo').hidden`)), '');
      w.check('活页条换回画廊这一组', await page.eval(`![...document.querySelectorAll('#railItems .rail-item')].some(x => x.offsetParent && x.dataset.key === 'new')`), '');
      await w.step('⌘Z 撤销', p => p.key('z', {modifiers: ['Meta']}));
      await sleep(600);
      w.check('撤销以后拿出来了', await count('Summer') === before, {before, after: await count('Summer')});
      // 松手在按条件的画廊上：不写入，缩略图飞回去（它在活页条上是 pointer-events: none，指针下面落空）。
      const writes = await inPage(page, `return (await (await fetch('/api/galleries')).json()).map(g => g.name + ':' + (g.kind || '')).join(',');`);
      const summerBefore = await count('Summer');
      await w.step('按住展示页的一张图拖到按条件的画廊上', p => dragOnto(p, '.stage .piece:nth-child(3) img', condName));
      await sleep(600);
      w.check('松手在按条件的画廊上：什么也没写，缩略图收回去了', await count('Summer') === summerBefore && !await page.eval(`!!document.querySelector('.lift')`)
        && await inPage(page, `return (await (await fetch('/api/galleries')).json()).map(g => g.name + ':' + (g.kind || '')).join(',');`) === writes, condName);

      // 网格里：选中一张，选择栏末尾有「放进画廊…」；按住选中的图拖到 Summer。
      await w.step('收藏换成瀑布，进入选择模式，点第一张', async p => {
        await p.click(showBtn('masonry')); await sleep(500); await gridReady(p);
        await p.click('#selectButton'); await sleep(200); await p.click('#grid .card[data-index="0"] img'); await sleep(300);
      });
      const barText = await page.eval(`[...document.querySelectorAll('#selectionBar button')].map(b => b.dataset.act || '')`);
      w.check('选择栏末尾有「放进画廊…」', barText.at(-1) === 'gallery-add', barText);
      before = await count('Summer');
      snap = await w.step('按住选中的图拖到活页条上的 Summer', p => dragOnto(p, '#grid .card[data-index="0"] img', 'Summer'), {shot: true});
      w.check('网格里拖的也放进去了', await count('Summer') === before + 1, {before, after: await count('Summer')});
      await w.step('⌘Z 撤销', p => p.key('z', {modifiers: ['Meta']}));
      await sleep(600);
      w.check('撤销以后拿出来了（网格）', await count('Summer') === before, {before, after: await count('Summer')});
      await w.step('Esc', p => p.key('Escape'));

      // 按天视图、详情卡片的预览图：按住拖到 Summer。
      const dayBefore = await count('Summer');
      await w.step('按天视图里按住一张图拖到活页条上的 Summer', async p => {
        await goto(p, '#/days'); await p.waitFor(`!!document.querySelector('.day-tile[data-id]:not([data-more]) img')`, 15000);
        await dragOnto(p, '.day-tile[data-id]:not([data-more]) img', 'Summer');
      }, {shot: true});
      w.check('按天视图里拖的也放进去了', await count('Summer') === dayBefore + 1, {before: dayBefore, after: await count('Summer')});
      w.check('拖过以后没打开详情卡片', !await page.eval(`!!document.querySelector('#inspector:not([hidden])')`), '');
      await w.step('⌘Z 撤销', p => p.key('z', {modifiers: ['Meta']}));
      await sleep(600);
      w.check('撤销以后拿出来了（按天）', await count('Summer') === dayBefore, {before: dayBefore, after: await count('Summer')});

      // 详情卡片：点开一张，按住预览图拖到 Summer；拖过以后不开大图。
      await w.step('图库里点开一张的详情卡片', async p => {
        await goto(p, '#/library'); await gridReady(p); await p.click('#grid .card[data-index="0"] img');
        await p.waitFor(`!document.getElementById('inspector').hidden && !!document.querySelector('#inspector .dc-preview img')`, 5000); await sleep(400);
      });
      const cardBefore = await count('Summer');
      await w.step('按住详情卡片的预览图拖到活页条上的 Summer', p => dragOnto(p, '#inspector .dc-preview', 'Summer'), {shot: true});
      w.check('详情卡片拖的也放进去了', await count('Summer') === cardBefore + 1, {before: cardBefore, after: await count('Summer')});
      w.check('拖过以后没开大图', await page.eval(`document.getElementById('viewer')?.hidden !== false`), '');
      await w.step('⌘Z 撤销', p => p.key('z', {modifiers: ['Meta']}));
      await sleep(600);
      w.check('撤销以后拿出来了（详情卡片）', await count('Summer') === cardBefore, {before: cardBefore, after: await count('Summer')});
      await w.step('Esc', p => p.key('Escape'));

      // #646：「更多像这张的」页上有条件也没有「保存为画廊」；普通图库上有。
      snap = await w.step('打开「更多像这张的」并加一个条件', async p => {
        await p.eval(`location.hash = '#/library?like=' + encodeURIComponent(JSON.stringify([${JSON.stringify(ids[0])}])) + '&has=' + encodeURIComponent('["fav"]')`);
        await gridReady(p);
      });
      w.check('「更多像这张的」页上没有「保存为画廊」', !await page.eval(`!!document.getElementById('saveGallery')`), '');
      await w.step('图库加一个条件', async p => { await p.eval(`location.hash = '#/library?has=' + encodeURIComponent('["source:other"]')`); await gridReady(p); });
      w.check('图库上有「保存为画廊」', await page.eval(`!!document.getElementById('saveGallery')`), '');
    },
  },

  {
    name: 'guide-focus',
    title: '新图库的导览：焦点在导览的按钮上，Enter 下一步，Tab 不跑到后面，Space 不开大图；详情栏打开时上手清单让开',
    covers: ['#386 1-5', '#386 1-1'],
    async run(w, page) {
      await w.step('（准备）重开引导并重新载入', async p => {
        await post(p, '/api/onboarding', {action: 'reopen'});
        await p.send('Page.reload');
        await p.waitFor(`document.getElementById('rail') && !document.getElementById('splash')`, 15000);
      });
      let snap = await w.step('等导览出现', p => p.waitFor(`!document.getElementById('tour').hidden`, 8000), {shot: true});
      w.check('导览出现', snap.overlays.includes('tour'), snap.overlays);
      w.check('焦点在导览里', /tour/.test(snap.active), snap.active);
      const bubble = () => page.eval(`(document.querySelector('#tour .tour-bubble')?.innerText || '').slice(0, 60)`);
      const text0 = await bubble();
      snap = await w.step('按 Enter', p => p.key('Enter'));
      const text1 = await bubble();
      w.check('Enter 走到下一步', text1 !== text0, {before: text0, after: text1});
      w.check('下一步焦点仍在导览里', /tour/.test(snap.active), snap.active);
      for (let i = 1; i <= 4; i++) snap = await w.step(`按 Tab 第 ${i} 次`, p => p.key('Tab'));
      w.check('Tab 留在导览里', /tour/.test(snap.active), snap.active);
      snap = await w.step('按 Space', p => p.key('Space'));
      w.check('Space 没有在导览下面打开大图', !snap.overlays.includes('viewer'), snap.overlays);
      snap = await w.step('Esc 跳过导览', p => p.key('Escape'));
      w.check('导览关了', !snap.overlays.includes('tour'), snap.overlays);

      await w.step('打开图库', () => goto(page, '#/library'));
      snap = await w.step('点一张开详情栏', p => p.click(cardAt(0)), {shot: true});
      const hit = await page.eval(`(() => { const b = [...document.querySelectorAll('#inspector .dc-foot button')].pop(); if (!b) return 'no-button';
        const r = b.getBoundingClientRect(); const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return top && (top === b || b.contains(top)) ? 'button' : (top?.closest('#checklist') ? 'checklist' : (top?.id || top?.className || 'other')); })()`);
      w.check('详情栏底部的按钮点得到（不被上手清单盖住）', hit === 'button', hit);
    },
  },
];
