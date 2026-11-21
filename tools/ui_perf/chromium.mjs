// 界面性能测量（#683）的 Chromium 驱动：经 DevTools 协议打开隔离宿主的页面，注入 probe.js，按计划逐步执行，并在
// 计划标出的窗口里录性能记录（Tracing），按主线程的事件汇总长任务、样式重算、布局、绘制、脚本、GC，以及所有线程上
// 的图片解码和光栅化。内存检查点读 GPU 进程和渲染进程的 footprint（tools/ui_perf/footprint.py）。
//
// 用法（由 tools/ui_perf/__main__.py 调用）：node chromium.mjs <参数 JSON 文件>，参数见 main()。结果 JSON 写到 stdout。
import {execFileSync} from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {Page, sleep} from '../ui_walk/cdp.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..', '..');

// 录性能记录的类别：主线程的任务和渲染阶段、帧，以及图片解码和光栅化所在的类别。
const CATEGORIES = ['toplevel', 'devtools.timeline', 'disabled-by-default-devtools.timeline',
  'disabled-by-default-devtools.timeline.frame', 'v8.execute', 'blink.user_timing'];
// 主线程上按名字归类的事件；同一类里嵌套的只算最外层。
const GROUPS = {
  style: ['UpdateLayoutTree', 'RecalculateStyles'],
  layout: ['Layout'],
  paint: ['PrePaint', 'Paint', 'Layerize', 'UpdateLayer', 'CompositeLayers', 'Commit'],
  script: ['FunctionCall', 'EvaluateScript', 'v8.evaluateModule', 'TimerFire', 'FireAnimationFrame', 'RunMicrotasks', 'v8.compile'],
  gc: ['MajorGC', 'MinorGC', 'V8.GCScavenger', 'V8.GCFinalizeMC', 'V8.GC_MC_BACKGROUND_MARKING', 'BlinkGC.AtomicPausePhase'],
  parseHTML: ['ParseHTML'],
};
// 所有线程上的：图片解码和光栅化。
const ANYWHERE = {
  decode: ['Decode Image', 'ImageDecodeTask', 'Decode LazyPixelRef', 'ImageFrameGenerator::decode'],
  raster: ['RasterTask'],
};
const LONG = 50;

class Browser {
  constructor(ws) { this.ws = ws; this.id = 0; this.waiting = new Map(); this.events = [];
    ws.addEventListener('message', e => { const m = JSON.parse(e.data); if (m.id) { const w = this.waiting.get(m.id); this.waiting.delete(m.id); if (w) m.error ? w.reject(new Error(m.error.message)) : w.resolve(m.result); } else for (const f of this.events) f(m); }); }
  static async open(port) {
    const v = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
    const ws = new WebSocket(v.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = () => reject(new Error('连不上浏览器的 CDP')); });
    const b = new Browser(ws); b.version = v.Browser; return b;
  }
  send(method, params = {}) { const id = ++this.id; this.ws.send(JSON.stringify({id, method, params})); return new Promise((resolve, reject) => this.waiting.set(id, {resolve, reject})); }
  once(method) { return new Promise(resolve => { const f = m => { if (m.method === method) { this.events = this.events.filter(x => x !== f); resolve(m.params); } }; this.events.push(f); }); }
}

async function readTrace(browser) {
  const done = browser.once('Tracing.tracingComplete');
  await browser.send('Tracing.end');
  const {stream} = await done;
  let text = '';
  for (;;) {
    const r = await browser.send('IO.read', {handle: stream, size: 1 << 20});
    text += r.base64Encoded ? Buffer.from(r.data, 'base64').toString() : r.data;
    if (r.eof) break;
  }
  await browser.send('IO.close', {handle: stream});
  const parsed = JSON.parse(text);
  return Array.isArray(parsed) ? parsed : parsed.traceEvents;
}

// 只算最外层：按开始时间排序后，被同类上一个事件完全包住的跳过。
function outer(events) {
  const sorted = [...events].sort((a, b) => a.ts - b.ts || b.dur - a.dur);
  const out = []; let end = -Infinity;
  for (const e of sorted) { if (e.ts >= end) { out.push(e); end = e.ts + e.dur; } }
  return out;
}
const ms = us => Math.round(us / 100) / 10;

export function summarize(events) {
  const threads = new Map();
  for (const e of events) if (e.ph === 'M' && e.name === 'thread_name') threads.set(`${e.pid}:${e.tid}`, e.args?.name);
  const complete = events.filter(e => e.ph === 'X' && typeof e.dur === 'number');
  // 页面的主线程：事件最多的 CrRendererMain。
  const counts = new Map();
  for (const e of complete) { const k = `${e.pid}:${e.tid}`; if (threads.get(k) === 'CrRendererMain') counts.set(k, (counts.get(k) || 0) + 1); }
  const main = [...counts].sort((a, b) => b[1] - a[1])[0]?.[0];
  const onMain = complete.filter(e => `${e.pid}:${e.tid}` === main);
  const tasks = outer(onMain.filter(e => e.name === 'RunTask' || e.name === 'ThreadControllerImpl::RunTask'));
  const long = tasks.filter(e => e.dur > LONG * 1000);
  const out = {
    longtasks: {count: long.length, total: ms(long.reduce((a, e) => a + e.dur, 0)), max: ms(Math.max(0, ...long.map(e => e.dur))),
      over100: long.filter(e => e.dur > 100000).length},
    mainBusy: ms(tasks.reduce((a, e) => a + e.dur, 0)),
  };
  for (const [group, names] of Object.entries(GROUPS)) {
    const hit = outer(onMain.filter(e => names.includes(e.name)));
    out[group] = {ms: ms(hit.reduce((a, e) => a + e.dur, 0)), count: hit.length};
  }
  for (const [group, names] of Object.entries(ANYWHERE)) {
    const hit = complete.filter(e => names.includes(e.name));
    const byThread = {};
    for (const e of hit) { const n = threads.get(`${e.pid}:${e.tid}`) || '?'; byThread[n] = (byThread[n] || 0) + e.dur; }
    out[group] = {ms: ms(hit.reduce((a, e) => a + e.dur, 0)), count: hit.length,
      threads: Object.fromEntries(Object.entries(byThread).sort((a, b) => b[1] - a[1]).slice(0, 3).map(([k, v]) => [k, ms(v)]))};
  }
  return out;
}

function footprint(pids) {
  const text = execFileSync('python3', ['-m', 'tools.ui_perf.footprint', ...pids.map(String)], {cwd: ROOT, encoding: 'utf8'});
  return JSON.parse(text);
}

async function main() {
  // {port, url, plan: [{op, args, label}], width, height, dpr, probe}
  const args = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
  const browser = await Browser.open(args.port);
  const page = await Page.open(args.port, {width: args.width, height: args.height, timeout: 600000});
  await page.send('Emulation.setDeviceMetricsOverride', {width: args.width, height: args.height, deviceScaleFactor: args.dpr, mobile: false});
  await page.navigate(args.url);
  const probe = fs.readFileSync(path.join(HERE, 'probe.js'), 'utf8');
  const call = async (op, a = {}) => {
    const r = await page.eval(`${probe}\n;window.__perf.call(${JSON.stringify(op)}, ${JSON.stringify(a)})`);
    if (!r.ok) throw new Error(`${op}：${r.error}`);
    return r.value;
  };
  const steps = [];
  let tracing = false;
  const result = {engine: 'chromium', browser: browser.version, steps};
  try {
    result.boot = await call('boot');
    for (const step of args.plan) {
      const t0 = Date.now();
      let value;
      if (step.op === 'traceStart') {
        await browser.send('Tracing.start', {traceConfig: {includedCategories: CATEGORIES, recordMode: 'recordContinuously'}, transferMode: 'ReturnAsStream'});
        tracing = true; value = {};
      } else if (step.op === 'traceStop') {
        value = summarize(await readTrace(browser)); tracing = false;
      } else if (step.op === 'memory') {
        if (step.args?.settle) await sleep(step.args.settle);
        const {processInfo} = await browser.send('SystemInfo.getProcessInfo');
        const wanted = processInfo.filter(p => ['GPU', 'renderer'].includes(p.type));
        const fp = footprint(wanted.map(p => p.id));
        value = {};
        for (const p of wanted) { const k = p.type === 'GPU' ? 'gpu' : 'renderer'; const r = fp[String(p.id)];
          // 渲染进程可能不止一个（about:blank 留下的）：取占用最大的。
          if (!value[k] || (r.footprint || 0) > (value[k].footprint || 0)) value[k] = r; }
        value.dom = await call('snapshot', step.args || {});
      } else {
        value = await call(step.op, step.args || {});
      }
      steps.push({op: step.op, label: step.label || '', seconds: Math.round((Date.now() - t0) / 100) / 10, value});
    }
  } catch (e) {
    result.error = String(e.message || e);
    if (tracing) { try { await readTrace(browser); } catch { /* 丢掉 */ } }
  }
  result.console = page.drainConsole();
  await page.close();
  process.stdout.write(JSON.stringify(result));
  process.exit(0);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
