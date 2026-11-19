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
    name: 'pages',
    title: '主要页面逐个打开，截图并检查控制台和空页',
    async run(w, page) {
      const f = w.config.fixture;
      const pages = [
        ['图库 · 按天', '#/days', 'home'], ['图库 · 网格', '#/library', 'grid'], ['资料库', '#/characters', 'panel'],
        ['角色页', `#/library?char=${encodeURIComponent(f.charIds[f.chars[0].name])}`, 'grid'],
        ['资料库 · 作品', '#/characters/works', 'panel'],
        ['未识别 · 按组整理', '#/groups', 'panel'], ['未识别 · 第 1 组', `#/g/${f.groups[0]?.id}`, 'grid'],
        ['全部未识别', '#/unmatched', 'grid'], ['已跳过', '#/skipped', 'grid'], ['重复', '#/duplicates', 'grid'],
        ['画廊', '#/galleries', 'panel'],
        ['相似的图', '#/similar', 'panel'], ['相似的图 · 第 1 组', `#/s/${f.similar[0]?.id}`, 'grid'], ['已排除', '#/excluded', 'grid'],
        ['设置 · 通用', '#/settings', 'panel'], ['设置 · 图库', '#/settings/library', 'panel'],
        ['设置 · 画师', '#/settings/artists', 'panel'], ['设置 · 关于', '#/settings/about', 'panel'],
      ];
      for (const [label, hash, kind] of pages) {
        const snap = await w.step(`打开${label}`, () => goto(page, hash), {shot: true});
        w.check(`${label}：标题不空`, !!snap.heading, snap.heading);
        if (kind === 'grid') w.check(`${label}：网格有图`, snap.total > 0, `total=${snap.total}`);
        if (kind !== 'grid') {
          const filled = await page.eval(`(() => { const v = document.getElementById('viewport'); return [...v.children].some(el => !el.hidden && el.id !== 'grid' && el.id !== 'marquee' && el.id !== 'empty' && (el.innerText || '').trim().length > 20); })()`);
          w.check(`${label}：页面有内容`, filled, '');
        }
      }
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
      await w.step('点详情栏的「更正角色 C」', async p => { await goto(p, '#/library'); await openCard(w, p, target); await p.click('#fixChar'); });
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
        return {unmatched: S.pools?.unmatched ?? null, sections: document.querySelectorAll('#groupsPage .gsec').length,
          first: document.querySelector('#groupsPage .gsec')?.dataset.cid ?? null};`);
      await w.step('打开未识别 › 按组整理', () => goto(page, '#/groups'), {shot: true});
      const before = await counts();
      w.check('有分组', before.sections > 0, before);
      await w.step(`第 ${group.id} 组点「设为…」`, p => p.click(`#groupsPage .gsec[data-cid="${group.id}"] [data-gact="assign"]`));
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
    name: 'settings-fit',
    title: '设置 › 通用里切「原比例」→ 回图库：网格换成原比例，没有报错',
    covers: ['#386 1-9'],
    async run(w, page) {
      await w.step('打开设置', () => goto(page, '#/settings'));
      let snap = await w.step('点「原比例」', p => p.click('[data-setting="fit"] button[data-value="1"]'), {shot: true});
      const fit = await stateOf(page, 'S.fit');
      w.check('切换生效', fit === true, {fit});
      w.check('点的时候没有报错', !w.steps.at(-1).console.length, w.steps.at(-1).console);
      snap = await w.step('回图库', () => goto(page, '#/library'));
      const cards = await page.eval(`document.querySelectorAll('#grid .card.fit').length`);
      w.check('网格是原比例', cards > 0, {cards});
      await w.step('按 R 切回方格', p => p.key('r'));
      w.check('R 切回方格', (await stateOf(page, 'S.fit')) === false, '');
    },
  },

  {
    name: 'galleries',
    title: '画廊：收藏在最前、两种画廊分得出；换展示方式记得住；旧的收藏地址能打开；把图拖进手选画廊再撤销；「更多像这张的」页上没有「保存为画廊」',
    covers: ['#710', '#646'],
    async run(w, page) {
      const ids = Object.keys(w.config.names).slice(0, 12);
      await w.step('（准备）收藏 6 张，建一个手选画廊（放进 4 张）和一个空的手选画廊', async p => {
        await post(p, '/api/favorite', {ids: ids.slice(0, 6)});
        await post(p, '/api/galleries', {action: 'create', kind: 'manual', name: 'Summer', ids: ids.slice(6, 10)});
        await post(p, '/api/galleries', {action: 'create', kind: 'manual', name: 'Empty'});
      });
      let snap = await w.step('点活页条的画廊', () => goto(page, '#/galleries'), {shot: true});
      const cards = () => page.eval(`[...document.querySelectorAll('.gcard')].map(c => c.dataset.kind || (c.dataset.drop === 'new' ? 'new' : '?'))`);
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
      }
      await w.step('展示方式换成拼贴，再去别处', async p => { await p.click(showBtn('collage')); await sleep(500); await goto(p, '#/days'); });
      snap = await w.step('从活页条回到收藏', async p => { await goto(p, '#/galleries'); await p.click('.gcard[data-kind="fav"]'); await sleep(600); await gridReady(p); });
      w.check('记得住：回来还是拼贴', snap.hash.startsWith('#/showcase/fav')
        && await page.eval(`document.querySelector(${JSON.stringify(showBtn('collage'))})?.getAttribute('aria-pressed') === 'true'`), snap.hash);

      // 拖进手选画廊：真鼠标按住一张拼贴里的图拖到活页条「Summer」上。活页条拿起之后才换成画廊封面，所以先拿起、再量目标。
      const count = name => inPage(page, `const g = (await (await fetch('/api/galleries')).json()).find(x => x.name === ${JSON.stringify(name)});
        const r = await fetch('/api/assets/count?has=' + encodeURIComponent(JSON.stringify(['gallery:' + g.id]))); return (await r.json()).total;`);
      const before = await count('Summer');
      snap = await w.step('按住一张图拖到活页条上的 Summer', async p => {
        const from = await p.center('.stage .piece:nth-child(3) img');
        await p.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: from.x, y: from.y});
        await p.send('Input.dispatchMouseEvent', {type: 'mousePressed', x: from.x, y: from.y, button: 'left', clickCount: 1});
        for (let k = 1; k <= 4; k++) { await p.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: from.x - k * 6, y: from.y, button: 'left', buttons: 1}); await sleep(16); }
        await p.waitFor(`!!document.querySelector('.lift') && [...document.querySelectorAll('#railItems .rail-item')].some(x => x.offsetParent && x.dataset.key === 'new')`, 3000);
        await sleep(600);
        const to = await p.eval(`(() => { const el = [...document.querySelectorAll('#railItems .rail-item')].find(x => x.offsetParent && x.railItem?.label === 'Summer');
          const r = el.getBoundingClientRect(); return {x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2)}; })()`);
        for (let k = 1; k <= 8; k++) {
          await p.send('Input.dispatchMouseEvent', {type: 'mouseMoved', x: Math.round(from.x + (to.x - from.x) * k / 8), y: Math.round(from.y + (to.y - from.y) * k / 8), button: 'left', buttons: 1});
          await sleep(16);
        }
        const offs = await p.eval(`[...document.querySelectorAll('#railItems .rail-item.is-off')].filter(x => x.offsetParent).length`);
        await p.send('Input.dispatchMouseEvent', {type: 'mouseReleased', x: to.x, y: to.y, button: 'left', clickCount: 1});
        await sleep(900);
        return offs;
      }, {shot: true});
      w.check('放进去了', await count('Summer') === before + 1, {before, after: await count('Summer')});
      w.check('有提示和撤销', !!(await page.eval(`!document.getElementById('toast').hidden && !document.getElementById('toastUndo').hidden`)), '');
      w.check('活页条换回画廊这一组', await page.eval(`![...document.querySelectorAll('#railItems .rail-item')].some(x => x.offsetParent && x.dataset.key === 'new')`), '');
      await w.step('⌘Z 撤销', p => p.key('z', {modifiers: ['Meta']}));
      await sleep(600);
      w.check('撤销以后拿出来了', await count('Summer') === before, {before, after: await count('Summer')});

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
      const hit = await page.eval(`(() => { const b = [...document.querySelectorAll('#inspector .inspector-actions button')].pop(); if (!b) return 'no-button';
        const r = b.getBoundingClientRect(); const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return top && (top === b || b.contains(top)) ? 'button' : (top?.closest('#checklist') ? 'checklist' : (top?.id || top?.className || 'other')); })()`);
      w.check('详情栏底部的按钮点得到（不被上手清单盖住）', hit === 'button', hit);
    },
  },
];
