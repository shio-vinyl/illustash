// 详情卡片里带「也许是 TA」建议的画师一节（ui/artists.js 的 artistBlock）：它的链接曾读取未定义的 TIPT（#173 改用 t() 时
// 遗留），所以凡是带 d.maybe 的图片都会抛出 ReferenceError，这一节始终画不出来。0.4 起是部件（#709），用 tests/dom.cjs 的真树。
// 英文页面见 tests/english_page.test.cjs。
const {test, before, after} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const dom = require('./dom.cjs');
let artists;
before(async () => { await web.ready(); dom.install(); artists = await web.load('ui/artists.js'); });
after(() => dom.uninstall());
const maybe = {id: 'a1', name: '合成画师', reason: '合成理由', homes: [{url: 'https://example.com/a1', label: 'example', site: 'Example'}]};
const block = d => { const b = artists.artistBlock({verdict() {}, find() {}, findMore() {}}); b.update(d); return b.el; };
const all = el => el.children.flatMap(c => [c, ...all(c)]);
const links = el => all(el).filter(x => x.classList.contains('artist-name'));

test('a maybe artist renders its line and library link in Chinese', () => {
  const el = block({artists: [], posts: [], maybe: [maybe]});
  const [link] = links(el);
  assert.equal(link.parentNode.textContent, '也许是 合成画师');
  assert.equal(link.getAttribute('href'), '#/library?artist=a1');
  assert.equal(link.getAttribute('title'), '在图库里只看 TA 的图');
  assert.match(el.textContent, /还没确定画师/);
  assert.match(el.textContent, /合成理由/);
});

test('a confirmed artist next to a maybe one keeps the same link title', () => {
  const el = block({artists: [{...maybe, id: 'a2', name: '合成画师二', count: 1, confirmed: true}], posts: [], maybe: [maybe]});
  assert.deepEqual(links(el).map(a => a.getAttribute('title')), ['在图库里只看 TA 的图', '在图库里只看 TA 的图']);
  const none = all(el).find(x => x.classList.contains('dc-artist-none'));
  assert.equal(none.hidden, true, '有确定的画师时不再提示查找');
});
