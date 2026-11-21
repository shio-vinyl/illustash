"""#151 安装后的适配器和现有的 App 生命周期，只用测试自己的合成图库包。

不构建也不查找二进制：由显式指定的候选宿主提供它旁边的 worker（或 ILLUSTASH_RESEARCH_ADAPTER_BIN）。
macOS 的直接测试在不配置 Python 的情况下运行安装后的原生适配器。计时测试沿用现有 #146 夹具的启动器/进程树
检查，围绕这个真实适配器产出的响应进行，从不 mock 计算程序。
"""
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest

import numpy as np
from PIL import Image

from tests import native_commands
from tests import test_native_lifecycle as lifecycle
from tests import test_native_research_compute as compute
from tests import test_native_relations_compute as relations
from tests import test_native_folder as folder_fixture
from tests import test_native_sync as sync_fixture
from tests.native_worker_fixture import command as fixture_command, exists as process_exists
from tests.fixed_expectations import unclocked
from tests.test_read_http import wait_until
from tools.wait_limit import WAIT

# 这些测试里的研究时限。产品默认值是 30 分钟；这两个值都不改变它。
WIDE_TIMEOUT_MS = '120000'
STALL_TIMEOUT_MS = '3000'


@unittest.skipUnless(sys.platform in ('darwin', 'win32'), 'Linux is frozen: no local computation')
class InstalledResearchComputeTest(compute.NativeResearchComputeTest):
    @classmethod
    def setUpClass(cls):
        configured = os.environ.get('ILLUSTASH_TERMS_BIN')
        if not configured:
            raise unittest.SkipTest('explicit candidate host and its built worker required')
        cls.core = cls.binary = Path(configured).resolve()
        cls.resources = Path(os.environ.get('ILLUSTASH_NATIVE_ROOT') or Path(__file__).resolve().parents[1]).resolve()
        cls.candidate_worker = Path(os.environ.get('ILLUSTASH_RESEARCH_ADAPTER_BIN') or
                                    cls.core.with_name('illustash-worker.exe' if os.name == 'nt' else 'illustash-worker')).resolve()
        if not cls.core.is_file() or not cls.candidate_worker.is_file():
            raise AssertionError('candidate host/worker missing; do not substitute an older binary')

    def setUp(self):
        compute.NativeResearchComputeTest.setUp(self)
        folder = Path(self.temp.name) / 'installed-bin'
        folder.mkdir()
        self.binary = folder / ('illustash-research.exe' if os.name == 'nt' else 'illustash-research')
        shutil.copy2(self.candidate_worker, self.binary)

    def test_unknown_kind_is_refused_without_partial_native_learning(self):
        before = self.rows(self.store.path)
        result, output, snapshot = self.request('unknown-research-kind', regroup=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('不支持的计算请求', result.stderr.decode(errors='replace'))
        self.assertFalse(output.exists())
        self.assertEqual(self.rows(snapshot / 'library.sqlite'), before)

    def test_duplicate_json_input_is_rejected_before_python_setup(self):
        root = Path(self.temp.name)
        request, response = root / 'duplicate.json', root / 'response.json'
        request.write_text('{"kind":"oc","format":"illustash-research/1","format":"illustash-research/1"}', encoding='utf-8')
        env = os.environ | {'PATH': ''}
        env.pop('ILLUSTASH_WORKER_PYTHON', None)
        result = subprocess.run([str(self.binary), str(request), str(response)], env=env,
                                capture_output=True, timeout=WAIT)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('ILLUSTASH_WORKER_PYTHON', result.stderr.decode(errors='replace'))
        self.assertFalse(response.exists())


class RecognitionTransitionRoutingTest(unittest.TestCase):
    setUpClass = InstalledResearchComputeTest.__dict__['setUpClass']

    def test_recognition_platform_gate_without_python_or_model_access(self):
        with tempfile.TemporaryDirectory(prefix='illustash-recognition-gate-') as temp:
            root = Path(temp)
            worker = root / ('illustash-recognition.exe' if os.name == 'nt' else 'illustash-recognition')
            shutil.copy2(self.candidate_worker, worker)
            request, output = root / 'request.json', root / 'response.json'
            request.write_text(json.dumps(dict(format='illustash-recognition/1', identity=dict(synthetic=True),
                                               assets=[], output=str(root / 'result'))), encoding='utf-8')
            # 在 macOS/Windows 上，原生的反例只看到这个测试自有根目录下不存在的资源；不访问真实的模型或安装。
            env = os.environ | dict(PATH='', ILLUSTASH_ROOT=str(root), ILLUSTASH_MODELS=str(root / 'absent-models'))
            env.pop('ILLUSTASH_WORKER_PYTHON', None)
            result = subprocess.run([str(worker), str(request), str(output)], env=env,
                                    capture_output=True, timeout=WAIT)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn('ILLUSTASH_WORKER_PYTHON', result.stderr.decode(errors='replace'))
            self.assertFalse(output.exists())
            self.assertFalse((root / 'result').exists())


class RecognitionTransitionReceiptTest(unittest.TestCase):
    """App 真实接收 #146 的合成 v1 协议夹具，从不做模型推理。

    只用生成的文件夹/PNG 和模型指纹的占位字节。
    复用 NativeSyncTest 的 worker 辅助函数；从不调用它的照片图库准备步骤。
    """
    setUpClass = InstalledResearchComputeTest.__dict__['setUpClass']
    _fixture_setup = folder_fixture.NativeFolderHTTPTest._fixture_setup
    package = folder_fixture.NativeFolderHTTPTest.package
    start = folder_fixture.NativeFolderHTTPTest.start
    stop = folder_fixture.NativeFolderHTTPTest.stop
    request = folder_fixture.NativeFolderHTTPTest.request
    sync = folder_fixture.NativeFolderHTTPTest.sync
    finish = sync_fixture.NativeSyncTest.finish
    protocol_fixture = sync_fixture.NativeSyncTest.worker
    def rows(self, tables):
        with sqlite3.connect(self.lib.db) as c:
            return {table: c.execute('SELECT * FROM ' + table + ' ORDER BY 1').fetchall() for table in tables}

    def setUp(self):
        folder_fixture.NativeFolderHTTPTest.setUp(self)
        self.stop(self.process)
        self.env.pop('ILLUSTASH_RECOGNITION_WORKER', None)
        self.env.pop('ILLUSTASH_RESEARCH_WORKER', None)
        self.env['ILLUSTASH_MODELS'] = str(self.resources / 'models')
        Image.new('RGB', (20, 30), 'red').save(self.new_folder / 'not-scanned.png')
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(self.sync()['state']['status'], 'needs_recognition')
        with sqlite3.connect(self.lib.source) as c:
            self.asset = c.execute('SELECT uuid FROM assets ORDER BY uuid').fetchone()[0]
        self.assertEqual(self.request('/api/tag', {'ids': [self.asset], 'tag': '合成人工保留'})['status'], 200)
        self.human = self.rows(['actions', 'assertions', 'asset_tags', 'term_verdicts'])

    def install_protocol_fixture(self, *, bad_sidecar=False):
        self.protocol_fixture()
        self.stop(self.process)
        script = self.root / 'synthetic-worker'
        if bad_sidecar:
            old = "Path(sys.argv[2]).write_text(json.dumps(dict(identity=r['identity'],packages=packages,failed=[])))"
            new = "(p/'character-claims.json').write_text('{}')\nPath(sys.argv[2]).write_text(json.dumps(dict(identity=r['identity'],packages=packages,failed=[],character_claims=[str(p/'character-claims.json')])))"
            source = script.read_text(encoding='utf-8')
            self.assertEqual(source.count(old), 1)
            script.write_text(source.replace(old, new), encoding='utf-8')
        named = self.root / ('illustash-recognition.cmd' if os.name == 'nt' else 'illustash-recognition')
        shutil.copy2(Path(self.env['ILLUSTASH_RECOGNITION_WORKER']), named)
        self.env['ILLUSTASH_RECOGNITION_WORKER'] = str(named)
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']

    def no_recognition_writes(self):
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute("SELECT count(*) FROM labels WHERE source='wd_v3'").fetchone()[0], 0)
            self.assertEqual(c.execute('SELECT count(*) FROM embeddings').fetchone()[0], 0)
            if c.execute("SELECT count(*) FROM sqlite_master WHERE name='recognition_imports'").fetchone()[0]:
                self.assertEqual(c.execute('SELECT count(*) FROM recognition_imports').fetchone()[0], 0)
        self.assertEqual(self.rows(list(self.human)), self.human)

    def test_native_library_new_legacy_reply_platform_gate_and_actual_receipt(self):
        self.install_protocol_fixture()
        state = self.sync()
        if sys.platform in ('darwin', 'win32'):
            self.assertEqual(state['state']['status'], 'error', state)
            self.assertIn('sidecar', state['state']['message'])
            self.no_recognition_writes()
        else:
            self.assertEqual(state['state']['status'], 'done', state)
            self.assertEqual(state['state']['imported'], 1, state)
            with sqlite3.connect(self.lib.source) as c:
                self.assertEqual(c.execute("SELECT status FROM recognition_imports").fetchall(), [('done',)])
                self.assertEqual(c.execute("SELECT model FROM embeddings WHERE uuid=? ORDER BY model", (self.asset,)).fetchall(), [('ccip',), ('wd',)])
                self.assertEqual(c.execute("SELECT value,source FROM labels WHERE uuid=? AND dim='rating'", (self.asset,)).fetchall(), [('general', 'wd_v3')])
            self.assertEqual(self.rows(list(self.human)), self.human)
        self.assertFalse(self.sentinel.exists())

    def test_native_library_new_declared_bad_sidecar_refuses_before_formal_import(self):
        self.install_protocol_fixture(bad_sidecar=True)
        state = self.sync()
        self.assertEqual(state['state']['status'], 'error', state)
        self.assertIn('character-claims.json', state['state']['message'])
        self.assertIn('missing field', state['state']['message'])
        self.no_recognition_writes()
        self.assertFalse(self.sentinel.exists())


@unittest.skipUnless(sys.platform in ('darwin', 'win32'), 'Linux is frozen: no local computation')
class NativeResearchPublicationTest(unittest.TestCase):
    setUpClass = InstalledResearchComputeTest.__dict__['setUpClass']
    package = lifecycle.NativeLifecycleTest.package
    start = lifecycle.NativeLifecycleTest.start
    stop = lifecycle.NativeLifecycleTest.stop
    request = lifecycle.NativeLifecycleTest.request

    def setUp(self):
        lifecycle.NativeLifecycleTest.setUp(self)
        self.stop(self.process)
        self.worker = self.bin / ('illustash-research.exe' if os.name == 'nt' else 'illustash-research')
        shutil.copy2(self.candidate_worker, self.worker)
        # 时限远超这里任何真实计算所需，即使机器繁忙也是如此：
        # 测试用自己的上限等结果，只有下面的超时用例设短时限（#219）。
        self.env.update(ILLUSTASH_RESEARCH_WORKER=str(self.worker), ILLUSTASH_RESEARCH_TIMEOUT_MS=WIDE_TIMEOUT_MS)
        self.env.pop('ILLUSTASH_WORKER_PYTHON', None)
        # #151 的基础夹具只做学习；#152 在下面扩展它。
        with sqlite3.connect(self.lib.source) as c:
            c.execute('CREATE TABLE IF NOT EXISTS embeddings(uuid TEXT,model TEXT,vec BLOB,PRIMARY KEY(uuid,model))')
            for i in range(9):
                ident = ('seed' if i < 6 else 'unmatched') + str(i)
                vector = np.zeros(768, np.float32)
                vector[0 if i < 6 else 1] = 1
                c.execute('INSERT INTO assets VALUES (?,?,?,?,?,?,?,?,?)',
                          (ident, ident + '.png', str(self.lib.path / (ident + '.png')), 120, 150, '2026-10-04', 'orig', 1, 0))
                c.execute("INSERT INTO embeddings VALUES (?,'ccip',?)", (ident, vector.tobytes()))
        # 原 Python Store 刷新并确认之后的图库包（录制，tests/fixed_expectations.py）。
        self.lib = self.rec.package('prepared', self.lib.path)
        self.entity = self.rec.value('entity')
        self.restart()
        self.finished()

    def restart(self):
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']

    def state(self):
        return json.loads(self.request('/api/clusters')['body'])['learner']

    def finished(self, status='idle'):
        wait_until(lambda: self.state()['status'] in ('idle', 'error'))
        self.assertEqual(self.state()['status'], status, self.state())

    def rows(self, tables):
        with sqlite3.connect(self.lib.db) as c:
            return {table: c.execute('SELECT * FROM ' + table + ' ORDER BY 1').fetchall() for table in tables}

    def timing_fixture(self):
        # 在 App 看到 worker 退出之前扣住一个真实的原生响应。这样不需要生产环境的延迟开关，
        # 就能做确定性的取消/指纹/重放测试。
        self.stop(self.process)
        self.mode, self.ready, self.release = (self.root / name for name in ('mode', 'ready', 'release'))
        self.saved = self.root / 'native-response.json'
        self.pid, self.child = self.root / 'fixture.pid', self.root / 'fixture-child.pid'
        # 打开图库包可能会启动它自己的启动任务。先让这个真实的原生任务完成，再扣住显式请求的 regroup，
        # 免得删除标记文件时抹掉已在运行的启动任务的就绪信号。
        self.mode.write_text('pass')
        bridge = self.root / 'research-timing-fixture'
        bridge.write_text('#!' + sys.executable + '\n' + f'''import json,os,sys,subprocess,time
from pathlib import Path
mode=Path({str(self.mode)!r}).read_text()
# Written whole, then renamed: a test that sees the file reads the full pid (#132).
Path({str(self.pid)!r}+'.tmp').write_text(str(os.getpid()));os.replace({str(self.pid)!r}+'.tmp',{str(self.pid)!r})
if mode=='replay':
    Path(sys.argv[2]).write_bytes(Path({str(self.saved)!r}).read_bytes())
    sys.exit(0)
# 'stall' holds a process tree without computing first, so a short deadline only has to fit this start.
if mode!='stall':
    code=subprocess.call([{str(self.worker)!r},sys.argv[1],sys.argv[2]])
    if code: sys.exit(code)
    if mode=='pass': sys.exit(0)
    Path({str(self.saved)!r}).write_bytes(Path(sys.argv[2]).read_bytes())
child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'])
Path({str(self.child)!r}+'.tmp').write_text(str(child.pid));os.replace({str(self.child)!r}+'.tmp',{str(self.child)!r})
Path({str(self.ready)!r}).write_text('native calculated')
deadline=time.monotonic()+25
while not Path({str(self.release)!r}).exists() and time.monotonic()<deadline: time.sleep(.01)
child.terminate();child.wait()
''', encoding='utf-8')
        self.env['ILLUSTASH_RESEARCH_WORKER'] = fixture_command(bridge)
        self.restart()
        self.finished()
        self.mode.write_text('hold')

    def held(self):
        for path in (self.ready, self.release, self.pid, self.child):
            path.unlink(missing_ok=True)
        self.assertEqual(self.request('/api/regroup', {})['body'], b'{"started": true}')
        # 就绪文件要等一次真实的研究计算完成，繁忙的机器可能让它远超 5 秒。
        wait_until(self.ready.exists)
        self.assertTrue(self.saved.exists())
        return int(self.pid.read_text()), int(self.child.read_text())

    def no_processes(self, pids):
        def pid_exists(pid):
            if os.name == 'nt':
                return process_exists(pid)
            try:
                os.kill(pid, 0)
                return True
            except ProcessLookupError:
                return False
        for pid in pids:
            wait_until(lambda: not pid_exists(pid))
        # 在 POSIX 上也保留 #146 的 worker 进程组检查。
        self.assertFalse(process_exists(pids[0]))

    def test_installed_startup_regroup_oc_original_body_and_repeat_preserve_history(self):
        before = self.rows(['actions', 'assertions'])
        self.assertGreater(len(self.rows(['prototypes'])['prototypes']), 0)
        self.assertGreater(len(self.rows(['clusters'])['clusters']), 0)
        for _ in range(2):
            self.assertEqual(self.request('/api/regroup', {})['body'], b'{"started": true}')
            self.finished()
        self.assertEqual(self.rows(['actions', 'assertions']), before)
        expected = self.rec.value('oc groups')
        response = self.request('/api/oc/groups?entity=' + self.entity)
        self.assertEqual(response['status'], 200, response)
        self.assertEqual(response['body'], expected.encode())
        self.assertFalse(self.sentinel.exists())

    def test_regroup_splits_by_lead_and_publishes_each_group_page(self):
        def get(url):
            reply = self.request(url)
            self.assertEqual(reply['status'], 200, reply)
            return json.loads(reply['body'])

        ids = [f'lead-split-{i}' for i in range(9)] + ['lead-near-0', 'lead-near-1']
        first, second, unknown, separate, neighbour = (frozenset(ids[i:i+2]) for i in (0, 2, 4, 6, 9))
        singleton = ids[8]
        x, y = '00000000-0000-0000-0000-000000007901', '00000000-0000-0000-0000-000000007902'
        added_at = '2026-10-04'
        rows = [(ident, ident + '.png', str(self.root / (ident + '.png')), 120, 150, added_at, 'orig')
                for ident in ids]
        # 前六张看起来相同，按线索拆成三组。接下来三张是另一组，只有最后一张有线索。
        # 最后两张离前六张 (1 - 0.85) / 2 = 0.075：比分组半径 0.07 远，自成一组；又在 0.08 以内，算「可能是同一人」。
        with sqlite3.connect(self.lib.source) as c:
            c.executemany("INSERT INTO assets VALUES (?,?,?,?,?,?,?,1,0)", rows)
            for i, ident in enumerate(ids):
                vector = np.zeros(768, np.float32)
                if i < 6:
                    vector[2] = 1
                elif i < 9:
                    vector[3] = 1
                else:
                    vector[[2, 4]] = .85, np.sqrt(1 - .85 ** 2)
                c.execute("INSERT INTO embeddings VALUES (?,'ccip',?)", (ident, vector.tobytes()))
        with sqlite3.connect(self.lib.db) as c:
            duplicates = c.execute("SELECT uuid,grp,size,pos,grp_added,rep FROM duplicates").fetchall()
        # 通过原生导入生成线索。CCIP 单路分数低于认领门槛，这些图仍留在未识别池。
        labels = [(ident, 'synthetic_x', 0.91, 'char_ext') for ident in sorted(first | {singleton})]
        labels += [(ident, 'synthetic_y', 0.91, 'char_ext') for ident in sorted(second)]
        native_commands.character_command(self.lib.db, dict(
            op='import', rows=rows, labels=labels, duplicates=duplicates, realistic=[],
            new_ids={'ccip': {'synthetic_x': x, 'synthetic_y': y}}, indexed=added_at + ' 00:00:00'))

        expected = {first: x, second: y, unknown: None, separate: None, neighbour: None}
        previous_ids = None
        for _ in range(2):
            # 相似扫描子类会把常规重分组请求改成 /api/similar。这个用例总是走真实的重分组入口。
            reply = lifecycle.NativeLifecycleTest.request(self, '/api/regroup', {})
            self.assertEqual(reply['status'], 200, reply)
            self.assertEqual(reply['body'], b'{"started": true}')
            self.finished()
            groups = [g for g in get('/api/clusters')['clusters'] if set(ids).intersection(g['members'])]
            self.assertEqual(len(groups), len(expected))
            self.assertEqual({frozenset(g['members']): g['lead']['id'] if g['lead'] else None for g in groups},
                             expected)
            group_ids = {frozenset(g['members']): g['cid'] for g in groups}
            self.assertEqual(len(set(group_ids.values())), len(expected))
            if previous_ids is not None:
                self.assertEqual(group_ids, previous_ids)
            previous_ids = group_ids
            # 子组按原组首张成员的顺序编号：指向 X 的、指向 Y 的、没有线索的。
            self.assertLess(group_ids[first], group_ids[second])
            self.assertLess(group_ids[second], group_ids[unknown])
            # 同一长相团拆出的三组不互相提示「可能是同一人」，提示照旧指向另一个长相团。
            near = {frozenset(g['members']): (g['near'], g['near_kind']) for g in groups}
            for part in (first, second, unknown):
                self.assertEqual(near[part], (group_ids[neighbour], 'look'), sorted(part))
            self.assertEqual(near[neighbour][1], 'look')
            self.assertIn(near[neighbour][0], {group_ids[part] for part in (first, second, unknown)})
            self.assertEqual(near[separate], (None, None))
            for g in groups:
                if g['lead']:
                    self.assertEqual(g['lead']['count'], g['size'])
                page = get(f"/api/assets?pool=cluster&cluster={g['cid']}&order=newest")
                self.assertEqual(page['total'], 2)
                self.assertCountEqual([item['uuid'] for item in page['items']], g['members'])
        unmatched = get('/api/ids?pool=unmatched&order=hinted')
        self.assertIn(singleton, unmatched)
        self.assertLess(unmatched.index(singleton), min(unmatched.index(ident) for ident in unknown))

    def test_inflight_human_change_rejects_old_snapshot_then_retries(self):
        # #194：过期的结果绝不发布；任务自行在新快照上重试。
        self.timing_fixture()
        before = self.rows(['prototypes', 'proto_claims', 'clusters', 'cluster_info'])
        pids = self.held()
        stale = json.loads(self.saved.read_text(encoding='utf-8'))['version']
        self.human_input_change()
        human = self.rows(['actions', 'assertions'])
        self.release.write_text('release')
        wait_until(lambda: self.state()['status'] == 'waiting')
        self.assertIn('自动重试', self.state()['message'])
        self.assertEqual(self.rows(list(before)), before)
        self.no_processes(pids)
        self.finished()
        self.assertNotEqual(int(self.pid.read_text()), pids[0])
        self.assertNotEqual(json.loads(self.saved.read_text(encoding='utf-8'))['version'], stale)
        self.assertEqual(self.rows(['actions', 'assertions']), human)

    def human_input_change(self):
        # 打标签也会写历史（actions），而学习会读它；#29 让每种任务只看自己读的内容。
        self.assertEqual(self.request('/api/tag', {'ids': ['a'], 'tag': '合成旧快照'})['status'], 200)

    def test_cancel_timeout_and_retry_use_existing_process_tree_contract(self):
        self.timing_fixture()
        before = self.rows(['prototypes', 'proto_claims', 'clusters', 'actions', 'assertions'])
        pids = self.held()
        with sqlite3.connect(self.lib.db) as c:
            intents = c.execute('SELECT count(*) FROM research_runs').fetchone()[0]
        for _ in range(3):
            self.assertEqual(self.request('/api/regroup', {})['body'], b'{"started": true}')
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM research_runs').fetchone()[0], intents)
        self.assertEqual(self.request('/api/regroup', {'action': 'cancel'})['body'], b'{"cancelled": true}')
        self.no_processes(pids)
        self.assertEqual(self.state()['status'], 'idle')
        self.assertEqual(self.rows(list(before)), before)
        self.stop(self.process)
        # 超时不能取决于真实计算有多快（#219）：被它停下的卡住进程树，
        # 只需要在窗口内完成一次解释器启动和一次 spawn。
        self.env['ILLUSTASH_RESEARCH_TIMEOUT_MS'] = STALL_TIMEOUT_MS
        self.mode.write_text('stall')
        for path in (self.ready, self.pid, self.child):
            path.unlink(missing_ok=True)
        self.restart()
        wait_until(self.ready.exists)
        self.finished('error')
        self.no_processes((int(self.pid.read_text()), int(self.child.read_text())))
        self.assertEqual(self.rows(list(before)), before)
        self.stop(self.process)
        self.env.update(ILLUSTASH_RESEARCH_WORKER=str(self.worker), ILLUSTASH_RESEARCH_TIMEOUT_MS=WIDE_TIMEOUT_MS)
        self.restart()
        self.finished()
        self.assertFalse(self.sentinel.exists())

    def test_switch_same_identity_and_host_exit_kill_held_native_result(self):
        self.timing_fixture()
        before = self.rows(['prototypes', 'proto_claims', 'clusters', 'actions', 'assertions'])
        pids = self.held()
        replacement = self.package('same-id')
        replacement.config['id'] = self.lib.config['id']
        (replacement.path / 'library.json').write_text(json.dumps(replacement.config), encoding='utf-8')
        self.assertEqual(self.request('/api/libraries/open', {'path': str(replacement.path)})['status'], 200)
        self.no_processes(pids)
        self.assertEqual(self.rows(list(before)), before)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.ready.unlink(missing_ok=True)
        self.pid.unlink(missing_ok=True)
        self.child.unlink(missing_ok=True)
        self.assertEqual(self.request('/api/libraries/open', {'path': str(self.lib.path)})['status'], 200)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        # 打开旧图库包可能会恢复它待处理的意图。等待并扣住这个自有任务。
        self.request('/api/regroup', {})
        wait_until(self.ready.exists)
        pids = int(self.pid.read_text()), int(self.child.read_text())
        self.stop(self.process)
        self.no_processes(pids)
        self.assertEqual(self.rows(list(before)), before)

    def test_old_real_response_replay_is_rejected_without_republishing(self):
        self.timing_fixture()
        before = self.rows(['prototypes', 'proto_claims', 'clusters', 'actions', 'assertions'])
        pids = self.held()
        self.release.write_text('release')
        self.finished()
        self.no_processes(pids)
        self.mode.write_text('replay')
        self.request('/api/regroup', {})
        self.finished('error')
        self.assertEqual(self.rows(list(before)), before)

    def test_ack_failure_is_atomic_and_actual_native_retry_preserves_human_history(self):
        self.stop(self.process)
        tables = ['prototypes', 'learn_state', 'proto_claims', 'weak_claims',
                  'clusters', 'cluster_info', 'actions', 'assertions', 'memberships']
        before = self.rows(tables)
        # 把所有已确认的种子重新嵌入到未匹配的方向上，会同时改变真实的原型和它的机器认领。人工输入保持不变。
        vector = np.zeros(768, np.float32)
        vector[1] = 1
        with sqlite3.connect(self.lib.source) as c:
            c.executemany("UPDATE embeddings SET vec=? WHERE uuid=? AND model='ccip'",
                          [(vector.tobytes(), 'seed' + str(i)) for i in range(6)])
        with sqlite3.connect(self.lib.db) as c:
            c.execute("CREATE TRIGGER fail_native_ack BEFORE UPDATE ON backend_effects WHEN NEW.status='done' BEGIN SELECT RAISE(ABORT,'synthetic ack'); END")
            effect = c.execute("INSERT INTO backend_effects(kind) VALUES ('index_changed')").lastrowid
        self.restart()
        wait_until(lambda: self.state()['status'] == 'error')
        self.finished('error')
        self.assertEqual(self.rows(tables), before)
        with sqlite3.connect(self.lib.db) as c:
            status, error = c.execute('SELECT status,error FROM backend_effects WHERE id=?', (effect,)).fetchone()
            self.assertEqual(status, 'pending')
            self.assertIn('synthetic ack', error)
            self.assertGreater(c.execute("SELECT count(*) FROM research_runs WHERE status='pending'").fetchone()[0], 0)
            c.execute('DROP TRIGGER fail_native_ack')
        self.stop(self.process)
        self.restart()
        self.finished()
        after = self.rows(tables)
        self.assertNotEqual(after['prototypes'], before['prototypes'])
        self.assertNotEqual(after['proto_claims'], before['proto_claims'])
        self.assertNotEqual(after['clusters'], before['clusters'])
        for table in ('actions', 'assertions'):
            self.assertEqual(after[table], before[table], table)
        self.assertEqual([row for row in after['memberships'] if row[2] != 'machine'],
                         [row for row in before['memberships'] if row[2] != 'machine'])
        # 重试成功后，必须落实三条新的 PROTO 机器归属。
        self.assertEqual({(row[0], row[1], row[3], row[5]) for row in after['memberships']
                          if row[2] == 'machine' and row[4] == 'PROTO'},
                         {(ident, self.entity, 'machine', 1.0)
                          for ident in ('unmatched6', 'unmatched7', 'unmatched8')})
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT status,error FROM backend_effects WHERE id=?', (effect,)).fetchone(), ('done', ''))
            self.assertEqual(c.execute("SELECT count(*) FROM research_runs WHERE status='pending'").fetchone()[0], 0)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute("SELECT count(*) FROM backend_effects WHERE status='pending' AND kind='index_changed'").fetchone()[0], 0)


@unittest.skipUnless(sys.platform in ('darwin', 'win32'), 'Linux is frozen: no local computation')
class NativeLearningSignalTest(unittest.TestCase):
    """#404：撤销、确认角色之后的学习落定信号 GET /api/learning，用上面的合成图库和真实安装的 worker。

    借用 NativeResearchPublicationTest 的夹具方法而不继承它，免得把它的用例再跑一遍；NativeRelationPublicationTest
    继承它，也不会把这里的用例带过去。
    """
    setUpClass = InstalledResearchComputeTest.__dict__['setUpClass']
    package = lifecycle.NativeLifecycleTest.package
    start = lifecycle.NativeLifecycleTest.start
    stop = lifecycle.NativeLifecycleTest.stop
    request = lifecycle.NativeLifecycleTest.request
    setUp = NativeResearchPublicationTest.setUp
    restart = NativeResearchPublicationTest.restart
    state = NativeResearchPublicationTest.state
    finished = NativeResearchPublicationTest.finished
    timing_fixture = NativeResearchPublicationTest.timing_fixture

    def learning(self, upto=None):
        response = self.request('/api/learning' + ('' if upto is None else '?upto=' + str(upto)))
        self.assertEqual(response['status'], 200, response)
        return json.loads(response['body'])

    def settle(self, first=None):
        first = first or self.learning()
        wait_until(lambda: self.learning(first['upto'])['state'] == 'done')
        return first['upto']

    def latest_verdict(self):
        with sqlite3.connect(self.lib.db) as c:
            return c.execute("SELECT coalesce(max(id),0) FROM backend_effects WHERE kind='verdict'").fetchone()[0]

    def shown(self):
        """页面撤销以后重读的计数和分组：未识别等各池的计数、各组的组号和成员、图库总数。"""
        pools = json.loads(self.request('/api/review-pools')['body'])
        clusters = json.loads(self.request('/api/clusters')['body'])['clusters']
        return {'pools': {p['kind']: p['count'] for p in pools},
                'clusters': sorted((c['cid'], sorted(c['members'])) for c in clusters),
                'total': json.loads(self.request('/api/bootstrap')['body'])['total']}

    def test_learning_signal_after_undo_gives_final_counts_on_first_reread(self):
        self.timing_fixture()
        self.mode.write_text('pass')
        self.settle()
        before = self.shown()
        group = next(members for _, members in before['clusters'] if any(m.startswith('unmatched') for m in members))
        self.assertGreaterEqual(len(group), 3, before)
        confirmed = self.request('/api/change', {'ids': group, 'action': 'confirmed', 'entity': self.entity})
        self.assertEqual(confirmed['status'], 200, confirmed)
        self.settle()
        learned = self.shown()
        self.assertNotEqual(learned['clusters'], before['clusters'])
        # 扣住撤销引起的那一次学习：学到的结果算好了，App 还没看到 worker 退出。
        for path in (self.ready, self.release, self.pid, self.child):
            path.unlink(missing_ok=True)
        self.mode.write_text('hold')
        undone = self.request('/api/undo', {'id': json.loads(confirmed['body'])['undo']['id']})
        self.assertEqual(undone['status'], 200, undone)
        written = self.latest_verdict()
        first = self.learning()
        self.assertEqual(first['state'], 'pending', first)
        self.assertGreaterEqual(first['upto'], written)
        wait_until(self.ready.exists)
        self.assertEqual(self.learning(first['upto'])['state'], 'pending')
        # 这时重读得到的是中间值：测试确实覆盖到了「还没落定」。
        self.assertNotEqual(self.shown(), before)
        self.release.write_text('release')
        self.settle(first)
        # 落定后的第一次重读就是最终值。
        self.assertEqual(self.shown(), before)
        self.assertFalse(self.sentinel.exists())

    def test_learning_signal_without_worker_is_done_on_first_read(self):
        self.stop(self.process)
        self.env.pop('ILLUSTASH_RESEARCH_WORKER')
        self.restart()
        self.assertEqual(self.learning()['state'], 'done')
        group = [f'unmatched{i}' for i in range(6, 9)]
        confirmed = self.request('/api/change', {'ids': group, 'action': 'confirmed', 'entity': self.entity})
        self.assertEqual(confirmed['status'], 200, confirmed)
        self.assertEqual(self.learning(), {'upto': self.latest_verdict(), 'state': 'done', 'learner': {'status': 'off'}})
        undone = self.request('/api/undo', {'id': json.loads(confirmed['body'])['undo']['id']})
        self.assertEqual(undone['status'], 200, undone)
        first = self.learning()
        self.assertEqual((first['upto'], first['state']), (self.latest_verdict(), 'done'))
        # 早于当前的编号照样能问；不是非负整数、或者比当前最大编号还大的拒绝。
        self.assertEqual(self.learning(first['upto'] - 1)['state'], 'done')
        for bad in (first['upto'] + 1, -1, 'x', '1.5'):
            self.assertEqual(self.request('/api/learning?upto=' + str(bad))['status'], 400, bad)
        # 空值和别的查询参数一样按没带处理（http_read::query）。
        self.assertEqual(json.loads(self.request('/api/learning?upto=')['body'])['upto'], first['upto'])

    def test_learning_signal_reports_error_when_the_worker_fails(self):
        self.stop(self.process)
        failing = self.root / 'failing-research-worker'
        failing.write_text('#!' + sys.executable + '\nimport sys\nsys.exit(3)\n', encoding='utf-8')
        self.env['ILLUSTASH_RESEARCH_WORKER'] = fixture_command(failing)
        self.restart()
        confirmed = self.request('/api/change', {'ids': ['unmatched6'], 'action': 'confirmed', 'entity': self.entity})
        self.assertEqual(confirmed['status'], 200, confirmed)
        first = self.learning()
        self.assertIn(first['state'], ('pending', 'error'), first)
        wait_until(lambda: self.learning(first['upto'])['state'] == 'error')
        failed = self.learning(first['upto'])
        self.assertEqual(failed['learner']['status'], 'error', failed)
        self.assertIn('diagnostic', failed['learner'])


@unittest.skipUnless(sys.platform in ('darwin', 'win32'), 'Linux is frozen: no local computation')
class InstalledRelationSnapshotTest(relations.NativeSimilarSnapshotTest):
    """同一个独立的缓存/精度参照，经真实的安装后适配器运行。"""
    setUpClass = InstalledResearchComputeTest.__dict__['setUpClass']

    def setUp(self):
        super().setUp()
        self.binary = self.root / ('illustash-research.exe' if os.name == 'nt' else 'illustash-research')
        shutil.copy2(self.candidate_worker, self.binary)
        # 父请求用空 PATH；同时禁止配置好的 Python。
        scoped = unittest.mock.patch.dict(os.environ)
        scoped.start()
        self.addCleanup(scoped.stop)
        os.environ.pop('ILLUSTASH_WORKER_PYTHON', None)


@unittest.skipUnless(sys.platform in ('darwin', 'win32'), 'Linux is frozen: no local computation')
class NativeRelationPublicationTest(NativeResearchPublicationTest):
    """启动任务加直接 similar 的混合情形，沿用现有的 App 原子性/生命周期测试。"""
    def setUp(self):
        super().setUp()
        self.stop(self.process)
        with sqlite3.connect(self.lib.source) as c:
            for (ident,) in c.execute('SELECT uuid FROM assets').fetchall():
                path = self.root / (ident + '.png')
                # 纹理相同的图片命中原有的提前相等判断，让生命周期测试跑得快，而不用拿假的计算程序替代。
                y,x=np.mgrid[:59,:87]
                pixels=np.stack(((x*7+y*3)%256,(x*11+y*17)%256,(x*23+y*13)%256),axis=2).astype(np.uint8)
                Image.fromarray(pixels).save(path)
                c.execute('UPDATE assets SET src_path=? WHERE uuid=?',(str(path),ident))
        self.lib = self.rec.package('relations', self.lib.path)
        self.restart()
        self.finished()
        self.assertGreater(len(self.rows(['image_sigs'])['image_sigs']),0)
        self.assertGreater(len(self.rows(['relations'])['relations']),0)

    def request(self, path, body=None):
        if path == '/api/regroup':
            path = '/api/similar'
        return super().request(path, body)

    def rows(self, tables):
        tables=list(tables)
        if any(t in tables for t in ('prototypes','clusters','image_sigs','relations')):
            tables=list(dict.fromkeys(tables+['image_sigs','relations','similar_paired','similar_retry']))
        return super().rows(tables)

    def timing_fixture(self):
        super().timing_fixture()
        # 启动那一轮之后、每个继承来的测试记录正式的修改前状态之前，强制产生一个真正改变了的图片结果。
        with sqlite3.connect(self.lib.db) as c:
            c.execute("DELETE FROM image_sigs WHERE uuid='seed0'")
            c.execute("DELETE FROM relations WHERE a='seed0' OR b='seed0'")
            c.execute("DELETE FROM similar_paired WHERE uuid='seed0'")

    def held(self):
        before=self.rows(['image_sigs','relations'])
        pids=super().held()
        response=json.loads(self.saved.read_text(encoding='utf-8'))
        # #31：App 请求变更列表；删掉的签名及其关系作为追加的行回来。
        self.assertIn('seed0',[r[0] for r in response['tables']['image_sigs']['append']])
        self.assertTrue(any('seed0' in r[:2] for r in response['tables']['relations']['append']))
        self.assertEqual(response['tables']['image_sigs']['delete'],[])
        return pids

    def human_input_change(self):
        # similar 读排除，不读标签（#29）：人工排除就是计算期间的输入变化。
        response=self.request('/api/curate',{'ids':['a'],'action':'exclude'})
        self.assertEqual(response['status'],200,response)

    def test_old_real_response_replay_is_rejected_without_republishing(self):
        self.timing_fixture()
        pids=self.held()
        self.release.write_text('release')
        self.finished()
        self.no_processes(pids)
        published=self.rows(['actions','assertions','prototypes','clusters'])
        self.mode.write_text('replay')
        self.request('/api/similar',{})
        self.finished('error')
        self.assertEqual(self.rows(list(published)),published)

    def test_manual_fold_preferences_and_human_history_survive_repeated_publication(self):
        history=self.rows(['actions','assertions','image_sigs','relations'])
        for kinds in (['identical'], [], ['duplicate','identical']):
            response=self.request('/api/fold',{'kinds': kinds})
            self.assertEqual(response['status'],200,response)
            self.assertEqual(self.request('/api/similar',{})['body'],b'{"started": true}')
            self.finished()
            actual=self.rows(['actions','assertions','image_sigs','relations'])
            for table in ('actions','assertions','image_sigs','relations'):
                self.assertEqual(actual[table],history[table],table)
            with sqlite3.connect(self.lib.db) as c:
                self.assertEqual(c.execute('SELECT count(*) FROM duplicates').fetchone()[0]>0,bool(kinds))
        self.assertFalse(self.sentinel.exists())

    def test_unchanged_and_small_results_write_only_changed_cache_rows(self):
        # #31：结果没变时不写相似度缓存行；重新计算的图片只写它自己的行。
        tables=('image_sigs','relations','similar_paired','similar_retry')
        self.stop(self.process)
        with sqlite3.connect(self.lib.db) as c:
            c.execute('CREATE TABLE synthetic_writes(tbl TEXT,op TEXT)')
            for table in tables:
                for op in ('INSERT','UPDATE','DELETE'):
                    c.execute(f"CREATE TRIGGER synthetic_{table}_{op} AFTER {op} ON {table} BEGIN INSERT INTO synthetic_writes VALUES ('{table}','{op}'); END")
        self.restart()
        self.finished()
        def ordered():
            with sqlite3.connect(self.lib.db) as c:
                return {t:c.execute('SELECT * FROM '+t+' ORDER BY rowid').fetchall() for t in tables}
        def writes():
            with sqlite3.connect(self.lib.db) as c:
                return c.execute('SELECT tbl,op,count(*) FROM synthetic_writes GROUP BY 1,2 ORDER BY 1,2').fetchall()
        def similar():
            with sqlite3.connect(self.lib.db) as c:c.execute('DELETE FROM synthetic_writes')
            self.assertEqual(self.request('/api/similar',{})['body'],b'{"started": true}')
            self.finished()
        before=ordered()
        similar()
        self.assertEqual(writes(),[])
        self.assertEqual(ordered(),before)
        with sqlite3.connect(self.lib.db) as c:
            c.execute("DELETE FROM image_sigs WHERE uuid='seed0'")
            c.execute("DELETE FROM relations WHERE a='seed0' OR b='seed0'")
            c.execute("DELETE FROM similar_paired WHERE uuid='seed0'")
        kept=ordered()
        similar()
        after=ordered()
        mine=lambda t,row:'seed0' in (row[:2] if t in ('relations','similar_retry') else row[:1])
        for table in tables:
            # 其他行留在原位；这张图片的行按完整替换时的顺序追加。
            self.assertEqual([r for r in after[table] if not mine(table,r)],kept[table],table)
            self.assertEqual(after[table][len(kept[table]):],[r for r in after[table] if mine(table,r)],table)
        self.assertEqual(after['image_sigs'],kept['image_sigs']+[r for r in before['image_sigs'] if r[0]=='seed0'])
        expected=[(t,'INSERT',n) for t in tables if (n:=sum(mine(t,r) for r in after[t]))]
        self.assertEqual(writes(),expected)

    def test_mixed_event_ack_failure_rolls_back_new_image_cache_then_retry_publishes(self):
        self.stop(self.process)
        with sqlite3.connect(self.lib.db) as c:
            c.execute("DELETE FROM image_sigs WHERE uuid='seed0'")
            c.execute("DELETE FROM relations WHERE a='seed0' OR b='seed0'")
            c.execute("DELETE FROM similar_paired WHERE uuid='seed0'")
            c.execute("CREATE TRIGGER fail_image_ack BEFORE UPDATE ON backend_effects WHEN NEW.status='done' BEGIN SELECT RAISE(ABORT,'synthetic image ack'); END")
            effect=c.execute("INSERT INTO backend_effects(kind) VALUES ('index_changed')").lastrowid
        before=self.rows(['actions','assertions','prototypes','clusters','memberships'])
        self.restart()
        self.finished('error')
        self.assertEqual(self.rows(list(before)),before)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT status FROM backend_effects WHERE id=?',(effect,)).fetchone()[0],'pending')
            c.execute('DROP TRIGGER fail_image_ack')
        self.stop(self.process)
        self.restart()
        self.finished()
        after=self.rows(list(before))
        self.assertNotEqual(after['image_sigs'],before['image_sigs'])
        self.assertNotEqual(after['relations'],before['relations'])
        for table in ('actions','assertions'):
            self.assertEqual(after[table],before[table],table)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT status,error FROM backend_effects WHERE id=?',(effect,)).fetchone(),('done',''))
        self.assertFalse(self.sentinel.exists())
