// 跑一条走查流程（#453）。由 python3 -m tools.ui_walk 调用，每条流程一个进程、一个刚复制的合成图库和一个宿主：
//   node tools/ui_walk/run.mjs --config <配置.json>
// 配置：{flow, lang, url, port（浏览器调试端口）, out（结果目录）, names（uuid → 文件名）, shots}
//   node tools/ui_walk/run.mjs --list   列出流程（名字和说明，JSON）
// 结果写到 <out>/<lang>/<flow>.json；截图在同一目录。

import {mkdirSync, readFileSync, writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {Page} from './cdp.mjs';
import {Walker} from './walker.mjs';
import {FLOWS} from './flows.mjs';

const args = process.argv.slice(2);
if (args[0] === '--list') {
  process.stdout.write(JSON.stringify(FLOWS.map(({name, title, covers = []}) => ({name, title, covers}))) + '\n');
  process.exit(0);
}
const config = JSON.parse(readFileSync(args[args.indexOf('--config') + 1], 'utf8'));
const flow = FLOWS.find(f => f.name === config.flow);
if (!flow) { console.error('没有这条流程：' + config.flow); process.exit(2); }
// 看门狗：CDP 或页面无响应时保留已完成步骤，直接报告失败。
let current = null, bootMs = null;
const watchdog = setTimeout(() => {
  console.error('走查超时：' + config.flow);
  // 写出已经走过的步骤，便于看出挂在哪一步。
  if (current) {
    const partial = current.result({title: flow.title, covers: flow.covers || [], crashed: `超时：最后完成的是第 ${current.n} 步`});
    try { writeFileSync(join(out, flow.name + '.partial.json'), JSON.stringify(partial, null, 1)); } catch { /* 写不了就算了 */ }
  }
  process.exit(3);
}, (config.timeout || 150) * 1000);

const out = join(config.out, config.lang);
mkdirSync(out, {recursive: true});
const ACCEPT = {zh: 'zh-CN,zh', en: 'en-US,en'}[config.lang] || config.lang;

const page = await Page.open(config.port);
const started = Date.now();
let result;
try {
  // 界面语言跟随浏览器（设置里默认「跟随系统」）：navigator.languages 由 acceptLanguage 决定。
  const {userAgent} = await page.send('Browser.getVersion');
  await page.send('Emulation.setUserAgentOverride', {userAgent, acceptLanguage: ACCEPT, platform: 'MacIntel'});
  await page.send('Emulation.setTimezoneOverride', {timezoneId: 'Asia/Shanghai'});
  await page.send('Page.addScriptToEvaluateOnNewDocument', {source: `window.__walkNames = ${JSON.stringify(config.names || {})};`});
  const w = new Walker(page, {flow: flow.name, out, shots: config.shots !== false, lang: config.lang});
  w.config = config;
  current = w;
  await boot(page, config.url);
  await flow.run(w, page);
  result = w.result({title: flow.title, covers: flow.covers || [], ms: Date.now() - started, bootMs});
} catch (e) {
  const extra = {title: flow.title, covers: flow.covers || [], crashed: String(e.stack || e), ms: Date.now() - started};
  result = current ? current.result(extra) : {flow: flow.name, lang: config.lang, steps: [], checks: [], ...extra};
} finally {
  await page.close();
}
writeFileSync(join(out, flow.name + '.json'), JSON.stringify(result, null, 1));
clearTimeout(watchdog);
process.exit(0);

// 打开宿主的一次性码地址（换会话 cookie），等启动画面消失、页面挂好；不刷新掩盖首次启动失败。
async function boot(page, url) {
  const bootStart = performance.now(), booted = `document.getElementById('rail') && !document.getElementById('splash')`;
  await page.navigate(url);
  const ready = await page.waitFor(booted, 15000);
  bootMs = Math.round(performance.now() - bootStart);
  if (!ready) {
    const why = await page.eval(`JSON.stringify({href: location.href.replace(/k=[^&#]+/, 'k=…'), title: document.title,
      rail: !!document.getElementById('rail'), splash: !!document.getElementById('splash'),
      visibility: document.visibilityState, text: (document.body?.innerText || '').slice(0, 200)})`).catch(e => String(e));
    const probe = await page.eval(`Promise.all(['/core/session.js?probe=1', '/api/bootstrap', '/favicon.svg'].map(async u => {
      const c = new AbortController(); setTimeout(() => c.abort(), 3000); const t = performance.now();
      try { const r = await fetch(u, {signal: c.signal}); await r.text(); return u + ' ' + r.status + ' ' + Math.round(performance.now() - t) + 'ms'; }
      catch (e) { return u + ' ' + e.name; } }))`).catch(e => String(e));
    throw new Error('页面没有启动（活页条没出现或启动画面没消失）：' + why + ' 再请求一次：' + JSON.stringify(probe) + ' 未完成的请求：' + JSON.stringify(page.stuck()));
  }
  await page.settle({quiet: 400, max: 8000});
}
