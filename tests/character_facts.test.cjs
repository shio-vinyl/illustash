// 运行：node --test tests/character_facts.test.cjs
// 角色页资料区的「常一起出现」「同作品」（ui/character_facts.js，#712）：同作品重名的两个角色在 56px 的窄头像下
// 名字会被截掉，形态短词另起一行（#849）。角色都是合成数据。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

let F, state;
test.before(async () => { await web.ready(); dom.install(); F = await web.load('ui/character_facts.js'); state = web.core().state; });
test.after(() => dom.uninstall());

test('同作品重名：名字一行，形态另起一行；手动取的名字和不重名的角色只有名字', () => {
  const saved = state.charMap;
  state.charMap = new Map([['b', {id: 'b', name: '合成角色', name_disambiguation: {qualifier: '基本形态', tag: 'b'}}]]);
  try {
    const facts = F.charFacts();
    facts.people({kind: 'work', people: [
      {c: {id: 'g', name: '合成角色', cover: 'u1', name_disambiguation: {qualifier: '近卫', tag: 'g'}}, count: 5},
      {c: {id: 'b', name: '合成角色', cover: 'u2'}, count: 3},
      {c: {id: 'u', name: '自取的名字', cover: 'u3'}, count: 2}]});
    const lines = [...facts.el.querySelectorAll('.char-peer')].map(a => a.children.filter(x => x.localName !== 'img').map(x => [x.localName, x.textContent]));
    assert.deepEqual(lines, [[['span', '合成角色'], ['small', '近卫']], [['span', '合成角色'], ['small', '基本形态']], [['span', '自取的名字']]]);
  } finally { state.charMap = saved; }
});
