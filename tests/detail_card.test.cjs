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

// 图的归属只有 {id, name}：同作品重名的两个角色按角色目录写上形态（#849），手动取名的原样写。
test('同作品重名：主角色、其他角色和「也可能是」都写上形态', () => {
  const role = (id, qualifier) => ({id, name: '合成角色', ...(qualifier ? {name_disambiguation: {qualifier, tag: id}} : {})});
  state.charMap = new Map([['g', role('g', '近卫')], ['b', role('b', '基本形态')], ['u', {id: 'u', name: '自取的名字'}]]);
  try {
    const d = detail({memberships: [machine('g', '合成角色', {sources: ['WD']}), machine('u', '自取的名字', {sources: ['WD']})],
      alternatives: [{id: 'b', name: '合成角色', weak: false, evidence: {sources: ['WD'], max_score: 0.8, wd: 0.8, ccip: null}}]});
    const who = card.identities();
    who.update(d);
    assert.equal(all(byClass(who.el, 'dc-name')[0]).find(x => x.localName === 'a').textContent, '合成角色（近卫）');
    assert.deepEqual(byClass(who.el, 'dc-other-name').map(x => x.textContent), ['自取的名字']);
    assert.deepEqual(byClass(who.el, 'dc-alt').map(b => b.textContent), ['1合成角色（基本形态）']);
    assert.match(titleOf(byClass(who.el, 'dc-alt')[0]), /^合成角色（基本形态） · /);
    const compact = card.identities({compact: true});
    compact.update(d);
    assert.deepEqual(byClass(compact.el, 'dc-other-name').map(x => x.textContent), ['合成角色（近卫）', '自取的名字']);
  } finally { state.charMap = new Map(); }
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

// 换成另一张、详情还没读回时（loading 之后）：卡片和画师行显示的还是上一张，改角色、画师的按钮不响应，翻页照常
// （#756 审核：画师的「是 TA」曾记到下一张上）。
test('读取途中只认翻页这类按钮；画师按钮记到正显示的那张', () => {
  const acts = [], verdicts = [];
  const c = card.detailCard({act: (name, data, e, d) => acts.push([name, d.uuid]),
    artist: {verdict: (uuid, id, name, v) => verdicts.push([uuid, id, v]), find() {}, findMore() {}}});
  const d = detail({memberships: [machine('c0', 'Old', {sources: ['WD']})], artists: [{id: 'p1', name: 'Painter', confirmed: false, count: 1}]});
  c.update(d, OPTS);
  const click = el => c.el.dispatchEvent(dom.event('click', {target: el}));
  const swap = all(c.el).find(x => x.dataset.act === 'swap'), next = all(c.el).find(x => x.dataset.act === 'nav' && x.dataset.step === '1');
  const yes = all(c.el).find(x => x.localName === 'button' && x.textContent === '是 TA');
  click(swap); yes.dispatchEvent(dom.event('click'));
  assert.deepEqual(acts, [['swap', 'a']]); assert.deepEqual(verdicts, [['a', 'p1', 'yes']]);
  c.loading('b');
  click(swap); yes.dispatchEvent(dom.event('click'));
  assert.deepEqual(acts, [['swap', 'a']], '读取途中「换一个」不响应');
  assert.deepEqual(verdicts, [['a', 'p1', 'yes']], '读取途中画师按钮不响应');
  click(next);
  assert.deepEqual(acts.at(-1), ['nav', 'b'], '翻页照常，按的是新的那张');
  c.update({...d, uuid: 'b'}, OPTS);
  yes.dispatchEvent(dom.event('click'));
  assert.deepEqual(verdicts.at(-1), ['b', 'p1', 'yes']);
});

// 收藏按钮只由卡片自己写（不带 data-fav，ui/favorites.js 的 paintHearts 不碰它）：收藏写入后 noteFavorite 调 fav(true)，
// 撤销后重读的详情是 fav: false，update 要把它写回去（#756 审核：曾经因为 ui/el.js 的缓存被跳过）。
test('收藏以后撤销：重读的详情把收藏按钮写回没收藏', () => {
  const c = card.detailCard({act() {}, artist: {verdict() {}, find() {}, findMore() {}}});
  const fav = all(c.el).find(x => x.dataset.act === 'fav');
  c.update(detail({fav: false}), OPTS);
  assert.equal(fav.getAttribute('aria-pressed'), 'false');
  assert.equal(fav.hasAttribute('data-fav'), false);
  c.fav(true);
  assert.equal(fav.getAttribute('aria-pressed'), 'true');
  c.update(detail({fav: false}), OPTS);
  assert.equal(fav.getAttribute('aria-pressed'), 'false');
  assert.equal(titleOf(fav), '收藏（F）');
});

test('第一次读回之前不露空架子；读回以后才露出正文', () => {
  const c = card.detailCard({act() {}, artist: {verdict() {}, find() {}, findMore() {}}});
  c.loading('a');
  assert.ok(c.el.classList.contains('is-empty'));
  c.update(detail({}), OPTS);
  assert.ok(!c.el.classList.contains('is-empty'));
});
