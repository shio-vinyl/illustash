"""界面性能测量（#683）：在隔离宿主 + 合成大图库上，用 Chromium 和 WebKit 跑同一组场景，记录帧、长任务、渲染各阶段
和滚动几屏以后的图形内存。

  python3 -m tools.ui_perf prepare [规格]                  # 建夹具、编译 WebKit 小程序（经 collab_run.py bg）
  python3 -m tools.ui_perf run [--root 检出] [--engine chromium,webkit] [--scenario …] [--runs 3] [--warmup 1] [规格]
                                                          # 计时：经 collab_run.py timing
  python3 -m tools.ui_perf report <运行目录>                # 按 runs/ 里的结果重写 report.md
  python3 -m tools.ui_perf list                            # 场景

规格：--images（默认 30000）、--days（45）、--characters（300）、--works（30）、--incoming（3000），见 fixture.py。

**宿主和前端**：宿主程序是 ILLUSTASH_TERMS_BIN，没设时取当前检出构建的那个（tools/dev_preview.py 的 binary()）；测性能要用
release 构建（debug 版生成缩略图、回读接口都慢得多，数字不代表真实情况），报告里写明是哪一个。`--root` 是宿主的 --root：
从 <root>/web/static 读前端，从 <root>/tags 读标签表；默认是当前检出。改前、改后用同一个宿主程序、同一份夹具，只换
--root，比较的就只是前端。

**场景**（SCENARIOS）：首页滚动（空闲）、首页同步导入中（先停在最上面看新图一批批进来，再往下滚）、图库网格滚动、大图
翻页、资料库长列表。每个场景每次用一份新恢复的图库（缩略图缓存保留）和新起的宿主、浏览器；先跑 --warmup 次不记（把这
个场景要用的缩略图生成好），再跑 --runs 次，报告取中位数。同步导入的新图是每次新导入的，缩略图当场生成，和真实同步一样。

**两个内核**：
- Chromium：chrome-headless-shell（查找顺序同 tools/ui_walk/browser.py），默认 `--chromium-mode gpu`：`--enable-gpu
  --use-angle=metal`，GPU 合成和光栅化都开着（本机实测 SystemInfo 的 gpu_compositing、rasterization 为 enabled，Metal
  渲染器），GPU 进程的图形内存才代表真实情况；`software` 是 `--disable-gpu`（SwiftShader，软件合成），图形内存不作数。
  经 DevTools 协议录性能记录（chromium.mjs）。
- WebKit：webkit.swift 编译成的小程序，系统 WKWebView 打开同一个地址（会在屏幕左上角开一个窗口，不抢焦点；测量期间
  别挡住它，被挡住时 WebKit 会停掉绘制）。没有性能记录和长任务 API，只有 probe.js 自己的帧间隔和 DOM 指标，内存读
  WebContent 进程和 WebKit GPU 进程。

两边视口都是 1440×900，`--dpr` 默认 2（本机屏幕是 Retina 时 WebKit 窗口自然是 2；首页按倍率选缩略图档位，#452），
滚动都是页面里每帧推进 scrollTop（probe.js），不经合成线程的滚动。

结果：<out>/runs/<内核>/<场景>-<n>.json（每次运行的全部步骤）、report.json、report.md（中位数和口径）。
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from tools import dev_preview
from tools.ui_perf import fixture
from tools.ui_walk.browser import find_chrome
from tools.ui_walk.host import Host

ROOT = dev_preview.ROOT
HERE = Path(__file__).resolve().parent
WIDTH, HEIGHT = 1440, 900
# 等子进程（一次场景运行）的上限：只防挂死（AGENTS.md「等待上限与性能断言」第 1 种）。
RUN_LIMIT = 900


def scroll(page: str, rounds: int, screens: int, label: str) -> list[dict]:
    steps = []
    for k in range(rounds):
        steps.append({'op': 'scroll', 'label': f'{label}{k + 1}', 'args': {'page': page, 'screens': screens, 'speed': 2400}})
        steps.append({'op': 'memory', 'label': f'滚 {(k + 1) * screens} 屏后', 'args': {'page': page}})
    return steps


# 每个场景的步骤：go 跳页，scroll / idle / flip 是测量窗口，memory 是内存检查点，traceStart / traceStop 圈出 Chromium
# 录性能记录的范围。依赖页面结构的细节在 probe.js 的 PAGES 里。
SCENARIOS: dict[str, dict] = {
    'home': {'title': '首页滚动（空闲）', 'plan': [
        {'op': 'go', 'args': {'page': 'home'}}, {'op': 'memory', 'label': '打开后', 'args': {'page': 'home'}},
        {'op': 'traceStart'}, *scroll('home', 5, 8, '滚动'), {'op': 'traceStop', 'label': '性能记录'},
        {'op': 'memory', 'label': '停下 3 秒后', 'args': {'page': 'home', 'settle': 3000}}]},
    'home-sync': {'title': '首页，同步导入中', 'sync': True, 'plan': [
        {'op': 'go', 'args': {'page': 'home'}}, {'op': 'memory', 'label': '打开后', 'args': {'page': 'home'}},
        {'op': 'syncStart', 'args': {'every': 4000}}, {'op': 'traceStart'},
        {'op': 'idle', 'label': '停在最上面 24 秒', 'args': {'page': 'home', 'ms': 24000}},
        {'op': 'memory', 'label': '停在最上面后', 'args': {'page': 'home'}},
        *scroll('home', 3, 8, '边导入边滚'), {'op': 'traceStop', 'label': '性能记录'},
        {'op': 'syncStop', 'label': '同步'}]},
    'library': {'title': '图库网格滚动', 'plan': [
        {'op': 'go', 'args': {'page': 'library'}}, {'op': 'memory', 'label': '打开后', 'args': {'page': 'library'}},
        {'op': 'traceStart'}, *scroll('library', 4, 10, '滚动'), {'op': 'traceStop', 'label': '性能记录'},
        {'op': 'memory', 'label': '停下 3 秒后', 'args': {'page': 'library', 'settle': 3000}}]},
    'viewer': {'title': '大图翻页', 'plan': [
        {'op': 'go', 'args': {'page': 'viewer'}}, {'op': 'memory', 'label': '打开前', 'args': {'page': 'viewer'}},
        {'op': 'traceStart'}, {'op': 'flip', 'label': '翻 40 张', 'args': {'flips': 40, 'every': 400}},
        {'op': 'traceStop', 'label': '性能记录'}, {'op': 'memory', 'label': '翻完 3 秒后', 'args': {'page': 'viewer', 'settle': 3000}}]},
    'characters': {'title': '资料库长列表', 'plan': [
        {'op': 'go', 'args': {'page': 'characters'}}, {'op': 'memory', 'label': '打开后', 'args': {'page': 'characters'}},
        {'op': 'traceStart'}, *scroll('characters', 3, 6, '滚动'), {'op': 'traceStop', 'label': '性能记录'},
        {'op': 'memory', 'label': '停下 3 秒后', 'args': {'page': 'characters', 'settle': 3000}}]},
}


def add_spec(parser: argparse.ArgumentParser) -> None:
    d = fixture.Spec()
    for name in ('images', 'days', 'characters', 'works', 'incoming'):
        parser.add_argument('--' + name, type=int, default=getattr(d, name))


def spec_of(args: argparse.Namespace) -> fixture.Spec:
    return fixture.Spec(args.images, args.days, args.characters, args.works, args.incoming)


def webkit_binary() -> Path:
    """编译 webkit.swift（按源码哈希缓存在 .perf.noindex/bin/）。"""
    source = HERE / 'webkit.swift'
    out = fixture.CACHE / 'bin' / f'perf-webkit-{hashlib.sha256(source.read_bytes()).hexdigest()[:12]}'
    if not out.is_file():
        out.parent.mkdir(parents=True, exist_ok=True)
        for old in out.parent.glob('perf-webkit-*'):
            old.unlink()
        print('编译 WebKit 小程序…', flush=True)
        subprocess.run(['swiftc', '-O', str(source), '-o', str(out)], check=True)
    return out


def node() -> str:
    exe = os.environ.get('ILLUSTASH_WALK_NODE') or shutil.which('node')
    if not exe:
        sys.exit('找不到 node（需要 Node 22 或更新）。')
    return exe


class Chromium:
    def __init__(self, mode: str, dpr: float):
        self.profile = Path(tempfile.mkdtemp(prefix='illustash-perf-chrome-'))
        gpu = ['--enable-gpu', '--use-angle=metal'] if mode == 'gpu' else ['--disable-gpu']
        args = [str(find_chrome()), '--remote-debugging-port=0', f'--user-data-dir={self.profile}', '--no-first-run',
                '--no-default-browser-check', '--hide-scrollbars', '--mute-audio', '--disable-extensions', '--disable-sync',
                '--disable-background-timer-throttling', '--disable-renderer-backgrounding',
                '--disable-backgrounding-occluded-windows', f'--force-device-scale-factor={dpr}',
                f'--window-size={WIDTH},{HEIGHT}', *gpu, 'about:blank']
        self.process = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env={**os.environ, 'TZ': fixture.TZ})
        port_file = self.profile / 'DevToolsActivePort'
        deadline = time.monotonic() + 60
        while not port_file.exists() or not port_file.read_text().strip():
            if self.process.poll() is not None or time.monotonic() > deadline:
                self.close()
                raise RuntimeError('无头浏览器没有启动')
            time.sleep(0.05)
        self.port = int(port_file.read_text().split()[0])

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
        shutil.rmtree(self.profile, ignore_errors=True)


def run_once(engine: str, name: str, fx: fixture.Fixture, core: Path, root: Path, args: argparse.Namespace, scratch: Path) -> dict:
    scenario = SCENARIOS[name]
    library = fx.reset()
    with Host(core, root, library, fx.live, extra=fixture.QUIET, env={'TZ': fixture.TZ}) as host:
        batches = fixture.Batches(fx) if scenario.get('sync') else None
        if batches:
            batches.__enter__()
        try:
            params = {'url': host.url, 'plan': scenario['plan'], 'width': WIDTH, 'height': HEIGHT}
            if engine == 'chromium':
                browser = Chromium(args.chromium_mode, args.dpr)
                try:
                    (scratch / 'args.json').write_text(json.dumps({**params, 'port': browser.port, 'dpr': args.dpr}), encoding='utf-8')
                    out = subprocess.run([node(), str(HERE / 'chromium.mjs'), str(scratch / 'args.json')], cwd=ROOT,
                                         capture_output=True, text=True, timeout=RUN_LIMIT)
                    if out.returncode or not out.stdout.strip():
                        raise RuntimeError('chromium.mjs 失败：' + out.stderr[-2000:])
                    result = json.loads(out.stdout)
                finally:
                    browser.close()
            else:
                target = scratch / 'webkit-out.json'
                target.unlink(missing_ok=True)
                (scratch / 'args.json').write_text(json.dumps({**params, 'probe': str(HERE / 'probe.js'), 'out': str(target),
                                                               'root': str(ROOT)}), encoding='utf-8')
                out = subprocess.run([str(webkit_binary()), str(scratch / 'args.json')], cwd=ROOT, capture_output=True, text=True,
                                     timeout=RUN_LIMIT)
                if out.returncode or not target.is_file():
                    raise RuntimeError('WebKit 小程序失败：' + out.stderr[-2000:])
                result = json.loads(target.read_text(encoding='utf-8'))
        finally:
            if batches:
                batches.__exit__()
                result_copied = batches.copied
            else:
                result_copied = 0
    if scenario.get('sync'):
        result['incoming_copied'] = result_copied
    return result


# ---------- 汇总 ----------

def leaves(value, prefix: str = '') -> dict[str, float]:
    out: dict[str, float] = {}
    if isinstance(value, dict):
        for k, v in value.items():
            out.update(leaves(v, f'{prefix}.{k}' if prefix else k))
    elif isinstance(value, (int, float)) and not isinstance(value, bool):
        out[prefix] = value
    return out


def flatten(result: dict) -> dict[str, float]:
    """一次运行 → {「步骤标签 | 指标路径」: 数}。没有标签的步骤（go、traceStart）不算。"""
    out: dict[str, float] = {}
    for step in result.get('steps', []):
        if step.get('label'):
            for k, v in leaves(step.get('value')).items():
                out[f"{step['label']} | {k}"] = v
    return out


def median_of(runs: list[dict]) -> dict[str, float]:
    flat = [flatten(r) for r in runs if not r.get('error')]
    keys = sorted({k for f in flat for k in f}, key=lambda k: list(flat[0]).index(k) if flat and k in flat[0] else 1e9)
    return {k: round(statistics.median([f[k] for f in flat if k in f]), 1) for k in keys}


def machine() -> dict:
    def sysctl(name: str) -> str:
        return subprocess.run(['sysctl', '-n', name], capture_output=True, text=True).stdout.strip()
    return {'model': sysctl('hw.model'), 'cpu': sysctl('machdep.cpu.brand_string'), 'cores': sysctl('hw.ncpu'),
            'memory_gb': round(int(sysctl('hw.memsize') or 0) / 2 ** 30), 'macos': platform.mac_ver()[0], 'arch': platform.machine()}


def git_head(root: Path) -> str:
    r = subprocess.run(['git', '-C', str(root), 'rev-parse', '--short', 'HEAD'], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else ''


# 报告里每个场景展示的指标：（标题，指标路径，适用的步骤类型）。
SHOWN = [('帧/秒', 'frames.fps'), ('帧间隔 p95', 'frames.p95'), ('最长帧', 'frames.max'), ('>50ms 帧', 'frames.over50'),
         ('长任务数', 'longtasks.count'), ('最长任务', 'longtasks.max'), ('删掉的已显示图', 'dom.removedImages'),
         ('其中在视野里', 'dom.removedVisible'), ('翻页到显示 p50', 'flip.p50'), ('翻页到显示 p95', 'flip.p95')]
TRACE = [('主线程忙', 'mainBusy'), ('长任务数', 'longtasks.count'), ('长任务合计', 'longtasks.total'), ('最长任务', 'longtasks.max'),
         ('样式重算', 'style.ms'), ('布局', 'layout.ms'), ('绘制', 'paint.ms'), ('脚本', 'script.ms'), ('GC', 'gc.ms'),
         ('图片解码', 'decode.ms'), ('解码次数', 'decode.count'), ('光栅化', 'raster.ms')]
MEMORY = [('GPU 进程图形', 'gpu.graphics'), ('GPU 进程总计', 'gpu.footprint'), ('渲染进程图形', 'renderer.graphics'),
          ('渲染进程总计', 'renderer.footprint'), ('WebContent 图形', 'webcontent.graphics'), ('WebContent 总计', 'webcontent.footprint'),
          ('元素', 'dom.elements'), ('<img>', 'dom.images'), ('JS 堆', 'dom.jsHeap')]


def fmt(x) -> str:
    return '—' if x is None else (f'{x:g}' if isinstance(x, (int, float)) else str(x))


def render(report: dict) -> str:
    meta = report['meta']
    lines = ['# 界面性能测量（#683）', '',
             f"- 时间：{meta['started']}；机器：{meta['machine']['model']}（{meta['machine']['cpu']}，{meta['machine']['cores']} 核，"
             f"{meta['machine']['memory_gb']} GB），macOS {meta['machine']['macos']} {meta['machine']['arch']}",
             f"- 宿主：`{meta['core']}`（{meta['core_kind']}）；前端：`{meta['root']}`（{meta['root_head'] or '不是 git 检出'}）",
             f"- 合成图库：{meta['fixture']['assets']} 张，{meta['fixture']['days']} 个加入日期（最多的一天 {meta['fixture']['largest_day']} 张），"
             f"{meta['fixture']['spec']['characters']} 个角色、{meta['fixture']['spec']['works']} 部作品，有角色的图 {meta['fixture']['with_character']} 张",
             f"- 视口 {WIDTH}×{HEIGHT}，dpr {meta['dpr']}；Chromium 模式 {meta['chromium_mode']}；每个场景预热 {meta['warmup']} 次不记，"
             f"记 {meta['runs']} 次，下表是中位数（毫秒、MiB、次数）", '']
    for engine, scenarios in report['results'].items():
        lines += [f'## {engine}：{report["versions"].get(engine, "")}', '']
        for name, data in scenarios.items():
            med = data['median']
            lines += [f"### {SCENARIOS[name]['title']}（{name}，{data['ok']}/{data['total']} 次成功）", '']
            if data.get('errors'):
                lines += ['失败：' + '；'.join(data['errors']), '']
            steps = list(dict.fromkeys(k.split(' | ')[0] for k in med))
            measured = [s for s in steps if s != '性能记录' and any(f'{s} | {p}' in med for _, p in SHOWN)]
            if measured:
                cols = [(t, p) for t, p in SHOWN if any(f'{s} | {p}' in med for s in measured)]
                lines += ['| 窗口 | ' + ' | '.join(t for t, _ in cols) + ' |', '|---' * (len(cols) + 1) + '|']
                lines += [f'| {s} | ' + ' | '.join(fmt(med.get(f'{s} | {p}')) for _, p in cols) + ' |' for s in measured]
                lines.append('')
            if any(k.startswith('性能记录 | ') for k in med):
                cols = [(t, p) for t, p in TRACE if f'性能记录 | {p}' in med]
                lines += ['性能记录（整段，主线程；解码、光栅化含所有线程）：', '',
                          '| ' + ' | '.join(t for t, _ in cols) + ' |', '|---' * len(cols) + '|',
                          '| ' + ' | '.join(fmt(med[f'性能记录 | {p}']) for _, p in cols) + ' |', '']
            mem = [s for s in steps if any(f'{s} | {p}' in med for _, p in MEMORY[:6])]
            if mem:
                cols = [(t, p) for t, p in MEMORY if any(f'{s} | {p}' in med for s in mem)]
                lines += ['| 内存检查点 | ' + ' | '.join(t for t, _ in cols) + ' |', '|---' * (len(cols) + 1) + '|']
                lines += [f'| {s} | ' + ' | '.join(fmt(med.get(f'{s} | {p}')) for _, p in cols) + ' |' for s in mem]
                lines.append('')
            if '同步 | started' in med:
                lines += [f"同步：发起 {fmt(med['同步 | started'])} 次，导入 {fmt(med.get('同步 | imported'))} 张，"
                          f"测量期间放进来源文件夹 {fmt(data.get('incoming_copied'))} 张。", '']
    return '\n'.join(lines) + '\n'


def summarize(out: Path, meta: dict) -> dict:
    results: dict[str, dict] = {}
    versions: dict[str, str] = {}
    for engine_dir in sorted((out / 'runs').iterdir()):
        for name in SCENARIOS:
            runs = [json.loads(p.read_text(encoding='utf-8')) for p in sorted(engine_dir.glob(f'{name}-[0-9]*.json'))]
            if not runs:
                continue
            ok = [r for r in runs if not r.get('error')]
            versions[engine_dir.name] = next((r.get('browser') for r in runs if r.get('browser')), '')
            results.setdefault(engine_dir.name, {})[name] = {
                'total': len(runs), 'ok': len(ok), 'median': median_of(ok) if ok else {},
                'errors': [r['error'] for r in runs if r.get('error')],
                'incoming_copied': statistics.median([r.get('incoming_copied', 0) for r in ok]) if ok and 'incoming_copied' in ok[0] else None}
    report = {'meta': meta, 'versions': versions, 'results': results}
    (out / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=1), encoding='utf-8')
    (out / 'report.md').write_text(render(report), encoding='utf-8')
    return report


# ---------- 命令 ----------

def cmd_prepare(args: argparse.Namespace) -> int:
    core = dev_preview.binary()
    fx = fixture.cached(spec_of(args), core, jobs=args.jobs)
    print(json.dumps(fx.summary, ensure_ascii=False, indent=1))
    print('WebKit 小程序：', webkit_binary())
    return 0


def cmd_run(args: argparse.Namespace) -> int:
    core = dev_preview.binary()
    root = (args.root or ROOT).resolve()
    if not (root / 'web' / 'static' / 'index.html').is_file():
        sys.exit(f'{root} 下没有 web/static/index.html')
    engines = args.engine.split(',')
    names = args.scenario.split(',') if args.scenario else list(SCENARIOS)
    unknown = [n for n in names if n not in SCENARIOS] + [e for e in engines if e not in ('chromium', 'webkit')]
    if unknown:
        sys.exit('不认识：' + '、'.join(unknown))
    fx = fixture.cached(spec_of(args), core)
    if 'webkit' in engines:
        webkit_binary()
    stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S')
    out = (args.out or fixture.CACHE / 'runs' / stamp).resolve()
    meta = {'started': stamp, 'machine': machine(), 'core': str(core),
            'core_kind': 'release' if '/release/' in str(core) else 'debug' if '/debug/' in str(core) else '未知构建',
            'root': str(root), 'root_head': git_head(root), 'fixture': fx.summary, 'dpr': args.dpr,
            'chromium_mode': args.chromium_mode, 'warmup': args.warmup, 'runs': args.runs, 'engines': engines, 'scenarios': names}
    (out / 'runs').mkdir(parents=True, exist_ok=True)
    (out / 'meta.json').write_text(json.dumps(meta, ensure_ascii=False, indent=1), encoding='utf-8')
    with tempfile.TemporaryDirectory(prefix='illustash-perf-') as tmp:
        for engine in engines:
            (out / 'runs' / engine).mkdir(exist_ok=True)
            for name in names:
                for k in range(args.warmup + args.runs):
                    kind = '预热' if k < args.warmup else f'第 {k - args.warmup + 1} 次'
                    print(f'{engine} {name} {kind}…', flush=True)
                    try:
                        result = run_once(engine, name, fx, core, root, args, Path(tmp))
                    except (RuntimeError, subprocess.TimeoutExpired) as e:
                        result = {'error': str(e)[:2000], 'steps': []}
                    if result.get('error'):
                        print('  失败：' + result['error'][:300], flush=True)
                    if k >= args.warmup:
                        (out / 'runs' / engine / f'{name}-{k - args.warmup + 1}.json').write_text(
                            json.dumps(result, ensure_ascii=False, indent=1), encoding='utf-8')
    fx.reset()
    summarize(out, meta)
    print(f'报告：{out / "report.md"}')
    return 0


def cmd_report(args: argparse.Namespace) -> int:
    out = args.dir.resolve()
    summarize(out, json.loads((out / 'meta.json').read_text(encoding='utf-8')))
    print(out / 'report.md')
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(prog='python3 -m tools.ui_perf', description='界面性能测量（#683）')
    sub = parser.add_subparsers(dest='cmd', required=True)
    p = sub.add_parser('prepare', help='建夹具、编译 WebKit 小程序')
    add_spec(p)
    p.add_argument('--jobs', type=int, default=3, help='画图的进程数')
    p = sub.add_parser('run', help='跑场景（经 collab_run.py timing）')
    add_spec(p)
    p.add_argument('--root', type=Path, help='宿主的 --root（读 <root>/web/static）；默认当前检出')
    p.add_argument('--engine', default='chromium,webkit')
    p.add_argument('--scenario', help='逗号分隔，默认全部：' + ','.join(SCENARIOS))
    p.add_argument('--runs', type=int, default=3)
    p.add_argument('--warmup', type=int, default=1)
    p.add_argument('--dpr', type=float, default=2)
    p.add_argument('--chromium-mode', choices=['gpu', 'software'], default='gpu')
    p.add_argument('--out', type=Path)
    p = sub.add_parser('report', help='按 runs/ 重写报告')
    p.add_argument('dir', type=Path)
    sub.add_parser('list', help='列出场景')
    args = parser.parse_args()
    if args.cmd == 'list':
        for name, s in SCENARIOS.items():
            print(f"{name}\t{s['title']}")
        return 0
    return {'prepare': cmd_prepare, 'run': cmd_run, 'report': cmd_report}[args.cmd](args)


if __name__ == '__main__':
    sys.exit(main())
