// ui/rows.js（#704、#676）：配置表声明的几种行画一次、按状态更新；轮询刷新不重建节点，焦点和打了一半的字不丢。
const test = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');

let R, S, document;
test.before(async () => { await web.ready(); document = dom.install(); R = await web.load('ui/rows.js'); S = await web.load('ui/scope.js'); });
test.after(() => dom.uninstall());

function settings(log) {
  return [
    {kind: 'switch', key: 'updates', label: '自动检查更新', hint: s => `每天一次 · ${s.when}`, get: s => s.updates, set: on => log.push(['updates', on]), disabled: s => s.busy},
    {kind: 'action', key: 'check', label: '检查新角色', button: '检查…', run: s => log.push(['check', s.busy]), hidden: s => !s.canCheck},
    {kind: 'info', key: 'version', label: '版本', value: s => s.version},
    {kind: 'select', key: 'lang', label: '界面语言', options: s => s.languages, get: s => s.lang, set: v => log.push(['lang', v])},
    {kind: 'field', key: 'mirror', label: '镜像地址', placeholder: 'https://…', get: s => s.mirror, set: v => log.push(['mirror', v])},
    {kind: 'list', key: 'libs', label: '图库', items: s => s.libraries, itemKey: x => x.id, itemText: x => x.name, itemHint: x => x.path,
     actions: [{label: '移除', run: x => log.push(['remove', x.id]), hidden: x => x.current}]},
  ];
}
const state = (extra = {}) => ({updates: true, when: '09:00', busy: false, canCheck: true, version: '0.4.0', lang: 'en', mirror: 'https://a',
  languages: [{value: 'zh-CN', label: '简体中文'}, {value: 'en', label: 'English'}],
  libraries: [{id: 1, name: '图库一', path: '/a', current: true}, {id: 2, name: '图库二', path: '', current: false}], ...extra});

test('六种行画一次，按状态写好标题、说明、控件', () => {
  const box = document.createElement('div'), log = [], panel = R.rows(box, settings(log));
  panel.update(state());
  assert.equal(box.childNodes.length, 6);
  const sw = panel.row('updates');
  assert.equal(sw.localName, 'label');
  assert.equal(sw.querySelector('b').textContent, '自动检查更新');
  assert.equal(sw.querySelector('small').textContent, '每天一次 · 09:00');
  assert.equal(sw.querySelector('input').checked, true);
  assert.equal(panel.row('check').querySelector('button').textContent, '检查…');
  assert.equal(panel.row('version').childNodes.at(-1).textContent, '0.4.0');
  const select = panel.row('lang').querySelector('select');
  assert.deepEqual(select.children.map(o => [o.value, o.textContent]), [['zh-CN', '简体中文'], ['en', 'English']]);
  assert.equal(select.value, 'en');
  const field = panel.row('mirror').querySelector('input');
  assert.deepEqual([field.getAttribute('type'), field.getAttribute('placeholder'), field.value], ['text', 'https://…', 'https://a']);
  const items = panel.row('libs').childNodes.at(-1).children;
  assert.equal(items.length, 2);
  assert.equal(items[0].querySelector('button').hidden, true, 'the current library cannot be removed');
  assert.equal(items[1].querySelector('small').hidden, true, 'no path, no hint');
  panel.dispose();
  assert.equal(box.childNodes.length, 0);
});

test('控件的改动交给 set / run，带上最近一次的状态', () => {
  const box = document.createElement('div'), log = [], panel = R.rows(box, settings(log));
  panel.update(state({busy: true}));
  const input = panel.row('updates').querySelector('input');
  assert.equal(input.disabled, true);
  panel.update(state());
  input.checked = false; input.dispatchEvent(dom.event('change'));
  panel.row('check').querySelector('button').dispatchEvent(dom.event('click'));
  const select = panel.row('lang').querySelector('select');
  select.value = 'zh-CN'; select.dispatchEvent(dom.event('change'));
  const field = panel.row('mirror').querySelector('input');
  field.value = 'https://b'; field.dispatchEvent(dom.event('change'));
  panel.row('libs').childNodes.at(-1).children[1].querySelector('button').dispatchEvent(dom.event('click'));
  assert.deepEqual(log, [['updates', false], ['check', false], ['lang', 'zh-CN'], ['mirror', 'https://b'], ['remove', 2]]);
  panel.update(state({canCheck: false}));
  assert.equal(panel.row('check').hidden, true);
  panel.dispose();
});

test('轮询刷新：节点不重建，焦点在输入框上时打了一半的字不被冲掉', () => {
  const box = document.createElement('div'), panel = R.rows(box, settings([]));
  panel.update(state());
  const field = panel.row('mirror').querySelector('input'), rowsBefore = [...box.childNodes];
  field.focus(); field.value = 'https://half';
  panel.update(state({version: '0.4.1'}));
  assert.equal(field.value, 'https://half');
  assert.deepEqual(box.childNodes, rowsBefore);
  assert.equal(panel.row('version').childNodes.at(-1).textContent, '0.4.1');
  field.blur();
  panel.update(state());
  assert.equal(field.value, 'https://a');
  // 列表按键复用：少了一项只摘那一项。
  const keep = panel.row('libs').childNodes.at(-1).children[0];
  panel.update(state({libraries: [{id: 1, name: '图库一', path: '/a', current: true}]}));
  assert.deepEqual(panel.row('libs').childNodes.at(-1).children, [keep]);
  panel.dispose();
});

test('挂在作用域下：父作用域释放时摘掉全部行和监听', () => {
  const parent = S.scope('settings'), box = document.createElement('div'), panel = R.rows(box, settings([]), {scope: parent});
  panel.update(state());
  const input = panel.row('updates').querySelector('input');
  parent.dispose();
  assert.equal(box.childNodes.length, 0);
  assert.equal(input.listenerCount('change'), 0);
  assert.throws(() => R.rows(box, [{kind: 'table', key: 'x'}]), /没有 table 这种行/);
});
