// 设置页各节（ui/settings_<节>.js，#714）的测试辅助：在 tests/dom.cjs 的假 DOM 里建一节、按 core/state.js 的状态更新，
// 读出文字、按 data-row 找行、点按钮。用法：
//   const dom = require('./dom.cjs'); before(async () => { await web.ready(); dom.install(); open = await view('library'); });
//   const v = open(); v.update(); v.text; v.row('manage'); v.press('manage', '导出当前图库'); v.dispose();
const web = require('./web.cjs');
const dom = require('./dom.cjs');

async function view(name) {
  const mod = await web.load(`ui/settings_${name}.js`), S = await web.load('ui/scope.js');
  return () => {
    const box = document.createElement('div'), scope = S.scope('test-' + name), {state} = web.core();
    const part = mod.build(box, scope);
    const v = {
      box, mod,
      update: () => { part.update(state); return v; },
      get text() { return visible(box); },
      row: key => box.querySelector(`[data-row="${key}"]`),
      buttons: key => v.row(key).querySelectorAll('button').filter(b => !b.hidden),
      button: (key, label) => v.buttons(key).find(b => b.textContent === label),
      press: (key, label) => { const b = v.button(key, label); if (!b) throw new Error(`没有「${label}」按钮：${key}`); b.dispatchEvent(dom.event('click')); },
      dispose: () => scope.dispose(),
    };
    return v.update();
  };
}
// 没藏起来的文字（hidden 的节点跳过），各段之间用 ' | ' 隔开。
function visible(node) {
  if (node.nodeType === 3) return node.data.trim();
  if (node.hidden) return '';
  return node.childNodes.map(visible).filter(Boolean).join(' | ');
}
module.exports = {view, visible};
