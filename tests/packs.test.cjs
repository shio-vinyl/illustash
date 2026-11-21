// 运行：node --test tests/packs.test.cjs
// 角色资料包（#602，合并了 #550 WD 角色扩展和 #559 CCIP 长尾包）：「设置 › 角色资料」一张卡片的各个状态、确认框、任务面板
// 的一项、结束时的提示和「有新版本」（ui/packs.js），状态的读取、POST 和轮询（core/packs.js）。回复是照
// native/terms/src/http_host_get.rs 的 `packs` 写的桩，数字都是合成的。
const {test, before, beforeEach} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const c = {};
before(async () => {
  await web.ready();
  Object.assign(c, await web.load('ui/packs.js'), await web.load('core/packs.js'), {state: web.core().state, session: web.core().session});
});

const part = (fields = {}) => ({format: 'illustash-wd-extension/1', heads: 228, candidate_only: 0, license: 'Apache-2.0',
  file: {path: 'wd-extension-v1.bin', size: 940701, sha256: 'ab'.repeat(32)}, ...fields});
const cur = (fields = {}) => ({version: '0.1', stage: 'preview', revision: 1, heads: 282, candidate_only: 0, license: 'Apache-2.0 AND MIT',
  builtin: true, note: null, size: 2214393, status: 'missing', have: 0,
  components: {wd: part(), ccip: part({format: 'illustash-557-ccip-offline/1', heads: 54, license: 'MIT',
    file: {path: 'A-longtail-p75.zip', size: 1273692, sha256: 'd3'.repeat(32)}})}, ...fields});
const lib = (fields = {}) => ({enabled: false, version: null, wd: false, ccip: false, prompt: null, images: 0, assigned: 0, vectors: 120,
  checked_at: null, blocked: null, ...fields});
const pack = ({current: p = {}, library: l = {}, download = null, task = null, offer = null, ccip = {}} = {}) => ({
  catalog: {configured: true, serial: null, checked_at: null, error: null}, current: cur(p), offer, download, library: lib(l), task,
  location: '/Users/synthetic/Library/Application Support/Illustash/Shared/extensions',
  wd: {package: {manual: 'https://github.com/shio-vinyl/illustash/blob/main/docs/INSTALL.md#角色资料包',
    location: '/Users/synthetic/Library/Application Support/Illustash/Shared/extensions/wd-extension-v1'}},
  ccip: {library: {needs_scan: false, error: null, ...ccip}}});
const READY = {status: 'ready', have: 2214393};
const ENABLED = {enabled: true, version: '0.1', wd: true, ccip: true, images: 7, assigned: 3, checked_at: '2026-11-16 08:00:00'};
// 按钮是数据（packButtons、packOfferInfo，#714），设置卡片和任务面板用的是同一份：换成好比的形状。
const B = list => (list || []).map(b => ({act: b.act, label: b.label, cls: `${b.primary ? 'primary' : 'quiet'} small`, attrs: b.disabled ? ` disabled title="${b.title}"` : ''}));
const buttons = B;
const labels = list => B(list).map(b => b.label);
const offer = x => B(c.packOfferInfo(x)?.buttons);
const info = x => c.packInfoItems(x).map(i => `${i.text}: ${i.hint}`).join('\n');
const results = x => c.packResultItems(x).map(i => i.hint ? `${i.text} · ${i.hint}` : i.text).join('\n');
beforeEach(() => { c.state.pack = null; c.session.token = 'library-a'; c.session.stale = false; });

test('没下载：主按钮写明两个文件合起来的大小，可以从文件导入，有手动下载说明', () => {
  const x = pack();
  assert.equal(c.packLine(x), '角色资料包还没下载（约 2.2 MB）');
  assert.equal(c.packDot(x), 'off');
  const b = buttons(c.packButtons(x));
  assert.deepEqual(b.map(x => [x.act, x.label]), [['download', '下载角色资料包（约 2.2 MB）'], ['import', '从文件导入…']]);
  assert.match(b[0].cls, /primary/);
  assert.deepEqual(c.packManual(x), {href: x.wd.package.manual, label: '手动下载说明'});
  assert.equal(c.packHasTask(x), false);
  assert.equal(c.packProgress(x), null);
  assert.deepEqual(labels(c.packButtons(x, true)), ['下载角色资料包（约 2.2 MB）'], '任务面板只放主按钮');
  assert.deepEqual(c.packDeleteButtons(x), []);
  assert.equal(c.packNote(x), '多认 282 个 WD 词表和角色索引里没有的角色。');
});

test('下了一部分：继续下载，未完成的部分可以删除', () => {
  const x = pack({current: {status: 'partial', have: 1214393}});
  assert.equal(c.packLine(x), '角色资料包还没下载完（已有 1.2 MB，共 2.2 MB）');
  assert.deepEqual(labels(c.packButtons(x)), ['继续下载（还差 1.0 MB）', '从文件导入…', '删除未完成的部分']);
  // 两个文件里有一个已经在本机（和别的版本共用），另一个还没开始下：不算下了一部分。
  const shared = pack({current: {status: 'missing', have: 1273692}});
  assert.equal(c.packLine(shared), '角色资料包还没下载（约 941 KB）');
  assert.deepEqual(labels(c.packButtons(shared)), ['下载角色资料包（约 941 KB）', '从文件导入…']);
});

test('下载中：进度、速度、来源、代理和取消；失败时写原因和每个来源的失败', () => {
  const x = pack({download: {kind: 'download', status: 'running', done: 1000000, total: 2214393, speed: 500000, source: 'github.com', proxy: 'proxy:7890'}});
  assert.equal(c.packLine(x), '正在下载 1.0 MB / 2.2 MB · 500 KB/秒 · 来自 github.com · 经代理 proxy:7890');
  assert.equal(c.packDot(x), 'busy');
  assert.deepEqual(c.packProgress(x), {done: 1000000, total: 2214393});
  assert.deepEqual(buttons(c.packButtons(x)).map(b => b.act), ['package-cancel']);
  // 首次准备时先读包目录（#630）：没有进度和取消按钮，照常算下载中（页面接着读）。
  const catalog = pack({download: {kind: 'catalog', status: 'running', done: 0, total: 0}});
  assert.equal(c.packLine(catalog), '正在读取角色资料包目录');
  assert.equal(c.packDot(catalog), 'busy');
  assert.equal(c.packProgress(catalog), null);
  assert.deepEqual(c.packButtons(catalog), []);
  const failed = pack({download: {kind: 'download', status: 'failed', message: '连接被重置', failures: [{source: 'github.com', why: '超时'}]}});
  assert.equal(c.packLine(failed), '角色资料包没能下载：连接被重置');
  assert.equal(c.packDot(failed), 'warn');
  assert.match(results(failed), /github.com：超时/);
  assert.equal(labels(c.packButtons(failed))[0], '重试');
  assert.equal(c.packLine(pack({download: {kind: 'download', status: 'cancelled'}})), '已取消');
});

test('坏 ZIP 导入后，卡片和任务都写出文件名和原因', () => {
  const download = {kind: 'import', status: 'failed', message: 'bad.zip 不是有效的 ZIP 文件'};
  const message = '角色资料包没能导入：bad.zip 不是有效的 ZIP 文件';
  for (const current of [{}, READY]) {
    const x = pack({current, download});
    assert.equal(c.packDot(x), 'warn');
    assert.equal(c.packLine(x), message);
    assert.equal(c.packTask(x).line, message);
    assert.equal(c.packTask(x).warn, true);
    assert.equal(c.downloadText(download, pack(), x), message);
  }
});

test('已下载、本图库没启用：已有识别结果的图库提示一次，可以启用或不用；新图库第一次同步时启用', () => {
  const ask = pack({current: READY, library: {prompt: 'ask'}});
  assert.equal(c.packLine(ask), '角色资料包现在默认启用，本图库还没启用');
  assert.match(c.packHintText(ask), /启用会给还没有归属的图补上机器归属/);
  assert.deepEqual(buttons(c.packButtons(ask)).map(b => [b.act, b.label]), [['enable-confirm', '启用…'], ['decline', '不用']]);
  assert.deepEqual(labels(c.packButtons(ask, true)), ['启用…'], '任务面板不放「不用」');
  const auto = pack({current: READY, library: {prompt: 'auto', vectors: 0}});
  assert.equal(c.packLine(auto), '第一次同步时启用');
  assert.deepEqual(labels(c.packButtons(auto)), ['启用…']);
  // 新图库第一次同步时包还没到（#630）：下载完成后由宿主启用，卡片不提示「不用」。
  const pending = pack({current: READY, library: {prompt: 'pending', vectors: 3}});
  assert.equal(c.packLine(pending), '等后台任务结束后启用');
  assert.deepEqual(labels(c.packButtons(pending)), ['启用…']);
  assert.doesNotMatch(c.packHintText(pending), /补上机器归属/);
  const waiting = pack({library: {prompt: 'pending', vectors: 3}, download: {kind: 'download', status: 'failed', message: '连接失败'}});
  assert.equal(c.packLine(waiting), '角色资料包没能下载：连接失败');
  assert.match(c.packHintText(waiting), /下载完成后启用/);
  assert.equal(labels(c.packButtons(waiting))[0], '重试');
  const declined = pack({current: READY, library: {prompt: 'declined'}});
  assert.equal(c.packLine(declined), '本图库未启用');
  assert.doesNotMatch(c.packHintText(declined), /补上机器归属/);
  // 不能启用时按钮禁用，提示写原因。没有识别结果不算不能启用。
  const syncing = buttons(c.packButtons(pack({current: READY, library: {prompt: 'ask', blocked: 'syncing'}})))[0];
  assert.match(syncing.attrs, /disabled title="正在同步，等它结束"/);
  assert.doesNotMatch(buttons(c.packButtons(pack({current: READY, library: {blocked: 'no_vectors'}})))[0].attrs, /disabled/);
});

test('启用中：扫描阶段报进度、可以取消；写入阶段不报百分比、不能取消；撤回中没有按钮', () => {
  const scan = pack({current: READY, task: {id: 't1', kind: 'enable', status: 'running', phase: 'scan', part: 'wd', done: 30, total: 120}});
  assert.equal(c.packLine(scan), '正在检查 30 / 120 张');
  assert.deepEqual(buttons(c.packButtons(scan)).map(b => b.act), ['cancel']);
  const start = pack({current: READY, task: {id: 't1', kind: 'enable', status: 'running', phase: 'scan', part: 'ccip', done: 0, total: 0}});
  assert.equal(c.packLine(start), '正在检查…');
  const write = pack({current: READY, task: {id: 't1', kind: 'enable', status: 'running', phase: 'write', done: 120, total: 120}});
  assert.equal(c.packLine(write), '正在写入…');
  assert.equal(c.packProgress(write), null);
  assert.match(buttons(c.packButtons(write))[0].attrs, /disabled title="正在写入，不能取消"/);
  const withdraw = pack({current: READY, library: ENABLED, task: {id: 't2', kind: 'withdraw', status: 'running', phase: 'write'}});
  assert.equal(c.packLine(withdraw), '正在撤回…');
  assert.deepEqual(c.packButtons(withdraw), []);
});

test('已启用：数字、上次检查、重新检查和撤回…；索引更新过、自动检查出错写在下面', () => {
  const x = pack({current: READY, library: ENABLED});
  assert.equal(c.packLine(x), '已启用 · 7 张图有扩展候选，其中 3 张由它补上归属');
  assert.equal(c.packDot(x), '');
  assert.match(c.packHintText(x), /上次检查/);
  assert.deepEqual(buttons(c.packButtons(x)).map(b => [b.act, b.label, b.cls]), [['enable', '重新检查', 'quiet small'], ['withdraw', '撤回…', 'quiet small']]);
  assert.equal(c.packLine(pack({current: READY, library: {...ENABLED, images: 0, assigned: 0}})), '已启用 · 没有扩展候选');
  assert.equal(c.packLine(pack({current: READY, library: {...ENABLED, assigned: 0}})), '已启用 · 7 张图有扩展候选');
  const stale = pack({current: READY, library: ENABLED, ccip: {needs_scan: true, error: '基础角色索引不在了'}});
  assert.match(c.packHintText(stale), /角色索引更新过，重新检查后才用上 · 上次自动检查没有完成：基础角色索引不在了/);
  assert.match(buttons(c.packButtons(stale))[0].cls, /primary/);
  assert.equal(c.packDot(stale), 'warn');
  // 已启用、包不在这台电脑上：写明新识别的图不检查，可以下载或撤回。
  const absent = pack({library: ENABLED});
  assert.match(c.packHintText(absent), /角色资料包不在这台电脑上，新识别的图不检查/);
  assert.deepEqual(buttons(c.packButtons(absent)).map(b => b.act), ['download', 'import', 'withdraw']);
  assert.match(buttons(c.packButtons(absent))[0].cls, /quiet/);
});

test('失败和取消：图库没有改动；CCIP 部分在 WD 部分提交以后失败或取消时写明 WD 部分已经更新', () => {
  const fail = (task, l = {}) => c.packLine(pack({current: READY, library: l, task: {id: 't', status: 'failed', message: '磁盘已满', ...task}}));
  assert.equal(fail({kind: 'enable'}), '启用没有完成，图库没有改动：磁盘已满');
  assert.equal(fail({kind: 'enable'}, ENABLED), '重新检查没有完成，图库没有改动：磁盘已满');
  assert.equal(fail({kind: 'withdraw'}, ENABLED), '撤回没有完成，图库没有改动：磁盘已满');
  assert.equal(fail({kind: 'auto'}, ENABLED), '检查新识别的图没有完成，图库没有改动：磁盘已满');
  assert.equal(fail({kind: 'enable', result: {wd: {changed: 2}}}, ENABLED), 'CCIP 长尾部分没有完成：磁盘已满；WD 扩展部分已经更新');
  const failed = pack({current: READY, library: ENABLED, task: {id: 't', kind: 'enable', status: 'failed', message: 'x'}});
  assert.deepEqual(buttons(c.packButtons(failed)).map(b => b.act), ['enable-retry', 'withdraw']);
  assert.deepEqual(buttons(c.packButtons(pack({current: READY, task: {id: 't', kind: 'withdraw', status: 'failed'}}))).map(b => b.act), ['withdraw-retry']);
  assert.equal(c.packLine(pack({current: READY, task: {id: 't', kind: 'enable', status: 'cancelled'}})), '已取消，图库没有改动');
  assert.equal(c.packLine(pack({current: READY, task: {id: 't', kind: 'enable', status: 'cancelled', result: {wd: {}}}})), '已取消；WD 扩展部分已经更新');
});

test('折叠区：存放位置、两个组成部分的文件、版本和许可；就位时可以删除', () => {
  const text = info(pack({current: READY}));
  assert.match(text, /存放位置: .*Illustash\/Shared\/extensions · 所有图库共用/);
  assert.match(text, /WD 扩展头: wd-extension-v1.bin · 941 KB · abababababab… · 228 个角色/);
  assert.match(text, /CCIP 长尾锚点: A-longtail-p75.zip · 1.3 MB · d3d3d3d3d3d3… · 54 个角色/);
  assert.match(text, /版本: 0\.1（测试版） · 282 个角色/);
  assert.match(info(pack({current: {...READY, version: '1.0', stage: 'release'}})), /版本: 1\.0 · 282 个角色/);
  assert.deepEqual(B(c.packDeleteButtons(pack({current: READY}))).map(b => [b.act, b.attrs]), [['delete', '']]);
  assert.deepEqual(B(c.packDeleteButtons(pack({current: READY, task: {id: 't', kind: 'enable', status: 'running', phase: 'scan'}}))).map(b => b.attrs),
    [' disabled title="正在下载或检查，等它结束再删除"']);
  assert.deepEqual(c.packInfoItems({}), []);
});

const OFFER = (fields = {}) => ({version: '0.2', stage: 'preview', revision: 2, heads: 311, candidate_only: 0, license: 'Apache-2.0 AND MIT',
  builtin: false, note: {'zh-CN': '比第一版多认 29 个角色', en: '29 more characters than the first version'}, size: 2338740,
  status: 'missing', have: 0, downloading: false, ...fields});

test('卡片：还没读到时没有按钮；已启用时有撤回，新版本单独一行', () => {
  assert.deepEqual(c.packButtons(null), []);
  const x = pack({current: READY, library: ENABLED, offer: OFFER()});
  assert.ok(c.packButtons(x).some(b => b.act === 'withdraw'));
  assert.match(c.packOfferInfo(x).what, /有新版本/);
});

test('有新版本：没下载时下载，下了一部分接着下，正在下载时进度在状态行，下好后已启用的图库可以换', () => {
  const enabled = fields => pack({current: READY, library: ENABLED, offer: OFFER(fields)});
  const o = c.packOfferInfo(enabled());
  assert.deepEqual([o.what, o.hint], ['有新版本 0.2（测试版）：311 个角色', '比第一版多认 29 个角色']);
  assert.deepEqual(offer(enabled()).map(b => [b.act, b.label, b.cls]), [['offer-download', '下载新版本（约 2.3 MB）', 'quiet small']]);
  assert.match(offer((pack({current: READY, offer: OFFER()})))[0].cls, /primary/, '没启用的图库下载是主按钮');
  assert.deepEqual(offer(enabled({status: 'partial', have: 338740})).map(b => b.label), ['继续下载新版本（还差 2.0 MB）']);
  // 新版本只换了 WD 部分：CCIP 文件已经在本机，按钮写还要下的大小，不说「继续」。
  assert.deepEqual(offer(enabled({status: 'missing', have: 1273692})).map(b => b.label), ['下载新版本（约 1.1 MB）']);
  const downloading = pack({current: READY, library: ENABLED, offer: OFFER({downloading: true}), download: {kind: 'download', status: 'running', done: 1, total: 9}});
  assert.deepEqual(offer(downloading), []);
  assert.match(offer((pack({current: READY, library: ENABLED, offer: OFFER(), task: {id: 't', kind: 'enable', status: 'running', phase: 'scan'}})))[0].attrs, /disabled/);
  assert.deepEqual(offer((enabled({status: 'ready'}))).map(b => [b.act, b.label, b.cls]), [['offer-switch', '换成新版本…', 'primary small']]);
  assert.match(offer((pack({current: READY, library: {...ENABLED, blocked: 'busy'}, offer: OFFER({status: 'ready'})})))[0].attrs, /disabled title="图库正在导入或导出"/);
  assert.equal(c.packOfferInfo(enabled({version: '1.0', stage: 'release', heads: 2100, candidate_only: 1872, note: null})).what,
    '有新版本 1.0：2,100 个角色，其中 1,872 个只给候选');
  assert.equal(c.packOfferInfo(pack({current: READY})), null);
  assert.equal(c.packOfferInfo(null), null);
});

test('有新版本：下载失败写原因、可以重试；提示说新版本', () => {
  const before = pack({current: READY, library: ENABLED, offer: OFFER({downloading: true}), download: {kind: 'download', status: 'running'}});
  const failed = pack({current: READY, library: ENABLED, offer: OFFER({downloading: true}), download: {kind: 'download', status: 'failed', message: '连接被重置'}});
  assert.equal(c.packOfferInfo(failed).hint, '新版本没能下载：连接被重置 · 比第一版多认 29 个角色');
  assert.deepEqual(offer(failed).map(b => [b.act, b.label]), [['offer-download', '重试']]);
  // 卡片上的版本（v1）已经就位，状态行照常写已启用。
  assert.equal(c.packLine(failed), '已启用 · 7 张图有扩展候选，其中 3 张由它补上归属');
  assert.equal(c.downloadText(failed.download, before, failed), '新版本没能下载：连接被重置');
  assert.equal(c.downloadText({kind: 'download', status: 'done'}, before, pack({current: READY, library: ENABLED, offer: OFFER({status: 'ready'})})), '新版本已下载');
  assert.equal(c.downloadText({kind: 'download', status: 'done'}, pack(), pack({current: READY})), '角色资料包已就位');
  assert.equal(c.downloadText({kind: 'import', status: 'done'}, pack(), pack()), '', '导入没凑齐时不提示，结果列在卡片里');
});

test('确认框写明做决定前要知道的事实', () => {
  const x = pack({current: READY, library: ENABLED});
  assert.equal(c.enableMessage(pack({current: READY})),
    '用角色资料包检查本图库 120 张图。还没有归属、也没有人工确认的图会补上一个机器归属，已有的归属和人工结论不变。以后可以撤回。');
  assert.equal(c.enableMessage(pack({current: {...READY, candidate_only: 1872}, library: {vectors: 0}})),
    '本图库还没有识别结果，新识别的图会用角色资料包检查。还没有归属、也没有人工确认的图会补上一个机器归属，已有的归属和人工结论不变。其中 1,872 个角色只给候选，不补归属。以后可以撤回。');
  assert.equal(c.withdrawMessage(x), '去掉本图库里角色资料包给出的全部候选，以及它补上的 3 个机器归属。你确认过、否定过的保留。撤回不进撤销历史，要恢复就重新启用。');
  assert.equal(c.switchMessage(OFFER({candidate_only: 2}), x),
    '用角色资料包检查本图库 120 张图。旧版本给出的扩展候选和由它补上的归属换成新版本的结果，已有的其他归属和人工结论不变。其中 2 个角色只给候选，不补归属。以后可以撤回。');
});

test('结束时的提示', () => {
  const after = pack({current: READY, library: ENABLED});
  const task = fields => ({id: 't', status: 'done', ...fields});
  assert.equal(c.finishedText(task({kind: 'enable', result: {changed: 3}}), pack({current: READY}), after), '角色资料包已启用：7 张图有扩展候选');
  assert.equal(c.finishedText(task({kind: 'enable', result: {changed: 0}}), after, after), '没有变化');
  assert.equal(c.finishedText(task({kind: 'enable', result: {changed: 2, ccip: {missing: 4, incompatible: 1}}}), after, after),
    '角色资料包已更新：7 张图有扩展候选；5 张图缺少兼容的向量，没有检查');
  assert.equal(c.finishedText(task({kind: 'withdraw', result: {changed: 3}}), after, pack({current: READY})), '已撤回角色资料包');
  assert.equal(c.finishedText(task({kind: 'auto', result: {changed: 0}}), after, after), '');
  assert.equal(c.finishedText(task({kind: 'auto', result: {changed: 1}}), after, after), '角色资料包已更新：7 张图有扩展候选');
  assert.equal(c.finishedText(task({kind: 'enable', result: {changed: 0}}), pack({current: READY}), pack({current: READY, library: {...ENABLED, images: 0}})),
    '角色资料包已启用，没有找到扩展候选');
});

// ---------- core/packs.js：读取、POST 和轮询 ----------
function server(replies) {
  const calls = [];
  globalThis.fetch = async (path, options = {}) => {
    calls.push([options.method || 'GET', path, options.body ? JSON.parse(options.body) : null]);
    const reply = replies.shift() || {body: pack()};
    return {status: reply.status || 200, ok: !reply.status, headers: {get: () => null}, json: async () => reply.body};
  };
  return calls;
}

test('读取和每个 POST 的请求体；关掉选择框的导入不改状态；换了图库的回复丢掉', async () => {
  const x = pack({current: READY});
  const calls = server(Array.from({length: 12}, () => ({body: x})));
  await c.loadPack();
  for (const action of ['download', 'import', 'cancel', 'discard', 'delete']) await c.packAction(action);
  await c.packAction('download', '0.2');
  await c.enablePack(); await c.enablePack('0.2'); await c.withdrawPack(); await c.declinePack();
  c.state.pack = pack({task: {id: 't9', kind: 'enable', status: 'done'}});
  await c.cancelPack();
  assert.deepEqual(calls.map(([m, p, b]) => [m, p, b]), [['GET', '/api/packs', null],
    ['POST', '/api/packs', {action: 'download'}], ['POST', '/api/packs', {action: 'import'}], ['POST', '/api/packs', {action: 'cancel'}],
    ['POST', '/api/packs', {action: 'discard'}], ['POST', '/api/packs', {action: 'delete'}],
    ['POST', '/api/packs', {action: 'download', version: '0.2'}],
    ['POST', '/api/packs', {action: 'enable'}], ['POST', '/api/packs', {action: 'enable', version: '0.2'}],
    ['POST', '/api/packs', {action: 'withdraw'}], ['POST', '/api/packs', {action: 'decline'}], ['POST', '/api/packs', {action: 'cancel', id: 't9'}]]);
  c.state.pack = x;
  server([{body: {cancelled: true}}]);
  assert.deepEqual(await c.packAction('import'), {cancelled: true});
  assert.equal(c.state.pack, x);
  globalThis.fetch = async () => { c.session.token = 'library-b'; return {status: 200, ok: true, headers: {get: () => null}, json: async () => pack()}; };
  await c.loadPack();
  assert.equal(c.state.pack, x);
  assert.equal(c.packOffer(pack({offer: OFFER()})).version, '0.2');
  assert.equal(c.packOffer(pack()), null);
});

test('启用：POST 后轮询到结束，只报一次', async () => {
  const running = pack({current: READY, task: {id: 't1', kind: 'enable', status: 'running', phase: 'scan', done: 0, total: 120}});
  const done = pack({current: READY, library: ENABLED, task: {id: 't1', kind: 'enable', status: 'done', result: {changed: 3}}});
  server([{body: running}, {body: done}, {body: done}]);
  const seen = [];
  const off = c.onPack(before => { seen.push(c.packFinished(before, c.state.pack)); });
  await c.enablePack();
  await c.loadPack();
  assert.deepEqual(seen, [null, done.task], '只在任务从运行变成结束时报一次');
  await c.loadPack();
  assert.equal(seen.length, 3); assert.equal(seen[2], null);
  off();
});

test('等着默认启用（#630）：包就位后接着读，宿主启动的启用任务结束时报一次；上次失败或取消了就不读', async () => {
  const awaiting = pack({current: READY, library: {prompt: 'pending', vectors: 3}});
  assert.equal(c.packAwaitingDefault(awaiting), true);
  assert.equal(c.packAwaitingDefault(pack({library: {prompt: 'pending'}})), false, '包还没就位');
  assert.equal(c.packAwaitingDefault(pack({current: READY, library: {prompt: 'ask'}})), false);
  for (const status of ['failed', 'cancelled']) {
    assert.equal(c.packAwaitingDefault(pack({current: READY, library: {prompt: 'pending'}, task: {id: 't0', kind: 'enable', status}})), false, status);
  }
  const running = pack({current: READY, library: {prompt: 'pending', vectors: 3}, task: {id: 't2', kind: 'enable', status: 'running', phase: 'scan', done: 0, total: 3}});
  const done = pack({current: READY, library: ENABLED, task: {id: 't2', kind: 'enable', status: 'done', result: {changed: 1}}});
  const calls = server([{body: awaiting}, {body: running}, {body: done}, {body: done}]);
  const seen = [];
  const off = c.onPack(before => { const t = c.packFinished(before, c.state.pack); if (t) seen.push(t); });
  await c.loadPack();
  const until = Date.now() + 10000;
  while (!seen.length && Date.now() < until) await new Promise(resolve => setTimeout(resolve, 50));
  off();
  assert.deepEqual(seen, [done.task]);
  assert.equal(calls.length, 3, '启用结束以后不再读');
  assert.equal(c.state.pack, done);
});

test('下载中读取失败一次：退避以后接着读，读到结束（#648）', async () => {
  const running = pack({download: {kind: 'download', status: 'running', done: 1, total: 10}});
  const done = pack({current: READY, download: {kind: 'download', status: 'done', done: 10, total: 10}});
  const calls = server([{body: running}, {status: 503, body: {error: '宿主忙'}}, {body: done}, {body: done}]);
  await c.loadPack();
  const until = Date.now() + 10000;
  while (c.state.pack !== done && Date.now() < until) await new Promise(resolve => setTimeout(resolve, 50));
  assert.equal(c.state.pack, done, '失败以后没有停住');
  await new Promise(resolve => setTimeout(resolve, 1200));
  assert.equal(calls.length, 3, '结束以后不再读');
});

test('取消晚了一步（写入阶段回 409）：提示事实，不带错误编号，再读一次状态；409 原样抛出后端的原文', async () => {
  const page = web.fakePage();
  c.wirePacks();
  c.state.pack = pack({current: READY, task: {id: 't1', kind: 'enable', status: 'running', phase: 'scan', done: 1, total: 9}});
  const calls = server([{status: 409, body: {error: '正在写入图库，无法取消'}}, {body: pack({current: READY, library: ENABLED})}]);
  c.runPack('cancel');   // 设置页卡片上的「取消」直接调 runPack
  await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(page.el('toastText').textContent, '已经开始写入，没能取消');
  assert.deepEqual(calls.map(x => x[1]), ['/api/packs', '/api/packs']);
  assert.deepEqual(calls[0][2], {action: 'cancel', id: 't1'});
  server([{status: 409, body: {error: '角色资料包还没就位'}}]);
  await assert.rejects(c.enablePack(), {message: '角色资料包还没就位', status: 409});
});

test('启用、撤回、删除、换成新版本都先确认；取消什么都不发；重新检查、重试、不用和取消下载不再确认', async () => {
  const page = web.fakePage();
  const answers = [], titles = [];
  page.document.body.append = d => { titles.push(d.attributes['aria-label']); d.showModal = () => { d.returnValue = answers.shift(); d.onclose(); }; };
  c.wirePacks();
  // 设置页卡片和任务面板的按钮都直接调 runPack（任务面板的由 ui/tasks.js 分派）。
  const click = async (root, act) => { await c.runPack(act); };
  const state = () => pack({current: READY, library: ENABLED, offer: OFFER({status: 'ready'})});
  c.state.route = {page: 'settings'};
  c.state.pack = state();
  const calls = server(Array.from({length: 12}, () => ({body: state()})));
  answers.push('cancel', 'cancel', 'cancel', 'cancel');
  await click('settingsPage', 'enable-confirm'); await click('settingsPage', 'withdraw'); await click('taskPanel', 'delete');
  await click('settingsPage', 'offer-switch');
  assert.deepEqual(titles, ['启用角色资料包', '撤回角色资料包', '删除角色资料包', '换成新版本']);
  assert.deepEqual(calls, []);
  answers.push('ok', 'ok', 'ok', 'ok');
  await click('settingsPage', 'enable-confirm'); await click('settingsPage', 'withdraw'); await click('taskPanel', 'delete');
  await click('settingsPage', 'offer-switch');
  assert.deepEqual(calls.map(x => x[2]), [{action: 'enable'}, {action: 'withdraw'}, {action: 'delete'}, {action: 'enable', version: '0.2'}]);
  titles.length = 0;
  calls.length = 0;
  c.state.pack = pack({current: READY, library: ENABLED, offer: OFFER()});
  await click('settingsPage', 'enable'); await click('taskPanel', 'enable-retry'); await click('settingsPage', 'decline');
  await click('taskPanel', 'package-cancel');
  c.state.pack = pack({current: READY, library: ENABLED, offer: OFFER()});
  await click('settingsPage', 'offer-download');
  assert.deepEqual(titles, []);
  assert.deepEqual(calls.map(x => x[2]), [{action: 'enable'}, {action: 'enable'}, {action: 'decline'}, {action: 'cancel'},
    {action: 'download', version: '0.2'}]);
});

test('这个页面导入以后逐个列出所选文件的结果，包括离线包里的包目录；关掉选择框和别的动作以后不列', async () => {
  const download = {kind: 'import', status: 'done', results: [{name: 'character-pack-0.2-offline.zip/packs.json', result: 'catalog'},
    {name: 'old-offline.zip/packs.json', result: 'catalog_old'}, {name: 'bad-offline.zip/packs.json', result: 'catalog_invalid'},
    {name: 'character-pack-0.2-offline.zip/wd-extension-v2.bin', result: 'accepted'}, {name: 'edited.bin', result: 'mismatch'},
    {name: 'notes.txt', result: 'unknown'}, {name: 'again.bin', result: 'already'},
    {name: 'character-pack-0.2-offline.zip/NOTICE.txt', result: 'extra'}]};
  const imported = pack({current: READY, download});
  const page = web.fakePage();
  c.state.route = {page: 'settings', section: 'characters'};
  c.wirePacks();
  server([{body: imported}, {body: {cancelled: true}}, {body: imported}]);
  const click = async act => {
    c.runPack(act);
    await new Promise(resolve => setTimeout(resolve, 10));
  };
  await click('enable');
  assert.equal(results(imported), '', '别的动作以后不列');
  await click('import');
  assert.equal(results(imported), '', '关掉选择框不算导入');
  await click('import');
  const html = results(imported);
  assert.match(html, /character-pack-0.2-offline.zip\/packs.json · 包目录，已读取/);
  assert.match(html, /old-offline.zip\/packs.json · 包目录比本机的旧，没有采用/);
  assert.match(html, /bad-offline.zip\/packs.json · 包目录签名不对，已拒绝/);
  assert.match(html, /wd-extension-v2.bin · 已接收/);
  assert.match(html, /edited.bin · SHA-256 不符，已拒绝/);
  assert.match(html, /notes.txt · 不是角色资料包/);
  assert.match(html, /again.bin · 已经有了/);
  assert.doesNotMatch(html, /NOTICE/, '离线包随附的许可和说明不列（#643）');
  assert.equal(results(pack({current: READY, download: {...download, results: download.results.filter(r => r.result === 'extra')}})), '');
  assert.equal(results(pack({download: {kind: 'download', status: 'done', results: []}})), '');
  // 切到别的页面再回来（每次换页都是新的 route）：不再列（#643）。
  c.state.route = {page: 'settings', section: 'about'};
  assert.equal(results(imported), '');
  c.state.route = {page: 'settings', section: 'characters'};
  assert.equal(results(imported), '');
});

test('任务面板的一项：有下载或任务时才列出，失败时打警告', () => {
  assert.equal(c.packTask(pack({current: READY})), null);
  const running = c.packTask(pack({current: READY, task: {id: 't', kind: 'enable', status: 'running', phase: 'scan', done: 3, total: 12}}));
  assert.deepEqual([running.key, running.name, running.busy, running.active, running.line], ['pack', '角色资料包', '检查角色资料包中', true, '正在检查 3 / 12 张']);
  assert.deepEqual(running.buttons.map(b => [b.attr, b.act]), [['pack', 'cancel']]);
  assert.deepEqual(running.progress, {done: 3, total: 12});
  const failed = c.packTask(pack({current: READY, task: {id: 't', kind: 'withdraw', status: 'failed', message: '磁盘已满', diagnostic: {id: 'D-1', stored: true}}}));
  assert.equal(failed.warn, true);
  assert.equal(failed.diagnostic.id, 'D-1');
  assert.equal(c.packTask(pack({download: {kind: 'import', status: 'running', done: 1, total: 2}})).busy, '导入角色资料包中');
});
