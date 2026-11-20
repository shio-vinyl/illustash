const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const path = require('node:path');
const web = require('./web.cjs');
// 页面注册表（core/pages.js，#342）：docs/WEB_UI_MAP.md 2.1 总表的每一行都能从注册表读出，并与文档一致；
// 注册表派生出的 cleanup()／browsing()／地址解析与改造前手写的判断逐页相同；活页条上亮哪个入口、哪一项（#706）。
let pages, route, state, rail;
before(async () => {
  await web.ready();
  pages = await web.load('core/pages.js');
  rail = await web.load('core/rail.js');
  ({state} = web.core()); route = web.core();
  globalThis.location = {hash: ''};
});

// 总表一行：页面名（文档里的第一列）→ 示例地址、注册表里的页面名，以及从文档读出来要对上的值。
// doc 是地址列里应当出现的文字。
const ROWS = {
  '图库 · 按天': {hash: '#/', page: 'days', doc: '`#/days`'},
  '图库 · 网格': {hash: '#/library?tag=x', page: 'library', doc: '`#/library`'},
  '图库 · 日历': {hash: '#/calendar', page: 'calendar', doc: '`#/calendar`'},
  '角色页': {hash: '#/library?char=c1', page: 'library', char: true, doc: '`#/library?char='},
  '更多像这张的': {hash: '#/library?like=%5B%22a%22%5D', page: 'library', doc: '`#/library?like='},
  '画廊': {hash: '#/galleries', page: 'galleries', doc: '`#/galleries`'},
  '画廊 · 一个画廊': {hash: '#/gallery/3?has=[]', page: 'gallery', doc: '`#/gallery/'},
  '资料库 · 角色': {hash: '#/characters', page: 'characters', doc: '`#/characters`'},
  '资料库 · 作品': {hash: '#/characters/works', page: 'characters', doc: '`#/characters/works`'},
  '资料库 · 画师': {hash: '#/characters/artists', page: 'characters', doc: '`#/characters/artists`'},
  '作品页': {hash: '#/w/w1', page: 'work', doc: '`#/w/<作品>`'},
  '设置': {hash: '#/settings/data', page: 'settings', doc: '`#/settings/'},
  '未识别 · 全部': {hash: '#/unmatched', page: 'unmatched', doc: '`#/unmatched`'},
  '未识别 · 已跳过': {hash: '#/skipped', page: 'skipped', doc: '`#/skipped`'},
  '未识别 · 按组整理': {hash: '#/groups', page: 'groups', doc: '`#/groups`'},
  '未识别 · 第 N 组': {hash: '#/g/12', page: 'group', doc: '`#/g/'},
  '重复 · 全部 / 角色不一致': {hash: '#/duplicates?dup=mixed', page: 'duplicates', doc: '`#/duplicates'},
  '重复 · 相似的图': {hash: '#/similar?kind=near', page: 'similar', doc: '`#/similar'},
  '重复 · 相似组 N': {hash: '#/s/7', page: 'sgroup', doc: '`#/s/'},
  '已排除 · 全部 / 按原因': {hash: '#/excluded?reason=screenshot', page: 'excluded', doc: '`#/excluded'},
  '已排除 · 待确认': {hash: '#/pending', page: 'pending', doc: '`#/pending`'},
  '随机抽查': {hash: '#/audit', page: 'audit', doc: '`#/audit`'},
  '身份整理': {hash: '#/identity', page: 'identity', doc: '`#/identity`'},
  '画师抽查': {hash: '#/artist-audit', page: 'artist-audit', doc: '`#/artist-audit`'},
};
// 文档里的文字 → 注册表的取值。认不出的文字让测试失败，提醒同时改两边。
const CLICK = [[/^浏览/, 'browse'], [/^整理/, 'cleanup']];
// 活页条一列：「入口」或「入口 › 二级项」，读成 core/rail.js 的 railAt 给的 {entry, item}（item 只比整理的队列）。
const ENTRY = [[/^图库/, 'library'], [/^画廊/, 'galleries'], [/^资料库/, 'catalog'], [/^整理/, 'tidy'], [/^设置/, 'settings'], [/^无/, '']];
const ITEM = [[/› 未识别/, 'unmatched'], [/› 重复/, 'duplicates'], [/› 已排除/, 'excluded'], [/› 随机抽查/, 'audit'], [/› 身份整理/, 'identity'], [/^/, '']];
const TOOLBAR = [[/^空$/, 'none'], [/^(搜索和筛选条|同按天)/, 'filters'], [/^仅子页签$/, 'tabs'], [/^角色｜作品/, 'index'], [/^搜索、/, 'library'], [/^同图库/, 'library'], [/^(子页签|三个子页签|排序)/, 'grid']];
const first = (table, text, what) => { const hit = table.find(([re]) => re.test(text)); assert.ok(hit, `文档里的${what}「${text}」认不出来，改了文档要同步改 tests/pages.test.cjs`); return hit[1]; };

function docRows() {
  const text = readFileSync(path.join(__dirname, '../docs/WEB_UI_MAP.md'), 'utf8'), start = text.indexOf('### 2.1 总表');
  const lines = text.slice(start).split('\n').slice(2);
  const rows = [];
  for (const line of lines) {
    if (!line.startsWith('|')) break;
    const cells = line.split('|').slice(1, -1).map(c => c.trim());
    if (cells[0] === '页面' || /^-+$/.test(cells[0])) continue;
    rows.push(cells);
  }
  return rows;
}
const spec = hash => { globalThis.location.hash = hash; const r = route.parseHash(); return {r, view: pages.viewOf(r)}; };

test('docs/WEB_UI_MAP.md 2.1 总表的每一行都能从注册表读出，且与文档一致', () => {
  const rows = docRows();
  assert.deepEqual(rows.map(c => c[0]), Object.keys(ROWS), '总表的行和测试里的清单一一对应');
  let before = '';
  for (const [label, address, area, toolbarCell, selection, inspector, , nav] of rows) {
    const toolbar = toolbarCell === '同上' ? before : toolbarCell;   // 「同上」是上一行的工具栏
    before = toolbar;
    const want = ROWS[label], {r, view} = spec(want.hash);
    assert.equal(r.page, want.page, `${label}：地址 ${want.hash} 落在哪一页`);
    assert.equal(!!r.char, !!want.char, `${label}：是不是角色页`);
    assert.ok(address.includes(want.doc), `${label}：地址列 ${address} 里应有 ${want.doc}`);
    assert.equal('#' + view.area, area.replace(/`/g, ''), `${label}：主区`);
    const at = rail.railAt(r);
    assert.equal(at.entry, first(ENTRY, nav, '活页条'), `${label}：活页条的入口`);
    if (at.entry === 'tidy') assert.equal(at.item, first(ITEM, nav, '活页条'), `${label}：活页条的二级项`);
    assert.equal(view.toolbar, first(TOOLBAR, toolbar, '工具栏'), `${label}：工具栏`);
    // 选择栏、详情栏：网格页才有（首页可以点图开详情栏）
    assert.equal(selection.startsWith('有'), view.area === 'grid', `${label}：选择栏`);
    assert.equal(inspector.startsWith('有'), view.area === 'grid' || view.area === 'homePage', `${label}：详情栏`);
  }
});

test('文档点击列里没有归到浏览／整理的，注册表里是页面自己处理', () => {
  for (const [label, , , , , , click] of docRows()) {
    const {view} = spec(ROWS[label].hash), kind = CLICK.find(([re]) => re.test(click))?.[1] || 'own';
    assert.equal(view.click, kind, label);
  }
});

test('注册表里的每个页面都在总表里有一行，反过来也是', () => {
  const inTable = new Set(Object.values(ROWS).map(x => x.page));
  for (const name of Object.keys(pages.PAGES)) assert.ok(inTable.has(name), `${name} 没有出现在 docs/WEB_UI_MAP.md 2.1 总表（新增页面要加一行）`);
  for (const name of inTable) assert.ok(pages.PAGES[name], `${name} 没有在注册表里登记`);
});

// ---------- 与改造前手写判断逐页相同 ----------
// 改造前的手写清单，加上 #353 的三处改动：相似组页（sgroup）归整理页，画师抽查（artist-audit）归面板页，新加入（recent）删掉。
const OLD = {
  cleanup: ['unmatched', 'skipped', 'excluded', 'pending', 'group', 'audit', 'sgroup'],
  browsing: ['library', 'duplicates', 'gallery'],
  panel: ['days', 'calendar', 'characters', 'work', 'settings', 'groups', 'similar', 'identity', 'artist-audit', 'galleries'],
  lead: ['unmatched', 'skipped', 'group'],
  // 原来侧栏的高亮：整理一类各页亮的那一项，现在是活页条整理里的那个队列（#706）
  nav: page => ({skipped: 'unmatched', groups: 'unmatched', group: 'unmatched', pending: 'excluded', similar: 'duplicates', sgroup: 'duplicates'})[page] || page,
  tidy: ['unmatched', 'skipped', 'groups', 'group', 'duplicates', 'similar', 'sgroup', 'excluded', 'pending', 'audit', 'identity'],
  valid: ['days', 'calendar', 'unmatched', 'skipped', 'duplicates', 'excluded', 'pending', 'audit', 'groups', 'similar', 'identity', 'artist-audit'],
};
test('cleanup()、browsing()、panelPage、LEAD_PAGES、活页条整理的选中项和改造前的手写清单一致', () => {
  const {cleanup, browsing, defaultSort} = route, {prefs} = web.core();
  for (const name of Object.keys(pages.PAGES)) for (const char of ['', 'c1']) {
    state.route = {page: name, char};
    assert.equal(cleanup(), !!char || OLD.cleanup.includes(name), `cleanup ${name} ${char}`);
    assert.equal(browsing(), !char && OLD.browsing.includes(name), `browsing ${name} ${char}`);
    // 角色筛选只存在于图库
    const view = pages.viewOf({page: name, char});
    assert.equal(!!pages.PAGES[name].panel, OLD.panel.includes(name), `panel ${name}`);
    assert.equal(!!(!char && view.lead), OLD.lead.includes(name) && !char, `lead ${name} ${char}`);
    // 整理一类亮整理入口，选中原来侧栏亮的那个队列
    if (!char && OLD.tidy.includes(name)) assert.deepEqual([view.entry, view.item], ['tidy', OLD.nav(name)], `活页条 ${name}`);
  }
  // 默认排序：角色页偏好、待认页共用偏好、其余图库偏好
  assert.equal(defaultSort({page: 'library', char: 'c1'}), prefs.get('sortCharacter', 'suspicious'));
  assert.equal(defaultSort({page: 'unmatched'}), prefs.get('sortUnmatched', prefs.get('sortLibrary', 'newest')));
  assert.equal(defaultSort({page: 'excluded'}), prefs.get('sortLibrary', 'newest'));
  // #353：画师抽查是面板页，工具栏是空的；相似组页是整理页（点击切换选中、工具栏是「全选」）；新加入页没有了
  assert.deepEqual([pages.PAGES['artist-audit'].area, pages.PAGES['artist-audit'].panel, pages.PAGES['artist-audit'].toolbar], ['groupsPage', true, 'none']);
  state.route = {page: 'sgroup', char: ''};
  assert.deepEqual([route.cleanup(), route.browsing()], [true, false]);
  assert.equal(pages.PAGES.recent, undefined);
});

test('主区是网格以外的页面都是面板页；单图快捷键的范围从注册表读', () => {
  for (const name of Object.keys(pages.PAGES)) {
    const p = pages.PAGES[name];
    assert.equal(!!p.panel, p.area !== 'grid', `${name}：panel 与主区`);
    assert.equal(pages.keyScope(p), p.area === 'grid' ? 'grid' : name === 'days' ? 'shown' : '', `${name}：快捷键范围`);
  }
  assert.equal(pages.keyScope(pages.CHARACTER), 'grid');
});

test('旧地址：#/recent（#353）、#/home 改写到图库的按天视图，#/tidy 改写到按组整理（#706）', async () => {
  for (const [hash, page] of [['#/recent', 'days'], ['#/recent?sort=oldest', 'days'], ['#/home', 'days'], ['#/tidy', 'groups']]) {
    const {r} = spec(hash);
    assert.deepEqual([r.page, r.legacy], [page, true], hash);
  }
  // 原型上的名字不是旧地址
  for (const hash of ['#/constructor', '#/toString', '#/__proto__']) assert.equal(spec(hash).r.page, 'library', hash);
  // applyRoute 把地址就地改成按天视图的
  const page = web.fakePage();
  const replaced = [];
  globalThis.history = {replaceState: (a, b, url) => replaced.push(url)};
  globalThis.window = {addEventListener() {}};
  globalThis.requestAnimationFrame = () => 0;
  globalThis.fetch = () => Promise.resolve({status: 200, ok: true, json: async () => ({days: [], next: ''})});
  const {applyRoute} = await web.load('ui/shell.js');
  globalThis.location.hash = '#/recent';
  await applyRoute();
  assert.equal(state.route.page, 'days');
  assert.deepEqual(replaced, ['#/days']);
  assert.equal(page.el('homePage').hidden, false);
});

test('地址：能认出的页面名、带编号的页面、子页和旧角色地址；写回去再读得到同一页', () => {
  const reads = {
    '': 'days', '#': 'days', '#/': 'days', '#/?artist=a1': 'library', '#/home': 'days', '#/library': 'library', '#/nope': 'library',
    '#/characters': 'characters', '#/characters/works': 'characters', '#/settings/network': 'settings', '#/g/3': 'group', '#/s/4': 'sgroup', '#/c/c9/skin': 'library',
    '#/g': 'library', '#/s/': 'library', '#/sgroup': 'library', '#/group': 'library', '#/character': 'library',
  };
  for (const name of OLD.valid) reads['#/' + name] = name;
  for (const [hash, page] of Object.entries(reads)) { const {r} = spec(hash); assert.equal(r.page, page, hash); }
  const legacy = spec('#/c/c9/skin').r;
  assert.deepEqual([legacy.char, legacy.variant, legacy.legacy], ['c9', 'skin', true]);
  assert.equal(spec('#/g/a%20b').r.id, 'a b');
  assert.equal(spec('#/characters/works').r.section, 'works');
  // 只有图库带角色、画师、检查这些筛选
  assert.equal(spec('#/unmatched?char=c1&artist=a').r.char, '');
  assert.equal(spec('#/library?char=c1').r.char, 'c1');
  // 按天、日历带图库的组合条件（三种视图共用筛选，#703），不带角色页
  assert.deepEqual(spec('#/days?has=%5B%22fav%22%5D&char=c1').r.has, ['fav']);
  assert.equal(spec('#/calendar?char=c1').r.char, '');
  assert.deepEqual(spec('#/unmatched?has=%5B%22fav%22%5D').r.has, []);
  for (const hash of ['#/days', '#/calendar', '#/library?day=2026-10-02', '#/characters/works', '#/settings/data', '#/g/3', '#/s/4', '#/groups', '#/duplicates?dup=mixed', '#/excluded?reason=screenshot', '#/library?char=c1&variant=v']) {
    const {r} = spec(hash);
    assert.equal(route.hashFor(r), hash, hash);
  }
  assert.equal(route.charHash('c1', 'v'), '#/library?char=c1&variant=v');
});

test('implement() 只接受登记过的页面名；每个页面都有标题以外的必要字段', () => {
  assert.throws(() => pages.implement({nope: {}}), /没有登记过的页面/);
  for (const name of Object.keys(pages.PAGES)) {
    const p = pages.PAGES[name];
    assert.ok(['grid', 'homePage', 'calendarPage', 'charIndex', 'settingsPage', 'groupsPage'].includes(p.area), `${name}.area`);
    assert.ok(['library', 'catalog', 'galleries', 'tidy', 'settings', ''].includes(p.entry), `${name}.entry`);
    assert.ok(['browse', 'cleanup', 'select', 'own'].includes(p.click), `${name}.click`);
    assert.ok(['none', 'tabs', 'index', 'library', 'filters', 'grid'].includes(p.toolbar), `${name}.toolbar`);
    if (p.toolbar === 'grid' || p.toolbar === 'library') assert.ok(p.sorts.every(s => ['suspicious', 'newest', 'oldest', 'hinted'].includes(s)), `${name}.sorts`);
    if (p.tabs) assert.ok(p.tab, `${name}.tab`);
    if (p.pool) assert.equal(p.area, 'grid', `${name}：有 pool 的页面用网格`);
  }
});

test('后端查询参数：pool、分组编号、筛选都从注册表来', () => {
  const params = (page, extra = {}) => { const p = pages.PAGES[page]; return {...(p.pool ? {pool: p.pool} : {}), ...(p.params?.({page, ...extra}) || {})}; };
  assert.deepEqual(params('unmatched'), {pool: 'unmatched'});
  assert.deepEqual(params('skipped'), {pool: 'skipped'});
  assert.deepEqual(params('duplicates', {dup: 'mixed'}), {pool: 'duplicates', dup: 'mixed'});
  assert.deepEqual(params('duplicates'), {pool: 'duplicates'});
  assert.deepEqual(params('excluded', {reason: 'screenshot'}), {pool: 'excluded', reason: 'screenshot'});
  assert.deepEqual(params('pending'), {pool: 'pending'});
  assert.deepEqual(params('audit'), {pool: 'audit'});
  assert.deepEqual(params('group', {id: '3'}), {pool: 'cluster', cluster: '3'});
  assert.deepEqual(params('sgroup', {id: '4'}), {pool: 'similar', cluster: '4'});
  assert.deepEqual(pages.PAGES.library.params({day: 'd', work: 'w', q: 'q', tag: 't', utag: 'u'}), {day: 'd', work: 'w', q: 'q', tag: 't', utag: 'u'});
  assert.equal(pages.poolIs({page: 'skipped'}, 'unmatched', 'skipped'), true);
  assert.equal(pages.poolIs({page: 'group'}, 'unmatched', 'skipped'), false);
});

// 未识别一类三个整理页的行为靠注册表属性，不靠页面名（#385）：设角色之后图离开 = lead，能跳过当前这张 = skip。
test('lead 页设角色后离开；全部未识别和组页能跳过，已跳过页不能', () => {
  const P = pages.PAGES;
  assert.deepEqual(Object.keys(P).filter(n => P[n].lead), ['unmatched', 'skipped', 'group']);
  assert.deepEqual(Object.keys(P).filter(n => P[n].skip), ['unmatched', 'group']);
  // 能跳过的页，选择栏也是「未识别」那一种（S 快捷键和选择栏的「跳过」按钮同源）
  for (const n of Object.keys(P).filter(n => P[n].skip)) assert.equal(P[n].bar, 'unmatched', n);
  assert.equal(pages.pageOf({page: 'group', id: '3'}).skip, true);
  assert.equal(pages.CHARACTER.skip, undefined);
});

test('整理页都登记了底栏提示（已知问题 10）', () => {
  for (const name of ['group', 'sgroup', 'unmatched', 'skipped', 'excluded', 'pending', 'audit']) assert.ok(pages.PAGES[name].hint?.(), name);
  assert.match(pages.PAGES.group.hint(), /S 跳过/);
  assert.doesNotMatch(pages.PAGES.sgroup.hint(), /S 跳过|C 设为角色/);
});

// 排序记忆（#386 的 2-26）：每类页面一个偏好键；图库和角色页的键就是设置里的两项默认排序，未识别一类共用一个，
// 其余各页各记各的；没记过的页面跟着图库默认排序。
test('each kind of page remembers its own sort', () => {
  const store = new Map(), saved = globalThis.localStorage;
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v)};
  try {
    const {sortKey, defaultSort} = route, {prefs} = web.core();
    assert.deepEqual(['library', 'unmatched', 'skipped', 'group', 'duplicates', 'excluded', 'pending', 'sgroup', 'audit'].map(page => sortKey({page})),
      ['sortLibrary', 'sortUnmatched', 'sortUnmatched', 'sortUnmatched', 'sort.duplicates', 'sort.excluded', 'sort.pending', 'sort.sgroup', 'sort.audit']);
    assert.equal(sortKey({page: 'library', char: 'c1'}), 'sortCharacter');
    prefs.set('sortLibrary', 'oldest');
    assert.equal(defaultSort({page: 'duplicates'}), 'oldest');
    prefs.set('sort.duplicates', 'newest');
    assert.deepEqual([defaultSort({page: 'duplicates'}), defaultSort({page: 'excluded'}), defaultSort({page: 'library'})], ['newest', 'oldest', 'oldest']);
    prefs.set('sortCharacter', 'newest');
    assert.equal(defaultSort({page: 'library', char: 'c1'}), 'newest');
  } finally { globalThis.localStorage = saved; }
});

// 按天、日历的工具栏（#706，#703 第 2 问）：只有搜索框和筛选条，没有排序、选择、显示方式；搜索词改了就地写进地址，
// 和筛选条上改条件一样，页面收到 render({samePage: true})（筛选变了只有这一个入口），作用域还是这一页的。
test('按天、日历的工具栏只有搜索框和筛选条；搜索后页面收到 render({samePage: true})', async () => {
  globalThis.history = {state: null, replaceState() {}};
  globalThis.window = {addEventListener() {}};
  const {renderChrome} = await web.load('ui/shell.js');
  const S = await web.load('ui/scope.js');
  for (const name of ['days', 'calendar']) {
    const page = web.fakePage(), calls = [], scope = S.page(name);   // 每次一个新的假页面：搜索框上的监听不跨页累积
    pages.implement({[name]: {render: opts => calls.push(opts)}});
    state.route = {page: name, q: '', has: [], not: []};
    renderChrome();
    const bar = page.el('toolbar');
    assert.equal(bar.hidden, false, name);
    assert.match(bar.innerHTML, /id="search"/, name);
    assert.doesNotMatch(bar.innerHTML, /id="sort"|id="selectButton"|id="fitToggle"/, name);
    const input = page.el('search');
    input.value = 'miku';
    for (const fn of input.listeners.input) fn({target: input, isComposing: false, keyCode: 0});
    await new Promise(r => setTimeout(r, 300));
    assert.deepEqual(calls.map(x => [x.samePage, x.scope === scope]), [[true, true]], name);
    assert.equal(state.route.q, 'miku', name);
  }
});

// 页面样板（docs/WEB_V2.md「页面样板」）：照例子写的页面，同一页又画几次（筛选变了、换子视图：render({samePage: true})）以后，
// 活着的作用域数不涨；作用域还是进页面时那一个。
test('照页面样板写：同一页重画几次，活着的作用域数不涨', async () => {
  const page = web.fakePage();
  globalThis.history = {state: null, replaceState() {}};
  globalThis.window = {addEventListener() {}};
  globalThis.requestAnimationFrame = () => 0;
  const S = await web.load('ui/scope.js'), {keyed} = await web.load('ui/keyed.js'), {applyRoute} = await web.load('ui/shell.js');
  S.setDev(true);
  let view = null, builds = 0;
  const notesView = scope => {
    builds++;
    const list = keyed(page.el('calendarPage'), {key: n => n, create: n => ({el: Object.assign(page.document.createElement('li'), {textContent: n})}), scope});
    return {scope, load: route => list.set(route.q ? [route.q] : [])};
  };
  pages.implement({calendar: {render: ({samePage, scope}) => { if (!samePage || !view?.scope.alive) view = notesView(scope); return view.load(state.route); }}});
  globalThis.location.hash = '#/calendar';
  await applyRoute();
  const first = S.currentPage(), count = S.live().count;
  for (const q of ['a', 'b', 'c']) { globalThis.location.hash = '#/calendar?q=' + q; await applyRoute(); }
  assert.equal(S.currentPage(), first, '同一页沿用作用域');
  assert.equal(S.live().count, count, '活着的作用域数不涨');
  assert.equal(builds, 1, '部件只建一次');
  S.setDev(false);
});

// 搜索词改了换一个新的路由对象（core/route.js 的 reRoute）：照页面样板写的页面，先停在 a 再打成 ab，a 的回复晚到也不画。
test('搜索时先发的请求后回来，它的结果不画（页面样板第 5 条）', async () => {
  const page = web.fakePage();
  globalThis.history = {state: null, replaceState() {}};
  globalThis.window = {addEventListener() {}};
  globalThis.requestAnimationFrame = () => 0;
  const {applyRoute, renderChrome} = await web.load('ui/shell.js');
  const pending = new Map(), drawn = [];
  const loadNotes = route => new Promise(resolve => pending.set(route.q, resolve));
  let view = null;
  const notesView = scope => ({scope, async load(route) {
    const notes = await loadNotes(route);
    if (!scope.alive || state.route !== route) return;
    drawn.push(notes);
  }});
  pages.implement({calendar: {render: ({samePage, scope}) => { if (!samePage || !view?.scope.alive) view = notesView(scope); return view.load(state.route); }}});
  globalThis.location.hash = '#/calendar';
  const entered = applyRoute();
  await new Promise(r => setTimeout(r, 0));
  pending.get('')(['全部']); await entered;
  renderChrome();
  const input = page.el('search'), type = async value => {
    input.value = value;
    for (const fn of input.listeners.input) fn({target: input, isComposing: false, keyCode: 0});
    await new Promise(r => setTimeout(r, 300));
  };
  await type('a'); await type('ab');
  pending.get('ab')(['ab 的结果']); await new Promise(r => setTimeout(r, 0));
  pending.get('a')(['a 的结果']); await new Promise(r => setTimeout(r, 0));
  assert.deepEqual(drawn, [['全部'], ['ab 的结果']]);
  assert.equal(state.route.q, 'ab');
});

// NSFW 切换同样换一个新的路由对象（ui/grid.js 的 setNsfw）：按天、日历这类页面在切换前发出的读取晚回来，结果不画。
test('切换 NSFW 前发出的请求后回来，它的结果不画', async () => {
  web.fakePage();
  globalThis.history = {state: null, replaceState() {}};
  globalThis.window = {addEventListener() {}};
  globalThis.requestAnimationFrame = () => 0;
  globalThis.fetch = async () => ({status: 200, ok: true, json: async () => ({})});
  const {applyRoute} = await web.load('ui/shell.js'), {setNsfw} = await web.load('ui/grid.js');
  const pending = new Map(), drawn = [];
  let view = null;
  const notesView = scope => ({scope, async load(route) {
    const notes = await new Promise(resolve => pending.set(state.nsfw, resolve));
    if (!scope.alive || state.route !== route) return;
    drawn.push(notes);
  }});
  const render = ({samePage, scope}) => { if (!samePage || !view?.scope.alive) view = notesView(scope); return view.load(state.route); };
  pages.implement({calendar: {render, refresh: () => view.load(state.route)}});
  state.nsfw = '';
  globalThis.location.hash = '#/calendar';
  const entered = applyRoute();
  await new Promise(r => setTimeout(r, 0));
  const before = state.route;
  setNsfw('hide');
  assert.notEqual(state.route, before, '换了新的路由对象');
  assert.equal(state.route.page, 'calendar');
  pending.get('hide')(['隐藏 NSFW 的结果']); await new Promise(r => setTimeout(r, 0));
  pending.get('')(['全部的结果']); await entered;
  assert.deepEqual(drawn, [['隐藏 NSFW 的结果']]);
  setNsfw('');
});
