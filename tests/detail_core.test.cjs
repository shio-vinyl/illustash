// 运行：node --test tests/detail_core.test.cjs
// core/detail.js（#709）：证据文字（#550 的数据包写法）、偶尔一问的节流、回答的内容，以及同一张图的详情只读一次。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');

let detail, state;
test.before(async () => { await web.ready(); detail = await web.load('core/detail.js'); state = web.core().state; });

test('证据：WD、外部索引、你的标注和数据包各自的写法', () => {
  assert.equal(detail.evidence({sources: ['WD_EXT'], max_score: 0.9312}), 'WD 扩展 0.93');
  assert.equal(detail.evidence({sources: ['CCIP_LONGTAIL'], max_score: 0.1234}), 'CCIP 长尾 0.123');
  assert.equal(detail.evidence({sources: ['CCIP', 'PROTO'], max_score: 0.5}), '外部索引 + 你的标注');
  assert.equal(detail.evidence({sources: ['CCIP', 'WD'], wd: 0.91, ccip: 0.123}), 'WD 0.91 · 外部索引 0.123');
  assert.equal(detail.evidence({sources: ['PROTO'], wd: null, ccip: 0.2}), '你的标注 0.200');
  assert.equal(detail.fromExt({sources: ['WD_EXT', 'CCIP']}), false, '和别的来源一起给出的不算只由扩展给出');
  assert.equal(detail.fromExt({sources: ['WD_EXT']}), true);
  assert.equal(detail.howFound({status: 'machine'}), '机器识别');
  assert.equal(detail.howFound({status: 'confirmed', origin: 'album'}), '相簿');
});

test('偶尔一问：浏览时隔三张问一次，同一张图再画时结论不变，被略过三次后不再问', () => {
  state.route = {page: 'library'};
  detail.resetAsk();
  const d = {question: {entity: 'c', kind: 'audit', name: 'X'}};
  const seen = ['a', 'b', 'c', 'd', 'e'].map(id => detail.shouldAsk(d, id, true));
  assert.deepEqual(seen, [true, false, false, false, true]);
  assert.equal(detail.shouldAsk(d, 'e', true), true, '同一张再画');
  assert.equal(detail.shouldAsk(d, 'f', false), false, '视图选项关了');
  detail.resetAsk();
  // 每次问了都没答（换到下一张就算略过一次），略过三次后本次访问不再问。
  for (const id of ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l']) detail.shouldAsk(d, id, true);
  assert.equal(detail.shouldAsk(d, 'z', true), false);
});

test('回答的内容：是、不是、是对手', () => {
  const cur = {q: {entity: 'c1', kind: 'ambiguous', rival: 'c2'}, ident: 'a'};
  assert.deepEqual(detail.askBody(cur, 'yes'), {ids: ['a'], entity: 'c1', kind: 'ambiguous', yes: ['a']});
  assert.deepEqual(detail.askBody(cur, 'no'), {ids: ['a'], entity: 'c1', kind: 'ambiguous', no: ['a']});
  assert.deepEqual(detail.askBody(cur, 'switch'), {ids: ['a'], entity: 'c1', kind: 'ambiguous', no: ['a'], switch: 'c2'});
});

test('同一张图、同一读取纪元里，详情卡片和大图底栏同时要只发一个请求；写入之后重新读', async () => {
  let calls = 0;
  globalThis.fetch = async () => { calls++; return {status: 200, ok: true, headers: {get: () => null}, json: async () => ({uuid: 'a'})}; };
  const [x, y] = await Promise.all([detail.readDetail('a'), detail.readDetail('a')]);
  assert.equal(calls, 1); assert.equal(x, y);
  state.readEpoch++;
  await detail.readDetail('a');
  assert.equal(calls, 2);
});
