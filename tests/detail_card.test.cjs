// 运行：node --test tests/detail_card.test.cjs
// 详情卡片的部件（ui/detail_card.js，#709）：角色身份块（扩展小字和证据文字，#550）、「也可能是」、大图底栏的紧凑写法、
// 以及「翻页只换变了的部分」：同一份数据再 update 一次不动 DOM，换一张图时卡片和图片节点都还是原来那些。
// 用 tests/dom.cjs 的真树；只测部件，读取和动作在 ui/detail.js。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

let card, state;
test.before(async () => { await web.ready(); dom.install(); card = await web.load('ui/detail_card.js'); state = web.core().state; state.charMap = new Map(); });
test.after(() => dom.uninstall());

const machine = (id, name, evidence) => ({id, name, status: 'machine', origin: 'machine', variants: [], evidence});
const detail = d => ({uuid: 'a', width: 800, height: 1200, added_at: '2026-11-01T10:20:00', filename: 'synthetic.png', src_kind: 'orig',
  memberships: [], alternatives: [], tags: [], user_tags: [], duplicates: [], tag_identities: [], ...d});
const OPTS = {ask: false, chart: true, same: true, meta: true, skip: false, resumeKey: false};
// 子树里所有元素（不含文字节点）。
const all = el => el.children.flatMap(c => [c, ...all(c)]);
const byClass = (el, name) => all(el).filter(x => x.classList.contains(name));
const titleOf = el => el.getAttribute('title') ?? el.title;
const visible = el => { for (let n = el; n; n = n.parentNode) if (n.hidden) return false; return true; };

test('扩展补上的机器归属：认出方式写分数，名字后面有「扩展」', () => {
  const who = card.identities();
  who.update(detail({memberships: [machine('c1', 'Newer', {sources: ['WD_EXT'], max_score: 0.9312})]}));
  const name = byClass(who.el, 'dc-name')[0];
  assert.match(name.textContent, /^Newer扩展$/);
  assert.match(byClass(who.el, 'dc-lead')[0].textContent, /机器识别 · WD 扩展 0\.93/);
  assert.equal(visible(all(who.el).find(x => x.dataset.act === 'confirm')), true, '机器认出的主角色有「没错」');
});

test('人工确认过的不再标「扩展」，也没有「没错」', () => {
  const who = card.identities();
  who.update(detail({memberships: [{...machine('c1', 'Newer', {sources: ['WD_EXT'], max_score: 0.93}), status: 'confirmed', origin: 'human'}]}));
  assert.equal(byClass(who.el, 'dc-ext').length, 0);
  assert.match(byClass(who.el, 'dc-lead')[0].textContent, /人工确认/);
  assert.equal(visible(all(who.el).find(x => x.dataset.act === 'confirm')), false);
});

test('「也可能是」：和 WD、外部索引并列，扩展项有小字，弱线索单独标', () => {
  const who = card.identities();
  who.update(detail({memberships: [machine('c0', 'Old', {sources: ['WD'], max_score: 0.95})], alternatives: [
    {id: 'c2', name: 'Strong', weak: false, evidence: {sources: ['CCIP', 'WD'], max_score: 0.9, wd: 0.91, ccip: 0.123}},
    {id: 'c3', name: 'Ext', weak: false, evidence: {sources: ['WD_EXT'], max_score: 0.876, wd: null, ccip: null}},
    {id: 'c4', name: 'Weak', weak: true, evidence: {sources: ['CCIP'], max_score: 0.1, wd: null, ccip: 0.1}}]}));
  const alts = byClass(who.el, 'dc-alt');
  assert.deepEqual(alts.map(b => b.textContent), ['1Strong', '2Ext扩展', '3Weak']);
  assert.equal(titleOf(alts[0]), 'Strong · WD 0.91 · 外部索引 0.123。点击：换成这个；⌥ 点击：也是');
  assert.equal(titleOf(alts[1]), 'Ext · WD 扩展 0.88。点击：换成这个；⌥ 点击：也是');
  assert.equal(titleOf(alts[2]), 'Weak · 弱线索（中档相似，不算归属） · 外部索引 0.100。点击：换成这个；⌥ 点击：也是');
  assert.ok(alts[2].classList.contains('is-weak'));
  assert.deepEqual(alts.map(b => b.dataset.i), ['0', '1', '2']);
  // WD 主归属不标扩展。
  assert.equal(byClass(byClass(who.el, 'dc-lead')[0], 'dc-ext').length, 0);
});

test('没有扩展证据时不出现扩展的小字；CCIP 长尾写三位小数', () => {
  const who = card.identities();
  who.update(detail({memberships: [machine('c0', 'Old', {sources: ['CCIP', 'PROTO'], max_score: 0.5})]}));
  assert.equal(byClass(who.el, 'dc-ext').length, 0);
  assert.match(byClass(who.el, 'dc-lead')[0].textContent, /机器识别 · 外部索引 \+ 你的标注/);
  who.update(detail({memberships: [machine('c1', 'Rare', {sources: ['CCIP_LONGTAIL'], max_score: 0.1234})]}));
  assert.match(byClass(who.el, 'dc-lead')[0].textContent, /CCIP 长尾 0\.123/);
  assert.equal(byClass(who.el, 'dc-ext').length, 1);
});

test('还没有角色：卡片上是小字「还没认出是谁」和「设为角色…」，大图底栏写「还没有角色」', () => {
  const big = card.identities(), compact = card.identities({compact: true});
  big.update(detail({})); compact.update(detail({}));
  assert.equal(all(big.el).some(x => x.localName === 'h2' && visible(x)), false, '不再用大字当标题');
  assert.match(all(big.el).filter(visible).map(x => x.textContent).join('|'), /还没认出是谁/);
  assert.equal(visible(all(big.el).find(x => x.dataset.act === 'also' && x.classList.contains('btn'))), true);
  assert.match(compact.el.textContent, /还没有角色/);
  compact.update(detail({unknown: true}));
  assert.match(compact.el.textContent, /已跳过，还没有角色/);
});

test('大图底栏：每个角色一行，主角色的 ⇄ 写着 C', () => {
  const who = card.identities({compact: true});
  who.update(detail({memberships: [machine('c0', 'Old', {sources: ['WD']}), machine('c1', 'Other', {sources: ['WD']})]}));
  const rows = byClass(who.el, 'dc-other');
  assert.deepEqual(rows.map(r => r.dataset.id), ['c0', 'c1']);
  const swaps = rows.map(r => all(r).find(x => x.dataset.act === 'swap'));
  assert.deepEqual(swaps.map(titleOf), ['换成…（C）', '换成…']);
});

test('同一份数据再 update 一次不动 DOM；换一张图时卡片和图片节点不重建', () => {
  const c = card.detailCard({act() {}, artist: {verdict() {}, find() {}, findMore() {}}});
  const d = detail({memberships: [machine('c0', 'Old', {sources: ['WD']})], user_tags: ['x'], duplicates: [{uuid: 'b', pos: 1, size: 2, added_at: '2026-11-02', names: []}],
    tags: [{value: 'long_hair', conf: 0.9}], color: '#336699'});
  c.update(d, OPTS);
  const img = c.img, root = c.el, lead = byClass(root, 'dc-lead')[0];
  assert.equal(img.getAttribute('src'), '/image?id=a&size=preview');
  assert.ok(root.classList.contains('tint'), '图的主色写在卡片上（环境光）');
  dom.resetWrites();
  c.update(d, OPTS);
  assert.equal(dom.writes(), 0, '值没变就不写');
  // 下一张：只换变了的部分，节点还是那些。
  c.update({...d, uuid: 'b', filename: 'other.png'}, OPTS);
  assert.equal(c.img, img); assert.equal(byClass(root, 'dc-lead')[0], lead);
  assert.equal(byClass(root, 'dc-file')[0].textContent, 'other.png');
});

test('底部：打开，加上整张图的决定（待确认是排除和保留）', () => {
  const c = card.detailCard({act() {}, artist: {verdict() {}, find() {}, findMore() {}}});
  c.update(detail({pending: true}), OPTS);
  const foot = byClass(c.el, 'dc-foot')[0].children;
  assert.deepEqual(foot.map(b => [b.dataset.act, b.dataset.curate || b.dataset.kind]), [['file', 'open'], ['curate', 'exclude'], ['curate', 'include']]);
  c.update(detail({unknown: true}), {...OPTS, resumeKey: true});
  assert.deepEqual(byClass(c.el, 'dc-foot')[0].children.map(b => b.textContent), ['打开', '恢复到未识别 S', '排除 X']);
});
