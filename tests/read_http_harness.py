"""在合成数据库上按固定期望检查读取路由的字节契约（#9 第 3 阶段）。

核心读取路由（`http_read::read` 加上应用的响应编码）由仅供测试的 `read-http` example 提供，
图库上下文是测试发布到状态文件里的那个。它们的字节与原 Python 参考服务器在同样数据库上对同样请求的响应比较，
这些响应经 ``tests/fixed_expectations.py`` 录制过一次（#9；参考实现已删除，无法再次录制）。
数据库本身在参考侧每次改动之后录制为临时根目录的包，并在下一个请求之前还原，
所以默认模式下不运行任何 Python 参考代码。
"""
import json
import os
from pathlib import Path
import secrets
import shutil
import sqlite3
import subprocess
import tempfile
import threading
import unittest
from contextlib import closing
from urllib.parse import quote

from tests.fixed_expectations import RECORDED_TODAY, Expectations, current_review_pools
from tests.test_read_http import SPAWN, free_port, stop_process, wait_until
from tools import verify_read_http as verify
from tools.wait_limit import WAIT

ROOT = Path(__file__).resolve().parents[1]
HEADERS = ('Content-Type', 'Content-Length', 'Cache-Control', 'Vary', 'X-Content-Type-Options',
           'Referrer-Policy', 'Content-Security-Policy')


def binary():
    configured = os.environ.get('ILLUSTASH_READ_HTTP_BIN')
    if configured:
        path = Path(configured)
    else:
        terms = os.environ.get('ILLUSTASH_TERMS_BIN')
        if not terms:
            raise unittest.SkipTest('set ILLUSTASH_TERMS_BIN or ILLUSTASH_READ_HTTP_BIN explicitly; skip is not acceptance')
        path = Path(terms).parent / 'examples' / ('read-http.exe' if os.name == 'nt' else 'read-http')
    if not path.is_file():
        raise RuntimeError('Build the test read host: cargo build --release --manifest-path native/terms/Cargo.toml '
                           '--features test-read-host --example read-http')
    return path


def publish(rec, server, state, **override):
    """原子地发布请求上下文（原先是参考服务器的就绪文件）；`override` 替换录制的上下文里的部分字段。"""
    # 会话 token 按原样录制：响应长度取决于它。
    context = {**rec.value('context'), **override}
    temp = state.with_suffix('.new')
    temp.write_text(json.dumps({**context, 'generation': secrets.token_hex(16),
                                'tags': str(ROOT / 'tags' / 'tags_v3.csv')}), encoding='utf-8')
    os.replace(temp, state)
    return context


class HTTPHarness(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.binary = binary()
        cls.temp = tempfile.TemporaryDirectory(prefix='illustash-r12-')
        cls.root = Path(cls.temp.name).resolve()
        cls.state_dir = tempfile.TemporaryDirectory(prefix='illustash-r12-state-')
        cls.state = Path(cls.state_dir.name) / 'state.json'
        cls.origin = f'http://127.0.0.1:{free_port()}'
        cls.rec = rec = Expectations(cls)
        rec.alias(cls.root, '@ROOT@')
        rec.alias(cls.origin, '@ORIGIN@')
        cls.server = cls.store = None
        rec.package('root', cls.root, library=False)
        cls.context = publish(rec, cls.server, cls.state)
        cls.log = tempfile.TemporaryFile()
        cls.host = subprocess.Popen([str(cls.binary), '--port', cls.origin.rsplit(':', 1)[1], '--state', str(cls.state)],
                                    env={**os.environ, **RECORDED_TODAY}, stdout=cls.log, stderr=cls.log, **SPAWN)
        wait_until(lambda: verify.fetch(cls.origin, {'url': '/api/fold'}, cls.origin)['status'] == 200)

    @classmethod
    def tearDownClass(cls):
        if os.name == 'nt' and cls.host.poll() is None:
            cls.host.terminate()   # 测试宿主不处理控制台信号
            cls.host.wait(WAIT)
        stop_process(cls.host)
        cls.log.close()
        cls.temp.cleanup()
        cls.state_dir.cleanup()
        if not False and cls.rec.position != len(cls.rec.entries):
            raise AssertionError(f'{cls.rec.test}: recorded reference values not reached')

    def setUp(self):
        self.rec = Expectations(self)
        self.addCleanup(self.rec.finish, self)
        for actual, placeholder in type(self).rec.aliases:
            self.rec.alias(actual, placeholder)
        # 每个测试都从录制的根目录和上下文开始，不受前面测试的影响。
        self.sync('start')
        self.publish()

    @property
    def db(self):
        return Path(self.context['db'])

    def connect(self, path=None):
        return closing(sqlite3.connect(path or self.db))

    def publish(self, **override):
        type(self).context = publish(self.rec, self.server, self.state, **override)

    def sync(self, label='changed'):
        """参考侧改动之后：录制临时根目录，或者还原它。"""
        self.rec.package(label, self.root, library=False)

    def reference(self, item):
        return self.rec.value('response')

    def pair(self, path, route='core', headers=None):
        item = {'url': quote(path, safe='/?&=;%+#'), 'headers': headers or {}}
        reference = self.reference(item)
        a, b = current_review_pools(path, reference), verify.fetch(self.origin, item, self.origin)
        self.assertIsNone(verify.difference(a, b), (path, verify.difference(a, b)))
        self.assertEqual(b['route'], route, path)
        for h in HEADERS:
            self.assertEqual(a['headers'].get(h), b['headers'].get(h), (path, h))
        return reference

    # ---- 图库切换到核心之前录制的响应（#201）----
    #
    # 有些录制读取的是切换到核心之前的图库（Python 时代，没有 `meta`）。core v8 拒绝这样的图库（只保留已切换的），
    # 所以这些响应不再实时读取，而是与切换之后的图库在当前核心版本上对同样请求的响应比较；
    # 切换本身改变了响应的位置，由各个测试列出并写明原因。

    def after_switch(self, before, differ):
        """从现在起，每次 `pair`（录制于切换之后，与实时的 v8 响应比较）还会比较切换之前在同一位置录制的响应，
        `differ` 里的位置除外。返回比较过的路径列表，供调用方检查每条录制的响应都走到了。"""
        compared = []
        recorded = self.pair

        def pair(path, *args, **kwargs):
            reference = recorded(path, *args, **kwargs)
            if len(compared) < len(before) and len(compared) not in differ:
                earlier = before[len(compared)]
                self.assertEqual((earlier['status'], earlier['body']), (reference['status'], reference['body']), path)
            compared.append(path)
            return reference

        self.pair = pair
        self.addCleanup(vars(self).pop, 'pair', None)
        return compared

    def live(self, before, path, headers=None):
        """把切换之前录制的响应与切换之后图库的实时 v8 响应比较。"""
        item = {'url': quote(path, safe='/?&=;%+#'), 'headers': headers or {}}
        reply = verify.fetch(self.origin, item, self.origin)
        self.assertIsNone(verify.difference(before, reply), (path, verify.difference(before, reply)))
        self.assertEqual(reply['route'], 'core', path)
        for h in HEADERS:
            self.assertEqual(before['headers'].get(h), reply['headers'].get(h), (path, h))
        return reply

    def corpus(self, *, sample, query_limit, keep=lambda item: True):
        return self.rec.value('corpus')

    def compare(self, requests, require_core=()):
        """verify_read_http 报告的格式，参考侧用录制值。"""
        expected = self.rec.value('responses')
        report = {'compared': 0, 'differ': 0, 'core_equal': 0, 'stages': set(), 'differences': []}
        for item, left in zip(requests, expected, strict=True):
            report['compared'] += 1
            report['stages'].add(item.get('stage'))
            right = verify.fetch(self.origin, item, self.origin)
            diff = verify.difference(current_review_pools(item['url'], left), right)
            if diff is None and right['route'] == 'core':
                report['core_equal'] += 1
            elif diff is None:
                diff = {'routing': right['route']}
            if diff is not None:
                report['differ'] += 1
                report['differences'].append({'request': item, **diff})
        for stage in require_core:
            if stage not in report['stages']:
                report['differ'] += 1
                report['differences'].append({'stage': stage, 'error': '核心验收阶段没有任何请求'})
        return report
