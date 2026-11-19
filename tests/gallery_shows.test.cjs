// 运行：node --test tests/gallery_shows.test.cjs
// 画廊的五种展示方式和收藏并入画廊（#710）：展示方式的登记表、每个画廊用哪一种、注册表随展示方式给的分行和排序、
// 收藏的新旧地址、活页条落在哪一项、画廊的放进与拿出、预览样图，以及 #646（「更多像这张的」页上没有「保存为画廊」）。
// 数据都是合成的。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
let shows, galleries, pages, route, rail, state;
before(async () => {
  await web.ready();
  shows = await web.load('core/gallery_shows.js');
  galleries = await web.load('core/galleries.js');
  pages = await web.load('core/pages.js');
  route = await web.load('core/route.js');
  rail = await web.load('core/rail.js');
  ({state} = web.core());
});
const reply = (body, status = 200) => ({status, ok: status < 300, headers: {get: () => null}, json: async () => body});
const condition = {id: 1, name: '海边', params: {has: '["tag:beach"]'}, cover_manual: false, position: 0};
const manual = {id: 2, name: '夏天', kind: 'manual', params: {has: '["gallery:2"]'}, cover_manual: false, position: 1};
let calls;
beforeEach(() => {
  shows.setShows({});
  Object.assign(state, {galleries: [condition, manual], nsfw: '', route: {page: 'galleries'}});
  calls = [];
  globalThis.fetch = async (path, init = {}) => { calls.push({path, body: init.body ? JSON.parse(init.body) : null}); return reply({undo: {id: 1, label: '已放进画廊'}}); };
});

test('展示方式的登记表：键不重；显示全部图的给网格的分行，展示页给放几张；没选过时是瀑布', () => {
  const ids = shows.SHOWS.map(s => s.id);
  assert.deepEqual(ids, ['collage', 'magazine', 'masonry', 'hall', 'color']);
  for (const s of shows.SHOWS) {
    assert.equal(typeof s.label(), 'string');
    if (s.all) assert.ok(['masonry', 'justified'].includes(s.grid.kind), s.id);
    else assert.ok(s.take > 0 && s.take <= 24, s.id);
  }
  assert.equal(shows.showFor(5), 'masonry');
  assert.equal(shows.showOf('nope').id, 'masonry', '认不出的按默认');
  assert.deepEqual(shows.SHOWS.filter(s => s.all).map(s => shows.pageFor(s.id)), ['gallery', 'gallery']);
  assert.deepEqual(shows.SHOWS.filter(s => !s.all).map(s => shows.pageFor(s.id)), ['showcase', 'showcase', 'showcase']);
  shows.chooseShow(5, 'hall');
  assert.equal(shows.showFor('5'), 'hall', '按画廊记，id 是数字还是字符串都一样');
  shows.forgetShow(5);
  assert.equal(shows.showFor(5), 'masonry');
});

test('注册表随展示方式：色彩按颜色排、排序下拉只有「按颜色」、等高行；瀑布照图库的排序、瀑布分行', () => {
  const page = pages.PAGES.gallery, r = {page: 'gallery', id: '2', has: ['gallery:2']};
  shows.chooseShow(2, 'color');
  assert.equal(page.params(r).order, 'color');
  assert.equal(page.params(r).has, '["gallery:2"]');
  assert.deepEqual(page.sorts(r), ['color']);
  assert.equal(page.gridLayout(r).kind, 'justified');
  shows.chooseShow(2, 'masonry');
  assert.equal(page.params(r).order, undefined);
  assert.deepEqual(page.sorts(r), ['newest', 'oldest']);
  assert.equal(page.gridLayout(r).kind, 'masonry');
  assert.equal(pages.PAGES.showcase.stage, true);
  assert.equal(pages.PAGES.showcase.panel, true);
});

test('收藏是内置的手选画廊：旧地址 #/library?has=["fav"] 改写到 #/gallery/fav；加了别的条件的不改写', () => {
  globalThis.location = {hash: '#/library?has=' + encodeURIComponent('["fav"]')};
  const r = route.parseHash();
  assert.deepEqual([r.page, r.id, r.legacy, r.has], ['gallery', 'fav', true, ['fav']]);
  assert.equal(route.hashFor({...r, legacy: undefined}).split('?')[0], '#/gallery/fav');
  globalThis.location = {hash: '#/library?has=' + encodeURIComponent('["fav","tag:beach"]')};
  assert.equal(route.parseHash().page, 'library');
  const fav = galleries.galleryOf('fav');
  assert.deepEqual([fav.kind, fav.builtin, fav.params.has], ['manual', true, '["fav"]']);
  assert.ok(galleries.droppable(fav) && galleries.droppable(manual) && !galleries.droppable(condition));
});

test('活页条：收藏在最前；一个画廊的网格页、展示页都选中它；链接按记着的展示方式去网格页或展示页', () => {
  shows.chooseShow(2, 'collage');
  const items = rail.galleryItems({1: 'u1', fav: 'uf'});
  assert.deepEqual(items.map(x => x.key), ['fav', 'g:1', 'g:2']);
  assert.deepEqual(items.map(x => x.href), ['#/gallery/fav', '#/gallery/1', '#/showcase/2']);
  assert.deepEqual(items.map(x => x.cover), ['uf', 'u1', null]);
  assert.equal(items[1].meta, '按条件的画廊');
  assert.equal(items[2].meta, '手选画廊');
  assert.deepEqual(rail.railAt({page: 'gallery', id: 'fav', has: ['fav']}), {entry: 'galleries', item: 'fav'});
  assert.deepEqual(rail.railAt({page: 'showcase', id: '2'}), {entry: 'galleries', item: 'g:2'});
  assert.deepEqual(rail.railAt({page: 'gallery', id: '1', has: ['tag:beach']}), {entry: 'galleries', item: 'g:1'});
});

test('定义画廊的那个条件：手选画廊是 gallery:<id>，收藏是 fav，按条件的没有；不在画廊页上也没有', () => {
  assert.equal(galleries.ownConditionOf({page: 'gallery', id: '2'}), 'gallery:2');
  assert.equal(galleries.ownConditionOf({page: 'gallery', id: 'fav'}), 'fav');
  assert.equal(galleries.ownConditionOf({page: 'gallery', id: '1'}), '');
  assert.equal(galleries.ownConditionOf({page: 'library', id: '2'}), '');
});

test('放进、拿出：手选画廊走 /api/galleries 的 add / remove，收藏走 /api/favorite；选区原样带上', async () => {
  await galleries.addTo('2', {ids: ['a', 'b']});
  await galleries.removeFrom(2, {filter: {has: '["gallery:2"]'}, exclude: ['c'], confirm: true});
  await galleries.addTo('fav', {ids: ['a']});
  await galleries.removeFrom('fav', {ids: ['a']});
  await galleries.createManual('新画廊', {ids: ['a']});
  assert.deepEqual(calls.map(c => [c.path, c.body]), [
    ['/api/galleries', {action: 'add', id: 2, ids: ['a', 'b']}],
    ['/api/galleries', {action: 'remove', id: 2, filter: {has: '["gallery:2"]'}, exclude: ['c'], confirm: true}],
    ['/api/favorite', {ids: ['a']}],
    ['/api/favorite', {ids: ['a'], add: false}],
    ['/api/galleries', {action: 'create', kind: 'manual', name: '新画廊', ids: ['a']}],
  ]);
});

test('列表卡片的预览：一次请求拿样图和张数；手动封面排第一张；条件失效给原因', async () => {
  galleries.forgetCovers();
  globalThis.fetch = async path => {
    calls.push({path});
    if (path.includes('gone')) return reply({error: '作品不存在'}, 400);
    return reply({total: 40, items: [{uuid: 'a', color: '#ff0000'}, {uuid: 'b', color: null}, {uuid: 'c', color: null}]});
  };
  const p = await galleries.previewOf(condition, {nsfw: 'hide', cover: 'c'});
  assert.deepEqual([p.total, p.items.map(x => x.uuid)], [40, ['c', 'a', 'b']]);
  const q = new URLSearchParams(calls[0].path.split('?')[1]);
  assert.deepEqual([q.get('has'), q.get('limit'), q.get('marks'), q.get('nsfw')], ['["tag:beach"]', '5', 'color', 'hide']);
  await galleries.previewOf(condition, {nsfw: 'hide', cover: 'c'});
  assert.equal(calls.length, 1, '同一画廊、同一筛选只读一次');
  const outside = await galleries.previewOf(manual, {cover: 'z'});
  assert.deepEqual(outside.items.map(x => x.uuid), ['z', 'a', 'b', 'c'], '封面不在前几张里也补上');
  assert.deepEqual(await galleries.previewOf({id: 9, params: {has: '["work:gone"]'}}), {error: '作品不存在'});
});

test('#646：「更多像这张的」页上有条件也没有「保存为画廊」；图库上有', async () => {
  web.fakePage();
  globalThis.window = {addEventListener() {}};
  const save = await web.load('ui/gallery_save.js');
  assert.equal(save.saveButton({page: 'library', has: ['fav', 'tag:beach'], like: '["a"]'}), '');
  assert.match(save.saveButton({page: 'library', has: ['tag:beach']}), /id="saveGallery"/);
  assert.equal(save.saveButton({page: 'gallery', id: '1', has: ['tag:beach', 'tag:sea']}), '', '画廊页上换成页头的「更新画廊」');
});
