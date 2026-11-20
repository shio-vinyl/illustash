// 英文页面上的资料库 · 画师（ui/artist_dir.js，#448）：排序、卡片的张数（复数）、副标题和空状态都有译文。
// 中文页面见 tests/artist_directory.test.cjs。画师是合成数据。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
web.language('en');
let dirs;
before(async () => { await web.ready(); dirs = await web.load('ui/artist_dir.js'); });

test('the artist directory reads in English', () => {
  const {state} = web.core();
  Object.assign(state, {route: {page: 'characters', section: 'artists'}, nsfw: ''});
  assert.deepEqual(dirs.ORDERS.map(([, label]) => label), ['Most pictures', 'Recently saved', 'Name']);
  assert.equal(dirs.cardData({id: 'synthetic', name: 'Synthetic Painter', count: 1, last: '2026-11-10 21:03:00', cover: 'u'}).sub, '1 picture');
  assert.equal(dirs.cardData({id: 'synthetic', name: 'Synthetic Painter', count: 1200, last: '', cover: 'u'}).sub, '1,200 pictures');
  Object.assign(dirs.dir, {total: 1});
  assert.equal(dirs.dirHeading(), '1 artist');
  Object.assign(dirs.dir, {total: 4011});
  assert.equal(dirs.dirHeading(), '4,011 artists');
  const empty = dirs.emptyState();
  assert.equal(empty.fact, 'No artists yet');
  assert.equal(empty.next[0].href, '#/settings/artists');
  dirs.dir.q = 'zzz';
  assert.equal(dirs.emptyState().fact, 'No matching artists.');
});
