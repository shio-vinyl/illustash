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

test('离开输入框后、新状态回来之前来一次轮询：打的字不被冲回旧值（#469）', async () => {
  const box = document.createElement('div');
  let resolve, saved = [];
  const panel = R.rows(box, [
    {kind: 'field', key: 'mirror', label: '镜像地址', get: s => s.mirror, set: v => new Promise(r => { saved.push(v); resolve = r; })},
    {kind: 'switch', key: 'updates', label: '自动检查更新', get: s => s.updates, set: () => {}},
  ]);
  panel.update({mirror: 'https://a', updates: false});
  const field = panel.row('mirror').querySelector('input');
  field.focus(); field.value = 'https://new'; field.blur(); field.dispatchEvent(dom.event('change'));
  assert.deepEqual(saved, ['https://new']);
  panel.update({mirror: 'https://a', updates: false});      // 轮询带回的还是旧值
  assert.equal(field.value, 'https://new');
  panel.update({mirror: 'https://new', updates: false});    // 存好了：新状态回来，清掉待提交的值
  resolve();
  await new Promise(r => setTimeout(r));
  panel.update({mirror: 'https://other', updates: false});  // 之后照常跟 state 走
  assert.equal(field.value, 'https://other');
  // 保存失败：promise 落定以后照 state 写回原值。
  field.value = 'https://bad'; field.dispatchEvent(dom.event('change'));
  panel.update({mirror: 'https://other', updates: false});
  assert.equal(field.value, 'https://bad');
  resolve(); await new Promise(r => setTimeout(r));
  panel.update({mirror: 'https://other', updates: false});
  assert.equal(field.value, 'https://other');
  // 开关的 set 不返回 promise：当作同步存好，下一次 update 照 state 写。
  const sw = panel.row('updates').querySelector('input');
  sw.checked = true; sw.dispatchEvent(dom.event('change'));
  panel.update({mirror: 'https://other', updates: false});
  assert.equal(sw.checked, false);
  panel.dispose();
});

test('配置表先整张校验：写错 kind 时容器里什么都没插', () => {
  const box = document.createElement('div');
  assert.throws(() => R.rows(box, [{kind: 'info', key: 'a', label: 'A', value: () => 1}, {kind: 'table', key: 'x'}]), /没有 table 这种行/);
  assert.equal(box.childNodes.length, 0);
});

test('设置页的选项（#714）：分段、一行几个按钮、圆点、进度条、链接、数字范围、链接项', async () => {
  const box = document.createElement('div'), log = [];
  let finish;
  const panel = R.rows(box, [
    {kind: 'select', key: 'theme', seg: true, label: '外观', options: [{value: '', label: '跟随系统'}, {value: 'dark', label: '深色'}],
     get: s => s.theme, set: v => { log.push(['theme', v]); return new Promise(() => {}); }},   // 还没存好
    {kind: 'action', key: 'models', label: s => s.line, dot: s => s.dot, progress: s => s.bar, link: s => s.manual,
     buttons: s => s.buttons, run: (s, act) => { log.push(['act', act]); return new Promise(r => { finish = r; }); }},
    {kind: 'action', key: 'works', label: '关注的作品', href: '#/characters/works', value: s => `${s.works} 部 ›`},
    {kind: 'field', key: 'min', label: '只提示图片较多的角色', type: 'number', min: 20, max: 5000, step: 10, unit: '张', get: s => s.min, set: v => log.push(['min', v])},
    {kind: 'list', key: 'history', label: '最近添加', items: s => s.history, itemKey: x => x.tag, itemText: x => x.name, itemHref: x => x.href || null,
     actions: [{label: (x, s) => s.armed === x.tag ? '再点一次确认移除' : '移除', run: x => log.push(['drop', x.tag])}]},
  ]);
  const s = {theme: 'dark', line: '正在下载', dot: 'busy', bar: {done: 1, total: 4}, manual: {href: 'https://x', label: '手动下载说明'},
    buttons: [{act: 'cancel', label: '取消'}], works: 3, min: 200, armed: null,
    history: [{tag: 'a', name: '甲', href: '#/c/1'}, {tag: 'b', name: '乙'}]};
  panel.update(s);
  const seg = panel.row('theme').querySelector('.seg');
  assert.deepEqual(seg.children.map(b => [b.textContent, b.getAttribute('aria-pressed')]), [['跟随系统', 'false'], ['深色', 'true']]);
  seg.children[0].dispatchEvent(dom.event('click'));
  assert.deepEqual(log.at(-1), ['theme', '']);
  assert.equal(seg.children[0].getAttribute('aria-pressed'), 'true', 'the pressed segment shows at once, before the state comes back');
  panel.update(s);
  assert.equal(seg.children[0].getAttribute('aria-pressed'), 'true', 'a poll with the old value does not move it back');
  const models = panel.row('models');
  assert.equal(models.querySelector('.row-dot').getAttribute('data-dot'), 'busy');
  assert.equal(models.querySelector('.row-bar').getAttribute('aria-valuenow'), '1');
  assert.equal(models.querySelector('.row-bar').firstChild.style.width, '25.0%');
  assert.equal(models.querySelector('.row-link').getAttribute('href'), 'https://x');
  const cancel = models.querySelector('button');
  cancel.dispatchEvent(dom.event('click')); cancel.dispatchEvent(dom.event('click'));
  assert.deepEqual(log.filter(x => x[0] === 'act'), [['act', 'cancel']], 'a pressed button waits for its run to settle');
  assert.equal(cancel.disabled, true);
  finish(); await new Promise(r => setTimeout(r));
  assert.equal(cancel.disabled, false);
  panel.update({...s, bar: null, dot: '', buttons: [{act: 'download', label: '下载', primary: true}, {act: 'import', label: '从文件导入…', disabled: true, title: '不行'}]});
  const [download, imp] = models.querySelectorAll('button');
  assert.equal(models.querySelector('.row-bar').hidden, true);
  assert.equal(download.classList.contains('is-primary'), true);
  assert.deepEqual([imp.disabled, imp.getAttribute('title')], [true, '不行']);
  const works = panel.row('works');
  assert.equal(works.querySelector('a').getAttribute('href'), '#/characters/works');
  assert.equal(works.querySelector('.row-value').textContent, '3 部 ›');
  const min = panel.row('min').querySelector('input');
  assert.deepEqual(['type', 'min', 'max', 'step'].map(k => min.getAttribute(k)), ['number', '20', '5000', '10']);
  assert.equal(panel.row('min').querySelector('.row-unit').textContent, '张');
  const items = panel.row('history').querySelector('.row-items').children;
  assert.equal(items[0].querySelector('a').getAttribute('href'), '#/c/1');
  assert.equal(items[1].querySelector('a').hidden, true);
  panel.update({...s, armed: 'b'});
  assert.equal(items[1].querySelector('button').textContent, '再点一次确认移除');
  panel.dispose();
});
