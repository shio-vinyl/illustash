// 图库网格的卡片部件（ui/grid_card.js，#708）：位置、主色、缩略图档位、悬停时的角色名、角上的东西随页面出现、
// 选中和光标、读坏的图、值没变不动 DOM、释放时放掉图。图片和角色都是合成数据。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

let G, S, document;
test.before(async () => {
  await web.ready();
  document = dom.install();
  G = await web.load('ui/grid_card.js');
  S = await web.load('ui/scope.js');
});
test.after(() => dom.uninstall());

const member = (id, name, extra = {}) => ({id, name, status: 'confirmed', variants: [], ...extra});
const item = (extra = {}) => ({uuid: 'u1', width: 600, height: 800, memberships: [member('c1', '合成甲')], color: '#336699', ...extra});
const ctx = (extra = {}) => G.gridContext({route: {}, view: {}, fit: true, cellW: 180, hints: true, hue: () => 0, ...extra});
const box = {x: 12, y: 300, w: 180, h: 240};
const byClass = (root, name) => {
  const out = [], walk = n => { for (const c of n.children) { if (c.classList.contains(name)) out.push(c); walk(c); } };
  walk(root); return out;
};
const one = (root, name) => byClass(root, name)[0];
function card() { const s = S.scope('card'); return {c: G.gridCard(s), s}; }

test('位置写 transform 和宽高，主色写进 --tint，缩略图按实际宽度取档位并登记', () => {
  const {c} = card();
  c.update({item: item(), index: 4, box, ctx: ctx()});
  assert.equal(c.el.style.getPropertyValue('transform'), 'translate(12px, 300px)');
  assert.equal(c.el.style.getPropertyValue('width'), '180px');
  assert.equal(c.el.getAttribute('data-id'), 'u1');
  assert.equal(c.el.getAttribute('data-index'), '4');
  assert.match(c.el.getAttribute('aria-label'), /合成甲，第 5 张/);
  assert.equal(c.el.style.getPropertyValue('--tint'), '#336699');
  const [img] = c.el.querySelectorAll('img');
  assert.match(img.getAttribute('src'), /^\/image\?id=u1&size=/);
  assert.equal(img.getAttribute('data-thumb'), 'u1');
  c.update({item: item(), index: 4, box: {...box, w: 400}, ctx: ctx()});
  assert.match(img.getAttribute('src'), /size=medium/);
});

test('悬停时的角色名：一位写名字；几位写第一位和「另外 N 位」，窄卡只写「N 位」；没有角色不写', () => {
  const {c} = card(), info = () => one(c.el, 'card-info');
  c.update({item: item(), index: 0, box, ctx: ctx()});
  assert.equal(info().children[0].textContent, '合成甲');
  assert.equal(info().children[1].textContent, '');
  assert.ok(!info().hidden);
  const many = item({memberships: [member('c1', '合成甲'), member('c2', '合成乙'), member('c3', '合成丙')]});
  c.update({item: many, index: 0, box, ctx: ctx()});
  assert.equal(info().children[0].textContent, '合成甲');
  assert.equal(info().children[1].textContent, '另外 2 位');
  c.update({item: many, index: 0, box: {...box, w: 100}, ctx: ctx()});
  assert.equal(info().children[0].textContent, '3 位');
  c.update({item: item({memberships: []}), index: 0, box, ctx: ctx()});
  assert.ok(info().hidden);
});

test('角色页的造型芯片、「可能是」按钮、随机抽查的标签：有芯片时不写悬停的名字', () => {
  const {c} = card(), chips = () => one(c.el, 'card-chips').children;
  c.update({item: item({memberships: [member('c1', '合成甲', {variants: [{name: '泳装'}]})]}), index: 0, box, ctx: ctx({route: {char: 'c1'}})});
  assert.deepEqual(chips().map(x => x.textContent), ['泳装']);
  assert.ok(one(c.el, 'card-info').hidden);
  c.update({item: item({memberships: [], hint: {id: 'c9', name: '合成丁'}}), index: 0, box, ctx: ctx()});
  assert.equal(chips()[0].getAttribute('data-hint'), '');
  assert.equal(chips()[0].textContent, '+ 合成丁');
  c.update({item: item({memberships: [], hint: {id: 'c9', name: '合成丁'}}), index: 0, box, ctx: ctx({hints: false})});
  assert.equal(chips().length, 0);
  const audited = item({audit: {id: 'c1', name: '合成甲', answer: null}, memberships: [member('c1', '合成甲'), member('c2', '合成乙', {status: 'machine'})]});
  c.update({item: audited, index: 0, box, ctx: ctx({view: {card: {audit: true}}, cellW: 80})});
  const tags = chips();
  assert.equal(tags.length, 2);
  assert.ok(tags[0].classList.contains('checked'));
  assert.deepEqual(tags[0].querySelectorAll('button').map(b => [b.getAttribute('data-tagact'), b.getAttribute('data-entity')]), [['yes', 'c1'], ['no', 'c1']]);
});

test('芯片的内容没变就不重建', () => {
  const {c} = card(), v = item({memberships: [member('c1', '合成甲', {variants: [{name: '泳装'}]})]}), c1 = ctx({route: {char: 'c1'}});
  c.update({item: v, index: 0, box, ctx: c1});
  const chip = one(c.el, 'card-chips').children[0];
  c.update({item: {...v}, index: 1, box: {...box, y: 600}, ctx: c1});
  assert.equal(one(c.el, 'card-chips').children[0], chip);
});

test('副本角标：重复页写第几份、带组的色调；别的页折叠写 ×N；格子太小时不写', () => {
  const {c} = card(), dup = () => one(c.el, 'dup-badge');
  const copy = item({dup: {grp: 'g', pos: 2, size: 3}});
  c.update({item: copy, index: 7, box, ctx: ctx({view: {card: {dup: true}}, hue: i => i * 30})});
  assert.equal(dup().textContent, '2/3');
  assert.equal(dup().style.getPropertyValue('--dup-hue'), '210');
  assert.ok(!dup().classList.contains('folded'));
  c.update({item: copy, index: 7, box, ctx: ctx()});
  assert.equal(dup().textContent, '×3');
  assert.ok(dup().classList.contains('folded'));
  assert.equal(dup().style.getPropertyValue('--dup-hue'), '');
  c.update({item: copy, index: 7, box, ctx: ctx({cellW: 60})});
  assert.ok(dup().hidden);
});

test('心形：收藏过的是实心；格子太小时只显示实心、不能点', () => {
  const {c} = card(), fav = () => one(c.el, 'fav-mark');
  c.update({item: item({fav: true}), index: 0, box, ctx: ctx()});
  assert.ok(fav().classList.contains('on'));
  assert.equal(fav().getAttribute('aria-pressed'), 'true');
  assert.equal(fav().getAttribute('data-fav'), 'u1');
  assert.equal(fav().getAttribute('title'), '取消收藏（F）');
  c.update({item: item({fav: false}), index: 0, box, ctx: ctx({cellW: 60})});
  assert.ok(fav().hidden);
  c.update({item: item({fav: true}), index: 0, box, ctx: ctx({cellW: 60})});
  assert.ok(!fav().hidden && fav().classList.contains('still'));
  assert.equal(fav().getAttribute('tabindex'), '-1');
});

test('选中、详情栏里那张、键盘光标写在 aria 和 is-cursor 上；值没变不动 DOM', () => {
  const {c} = card();
  c.update({item: item(), index: 0, box, ctx: ctx()});
  c.mark({selected: true, focused: false, cursor: true});
  assert.equal(c.el.getAttribute('aria-pressed'), 'true');
  assert.equal(c.el.getAttribute('aria-current'), null);
  assert.ok(c.el.classList.contains('is-cursor'));
  const before = dom.writes();
  c.mark({selected: true, focused: false, cursor: true});
  c.update({item: item(), index: 0, box, ctx: ctx()});
  assert.equal(dom.writes(), before, '同样的数据再写一遍不动 DOM');
  c.mark({selected: false, focused: true, cursor: false});
  assert.equal(c.el.getAttribute('aria-pressed'), 'false');
  assert.equal(c.el.getAttribute('aria-current'), 'true');
});

test('图读坏了记在根节点上，换一张图再去掉；释放时放掉图', () => {
  const {c, s} = card();
  c.update({item: item(), index: 0, box, ctx: ctx()});
  const [img] = c.el.querySelectorAll('img');
  img.dispatchEvent({type: 'error'});
  assert.ok(c.el.hasAttribute('data-broken'));
  c.update({item: item({uuid: 'u2'}), index: 0, box, ctx: ctx()});
  assert.ok(!c.el.hasAttribute('data-broken'));
  c.dispose();
  assert.equal(img.getAttribute('src'), null);
  assert.equal(img.listenerCount('error'), 0);
  assert.ok(!s.alive);
});

test('原始比例里宽高比超出上下限的图整张显示；方格总是裁满', () => {
  const {c} = card(), wide = item({width: 3000, height: 1000});
  c.update({item: wide, index: 0, box, ctx: ctx()});
  assert.ok(c.el.classList.contains('is-whole'));
  c.update({item: wide, index: 0, box, ctx: ctx({fit: false})});
  assert.ok(!c.el.classList.contains('is-whole'));
  c.update({item: item(), index: 0, box, ctx: ctx()});
  assert.ok(!c.el.classList.contains('is-whole'));
});
