// 角色页的横幅和资料（#712）：core/character.js 挑横幅图、同作品的角色、请求参数和缓存。角色和图片都是合成数据。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');

let C, S;
test.before(async () => {
  await web.ready();
  C = await web.load('core/character.js');
  S = (await web.load('core/state.js')).state;
});

const item = (uuid, width, height, color = null) => ({uuid, width, height, color});

test('横幅：最近的图里第一张够宽的，都不够宽就用最近的一张，没有图是 null', () => {
  assert.equal(C.pickBanner([item('a', 600, 800), item('b', 1600, 900), item('c', 2000, 900)]).uuid, 'b');
  assert.equal(C.pickBanner([item('a', 600, 800), item('b', 800, 800)]).uuid, 'a');
  assert.equal(C.pickBanner([]), null);
});

test('同作品：作品相同、不是自己、不是只当标签的、有图的，按张数排，最多 6 个', () => {
  const me = {id: 'me', work_id: 'w', count: 5};
  const chars = [me, {id: 'x', work_id: 'w', count: 3, name: '合成乙'}, {id: 'y', work_id: 'w', count: 9, name: '合成甲'},
    {id: 'z', work_id: 'other', count: 99}, {id: 'm', work_id: 'w', count: 50, mascot: true}, {id: 'e', work_id: 'w', count: 0},
    ...Array.from({length: 8}, (_, i) => ({id: 'n' + i, work_id: 'w', count: 1, name: '合成' + i}))];
  const peers = C.peersOf(me, chars);
  assert.deepEqual(peers.slice(0, 2).map(c => c.id), ['y', 'x']);
  assert.equal(peers.length, C.PEERS);
  assert.ok(!peers.some(c => ['me', 'z', 'm', 'e'].includes(c.id)));
  assert.deepEqual(C.peersOf({id: 'o', kind: 'oc', work_id: 'w'}, chars), []);
  assert.deepEqual(C.peersOf({id: 'u'}, chars), []);
});

test('读取：图、统计和动态各请求一次，同一角色和数据版本再读走缓存，数据版本变了重读', async () => {
  const seen = [];
  globalThis.fetch = async path => {
    seen.push(String(path).split('?')[0]);
    const body = path.startsWith('/api/assets') ? {items: [item('a', 600, 800, '#123456'), item('b', 1800, 900, '#654321'), item('c', 700, 700), item('d', 500, 500)]}
      : path.startsWith('/api/character/stats') ? {count: 4, first: '2026-01-02', months: [{month: '2026-10', count: 1}, {month: '2026-11', count: 3}]}
      : {me: {recent: 2, last: '2026-11-19'}};
    return {ok: true, status: 200, json: async () => body};
  };
  S.chars = []; S.nsfw = ''; S.readEpoch = 1;
  C.forgetCharacter();
  const data = await C.loadCharacter({id: 'me', count: 4});
  assert.equal(data.banner.uuid, 'b');
  assert.deepEqual(data.recent.map(x => x.uuid), ['a', 'b', 'c']);
  assert.equal(data.months.length, 2);
  assert.equal(data.recent30, 2);
  assert.deepEqual(seen.sort(), ['/api/assets', '/api/character/activity', '/api/character/stats']);
  await C.loadCharacter({id: 'me', count: 4});
  assert.equal(seen.length, 3);
  S.readEpoch = 2;
  await C.loadCharacter({id: 'me', count: 4});
  assert.ok(seen.length > 3);
});
