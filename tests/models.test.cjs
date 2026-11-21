// 运行：node --test tests/models.test.cjs
// 不带识别模型的安装包在首次使用时下载模型（#168）：ui/models.js 和 core/models.js。
const {test, before} = require('node:test');
const assert = require('node:assert/strict');
const web = require('./web.cjs');
const context = {};
before(async () => {
  await web.ready();
  const ui = await web.load('ui/models.js'), core = await web.load('core/models.js');
  Object.assign(context, ui, core, {state: web.core().state, syncLine: web.core().syncLine});
});
const MB = 1e6;
const models = (fields = {}) => ({on_demand: true, status: 'missing', total: 618 * MB, have: 0, partial: false, missing: ['a'], job: null,
  manual: 'https://github.com/shio-vinyl/illustash/blob/main/docs/INSTALL.md#识别模型', mirror: 'https://hf-mirror.com', mirror_default: 'https://hf-mirror.com',
  sources: ['hf-mirror.com', 'modelscope.cn'], files: [], location: '/synthetic', ...fields});

test('only an install that downloads its models waits for them', () => {
  assert.equal(context.modelsWaiting(models()), true);
  assert.equal(context.modelsWaiting(models({status: 'ready'})), false);
  assert.equal(context.modelsWaiting(models({on_demand: false})), false);
  assert.equal(context.modelsWaiting(undefined), false);
});

test('the download button says how much; nothing starts without it', () => {
  const m = models();
  assert.equal(context.modelsLine(m), '还没下载（约 618.0 MB）');
  const actions = context.modelsActions(m);
  assert.match(actions, /data-models="download">下载模型（约 618.0 MB）</);
  assert.match(actions, /data-models="import"/);
  assert.doesNotMatch(actions, /discard/);
  // 已经有一部分：可以继续，没下完的部分可以删除。
  const part = models({have: 100 * MB, partial: true, missing: ['wd_swinv2_v3_emb.onnx', 'ccip/model_feat.onnx']});
  assert.equal(context.modelsLine(part), '还缺 wd_swinv2_v3_emb.onnx、ccip/model_feat.onnx（已有 100.0 MB）');
  assert.match(context.modelsActions(part), /继续下载（还差 518.0 MB）/);
  // 只剩一个小文件：按字节显示大小，而不是 0 KB。
  assert.match(context.modelsActions(models({have: 618 * MB - 147, partial: true})), /继续下载（还差 147 字节）/);
  // 第一次尝试失败只留下空文件夹：没有可删的。
  assert.doesNotMatch(context.modelsActions(models({have: 0, partial: true})), /discard/);
  assert.match(context.modelsActions(part), /data-models="discard"/);
  assert.equal(context.modelsActions(part, true).includes('import'), false, 'the task panel shows the main button only');
  assert.equal(context.modelsActions(models({status: 'ready'})), '');
});

test('a running download shows progress, speed, source and a cancel button', () => {
  const m = models({job: {kind: 'download', status: 'running', done: 300 * MB, total: 618 * MB, speed: 7 * MB, source: 'modelscope.cn', proxy: null}});
  assert.equal(context.modelsLine(m), '正在下载 300.0 MB / 618.0 MB · 7.0 MB/秒 · 来自 modelscope.cn');
  assert.match(context.modelsBar(m), /width:48.5%/);
  assert.equal(context.modelsActions(m), '<button class="quiet small" data-models="cancel">取消</button>');
  m.job.proxy = '127.0.0.1:7890';
  assert.match(context.modelsLine(m), /经代理 127.0.0.1:7890$/);
});

test('a failed download names the file and each source with its reason', () => {
  const m = models({job: {kind: 'download', status: 'failed', failed_file: 'ccip/model_feat.onnx', message: 'x',
    failures: [{source: 'hf-mirror.com', why: 'HTTP 404'}, {source: 'modelscope.cn', why: 'SHA-256 与清单不符，已丢弃'}]}});
  assert.equal(context.modelsLine(m), 'ccip/model_feat.onnx 没能下载，每个来源都试过了');
  assert.match(context.modelsFailures(m), /hf-mirror.com：HTTP 404/);
  assert.match(context.modelsFailures(m), /modelscope.cn：SHA-256 与清单不符，已丢弃/);
  assert.equal(context.modelsLine(models({job: {status: 'failed', message: '磁盘空间不足：还需要约 700 MB，可用约 12 MB'}})),
    '下载没有完成：磁盘空间不足：还需要约 700 MB，可用约 12 MB');
  assert.equal(context.modelsLine(models({job: {status: 'cancelled'}, have: 1})), '已取消，下好的部分会保留，下次接着下');
});

test('an import lists what it made of each chosen file', () => {
  const job = {kind: 'import', status: 'done', results: [{name: 'a.zip/model.onnx', result: 'accepted'}, {name: 'metrics.json', result: 'mismatch'}, {name: 'x.txt', result: 'unknown'}]};
  const html = context.importSummary(job);
  assert.match(html, /a.zip\/model.onnx<\/bdi> · 已接收/);
  assert.match(html, /metrics.json<\/bdi> · SHA-256 不符，已拒绝/);
  assert.match(html, /x.txt<\/bdi> · 不是需要的模型文件/);
  assert.equal(context.importSummary({kind: 'download', results: []}), '');
});

test('the guide bar explains what works without the models, and goes once they are in place', () => {
  context.state.models = models();
  const html = context.modelsNotice();
  assert.match(html, /识别要用的模型还没下载（约 618.0 MB）。图片会先加入图库，可以浏览和手动整理；首次准备结束后会自动开始识别。/);
  assert.match(html, /手动下载说明/);
  context.state.models = models({status: 'ready'});
  assert.equal(context.modelsNotice(), '');
  context.state.models = models({on_demand: false});
  assert.equal(context.modelsNotice(), '');
});

test('a finished job is reported once', () => {
  const running = models({job: {status: 'running'}}), done = models({job: {status: 'done'}});
  assert.deepEqual(context.modelsFinished(running, done), {status: 'done'});
  assert.equal(context.modelsFinished(done, done), null);
  assert.equal(context.modelsFinished(undefined, done), null);
});

test('pictures waiting for the models say so in the sync line', () => {
  context.state.models = models();
  assert.equal(context.syncLine({status: 'needs_recognition', total: 3}), '有 3 张图等待识别（识别模型还没下载）');
  context.state.models = models({on_demand: false});
  assert.equal(context.syncLine({status: 'needs_recognition', total: 3}), '有 3 张图等待识别（识别服务未启动）');
});

test('base index progress mentions waiting only during known initial setup', async () => {
  const fetch = globalThis.fetch, setTimeout = globalThis.setTimeout;
  const previous = {models: context.state.models, characterBase: context.state.characterBase, sync: context.state.sync};
  const file = 'character-index/synthetic.bin';
  const ready = models({status: 'ready'});
  const downloading = {...ready, job: {status: 'running', kind: 'download', file, done: MB, total: 2 * MB}};
  context.state.characterBase = {included: true, file: {path: file}};
  context.state.sync = {status: 'needs_recognition', total: 3};
  let response = models();
  globalThis.setTimeout = () => 0;
  globalThis.fetch = async () => ({ok: true, json: async () => response});
  try {
    await context.loadModels();
    response = downloading;
    await context.loadModels();
    assert.match(context.modelsNotice(), /正在下载基础角色索引.*下载结束后开始识别。/);
    assert.equal(context.syncLine(context.state.sync), '有 3 张图等待识别（正在下载基础角色索引）');
    response = {...ready, job: {status: 'failed', kind: 'download', failed_file: file}};
    await context.loadModels();
    assert.equal(context.modelsNotice(), '');
    await context.loadModels();
    response = downloading;
    await context.loadModels();
    assert.equal(context.modelsNotice(), '', '补下载索引不会让再次识别进入首次准备等待');

    response = ready;
    await context.loadModels();
    context.state.models = null;
    response = downloading;
    await context.loadModels();
    await context.loadModels();
    assert.match(context.modelsNotice(), /正在下载基础角色索引/);
    assert.doesNotMatch(context.modelsNotice(), /下载结束后开始识别/);
    assert.doesNotMatch(context.syncLine(context.state.sync), /正在下载基础角色索引/);
    context.state.sync = {status: 'recognising', done: 2, total: 3};
    assert.equal(context.modelsNotice(), '');
    assert.match(context.syncLine(context.state.sync), /正在识别新图片 2\/3/);
  } finally {
    globalThis.fetch = async () => ({ok: true, json: async () => ready});
    await context.loadModels();
    Object.assign(globalThis, {fetch, setTimeout});
    Object.assign(context.state, previous);
  }
});

test('downloaded models can be deleted, but not while something uses them', () => {
  context.state.models = models({status: 'ready'});
  context.state.sync = {status: 'done'};
  assert.deepEqual(context.modelsDeleteButtons(), [{act: 'delete', label: '删除已下载的模型', disabled: false, title: ''}]);
  context.state.sync = {status: 'recognising'};
  assert.deepEqual(context.modelsDeleteButtons(), [{act: 'delete', label: '删除已下载的模型', disabled: true, title: '正在识别或下载，等它结束再删除'}]);
  context.state.sync = {status: 'done'};
  assert.equal(context.modelsDeleteButtons(models({status: 'ready', job: {status: 'running'}}))[0].disabled, true);
  assert.deepEqual(context.modelsDeleteButtons(models({status: 'ready', on_demand: false})), [], 'models that came with the install stay');
  assert.deepEqual(context.modelsDeleteButtons(models()), [], 'nothing to delete yet');
});

test('the acceleration switch shows only where there is an accelerator, and says when it fell back (#270)', () => {
  assert.equal(context.accelerationHint(models()), '', 'an older host without the field');
  assert.equal(context.accelerationHint(models({acceleration: {available: false, backend: null, on: true, last: {}}})), '');
  const on = models({acceleration: {available: true, backend: 'coreml', on: true, last: {}}});
  assert.match(context.accelerationHint(on), /Core ML 加速标签与角色特征/);
  assert.equal(context.accelerationFell(on), '');
  const off = models({acceleration: {available: true, backend: 'directml', on: false, last: {}}});
  assert.match(context.accelerationHint(off), /用显卡计算/);
  const fell = {ccip: {requested: 'coreml', used: 'cpu', reason: 'coreml <ccip>: failed', at: 1}};
  assert.equal(context.accelerationFell(models({acceleration: {available: true, backend: 'coreml', on: true, last: fell}})),
    '上次没能用上加速，已改用 CPU：coreml <ccip>: failed');
  assert.equal(context.accelerationFell(models({acceleration: {available: true, backend: 'coreml', on: false, last: fell}})), '',
    'switched off: nothing to explain');
});

test('WD 加速文件可补下载，缺失不阻塞识别，下载中仍可取消', () => {
  const m = models({status: 'ready', optional_missing: true, total: 1085 * MB, have: 618 * MB});
  assert.equal(context.modelsWaiting(m), false);
  assert.match(context.modelsLine(m), /WD 暂用 CPU/);
  assert.match(context.modelsActions(m), /下载 WD 加速文件（约 467.0 MB）/);
  assert.match(context.modelsActions(m), /data-models="import"/);
  assert.match(context.modelsActions({...m, job: {kind: 'download', status: 'running'}}), /data-models="cancel"/);
  assert.match(context.modelsActions({...m, optional_missing: false, acceleration: {backend: 'coreml', on: true, last: {wd: {reason: '损坏'}}}}), /修复 WD 加速文件/);
});

test('基础角色索引（#607）：缺文件时提示和按钮按缺的是哪个写，下好后按图库启用或撤回', () => {
  const base = {included: true, characters: 7367, file: {path: 'character-index/character-index-v1.bin'}, available: false,
    enabled: false, withdrawn: false, running: false, writable: true, mode: null};
  const files = [{path: 'coreml/wd.onnx', optional: true}, {path: base.file.path, optional: true}];
  const saved = context.state.characterBase;
  try {
    context.state.characterBase = base;
    const both = models({status: 'ready', optional_missing: true, total: 1110 * MB, have: 618 * MB, files, missing: files.map(f => f.path)});
    assert.equal(context.modelsLine(both), '识别模型已就位；WD 加速文件和基础角色索引尚未就位。');
    assert.match(context.modelsActions(both), /下载 WD 加速文件和基础角色索引（约 492.0 MB）/);
    const only = {...both, have: 1085 * MB, missing: [base.file.path]};
    assert.equal(context.modelsLine(only), '识别模型已就位；基础角色索引尚未就位，只认得 WD 词表里的角色。');
    assert.match(context.modelsActions(only), /下载基础角色索引（约 25.0 MB）/);
    assert.equal(context.finishedNote({status: 'failed', failed_file: base.file.path}, true, only),
      '识别模型已就位，开始识别；基础角色索引没能下载，可以在「设置 › 数据」里重试。');
    // 文件还没下载：这一行不出现，由上面的下载负责。
    const line = b => [context.characterBaseText(b), context.characterBaseButtons(b).map(x => x.act).join()];
    assert.deepEqual(line(base), ['', '']);
    assert.deepEqual(line({...base, available: true}), ['基础角色索引：已下载，这个图库还没启用', 'base-enable']);
    assert.deepEqual(line({...base, available: true, enabled: true}), ['基础角色索引：已启用，7,367 个角色', 'base-withdraw']);
    assert.deepEqual(line({...base, available: true, withdrawn: true}), ['基础角色索引：已撤回', 'base-enable']);
    assert.deepEqual(line({...base, available: true, enabled: true, running: true}), ['基础角色索引正在更新', '']);
    assert.deepEqual(line({...base, available: true, writable: false}), ['', '']);
    assert.deepEqual(line({...base, included: false}), ['', '']);
    // 首次准备时图库还没有识别结果，启用提示不说「0 张」。
    const {indexEndText} = web.core();
    assert.equal(indexEndText({status: 'done', kind: 'base', enabled: true, changed: 0}), '基础角色索引已启用');
    assert.equal(indexEndText({status: 'done', kind: 'base', enabled: false, changed: 3}), '基础角色索引已撤回，3 张图片的识别结果已更新');
  } finally { context.state.characterBase = saved; }
});

test('a base index update keeps polling even when the general index task missed its running state', async () => {
  const {fetch, setTimeout, clearTimeout} = globalThis;
  const saved = context.state.characterBase;
  const replies = [{running: true}, null, {running: false, included: true, enabled: true, available: true, writable: true}];
  const timers = new Map();
  let timer = 0, calls = 0;
  globalThis.setTimeout = (fn, delay) => { assert.equal(delay, 1000); timers.set(++timer, fn); return timer; };
  globalThis.clearTimeout = id => timers.delete(id);
  globalThis.fetch = async path => {
    assert.equal(path, '/api/index/base');
    calls++;
    const value = replies.shift();
    if (!value) throw new Error('synthetic temporary failure');
    return {ok: true, json: async () => value};
  };
  try {
    await context.loadCharacterBase();
    assert.equal(context.state.characterBase.running, true);
    await timers.get(timer)();
    assert.equal(context.state.characterBase.running, true);
    await timers.get(timer)();
    assert.deepEqual(context.characterBaseButtons().map(b => b.act), ['base-withdraw']);
    assert.equal(calls, 3);
    assert.equal(timers.size, 0);
  } finally {
    Object.assign(globalThis, {fetch, setTimeout, clearTimeout});
    context.state.characterBase = saved;
  }
});

test('a base index read waits for a pending activation before checking progress', async () => {
  const {fetch, setTimeout, clearTimeout} = globalThis;
  const saved = context.state.characterBase;
  const base = {included: true, available: true, writable: true, running: false, enabled: false};
  const reply = value => ({ok: true, json: async () => value});
  const calls = [], timers = new Map();
  let respond, timer = 0, written = false, complete = false;
  context.state.characterBase = base;
  globalThis.setTimeout = fn => { timers.set(++timer, fn); return timer; };
  globalThis.clearTimeout = id => timers.delete(id);
  globalThis.fetch = async (path, options) => {
    assert.equal(path, '/api/index/base');
    if (options?.method === 'POST') {
      calls.push('POST');
      return new Promise(resolve => { respond = resolve; });
    }
    calls.push(written ? 'GET after write' : 'GET before write');
    return reply({...base, running: written && !complete, enabled: complete});
  };
  try {
    const activate = context.characterBasePost(true);
    const refresh = context.loadCharacterBase();
    written = true;
    respond(reply({...base, running: true}));
    await Promise.all([activate, refresh]);
    assert.deepEqual(calls, ['POST', 'GET after write']);
    assert.equal(context.state.characterBase.running, true);
    complete = true;
    await timers.get(timer)();
    assert.deepEqual(context.characterBaseButtons().map(b => b.act), ['base-withdraw']);
    assert.equal(timers.size, 0);
  } finally {
    Object.assign(globalThis, {fetch, setTimeout, clearTimeout});
    context.state.characterBase = saved;
  }
});

// 依次读到这些状态（core/models.js 的 adopt 的做法），返回每次读取后「模型刚到」的判断（#375）。第一个 before 为空：
// 页面刚打开。
function arrivals(...seen) {
  let before, started = null;
  return seen.map(after => {
    const start = started;
    started = context.readyWhenStarted(before, after, start);
    const arrived = context.modelsArrived(before, start, after);
    before = after;
    return arrived;
  });
}
test('only a job that brought the models in starts recognition (#375)', () => {
  const missing = models(), ready = models({status: 'ready'});
  const run = (m, kind = 'download') => ({...m, job: {kind, status: 'running'}});
  const end = (m, status, kind = 'download') => ({...m, job: {kind, status}});
  // 首次下载：原始四文件在任务中途激活（status 先变 ready），之后下加速文件；成功、失败、取消都要开始识别。
  for (const status of ['done', 'failed', 'cancelled'])
    assert.deepEqual(arrivals(missing, run(missing), run(ready), end(ready, status)), [false, false, false, true], status);
  // 原始模型没下完就失败：模型还没就位。
  assert.deepEqual(arrivals(missing, run(missing), end(missing, 'failed')), [false, false, false]);
  // 导入补齐了模型。
  assert.deepEqual(arrivals(missing, run(missing, 'import'), end(ready, 'done', 'import')), [false, false, true]);
  // 页面在任务中途打开（原始模型已激活）：无从判断，照旧开始识别。
  assert.deepEqual(arrivals(run(ready), end(ready, 'done')), [false, true]);
  assert.deepEqual(arrivals(run(missing), end(ready, 'cancelled')), [false, true]);
  // 模型本来就在：补下或修复加速文件、导入，不管结局如何都不算。
  for (const status of ['done', 'failed', 'cancelled'])
    assert.deepEqual(arrivals(ready, run(ready), run(ready), end(ready, status)), [false, false, false, false], status);
  assert.deepEqual(arrivals(ready, run(ready, 'import'), end(ready, 'done', 'import')), [false, false, false]);
  // 只看到上一个任务的结局、没看到它在运行：没有「刚结束」的任务。
  assert.deepEqual(arrivals(missing, end(ready, 'done')), [false, false]);
});

test('models that came with the install offer nothing that writes into the app (#375)', () => {
  const bundled = models({on_demand: false, status: 'ready', optional_missing: true, total: 1085 * MB, have: 618 * MB});
  assert.match(context.modelsLine(bundled), /WD 暂用 CPU/, 'the status line still says why WD uses the CPU');
  assert.equal(context.modelsActions(bundled), '');
  const fell = {available: true, backend: 'coreml', on: true, last: {wd: {reason: '损坏'}}};
  assert.equal(context.modelsActions({...bundled, optional_missing: false, acceleration: fell}), '');
  // 开发用的不完整文件夹：原有的下载和导入照旧。
  const partial = context.modelsActions(models({on_demand: false}));
  assert.match(partial, /data-models="download">下载模型/);
  assert.match(partial, /data-models="import"/);
});

test('repairing the WD acceleration file is offered only while acceleration is on (#375)', () => {
  const m = models({status: 'ready', optional_missing: false, acceleration: {available: true, backend: 'coreml', on: true, last: {wd: {reason: '损坏'}}}});
  assert.match(context.modelsActions(m), /data-models="download">修复 WD 加速文件</);
  assert.equal(context.modelsActions({...m, acceleration: {...m.acceleration, on: false}}), '');
  assert.equal(context.modelsActions({...m, acceleration: {...m.acceleration, backend: 'directml'}}), '');
});

test('the mirror note mentions the WD acceleration file only where the platform has one (#375)', () => {
  const file = (path, optional) => ({path, size: 1, sha256: '0'.repeat(64), optional});
  const plain = context.mirrorNote(models({files: [file('wd/model.onnx', false)]}));
  assert.match(plain, /先从这个地址下载，不行再换 ModelScope。/);
  assert.doesNotMatch(plain, /WD 加速文件/);
  assert.match(context.mirrorNote(models({files: [file('wd/model.onnx', false), file('wd/model-coreml.onnx', true)]})),
    /原始模型先从这个地址下载.*WD 加速文件从项目的 GitHub Release 下载。/);
});
test('the note when a job ends says both things in one toast, and calls the optional file the WD acceleration file (#375)', () => {
  const files = [{path: 'wd/model.onnx', size: 1, sha256: '0', optional: false}, {path: 'coreml/wd-swinv2-v3-v1.onnx', size: 1, sha256: '0', optional: true}];
  const ready = models({status: 'ready', files}), missing = models({files, missing: ['wd/model.onnx']});
  const failed = file => ({kind: 'download', status: 'failed', failed_file: file, message: 'x'});
  // 刚到且成功、取消（取消的只能是加速文件）。
  assert.equal(context.finishedNote({kind: 'download', status: 'done'}, true, ready), '识别模型已就位，开始识别');
  assert.equal(context.finishedNote({kind: 'download', status: 'cancelled'}, true, ready), '识别模型已就位，开始识别');
  // 刚到而加速文件没下成：一句里说清两件事，不写路径。
  const both = context.finishedNote(failed('coreml/wd-swinv2-v3-v1.onnx'), true, ready);
  assert.equal(both, '识别模型已就位，开始识别；WD 加速文件没能下载，WD 暂用 CPU。');
  assert.doesNotMatch(both, /coreml\//);
  // 刚到而失败的原因没有对应文件（如磁盘空间）：原因接在后面。
  assert.equal(context.finishedNote({kind: 'download', status: 'failed', message: '磁盘空间不足'}, true, ready), '识别模型已就位，开始识别。下载没有完成：磁盘空间不足');
  // 模型本来就在：只说失败。
  assert.equal(context.finishedNote(failed('coreml/wd-swinv2-v3-v1.onnx'), false, ready), 'WD 加速文件没能下载，WD 暂用 CPU。');
  assert.equal(context.finishedNote({kind: 'download', status: 'cancelled'}, false, ready), '');
  assert.equal(context.finishedNote({kind: 'import', status: 'done', results: []}, false, ready), '', 'an import once the models are in place');
  // 原始文件没下成：模型没就位，照旧写路径。
  assert.equal(context.finishedNote(failed('wd/model.onnx'), false, missing), 'wd/model.onnx 没能下载，每个来源都试过了');
  // 导入后模型还没齐。
  assert.equal(context.finishedNote({kind: 'import', status: 'done', results: []}, false, missing), '还缺 1 个文件');
});

test('fallbacks count only while acceleration is on and a reason was written (#375)', () => {
  const accel = (last, on = true) => models({status: 'ready', acceleration: {available: true, backend: 'coreml', on, last}});
  const wd = {requested: 'coreml', used: 'cpu', reason: '加速文件损坏', at: 5}, ccip = {requested: 'coreml', used: 'coreml', reason: null, at: 5};
  assert.deepEqual(Object.keys(context.fallbacks(accel({wd, ccip}))), ['wd']);
  assert.deepEqual(context.fallbacks(accel({wd}, false)), {});
  // 宿主不报装好加速文件之前的 WD 记录：没有记录就没有「修复」和红字。
  const settled = accel({ccip});
  assert.deepEqual(context.fallbacks(settled), {});
  assert.equal(context.modelsActions(settled), '');
  assert.equal(context.accelerationFell(settled), '');
});

test('a sync that ends reads the models again, so the card follows the latest recognition (#375)', async () => {
  web.fakePage();
  const reads = [];
  globalThis.fetch = async path => {
    reads.push(path);
    const body = path === '/api/sync' ? {state: {status: 'done', imported: 0}, history: [], damaged: null} : models({status: 'ready'});
    return {ok: true, status: 200, json: async () => body};
  };
  context.wireModels();
  await new Promise(done => setTimeout(done, 0));
  const before = reads.filter(p => p === '/api/models').length;
  context.state.sync = {status: 'recognising'};
  await web.core().pollSync();
  await new Promise(done => setTimeout(done, 0));
  assert.equal(reads.filter(p => p === '/api/models').length, before + 1);
  // 没有在进行的同步，读到的状态也没变：不再读。
  await web.core().pollSync();
  await new Promise(done => setTimeout(done, 0));
  assert.equal(reads.filter(p => p === '/api/models').length, before + 1);
  delete globalThis.fetch;
});

// #429（#386 的 2-42）：点了按钮先禁用，取消系统选择框或请求失败以后卡片内容没变、不会重画，按钮也要恢复。
test('a models button comes back after the file picker is cancelled or the request fails (#429)', async () => {
  const page = web.fakePage(), clicks = [];
  page.document.addEventListener = (type, fn) => { if (type === 'click') clicks.push(fn); };
  let answer;
  globalThis.fetch = async (path, options) => {
    if (options?.method === 'POST') return answer();
    return {ok: true, status: 200, json: async () => models()};
  };
  context.wireModels();
  await new Promise(done => setTimeout(done, 0));
  const press = async button => {
    for (const fn of clicks) fn({target: {closest: sel => sel === '[data-models]' ? button : null}});
    assert.equal(button.disabled, true, 'disabled while waiting');
    for (let i = 0; i < 5; i++) await new Promise(done => setTimeout(done, 0));
  };
  for (const [what, reply] of [
    ['cancelled', async () => ({ok: true, status: 200, json: async () => ({cancelled: true})})],
    ['failed', async () => { throw new Error('offline'); }],
  ]) {
    answer = reply;
    const button = {dataset: {models: 'import'}, disabled: false};
    await press(button);
    assert.equal(button.disabled, false, `back after the request ${what}`);
  }
  assert.equal(page.el('toastText').textContent, 'offline');
  delete globalThis.fetch;
});
