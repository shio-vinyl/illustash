"""界面性能测量工具（tools/ui_perf，#683）自身的检查：不起宿主、不开浏览器。

真正的测量要宿主、无头浏览器和 WebKit 小程序，在本机按 docs/LOCAL_CHECKS.md「界面性能测量」经 collab_run.py timing
运行；这里只核对离线的部分：夹具的规划（张数、分天、角色归属）、合成图的文件格式、性能记录的汇总和报告的中位数。
"""
import json
import subprocess
import tempfile
import unittest
import zlib
from pathlib import Path

from tools.ui_perf import __main__ as perf
from tools.ui_perf import fixture

ROOT = Path(__file__).resolve().parent.parent


class FixtureTest(unittest.TestCase):
    def test_days_cover_all_images_and_are_deterministic(self):
        spec = fixture.Spec(images=3000, days=12)
        days = fixture.day_counts(spec)
        self.assertEqual(len(days), 12)
        self.assertEqual(sum(n for _, n in days), 3000)
        self.assertEqual(len({d for d, _ in days}), 12)
        self.assertEqual(max(d for d, _ in days), fixture.END_DAY)
        self.assertEqual(days, fixture.day_counts(spec))

    def test_plan_puts_each_image_in_its_day(self):
        spec = fixture.Spec(images=500, days=6, incoming=40)
        library, incoming = fixture.plan(spec)
        self.assertEqual((len(library), len(incoming)), (500, 40))
        self.assertEqual(len({p.rel for p in library}), 500)
        self.assertEqual(len({p.ident for p in library + incoming}), 540)
        self.assertTrue(all(p.stamp == 0 for p in incoming))
        import datetime
        from zoneinfo import ZoneInfo
        zone = ZoneInfo(fixture.TZ)
        for p in library[::37]:
            day = datetime.datetime.fromtimestamp(p.stamp, zone).strftime('%Y%m%d')
            self.assertEqual(p.rel.split('/')[0], day)

    def test_stamped_png_is_valid_and_bytes_differ(self):
        base = fixture.png(64, 48, 3)
        a, b = fixture.stamped(base, 1), fixture.stamped(base, 2)
        self.assertNotEqual(a, b)
        for data in (a, b):
            self.assertEqual(data[:8], b'\x89PNG\r\n\x1a\n')
            pos, kinds = 8, []
            while pos < len(data):
                n = int.from_bytes(data[pos:pos + 4], 'big')
                kind, body = data[pos + 4:pos + 8], data[pos + 8:pos + 8 + n]
                self.assertEqual(int.from_bytes(data[pos + 8 + n:pos + 12 + n], 'big'), zlib.crc32(kind + body))
                kinds.append(kind)
                pos += 12 + n
            self.assertEqual(kinds, [b'IHDR', b'tEXt', b'IDAT', b'IEND'])

    def test_memberships_are_long_tailed_and_bounded(self):
        spec = fixture.Spec(images=4000, characters=40)
        owned = fixture.memberships(spec, 4000)
        self.assertEqual(len(owned), 40)
        per_image = {}
        for members in owned:
            for i in members:
                per_image[i] = per_image.get(i, 0) + 1
        self.assertLessEqual(max(per_image.values()), 2)
        self.assertGreater(len(owned[0]), 3 * len(owned[-1]))
        works, chars = fixture.names(spec)
        self.assertEqual(len(set(works + chars)), spec.works + spec.characters)


class ReportTest(unittest.TestCase):
    def test_median_and_render(self):
        runs = [{'browser': 'X', 'steps': [
            {'op': 'scroll', 'label': '滚动1', 'value': {'frames': {'fps': fps, 'p95': 18, 'over50': n}, 'dom': {'removedVisible': 0}}},
            {'op': 'memory', 'label': '滚 8 屏后', 'value': {'gpu': {'graphics': g, 'footprint': 400}, 'dom': {'elements': 900}}}]}
            for fps, n, g in ((58, 1, 300), (60, 0, 320), (59, 3, 310))]
        med = perf.median_of(runs)
        self.assertEqual(med['滚动1 | frames.fps'], 59)
        self.assertEqual(med['滚动1 | frames.over50'], 1)
        self.assertEqual(med['滚 8 屏后 | gpu.graphics'], 310)
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            (out / 'runs' / 'chromium').mkdir(parents=True)
            for k, r in enumerate(runs, 1):
                (out / 'runs' / 'chromium' / f'home-{k}.json').write_text(json.dumps(r), encoding='utf-8')
            # home-sync 的结果不能算进 home。
            (out / 'runs' / 'chromium' / 'home-sync-1.json').write_text(json.dumps({'error': 'x', 'steps': []}), encoding='utf-8')
            meta = {'started': 't', 'machine': {'model': 'M', 'cpu': 'C', 'cores': '8', 'memory_gb': 16, 'macos': '26', 'arch': 'arm64'},
                    'core': 'c', 'core_kind': 'release', 'root': 'r', 'root_head': 'abc', 'dpr': 2, 'chromium_mode': 'gpu',
                    'warmup': 1, 'runs': 3, 'fixture': {'assets': 10, 'days': 2, 'largest_day': 6, 'with_character': 5,
                                                        'spec': {'characters': 3, 'works': 1}}}
            report = perf.summarize(out, meta)
            self.assertEqual(report['results']['chromium']['home']['ok'], 3)
            self.assertEqual(report['results']['chromium']['home-sync']['ok'], 0)
            text = (out / 'report.md').read_text(encoding='utf-8')
            self.assertIn('| 滚动1 | 59 |', text)
            self.assertIn('| 滚 8 屏后 | 310 |', text)


class TraceTest(unittest.TestCase):
    def test_summarize_counts_outer_events_on_main_thread(self):
        events = [
            {'ph': 'M', 'name': 'thread_name', 'pid': 1, 'tid': 10, 'args': {'name': 'CrRendererMain'}},
            {'ph': 'M', 'name': 'thread_name', 'pid': 1, 'tid': 11, 'args': {'name': 'Compositor'}},
            {'ph': 'X', 'name': 'RunTask', 'pid': 1, 'tid': 10, 'ts': 0, 'dur': 80000},
            {'ph': 'X', 'name': 'FunctionCall', 'pid': 1, 'tid': 10, 'ts': 1000, 'dur': 30000},
            {'ph': 'X', 'name': 'FunctionCall', 'pid': 1, 'tid': 10, 'ts': 2000, 'dur': 10000},   # 嵌套，不重复算
            {'ph': 'X', 'name': 'Layout', 'pid': 1, 'tid': 10, 'ts': 40000, 'dur': 5000},
            {'ph': 'X', 'name': 'RunTask', 'pid': 1, 'tid': 10, 'ts': 100000, 'dur': 10000},
            {'ph': 'X', 'name': 'Decode Image', 'pid': 1, 'tid': 11, 'ts': 0, 'dur': 4000},
        ]
        script = ("import('./tools/ui_perf/chromium.mjs').then(m => process.stdout.write(JSON.stringify(m.summarize("
                  + json.dumps(events) + '))))')
        out = subprocess.run(['node', '--input-type=module', '-e', script], cwd=ROOT, capture_output=True, text=True, check=True)
        s = json.loads(out.stdout)
        self.assertEqual(s['longtasks'], {'count': 1, 'total': 80, 'max': 80, 'over100': 0})
        self.assertEqual(s['mainBusy'], 90)
        self.assertEqual(s['script'], {'ms': 30, 'count': 1})
        self.assertEqual(s['layout'], {'ms': 5, 'count': 1})
        self.assertEqual(s['decode']['ms'], 4)
        self.assertEqual(s['decode']['threads'], {'Compositor': 4})

    def test_scenarios_name_pages_the_probe_knows(self):
        probe = (ROOT / 'tools' / 'ui_perf' / 'probe.js').read_text(encoding='utf-8')
        for name, s in perf.SCENARIOS.items():
            for step in s['plan']:
                page = (step.get('args') or {}).get('page')
                if page:
                    self.assertIn(f'    {page}: {{', probe, f'{name} 用到的页面 {page} 不在 probe.js 的 PAGES 里')


if __name__ == '__main__':
    unittest.main()
