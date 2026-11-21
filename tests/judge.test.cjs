// 判断卡（ui/judge.js，#787）自己的配置表：when、disabled、primary、quiet、title、href、status、tip、max 和 rest、busy、onOff、shrink。
// 认一认、按组整理、可能认错三页的测试只间接用到其中几项，这里逐项测。假 DOM 是 tests/dom.cjs。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

let J, S;
before(async () => {
  await web.ready();
  dom.install();
  Object.assign(globalThis, {matchMedia: () => ({matches: true, addEventListener() {}}), requestAnimationFrame: cb => setTimeout(() => cb(0), 0)});
  J = await web.load('ui/judge.js'); S = await web.load('ui/scope.js');
});
const pics = n => Array.from({length: n}, (_, i) => ({uuid: 'p' + i, width: 300, height: 200}));
const btn = (j, key) => j.acts.querySelectorAll('[data-ref]').find(b => b.getAttribute('data-ref') === key);
const tiles = j => j.el.querySelectorAll('[data-peek]').filter(x => !x.inert);
const press = (j, uuid) => { const t = tiles(j).find(x => x.dataset.peek === uuid); t.parentNode.dispatchEvent(dom.event('click', {target: t})); };
function card(extra = {}) {
  const runs = [];
  const j = J.judgeCard({scope: S.scope('judge-test'), tip: {off: '点一下：不算', on: () => '点一下：算回'}, status: ({k}) => k ? `${k} 张点掉了` : '', ...extra,
    actions: extra.actions || [
      {key: 'yes', label: ({k, kept}) => k ? `其余 ${kept.length} 张` : '都是', primary: true, disabled: ({kept}) => !kept.length, run: off => runs.push(['yes', [...off]])},
      {key: 'other', label: '点掉的是…', when: ({k}) => k > 0, title: ({k}) => `${k} 张`, run: off => runs.push(['other', [...off]])},
      {key: 'later', label: '以后再说', quiet: true, run: () => runs.push(['later'])},
      {key: 'open', label: '逐张看', quiet: true, href: () => '#/g/1'},
    ]});
  return {j, runs};
}

test('按钮：文字、when、disabled、primary/quiet、title 随点掉的张数变；run 收到点掉的集合的拷贝', () => {
  const {j, runs} = card();
  j.update(pics(3));
  assert.equal(btn(j, 'yes').textContent, '都是'); assert.ok(btn(j, 'yes').classList.contains('is-primary'));
  assert.equal(btn(j, 'other').hidden, true); assert.ok(btn(j, 'later').classList.contains('is-quiet'));
  assert.equal(j.acts.querySelector('[role="status"]').textContent, '');
  press(j, 'p1');
  assert.equal(btn(j, 'yes').textContent, '其余 2 张'); assert.equal(btn(j, 'other').hidden, false); assert.equal(btn(j, 'other').getAttribute('title'), '1 张');
  assert.equal(j.acts.querySelector('[role="status"]').textContent, '1 张点掉了');
  btn(j, 'other').dispatchEvent(dom.event('click'));
  j.off.add('zzz');   // run 拿到的是拷贝：之后改部件里的集合不影响已经交出去的
  assert.deepEqual(runs, [['other', ['p1']]]);
  press(j, 'p0'); press(j, 'p2');
  assert.equal(btn(j, 'yes').disabled, true, '全部点掉：没有「其余」可写');
  j.dispose();
});

test('图：提示可以是函数；点一下切换 aria-pressed，onOff 收到同一个集合；href 的按钮是链接、不调 run', () => {
  const seen = [];
  const {j, runs} = card({onOff: off => seen.push([...off])});
  j.update(pics(2));
  const [a] = tiles(j);
  assert.match(a.getAttribute('title'), /^点一下：不算 · /);
  press(j, 'p0');
  assert.equal(a.getAttribute('aria-pressed'), 'true'); assert.match(a.getAttribute('title'), /^点一下：算回 · /);
  press(j, 'p0');
  assert.equal(a.getAttribute('aria-pressed'), 'false');
  assert.deepEqual(seen, [['p0'], []]);
  assert.equal(btn(j, 'open').localName, 'a'); assert.equal(btn(j, 'open').getAttribute('href'), '#/g/1');
  btn(j, 'open').dispatchEvent(dom.event('click'));
  assert.deepEqual(runs, []);
  j.dispose();
});

test('max 和 rest：只画前 max 张，超出的收成「+N」链接；n、kept 仍按全部算', () => {
  const {j} = card({max: 4});
  j.update(pics(10), {rest: '#/g/1'});
  assert.equal(tiles(j).length, 4);
  const more = j.el.querySelectorAll('a').find(a => a.getAttribute('href') === '#/g/1');
  assert.equal(more.textContent, '+6');
  press(j, 'p0');
  assert.equal(btn(j, 'yes').textContent, '其余 9 张');
  j.update(pics(3), {rest: '#/g/1'});
  assert.equal(j.el.querySelectorAll('a').length, 0, '不超出就没有「+N」');
  j.dispose();
});

test('update 给的 off 原样沿用（重读以后点掉的不复位）；不给就是空的', () => {
  const {j} = card();
  const off = new Set(['p1']);
  j.update(pics(3), {off});
  assert.equal(j.off, off); assert.equal(btn(j, 'yes').textContent, '其余 2 张');
  j.update(pics(3));
  assert.equal(j.off.size, 0);
  j.dispose();
});

test('busy：按钮和图都点不动，点了也不切换、不调 run；放开以后照常', () => {
  const {j, runs} = card();
  j.update(pics(2));
  j.busy(true);
  assert.equal(btn(j, 'later').disabled, true); assert.ok(tiles(j).every(x => x.disabled));
  press(j, 'p0');
  btn(j, 'later').dispatchEvent(dom.event('click'));
  assert.equal(j.off.size, 0); assert.deepEqual(runs, []);
  j.busy(false);
  press(j, 'p0');
  assert.equal(j.off.size, 1); assert.equal(btn(j, 'later').disabled, false);
  j.dispose();
});
