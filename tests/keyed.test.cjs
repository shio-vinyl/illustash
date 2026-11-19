// ui/keyed.js（#704）：按键复用部件，只调换次序和增删；没动的节点不挪；退场中的节点设 inert，动画结束再摘。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

let K, document;
test.before(async () => { await web.ready(); document = dom.install(); K = await web.load('ui/keyed.js'); });
test.after(() => dom.uninstall());

function chips(container, hooks = {}) {
  const made = [], disposed = [];
  const list = K.keyed(container, {
    key: item => item.id,
    create: item => {
      const el = document.createElement('b'); el.textContent = item.name; made.push(item.id);
      return {el, update: next => { el.textContent = next.name; }, dispose: () => disposed.push(item.id)};
    },
    ...hooks,
  });
  return {list, made, disposed};
}
const names = container => container.childNodes.map(n => n.textContent);
const items = text => text.split('').map(c => ({id: c, name: c}));

test('按键复用：内容更新、次序调换，不重建', () => {
  const box = document.createElement('div'), {list, made, disposed} = chips(box);
  list.set(items('abc'));
  const b = box.childNodes[1];
  list.set([{id: 'c', name: 'C'}, {id: 'a', name: 'A'}, {id: 'b', name: 'B'}]);
  assert.deepEqual(names(box), ['C', 'A', 'B']);
  assert.deepEqual(made, ['a', 'b', 'c']);
  assert.equal(box.childNodes[2], b);
  list.set(items('bd'));
  assert.deepEqual(names(box), ['b', 'd']);
  assert.deepEqual(disposed.sort(), ['a', 'c']);
  assert.equal(list.size, 2);
  assert.equal(list.get('d').el, box.childNodes[1]);
  assert.throws(() => list.set(items('aa')), /键重复/);
});

test('只挪最长递增子序列以外的节点：把第一个移到最后只动一个节点', () => {
  const box = document.createElement('div'), {list} = chips(box);
  list.set(items('abcdef'));
  const moved = [];
  const insert = box.insertBefore.bind(box);
  box.insertBefore = (node, before) => { moved.push(node.textContent); return insert(node, before); };
  list.set(items('bcdefa'));
  assert.deepEqual(names(box), 'bcdefa'.split(''));
  assert.deepEqual(moved, ['a']);
  moved.length = 0;
  list.set(items('fbcdea'));
  assert.deepEqual(names(box), 'fbcdea'.split(''));
  assert.deepEqual(moved, ['f']);
  assert.deepEqual([...K.increasing([3, -1, 0, 1, 5, 2])].sort(), [2, 3, 5]);
});

test('enter 第一次不调（appear 时调），move 在序号变了时调', () => {
  const box = document.createElement('div'), log = [];
  const {list} = chips(box, {enter: (el, item) => log.push('enter ' + item.id), move: (el, item, from, to) => log.push(`move ${item.id} ${from}->${to}`)});
  list.set(items('ab'));
  assert.deepEqual(log, []);
  list.set(items('cab'));
  assert.deepEqual(log, ['enter c', 'move a 0->1', 'move b 1->2']);
  const other = document.createElement('div'), seen = [];
  chips(other, {appear: true, enter: (el, item) => seen.push(item.id)}).list.set(items('xy'));
  assert.deepEqual(seen, ['x', 'y']);
});

test('退场：设 inert、留在原处，exit 的 promise 落定后才摘掉和释放；同键回来建新的', async () => {
  const box = document.createElement('div');
  let finish;
  const {list, disposed} = chips(box, {exit: el => new Promise(r => { finish = r; })});
  list.set(items('abc'));
  const b = box.childNodes[1];
  list.set(items('ac'));
  assert.equal(b.parentNode, box);
  assert.equal(b.inert, true);
  assert.deepEqual(names(box), ['a', 'b', 'c']);
  // 退场中又排了一次：退场节点不挡住其余节点的次序。
  list.set(items('cab'));
  assert.deepEqual(names(box).filter((n, i) => !box.childNodes[i].inert), ['c', 'a', 'b']);
  await new Promise(r => setTimeout(r));
  finish();
  await new Promise(r => setTimeout(r));
  assert.equal(b.parentNode, null);
  assert.deepEqual(disposed, ['b']);
  assert.deepEqual(names(box), ['c', 'a', 'b']);
  list.dispose(); list.dispose();
  assert.equal(box.childNodes.length, 0);
  assert.deepEqual(disposed.sort(), ['a', 'b', 'b', 'c']);
});

test('第一次有数据的 set 不播进场（先 set([]) 也一样），之后新来的才播', () => {
  const box = document.createElement('div'), log = [];
  const {list} = chips(box, {enter: (el, item) => log.push(item.id)});
  list.set([]);
  list.set(items('ab'));
  assert.deepEqual(log, []);
  list.set(items('abc'));
  assert.deepEqual(log, ['c']);
});

test('create 中途抛错：这一轮建好的新部件释放掉，列表保持原样', () => {
  const box = document.createElement('div'), disposed = [], updated = [];
  const list = K.keyed(box, {
    key: item => item.id,
    create: item => {
      if (item.id === 'x') throw new Error('bad item');
      const el = document.createElement('b'); el.textContent = item.id;
      return {el, update: next => updated.push(next.id), dispose: () => disposed.push(item.id)};
    },
  });
  list.set(items('ab'));
  assert.throws(() => list.set(items('abcx')), /bad item/);
  assert.deepEqual(disposed, ['c']);
  assert.deepEqual(updated, [], 'reused parts were not touched');
  assert.deepEqual(names(box), ['a', 'b']);
});

test('dispose 立刻释放还在退场的部件；挂在作用域下随它释放', async () => {
  const S = await web.load('ui/scope.js'), parent = S.scope('page'), box = document.createElement('div'), disposed = [];
  const list = K.keyed(box, {
    key: item => item.id, scope: parent, exit: () => new Promise(() => {}),
    create: item => { const el = document.createElement('b'); el.textContent = item.id; return {el, dispose: () => disposed.push(item.id)}; },
  });
  list.set(items('ab'));
  list.set(items('a'));
  await new Promise(r => setTimeout(r));
  assert.deepEqual(disposed, [], 'b is still leaving');
  parent.dispose();
  assert.deepEqual(disposed.sort(), ['a', 'b']);
  assert.equal(box.childNodes.length, 0);
  list.set(items('c'));
  assert.equal(box.childNodes.length, 0, 'a disposed list ignores set');
});

test('被挪的节点里有焦点：挪完放回原处', () => {
  const box = document.createElement('div');
  document.body.appendChild(box);
  const list = K.keyed(box, {
    key: item => item.id,
    create: item => { const el = document.createElement('div'), input = document.createElement('input'); el.appendChild(input); el.input = input; return {el}; },
  });
  list.set(items('abcd'));
  const input = list.get('a').el.input;
  input.focus();
  list.set(items('bcda'));   // 只有 a 被挪
  assert.equal(document.activeElement, input);
  box.remove();
});
