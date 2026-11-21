// #41：图库卡片说明可分享的导出不含哪些内容（图片、溯源缓存、导入过的 X 关注），
// 以及换到另一台电脑要重新导入 X 归档；导出的每种状态下只说一次。
const {test, before, after} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');
const {view} = require('./settings_view.cjs');
let open;
before(async () => { await web.ready(); dom.install(); open = await view('library'); });
after(() => dom.uninstall());

// 图库那一组在 ui/settings_libraries.js（「设置 › 图库」的第一组）。
function render(exportState) {
  const {state} = web.core();
  Object.assign(state, {route: {page: 'settings'}, importTask: undefined, libraries: {
    info: {name: '合成图库', path: '/tmp/synthetic.pilib', sources: [], test: null},
    current: '/tmp/synthetic.pilib', home: '/tmp', libraries: [], export: exportState}});
  const v = open(), text = v.text;
  v.dispose();
  return text;
}
const notice = '不含图片、画师出处缓存和导入过的 X 关注列表；在新电脑上要回查画师，需要重新导入 X 归档。';
const count = text => text.split(notice).length - 1;

test('the export notice appears once whatever the export state', () => {
  for (const exportState of [{status: 'idle'}, {status: 'running', path: '/tmp/a.zip'}, {status: 'error', error: '合成错误'}]) {
    assert.equal(count(render(exportState)), 1, exportState.status);
  }
});

test('a finished export repeats the notice next to its path instead of below', () => {
  const text = render({status: 'done', path: '/tmp/synthetic.pilib.zip', bytes: 30e6});
  assert.equal(count(text), 1);
  assert.match(text, /已导出：\/tmp\/synthetic\.pilib\.zip（30 MB）。不含图片/);
});
