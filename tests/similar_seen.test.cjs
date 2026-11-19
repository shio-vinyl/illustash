// #427：相似组的「看过了」。列表不显示看过的组，「看过的 N 组」只列它们；组页标题上标记、取消，撤销后列表重画。
// #601：检查进行中的轮询只就地改状态行和进度条，组卡片按组号比对；组列表分页。
// 页面是假的（tests/web.cjs），组和图都是合成的。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
before(web.ready);

const tick = () => new Promise(resolve => setImmediate(resolve));
let shell, pages, page, state, requests, posts, groups, scan = {status: 'done'}, timers = [];
// 「相似的图」页的几块：真页面上由 innerHTML 建出来，这里先放好，querySelector 按类名取。
class Box extends web.FakeElement { set innerHTML(v) { this._html = v; this.children = []; } get innerHTML() { return this._html || ''; } }
const parts = {};
function similarParts() {
  Object.assign(parts, {dot: new web.FakeElement('span'), text: new web.FakeElement('span'), bar: new Box(), progress: {hidden: true, firstElementChild: {style: {}}},
    grid: new Box(), more: new Box()});
  const status = {firstElementChild: parts.dot, lastElementChild: parts.text};
  const map = {'.similar-list': parts.grid, '.similar-status': status, '.groups-bar': parts.bar, '.progress': parts.progress, '.similar-more': parts.more};
  page.el('groupsPage').querySelector = sel => map[sel] || null;
}
const cards = () => parts.grid.children;
const listText = () => cards().map(c => c._sectionHTML).join('') + parts.grid.innerHTML;
async function wired() {
  if (shell) return;
  page = web.fakePage();
  similarParts();
  globalThis.window = {addEventListener() {}};
  globalThis.history = {replaceState() {}};
  globalThis.requestAnimationFrame = () => 0;
  globalThis.matchMedia = () => ({matches: true});
  globalThis.IntersectionObserver = class { observe() {} unobserve() {} disconnect() {} };
  const store = new Map();
  globalThis.localStorage = {getItem: k => store.has(k) ? store.get(k) : null, setItem: (k, v) => store.set(k, v), removeItem: k => store.delete(k)};
  ({state} = web.core());
  shell = await web.load('ui/shell.js'); await web.load('ui/review.js'); pages = await web.load('core/pages.js');
  globalThis.fetch = (path, init) => {
    const q = new URLSearchParams(path.split('?')[1] || '');
    let body = [];
    if (init?.method === 'POST') {
      posts.push([path, JSON.parse(init.body)]);
      body = {undo: {id: posts.length, label: '已标为看过', ts: ''}};
    } else {
      requests.push(path);
      if (path.startsWith('/api/similar?')) body = {groups: groups.map(g => q.get('seen') === '1' ? g : (({seen, ...rest}) => rest)(g)), state: scan};
      else if (path.startsWith('/api/assets?')) body = {total: 2, items: [{uuid: 'a', memberships: [], width: 3, height: 4}, {uuid: 'b', memberships: [], width: 3, height: 4}]};
      else if (path.startsWith('/api/assets/count')) body = {total: 2};
    }
    return Promise.resolve({status: 200, ok: true, json: async () => body});
  };
}
async function visit(hash) {
  requests = []; posts = [];
  location.hash = hash;
  await shell.applyRoute(); await tick(); await tick();
}
const group = (gid, seen, size = 2) => ({gid, size, covers: ['a', 'b'].slice(0, size), kinds: {duplicate: 1}, seen});

test('看过的组不在列表里，「看过的 N 组」只列它们', async () => {
  await wired();
  globalThis.location = {hash: ''};
  Object.assign(state, {route: {page: 'days'}, nsfw: '', guide: undefined});
  groups = [group(3, false), group(7, true)];
  await visit('#/similar');
  assert.ok(requests.some(p => p.startsWith('/api/similar?') && p.includes('seen=1')));
  assert.match(listText(), /href="#\/s\/3"/);
  assert.doesNotMatch(listText(), /href="#\/s\/7"/);
  assert.match(parts.bar.innerHTML, /href="#\/similar\?seen=1">看过的 1 组 →/);
  await visit('#/similar?seen=1');
  assert.match(listText(), /href="#\/s\/7"/);
  assert.doesNotMatch(listText(), /href="#\/s\/3"/);
  assert.match(parts.text.textContent, /看过的 1 组 · 共 2 张/);
  assert.match(parts.bar.innerHTML, /没看过的 1 组 →/);
  groups = [group(7, true)];
  await visit('#/similar');
  assert.match(listText(), /相似的组都看过了。/);
});

test('撤销之后相似的图页重画（#386 的 2-41）', async () => {
  await wired();
  groups = [group(3, false)];
  await visit('#/similar');
  const [three] = cards();
  groups = [group(3, false), group(4, false)];
  requests = [];
  await pages.pageOf(state.route).refresh('undo');
  assert.ok(requests.some(p => p.startsWith('/api/similar?')));
  assert.match(listText(), /href="#\/s\/4"/);
  assert.equal(cards()[0], three, '没变的组留着原来的卡片');
});

test('检查进行中的轮询只就地更新状态行和进度条，只换变了的组（#601）', async () => {
  await wired();
  const realSet = globalThis.setTimeout, realClear = globalThis.clearTimeout;
  globalThis.setTimeout = (fn, ms) => { timers.push({fn, ms}); return timers.length; };
  globalThis.clearTimeout = () => {};
  try {
    scan = {status: 'comparing', done: 1, total: 4, found: 2};
    groups = [group(3, false), group(4, false), group(5, false)];
    timers = [];
    await visit('#/similar');
    const [three, four, five] = cards();
    assert.equal(parts.progress.hidden, false); assert.equal(parts.progress.firstElementChild.style.width, '25%');
    assert.equal(parts.dot.classList.contains('busy'), true);
    assert.match(parts.text.textContent, /3 组 · 共 6 张 · 正在逐对比较 1\/4/);
    const bar = parts.bar.innerHTML;
    const poll = timers.find(x => x.ms === 3000); assert.ok(poll, '检查进行中 3 秒后再读');
    // 组没变：卡片和工具条都不动。
    scan = {...scan, done: 2}; timers = [];
    poll.fn(); await tick(); await tick();
    assert.deepEqual(cards(), [three, four, five]);
    assert.equal(parts.progress.firstElementChild.style.width, '50%');
    assert.equal(parts.bar.innerHTML, bar);
    // 4 组变大、5 组排到前面、来了 6 组：只换 4 组、新建 6 组，5 组只挪位置。
    scan = {...scan, done: 3}; groups = [group(5, false), group(3, false), group(4, false, 3), group(6, false)];
    const next = timers.find(x => x.ms === 3000); timers = [];
    next.fn(); await tick(); await tick();
    assert.equal(cards()[0], five); assert.equal(cards()[1], three);
    assert.notEqual(cards()[2], four); assert.match(cards()[2]._sectionHTML, /3 张/);
    assert.match(cards()[3]._sectionHTML, /href="#\/s\/6"/);
    // 检查完了：进度条收起，不再轮询。
    scan = {status: 'done'};
    const last = timers.find(x => x.ms === 3000); timers = [];
    last.fn(); await tick(); await tick();
    assert.equal(parts.progress.hidden, true); assert.equal(parts.dot.classList.contains('busy'), false);
    assert.ok(!timers.some(x => x.ms === 3000));
  } finally { globalThis.setTimeout = realSet; globalThis.clearTimeout = realClear; scan = {status: 'done'}; }
});

test('组列表先显示 40 组，「再显示」接着往下；撤销后留在已显示的组数，重新进来回到 40 组（#601）', async () => {
  await wired();
  groups = Array.from({length: 45}, (_, i) => group(i + 1, false));
  await visit('#/similar');
  assert.equal(cards().length, 40);
  assert.match(parts.more.innerHTML, /再显示 5 组/);
  const first = cards()[0];
  parts.more.onclick({target: {closest: () => true}});
  assert.equal(cards().length, 45); assert.equal(cards()[0], first);
  assert.equal(parts.more.innerHTML, '');
  groups = groups.slice(1);
  await pages.pageOf(state.route).refresh('undo');
  assert.equal(cards().length, 44);
  await visit('#/similar?kind=near');
  assert.equal(cards().length, 40);
});

test('组页标题的「看过了」标记这组，回到列表；看过的组按钮是「标为没看过」', async () => {
  await wired();
  groups = [group(7, false)];
  await visit('#/s/7');
  assert.deepEqual(state.sgroupSeen, {seen: false, size: 2, nsfw: ''});
  assert.match(page.el('heading').innerHTML, /id="sgroupSeen"[^>]*>看过了</);
  await page.el('sgroupSeen').listeners.click.at(-1)();
  assert.deepEqual(posts, [['/api/similar/seen', {gid: 7, size: 2, nsfw: '', seen: true}]]);
  assert.equal(location.hash, '#/similar');
  groups = [group(7, true)];
  await visit('#/s/7');
  assert.match(page.el('heading').innerHTML, /id="sgroupSeen"[^>]*>标为没看过</);
  await page.el('sgroupSeen').listeners.click.at(-1)();
  assert.deepEqual(posts, [['/api/similar/seen', {gid: 7, size: 2, nsfw: '', seen: false}]]);
  assert.equal(location.hash, '#/s/7', '取消以后留在这一组');
  assert.match(page.el('heading').innerHTML, /id="sgroupSeen"[^>]*>看过了</);
  // 组不在了（组号重建）：不显示按钮。
  groups = [];
  await visit('#/s/7');
  assert.equal(state.sgroupSeen, null);
  assert.doesNotMatch(page.el('heading').innerHTML, /sgroupSeen/);
});
