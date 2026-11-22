"""M1 原生生命周期验收，只用显式给出的临时图库包。

需要 ILLUSTASH_TERMS_BIN 指向被测的 M1 release；从不构建或查找更旧的二进制。Python 只用来构造夹具。
原 Python 参照服务器的响应、图库包和预期状态已通过 tests/fixed_expectations.py 录制（#9）；原生的键可以复现
（store::new_key 的测试计数器）。启动的原生进程拿到的 PATH 里只有会失败的 Python 哨兵。
覆盖文件夹和照片图库包的新建；现有图库的升级、worker 任务的关闭和 Windows 进程树验收不在范围内。
"""
from contextlib import chdir, closing
import gc
import hashlib
import http.client
import json
import os
import re
from pathlib import Path
import shutil
import signal
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
from urllib.parse import urlencode

from tests.fixed_expectations import Library, Expectations, RECORDED_TODAY, TERMS_VERSION, unclocked, current_review_pools
from tests.synthetic_source import source_fixture as fixture, synthetic_reference
from tests.test_read_http import INTERRUPT, SPAWN, free_port, wait_until, wait_human_backup
from tools import verify_read_http
from tools.verify_read_http import cookie, enter, fetch
from tools.wait_limit import WAIT

CORE_READS = (
    '/api/assets', '/api/assets/count', '/api/ids', '/api/detail?id=a', '/api/home', '/api/fold',
    '/api/review-pools', '/api/characters', '/api/tagged', '/api/character/profile',
    '/api/character/activity', '/api/character/stats',
    '/api/identity/count', '/api/identity/queue',
    '/api/bootstrap', '/api/user-tags', '/api/general-tags', '/api/artists',
    '/api/artist', '/api/artist-audit', '/api/entities', '/api/works',
)
# 参考实现在 Windows 上有句柄泄漏、非法文件名和提前断开连接，逐字节对照的期望录自 POSIX，只在 POSIX 上验收；
# Windows 上同一批原生行为由 tools/windows_acceptance.py 直接验证。
REFERENCE_PARITY_ONLY = unittest.skipIf(os.name == 'nt', '固定期望录自 POSIX 上的 Python 参考实现；Windows 原生行为见 tools/windows_acceptance.py')
HEADERS = ('Content-Type', 'Content-Length', 'Content-Encoding', 'Cache-Control', 'Vary',
           'X-Content-Type-Options', 'Referrer-Policy', 'Content-Security-Policy')


def difference(left, right):
    """两个响应的字节差异；类似时钟的文本（原生宿主会打上当前时间，例如新图库的索引时间）按 `<clock>` 比较。"""
    masked = [dict(r, body=unclocked(r['body'])) if 'body' in r else r for r in (left, right)]
    return verify_read_http.difference(*masked)


def dump(db):
    """所有结构语句和行语句，排成有序列表：内容相同就相等，不论结构行的顺序如何（恢复的录制可能会重建触发器）。"""
    with closing(sqlite3.connect(db)) as c:
        return sorted(c.iterdump())


IMPORT_ENDED = ('done', 'error', 'cancelled')


def import_task(test, url, body, *, token=None, timeout=WAIT, seen=None):
    """#88：归档导入和识别结果导入是后台任务。启动一个，轮询 GET /api/imports 直到结束，然后按原来的同步请求
    那样应答：启动本身的拒绝、200 加结果、错误状态和消息，或者被取消时的 409。`seen` 收集轮询到的每个状态。"""
    started = test.request(url, body, token=token)
    if started['status'] != 200:
        return started
    task = json.loads(started['body'])['task']
    test.assertEqual(task['status'], 'running', task)
    state = task

    def ended():
        nonlocal state
        state = json.loads(test.request('/api/imports')['body'])
        test.assertEqual(state.get('id'), task['id'], state)
        if seen is not None:
            seen.append(state)
        return state['status'] in IMPORT_ENDED

    wait_until(ended, timeout=timeout)
    if state['status'] == 'done':
        payload, status = state['result'], 200
    elif state['status'] == 'cancelled':
        payload, status = {'error': state['reason']}, 409
    else:
        payload, status = {'error': state['error']}, state['code']
    return {**started, 'status': status, 'body': json.dumps(payload, ensure_ascii=False).encode()}


STARTUP_DEFAULT = {'sync': True, 'similar': True, 'index_check': True, 'names': True, 'provenance': True}
LOCAL_LIMIT = re.compile(r', "local_limit": \d+')


def without_local_limit(value):
    """#100 去掉了本地识别数量上限这项偏好设置。录制里它存放和应答时都排在其他偏好之后；这里是去掉它之后的
    同一份录制（请求也不再发送它）。"""
    if isinstance(value, list):
        return [without_local_limit(v) for v in value]
    if isinstance(value, str):
        return LOCAL_LIMIT.sub('', value)
    if b'"local_limit"' not in value.get('body', b''):
        return value
    body = LOCAL_LIMIT.sub('', value['body'].decode()).encode()
    return {**value, 'body': body, 'headers': {**value['headers'], 'Content-Length': str(len(body))}}


def startup(db):
    """应用读到的启动偏好设置（默认值加上已存的改动）。"""
    with closing(sqlite3.connect(db)) as c:
        row = c.execute("SELECT value FROM prefs WHERE key='startup'").fetchone()
    return {**STARTUP_DEFAULT, **(json.loads(row[0]) if row else {})}


def files(root):
    """按内容列出静止夹具里的每个文件，不排除后台记录或 SQLite 协调文件。"""
    return {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob('*') if p.is_file()}


class NativeLifecycleTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        configured = os.environ.get('ILLUSTASH_TERMS_BIN')
        if not configured:
            raise unittest.SkipTest('set ILLUSTASH_TERMS_BIN explicitly to the M1 release')
        cls.binary = Path(configured).resolve()
        if not cls.binary.is_file():
            raise unittest.SkipTest('configured M1 release does not exist')
        cls.resources = Path(os.environ.get('ILLUSTASH_NATIVE_ROOT') or cls.binary.parents[4]).resolve()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='illustash-native-lifecycle-')
        self.addCleanup(self.temp.cleanup)
        # 参考实现的 Store.refresh 在引用环里留着连接；Windows 删不掉仍打开的文件，先释放再清目录。
        self.root = Path(self.temp.name).resolve()
        self.rec = Expectations(self)
        self.addCleanup(self.rec.finish, self)
        self.rec.alias(self.root, '@ROOT@')
        self.support, self.shared = self.root / 'support', self.root / 'shared'
        self.home = self.root / 'home' / 'Pictures' / 'Illustash'
        self.shared.mkdir()
        NativeLifecycleTest.confine_library_helpers(self)
        # 任何子进程都不能悄悄运行 Python，或使用调用方真实的应用支持目录。
        self.sentinel = self.root / 'python-was-launched'
        self.bin = self.root / 'bin'; self.bin.mkdir()
        for name in ('python', 'python3', 'python3.12'):
            executable = self.bin / name
            executable.write_text('#!/bin/sh\n/bin/echo forbidden > "$ILLUSTASH_PYTHON_SENTINEL"\nexit 97\n')
            executable.chmod(0o755)
        # Windows 上原生核心按 USERPROFILE 定位主目录，两个都指向合成根目录。
        self.env = {**os.environ, 'PATH': str(self.bin), 'HOME': str(self.root / 'home'), 'USERPROFILE': str(self.root / 'home'),
                    'ILLUSTASH_APP_SUPPORT': str(self.support),
                    'ILLUSTASH_SHARED': str(self.shared), 'ILLUSTASH_PYTHON_SENTINEL': str(self.sentinel),
                    'ILLUSTASH_TERMS_BIN': str(self.binary),
                    # 原生的键可以复现，录制的预期才成立（store::new_key 的测试钩子）。
                    'ILLUSTASH_TEST_KEY_COUNTER': str(self.root / 'key-counter'), **RECORDED_TODAY}
        if self._testMethodName.startswith('test_native_library_new'):
            self.new_reference = synthetic_reference(self.shared)
            self.new_folder = self.root / 'downloads-Pixiv'; self.new_folder.mkdir()
            (self.new_folder / 'not-scanned.png').write_bytes(b'not an image')
            # 相对路径或空的文件夹输入，相对于同一个隔离的应用根目录解析。
            (self.root / 'tags').mkdir()
            (self.root / 'tags' / 'tags_v3.csv').write_bytes((self.resources / 'tags' / 'tags_v3.csv').read_bytes())
            self.resources = self.root
            working_directory = chdir(self.root)
            working_directory.__enter__()
            self.addCleanup(working_directory.__exit__, None, None, None)
            first = self.root / 'first.pilib'
            result = subprocess.run([str(self.binary), 'library-create', '--dest', str(first),
                                     '--folder', str(self.new_folder), '--reference', str(self.new_reference),
                                     '--name', 'first'], env=self.env, capture_output=True, timeout=WAIT)
            self.assertEqual(result.returncode, 0, result.stderr.decode(errors='replace'))
            self.assertEqual(json.loads(result.stdout), {'path': str(first), 'name': 'first'})
            # 参照打开后的图库包（录制；原生新建的输出可以复现）。
            self.lib = self.rec.package('first', first)
            self.assertFalse(self.sentinel.exists())
        else:
            self.lib = self.package('first')
        self.port = free_port(); self.origin = f'http://127.0.0.1:{self.port}'
        self.rec.alias(self.origin, '@ORIGIN@')
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.rec.alias(self.token, '@TOKEN@')

    def confine_library_helpers(self):
        """部分测试用来构造输入的 Python 图库辅助函数，绝不会看到用户真实的文件夹。"""
        from tests import library_fixture as library
        for name, value in (('APP_SUPPORT', self.support), ('SHARED', self.shared),
                            ('HOME', self.home), ('REFERENCE', self.shared / 'reference.sqlite')):
            scoped = patch.object(library, name, value)
            scoped.start(); self.addCleanup(scoped.stop)

    def package(self, name):
        """原 Python 应用新建并刷新后的合成图库包（固定录制）。"""
        return self.rec.package(name, self.root / (name + '.pilib'))

    def stop(self, process):
        if process.poll() is None:
            process.send_signal(INTERRUPT)
            try:
                process.wait(WAIT)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait(WAIT)

    def start(self, path, extra=(), command=None):
        port = self.port if not command else free_port()
        log = tempfile.TemporaryFile()
        args = list(command or [str(self.binary), 'http']) + ['--no-auto-sync', '--library', str(path),
                '--port', str(port), '--root', str(self.resources), '--no-names', '--no-index-check', *extra]
        process = subprocess.Popen(args, env=self.env, stdout=log, stderr=log, **SPAWN)
        self.addCleanup(log.close)
        self.addCleanup(self.stop, process)
        origin = f'http://127.0.0.1:{port}'
        try:
            enter(origin, log, process)
        except AssertionError:
            log.seek(0)
            self.fail(f'native startup failed ({process.poll()}): {log.read().decode(errors="replace")}')
        wait_human_backup(self, origin)
        return process, log

    def clone_closed_library(self, target):
        """停止自有宿主后复制完整包，再重新打开；活库的 WAL/SHM 不能靠文件复制取得一致快照。"""
        self.stop(self.process)
        shutil.copytree(self.lib.path, target)
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']

    def request(self, url, body=None, *, base=None, token=None, headers=None):
        item = {'url': url, 'headers': headers or {}}
        if body is not None:
            item.update(method='POST', body=json.dumps(body, ensure_ascii=False).encode('utf-8'))
            item['headers'] = {'Content-Type': 'application/json', 'X-CSRF-Token': token or self.token,
                               **item['headers']}
        return fetch(base or self.origin, item, self.origin)

    def reference_response(self, url, body=None, **kwargs):
        """原参照服务器对同一请求的响应（录制）。"""
        response = without_local_limit(self.rec.value('response'))
        if url in ('/api/settings', '/api/startup', '/api/cache', '/api/libraries') and response['status'] == 200:
            # library_bytes 是当前主库加 WAL 的物理字节数；v9 首备份会写入新表，旧 schema 的冻结数字不适用。
            # 独立 stat 得到精确整数，只替换 package.library_bytes 与 library.bytes，保留其他字节和响应头。
            files = (self.lib.db, Path(str(self.lib.db) + '-wal'))
            size = sum(path.stat().st_size for path in files if path.exists())
            data = response['body']
            patterns = [rb'("library_bytes": )\d+']
            if url != '/api/libraries':
                patterns.append(rb'("library": \{"path": "(?:\\.|[^"\\])*", "bytes": )\d+')
            for pattern in patterns:
                data, count = re.subn(pattern, lambda match: match[1] + str(size).encode(), data)
                self.assertEqual(count, 1)
            response = {**response, 'body': data}
        return response

    def pair(self, url, body=None, **kwargs):
        left = current_review_pools(url, NativeLifecycleTest.reference_response(self, url, body, **kwargs))
        right = self.request(url, body, **kwargs)
        self.assertIsNone(difference(left, right), (url, difference(left, right)))
        for header in HEADERS if right['status'] < 400 else ():
            self.assertEqual(left['headers'].get(header), right['headers'].get(header), (url, header))
        return right

    def test_22_core_reads_original_bytes_headers_and_errors(self):
        self.assertEqual(len(CORE_READS), 22)
        for url in CORE_READS:
            with self.subTest(url=url):
                self.assertEqual(self.pair(url)['route'], 'core')
        for url in ('/api/assets?limit=bad', '/api/assets?nsfw=hide', '/api/characters?q=Alice',
                    '/api/identity/queue?limit=0', '/api/artist-audit?n=bad'):
            self.pair(url)

        # 菜单应答经过原生 HTTP，并把规范 tag 交给同一个图库的筛选接口。
        for query in ('蓝发', '青髪', 'ＢＬＵＥ ＨＡＩＲ', '蓝毛'):
            with self.subTest(query=query):
                params = urlencode({'format': 'menu', 'q': query})
                result = self.request(f'/api/general-tags?{params}')
                self.assertEqual(result['status'], 200)
                self.assertIn({'tag': 'blue_hair', 'zh': '蓝发', 'ja': '青髪'}, json.loads(result['body']))
        for query, expected in (
            ('ロングヘア', {'tag': 'long_hair', 'zh': '长发', 'ja': 'ロングヘア'}),
            ('eyeglasses', {'tag': 'glasses', 'zh': '眼镜', 'ja': None}),
            ('night sky', {'tag': 'night_sky', 'zh': None, 'ja': None}),
        ):
            params = urlencode({'format': 'menu', 'q': query})
            self.assertIn(expected, json.loads(self.request(f'/api/general-tags?{params}')['body']))
        result = self.request('/api/general-tags?format=menu')
        self.assertEqual(len(json.loads(result['body'])), 30)
        result = self.request('/api/general-tags?format=menu&q=explicit')
        self.assertNotIn('explicit', [item['tag'] for item in json.loads(result['body'])])
        result = self.request('/api/ids?tag=blue_hair')
        self.assertEqual(result['status'], 200)
        self.assertEqual(json.loads(result['body']), ['a'])
        self.assertFalse(self.sentinel.exists())

    def test_host_origin_csrf_security_and_unavailable_forward(self):
        for headers in ({'Host': 'evil.example'}, {'Origin': 'https://evil.example'}):
            self.assertEqual(self.pair('/api/bootstrap', headers=headers)['status'], 403)
        before = dump(self.lib.db)
        self.assertEqual(self.pair('/api/startup', {'sync': False}, token='bad')['status'], 403)
        self.assertEqual(self.pair('/api/startup', {'sync': False},
                                   headers={'Origin': 'https://evil.example'})['status'], 403)
        self.assertEqual(before, dump(self.lib.db))
        response = self.request('/api/local-links')
        self.assertEqual(response['status'], 200)
        self.assertEqual(response['route'], 'core')
        self.assertEqual(json.loads(response['body']), [])

    def test_settings_startup_values_errors_and_database_state(self):
        self.assertEqual(self.pair('/api/settings')['status'], 200)
        self.assertEqual(self.pair('/api/libraries')['status'], 200)
        changes = {'sync': False, 'similar': False}
        # 在两边任一 HTTP 写入之前，先在独立的克隆上算出预期的数据库改动。
        expected = without_local_limit(self.rec.value('expected dump'))
        response = self.request('/api/startup', changes)
        self.assertEqual(response['status'], 200)
        self.assertEqual(startup(self.lib.db), {**STARTUP_DEFAULT, **changes})
        # 数据库按 #201 的对照基准比较：core v8 改了录制的 dump 里写明的结构（仍然读取 `expected`，
        # 让录制的值对得上）。
        del expected
        self.assertTrue(self.rec.baseline('expected dump', self.lib.path))
        written = dump(self.lib.db)
        left = self.reference_response('/api/startup', changes)
        self.assertIsNone(difference(left, response), difference(left, response))
        for header in HEADERS if response['status'] < 400 else ():
            self.assertEqual(left['headers'].get(header), response['headers'].get(header), header)
        self.assertEqual(dump(self.lib.db), written)
        for invalid in ({}, {'new': True}, {'sync': 1}, {'local_limit': True},
                        {'local_limit': 99}, {'local_limit': 100001}):
            before = dump(self.lib.db)
            self.assertEqual(self.pair('/api/startup', invalid)['status'], 400)
            self.assertEqual(before, dump(self.lib.db))

    def test_raw_post_framing_and_multi_key_error_order(self):
        raw_cases = [(b'{}', 'text/plain'), (b'', 'application/json')]
        raw_cases += [(body, 'application/json') for body in (
            b'[]', b'{', b'{"sync": false', b'{"sync":"\\x"}', b'{"sync":"a\x01"}',
            b'{"sync":"unfinished}', b'{"sync":"\\uXYZA"}', b'{"sync": tru}',
            b'{"sync":}', b'{"sync":true}garbage', b'{"local_limit":NaN}',
            b'{"local_limit":Infinity}', b'{"local_limit":-Infinity}',
            b'{"local_limit":100.0}', b'{"local_limit":1e400}',
            b'{"local_limit":' + b'9' * 100 + b'}')]
        for body, content_type in raw_cases:
            with self.subTest(body=body, content_type=content_type):
                before = dump(self.lib.db)
                item = {'url': '/api/startup', 'method': 'POST', 'body': body,
                        'headers': {'Content-Type': content_type, 'X-CSRF-Token': self.token}}
                left = self.rec.value('response')
                right = fetch(self.origin, item, self.origin)
                self.assertEqual(left['status'], 400)
                self.assertIsNone(difference(left, right), difference(left, right))
                self.assertEqual(before, dump(self.lib.db))
        for invalid in ({'sync': 1, 'local_limit': 99}, {'local_limit': 99, 'sync': 1}):
            with self.subTest(invalid=invalid):
                before = dump(self.lib.db)
                self.assertEqual(self.pair('/api/startup', invalid)['status'], 400)
                self.assertEqual(before, dump(self.lib.db))

    @REFERENCE_PARITY_ONLY
    def test_native_library_new_raw_http_data_then_open_rotates_session(self):
        self.check_library_new_data_then_open('folder')

    @REFERENCE_PARITY_ONLY
    def test_native_library_new_photos_raw_http_data_then_open_rotates_session(self):
        self.check_library_new_data_then_open('photos')

    def check_library_new_data_then_open(self, kind):
        started = time.time() - 2
        registry = self.support / 'libraries.json'
        token, current = self.token, (dump(self.lib.source), dump(self.lib.db))
        folder_before = files(self.new_folder)
        names = ('合成 ✨', 'a/b:c', '   ', 123, True, None, '\x1c控制\x1f',
                 {'称呼': ['x', None, True], 'quote': "it's"}, ['x', {'nested': False}])
        bodies = [{'kind': 'folder', 'folder': str(self.new_folder), 'preset': 'other', 'name': name}
                  for name in names]
        bodies += [{'kind': 'folder', 'folder': 'downloads-Pixiv', 'preset': '', 'name': 'relative-folder'},
                   {'kind': 'folder', 'folder': '', 'name': 'empty-folder'},
                   {'kind': 'folder', 'folder': 'downloads-Pixiv/missing/..', 'name': 'nonstrict-resolution'},
                   {'kind': 'folder', 'folder': 'downloads-Pixiv/..', 'name': 'parent-folder'}]
        if kind == 'photos':
            # 旧 HTTP 对这种类型忽略 folder/preset/photos 字段；保持这个契约。
            bodies = [{**body, 'kind': kind, 'folder': ['ignored'], 'preset': 'ignored',
                       'photos': '/ignored-override'} for body in bodies[:len(names)]]
            bodies.append({'kind': kind, 'name': names[0]})  # 名字重复时加唯一路径后缀
        for index, body in enumerate(bodies):
            name = body['name']
            with self.subTest(body=body):
                registry_before = registry.read_bytes()
                left = self.reference_response('/api/libraries/new', body)
                if name is not None and not isinstance(name, str):
                    # #252 C2：非字符串不再创建图库，当前会话、登记和来源均不变。
                    before = files(self.home)
                    directories = set((self.root / 'home').rglob('*'))
                    right = self.request('/api/libraries/new', body)
                    self.assertEqual(right['status'], 400)
                    self.assertEqual(json.loads(right['body']), {'error': '图库名称必须是字符串'})
                    self.assertIsNone(difference(left, right))
                    self.assertEqual(files(self.home), before)
                    self.assertEqual(set((self.root / 'home').rglob('*')), directories)
                    self.assertEqual(registry.read_bytes(), registry_before)
                    self.assertEqual(current, (dump(self.lib.source), dump(self.lib.db)))
                    self.assertEqual(json.loads(self.request('/api/bootstrap')['body'])['token'], token)
                    self.assertEqual(files(self.new_folder), folder_before)
                    continue
                self.assertEqual(left['status'], 200, left['body'])
                dest = Path(json.loads(left['body'])['path'])
                self.assertTrue(dest.is_relative_to(self.root))
                self.assertNotEqual(dest, self.lib.path)
                right = self.request('/api/libraries/new', body)
                self.assertIsNone(difference(left, right),
                                  (name, left['status'], right['status'], left['body'], right['body']))
                for header in HEADERS:
                    self.assertEqual(left['headers'].get(header), right['headers'].get(header), (name, header))
                with closing(sqlite3.connect(dest / 'library.sqlite')) as c:
                    ids = dict(c.execute("SELECT l.key,t.key FROM term_links l JOIN terms t ON t.id=l.term_id "
                                         "WHERE l.system='ccip' AND l.current=1 AND t.kind='character'"))
                self.assertEqual(set(ids), {'alice', 'bob'})
                from tests.test_native_library_create import NativeLibraryCreateTest
                comparator = NativeLibraryCreateTest()
                comparator.started = started
                old = self.rec.value('old library')
                self.assertEqual(old['source'], comparator.state(dest / 'source.sqlite'))
                # 应用数据库按 #201 的对照基准比较（core v8 改了它的结构）。
                comparator.state(dest / 'library.sqlite', application=True)
                comparator.rec = self.rec
                comparator.baseline(f'fresh library {kind} {index}', dest)
                config = json.loads((dest / 'library.json').read_text())
                self.assertEqual({k: v for k, v in old['config'].items() if k not in {'id', 'created_at'}},
                                 {k: v for k, v in config.items() if k not in {'id', 'created_at'}})
                comparator.timestamp(config['created_at'], iso=True)
                self.assertEqual(current, (dump(self.lib.source), dump(self.lib.db)))
                self.assertEqual(json.loads(self.request('/api/bootstrap')['body'])['token'], token)
                recent = json.loads(registry.read_bytes())['recent']
                self.assertEqual(recent[0]['path'], str(self.lib.path))
                self.assertEqual(recent[-1]['path'], str(dest))
                self.assertEqual(files(self.new_folder), folder_before)
        for url in CORE_READS:
            self.assertEqual(self.pair(url)['route'], 'core')
        self.assertEqual(current, (dump(self.lib.source), dump(self.lib.db)))
        self.assertEqual(json.loads(self.request('/api/settings')['body'])['package']['path'], str(self.lib.path))
        self.assertEqual(self.request('/api/startup', {'sync': True}, token=token)['status'], 200)
        self.assertEqual(self.request('/api/libraries/open', {'path': str(dest)}, token=token)['status'], 200)
        wait_human_backup(self, self.origin)
        new_token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertNotEqual(new_token, token)
        before = dump(dest / 'library.sqlite')
        self.assertEqual(self.request('/api/startup', {'sync': False}, token=token)['status'], 403)
        self.assertEqual(before, dump(dest / 'library.sqlite'))
        self.assertEqual(json.loads(self.request('/api/settings')['body'])['package']['path'], str(dest))
        self.token = new_token
        self.rec.alias(self.token, '@TOKEN@')
        for url in CORE_READS:
            self.assertEqual(self.pair(url)['route'], 'core')
        self.assertFalse(self.sentinel.exists())


    @REFERENCE_PARITY_ONLY
    def test_native_library_new_invalid_inputs_and_reference_errors_are_byte_exact(self):
        base = {'kind': 'folder', 'folder': str(self.new_folder), 'preset': 'other', 'name': 'bad-input'}
        for name in (False, 0, [], {}):
            for kind in ('folder', 'photos', 'sources', 'invalid'):
                with self.subTest(name=name, kind=kind):
                    before = files(self.home)
                    directories = set((self.root / 'home').rglob('*'))
                    databases = (dump(self.lib.source), dump(self.lib.db))
                    registry = (self.support / 'libraries.json').read_bytes()
                    # 名称类型先于来源、路径和 kind 校验，拒绝时不能建立目录或替换会话。
                    body = {'kind': kind, 'name': name}
                    if kind == 'sources':
                        body['sources'] = []
                    response = self.request('/api/libraries/new', body)
                    self.assertEqual(response['status'], 400)
                    self.assertEqual(json.loads(response['body']), {'error': '图库名称必须是字符串'})
                    self.assertEqual(files(self.home), before)
                    self.assertEqual(set((self.root / 'home').rglob('*')), directories)
                    self.assertEqual((dump(self.lib.source), dump(self.lib.db)), databases)
                    self.assertEqual((self.support / 'libraries.json').read_bytes(), registry)
                    self.assertEqual(json.loads(self.request('/api/bootstrap')['body'])['token'], self.token)
        for change in ({'kind': 'invalid'}, {'kind': None}, {'folder': str(self.root / 'missing-folder')},
                       {'folder': 123}, {'folder': True}, {'folder': 1.5}, {'folder': ['x']}, {'folder': {'x': 1}},
                       {'folder': 'bad\0folder'}, {'preset': 'invalid'}, {'preset': 123}, {'name': 'bad\0name'},
                       {'folder': str(self.root / 'missing-folder'), 'preset': 'invalid'},
                       {'folder': 'missing/deeper/..'}, {'folder': 'downloads-Pixiv/not-scanned.png'},
                       {'name': 'x' * 256, 'folder': str(self.root / 'missing-folder'), 'preset': 'invalid'}):
            with self.subTest(change=change):
                before, registry = (dump(self.lib.source), dump(self.lib.db)), (self.support / 'libraries.json').read_bytes()
                response = self.pair('/api/libraries/new', {**base, **change})
                self.assertEqual(response['status'], 400)
                if change == {'name': 'bad\0name'}:
                    self.assertEqual(json.loads(response['body']), {'error': '无法创建图库：名称不能包含空字符（NUL）'})
                self.assertEqual(before, (dump(self.lib.source), dump(self.lib.db)))
                self.assertEqual(registry, (self.support / 'libraries.json').read_bytes())
                self.assertEqual(json.loads(self.request('/api/bootstrap')['body'])['token'], self.token)
        reference_bytes = self.new_reference.read_bytes()
        for kind, status in (('corrupt', 500),):
            with self.subTest(reference=kind):
                if kind == 'missing':
                    self.new_reference.unlink()
                else:
                    self.new_reference.write_bytes(b'not a sqlite database')
                before = (dump(self.lib.source), dump(self.lib.db))
                try:
                    response = self.pair('/api/libraries/new', base)
                    self.assertEqual(response['status'], status)
                    self.assertEqual(before, (dump(self.lib.source), dump(self.lib.db)))
                    self.assertFalse((self.home / 'bad-input.pilib').exists())
                finally:
                    self.new_reference.write_bytes(reference_bytes)
        for kind in ('pictures-file', 'dangling-dest', 'long-name'):
            with self.subTest(filesystem=kind):
                body = {**base, 'name': kind}
                if kind == 'pictures-file':
                    pictures = self.root / 'home' / 'Pictures'
                    # 之前失败的新建可能留下空的父目录；只删除测试自有的空目录，以便放进一个普通文件，
                    # 绝不递归删除调用方的内容。
                    if self.home.exists():
                        self.home.rmdir()
                    if pictures.exists():
                        pictures.rmdir()
                    pictures.parent.mkdir(parents=True, exist_ok=True)
                    pictures.write_bytes(b'not a directory')
                elif kind == 'dangling-dest':
                    dest = self.home / 'dangling-dest.pilib'
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    missing = self.root / 'missing-link-target.pilib'
                    dest.symlink_to(missing)
                else:
                    body['name'] = 'x' * 256  # ASCII 字节数超过路径组件 255 字节的上限。
                before = (dump(self.lib.source), dump(self.lib.db))
                registry = (self.support / 'libraries.json').read_bytes()
                try:
                    left = self.reference_response('/api/libraries/new', body)
                    right = self.request('/api/libraries/new', body)
                    self.assertEqual((left['status'], right['status']), (400, 400))
                    self.assertEqual(left['body'], right['body'])
                    self.assertEqual(before, (dump(self.lib.source), dump(self.lib.db)))
                    self.assertEqual(registry, (self.support / 'libraries.json').read_bytes())
                    self.assertEqual(json.loads(self.request('/api/bootstrap')['body'])['token'], self.token)
                    if kind == 'pictures-file':
                        self.assertEqual(pictures.read_bytes(), b'not a directory')
                    elif kind == 'dangling-dest':
                        self.assertTrue(dest.is_symlink())
                        self.assertEqual(dest.readlink(), missing)
                        self.assertFalse(missing.exists())
                finally:
                    if kind == 'pictures-file':
                        pictures.unlink()
                    elif kind == 'dangling-dest':
                        dest.unlink()
        self.assertFalse(self.sentinel.exists())

    @REFERENCE_PARITY_ONLY
    def test_native_library_new_photos_failure_cleanup_and_security(self):
        body = {'kind': 'photos', 'name': 'photos-failure'}
        photos = self.root / 'home' / 'Pictures' / 'Photos Library.photoslibrary'
        photos.mkdir(parents=True)
        (photos / 'untouched').write_bytes(b'synthetic Photos sentinel')
        photos_before = files(photos)
        current = (dump(self.lib.source), dump(self.lib.db))
        registry = self.support / 'libraries.json'
        registry_before = registry.read_bytes()
        reference_before = self.new_reference.read_bytes()
        for kind, status in (('csrf', 403), ('name', 400), ('corrupt', 500),
                             ('dangling', 400)):
            with self.subTest(kind=kind):
                kwargs = {}
                request = body
                if kind == 'csrf':
                    kwargs['token'] = 'invalid'
                elif kind == 'name':
                    request = {**body, 'name': 'bad\0name'}
                elif kind == 'missing':
                    self.new_reference.unlink()
                elif kind == 'corrupt':
                    self.new_reference.write_bytes(b'not sqlite')
                elif kind == 'dangling':
                    dest = self.home / 'photos-failure.pilib'
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    dest.symlink_to(self.root / 'missing-package')
                try:
                    left = self.reference_response('/api/libraries/new', request, **kwargs)
                    right = self.request('/api/libraries/new', request, **kwargs)
                    self.assertEqual(left['status'], status)
                    self.assertEqual(right['status'], status)
                    if kind in ('name', 'dangling'):
                        self.assertEqual(left['body'], right['body'])
                    self.assertEqual(current, (dump(self.lib.source), dump(self.lib.db)))
                    self.assertEqual(registry_before, registry.read_bytes())
                    self.assertEqual(photos_before, files(photos))
                    self.assertEqual(json.loads(self.request('/api/bootstrap')['body'])['token'], self.token)
                    self.assertFalse((self.home / 'photos-failure.pilib').exists())
                    if kind == 'dangling':
                        self.assertTrue(dest.is_symlink())
                finally:
                    self.new_reference.write_bytes(reference_before)
                    if kind == 'dangling':
                        dest.unlink()
        self.assertFalse(self.sentinel.exists())

    @REFERENCE_PARITY_ONLY
    def test_native_library_new_photos_registry_failure_preserves_published_package_and_current(self):
        self.check_library_new_registry_failure('photos')

    @REFERENCE_PARITY_ONLY
    def test_native_library_new_registry_failure_preserves_published_package_and_current(self):
        self.check_library_new_registry_failure('folder')

    def check_library_new_registry_failure(self, kind):
        body = {'kind': kind, 'folder': str(self.new_folder), 'preset': 'other', 'name': 'registry-failure'}
        registry = self.support / 'libraries.json'
        registry_before, current = registry.read_bytes(), (dump(self.lib.source), dump(self.lib.db))
        blocked = registry.with_suffix('.tmp'); blocked.mkdir()
        left = self.reference_response('/api/libraries/new', body)
        self.assertEqual(left['status'], 400)
        dest = self.home / 'registry-failure.pilib'
        self.assertTrue(dest.is_relative_to(self.root))
        right = self.request('/api/libraries/new', body)
        # #97（2026-10-12 用户决定）：已发布的图库包照常报告，并说明它为什么没列进去，而不是返回录制里的 400；
        # 再新建一次只会多出第二个带编号的包。
        self.assertEqual(right['status'], 200, right['body'])
        made = json.loads(right['body'])
        self.assertEqual((Path(made['path']).resolve(), made['name'], made['listed']), (dest.resolve(), 'registry-failure', False))
        self.assertTrue(made['error'])
        self.assertEqual(current, (dump(self.lib.source), dump(self.lib.db)))
        self.assertEqual(registry_before, registry.read_bytes())
        self.assertEqual(json.loads(self.request('/api/bootstrap')['body'])['token'], self.token)
        self.assertEqual(json.loads(self.request('/api/settings')['body'])['package']['path'], str(self.lib.path))
        with closing(sqlite3.connect(dest / 'library.sqlite')) as c:
            self.assertEqual(c.execute('PRAGMA quick_check').fetchone()[0], 'ok')
            self.assertEqual(c.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0], '36')
            self.assertEqual(c.execute("SELECT value FROM meta WHERE key='terms_schema'").fetchone()[0], str(TERMS_VERSION))
            self.assertEqual(c.execute("SELECT count(*) FROM term_links WHERE system='ccip' AND current=1").fetchone()[0], 2)
        for url in CORE_READS:
            self.assertEqual(self.pair(url)['route'], 'core')
        blocked.rmdir()
        self.assertEqual(self.request('/api/libraries/open', {'path': str(dest)})['status'], 200)
        wait_human_backup(self, self.origin)
        new_token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertNotEqual(new_token, self.token)
        before = dump(dest / 'library.sqlite')
        self.assertEqual(self.request('/api/startup', {'sync': True})['status'], 403)
        self.assertEqual(before, dump(dest / 'library.sqlite'))
        self.assertFalse(self.sentinel.exists())

    def test_backup_includes_current_committed_wal_human_data(self):
        with sqlite3.connect(self.lib.db) as keeper:
            keeper.execute('PRAGMA journal_mode=WAL')
            keeper.execute('PRAGMA wal_autocheckpoint=0')
            self.assertEqual(self.request('/api/tag', {'ids': ['b'], 'tag': 'committed-wal-tag'})['status'], 200)
            self.assertTrue(Path(str(self.lib.db) + '-wal').stat().st_size > 0)
            response = self.request('/api/backup', {})
            self.assertEqual(response['status'], 200)
            path = Path(json.loads(response['body'])['path'])
            self.assertEqual(path.parent, self.lib.backups)
            with sqlite3.connect(path) as backup:
                self.assertEqual(backup.execute('PRAGMA quick_check').fetchone()[0], 'ok')
                self.assertEqual(backup.execute("SELECT tag FROM asset_tags WHERE uuid='b'").fetchall(),
                                 [('committed-wal-tag',)])
                self.assertGreater(backup.execute('SELECT count(*) FROM actions WHERE undone=0').fetchone()[0], 0)

    def test_switch_same_id_different_instance_rotates_token_and_failed_switch_preserves_state(self):
        target = self.package('second')
        target.save({**target.config, 'id': self.lib.config['id']})
        old_token = self.token
        response = self.request('/api/libraries/open', {'path': str(target.path)})
        self.assertEqual(response['status'], 200)
        wait_human_backup(self, self.origin)
        value = json.loads(response['body'])
        self.assertEqual((value['path'], value['name']), (str(target.path), 'second'))
        new_token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertNotEqual(new_token, old_token)
        before = dump(target.db)
        self.assertEqual(self.request('/api/startup', {'sync': False}, token=old_token)['status'], 403)
        self.assertEqual(before, dump(target.db))
        self.token = new_token
        self.assertEqual(self.request('/api/startup', {'similar': False})['status'], 200)
        self.assertEqual(startup(target.db)['similar'], False)
        self.assertEqual(startup(self.lib.db)['similar'], True)
        before = dump(target.db)
        self.assertEqual(self.request('/api/libraries/open', {'path': str(self.root / 'missing.pilib')})['status'], 400)
        self.assertEqual(before, dump(target.db))
        self.assertEqual(json.loads(self.request('/api/bootstrap')['body'])['token'], new_token)
        self.assertEqual(json.loads(self.request('/api/settings')['body'])['package']['path'], str(target.path))
        upgrade = self.package('needs-upgrade')
        for key, value in (('terms_names', 'pending'), ('schema_version', '35')):
            with self.subTest(rejected_open=key):
                with closing(sqlite3.connect(upgrade.db)) as c:
                    old = c.execute('SELECT value FROM meta WHERE key=?', (key,)).fetchone()[0]
                    c.execute('UPDATE meta SET value=? WHERE key=?', (value, key))
                    c.commit()
                    c.execute('PRAGMA journal_mode=DELETE')
                unchanged = files(upgrade.path)
                response = self.request('/api/libraries/open', {'path': str(upgrade.path)})
                # 切换到 core 之前的图库：core v8 不再升级它，并提示改用 pre-v8 版本（#201，用户 2026-10-19 决定；
                # 以前是通用的写入失败，500）。
                self.assertEqual(response['status'], 400)
                self.assertEqual(response['body'], json.dumps({
                    'error': '这个图库还没有完全切换到词条核心，这个版本不再升级它：请先用 pre-v8 版本打开并升级，再用这个版本打开。'},
                    ensure_ascii=False).encode())
                self.assertEqual(unchanged, files(upgrade.path))
                self.assertEqual(before, dump(target.db))
                self.assertEqual(json.loads(self.request('/api/bootstrap')['body'])['token'], new_token)
                self.assertEqual(json.loads(self.request('/api/settings')['body'])['package']['path'], str(target.path))
                with closing(sqlite3.connect(upgrade.db)) as c:
                    c.execute('UPDATE meta SET value=? WHERE key=?', (old, key))
                    c.commit()
        self.assertFalse(self.sentinel.exists())

    def test_request_started_before_switch_cannot_write_new_library(self):
        target = self.package('second')
        body = b'{"similar": false}'
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=WAIT)
        self.addCleanup(conn.close)
        conn.putrequest('POST', '/api/startup', skip_host=True)
        for key, value in {'Host': self.origin.split('://')[1], 'Origin': self.origin, 'Cookie': cookie(self.origin),
                           'Content-Type': 'application/json', 'Content-Length': str(len(body)),
                           'X-CSRF-Token': self.token}.items():
            conn.putheader(key, value)
        conn.endheaders(); conn.send(body[:8])
        # 旧请求还在途中，请求体不完整；第二个连接可以切换。
        self.assertEqual(self.request('/api/libraries/open', {'path': str(target.path)})['status'], 200)
        wait_human_backup(self, self.origin)
        before = dump(target.db)
        conn.send(body[8:]); response = conn.getresponse(); response.read()
        self.assertEqual(response.status, 403)
        self.assertEqual(before, dump(target.db))

    @unittest.skipIf(os.name == 'nt', '目录符号链接在 Windows 上需要额外权限')
    def test_retargeted_alias_keeps_the_session_on_the_opened_library(self):
        # 副本与原图库共用配置 ID 和每张图片的 UUID；只有名字能区分两者。
        self.stop(self.process)
        other = self.root / 'other.pilib'
        shutil.copytree(self.lib.path, other)
        config = json.loads((other / 'library.json').read_text(encoding='utf-8'))
        (other / 'library.json').write_text(json.dumps({**config, 'name': 'other'}, ensure_ascii=False),
                                            encoding='utf-8')
        alias = self.root / 'alias.pilib'
        alias.symlink_to(self.lib.path, target_is_directory=True)
        self.process, self.log = self.start(alias)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        alias.unlink(); alias.symlink_to(other, target_is_directory=True)
        untouched = {name: dump(other / name) for name in ('library.sqlite', 'source.sqlite')}
        self.assertEqual(self.request('/api/tag', {'ids': ['b'], 'tag': 'opened-library-tag'})['status'], 200)
        self.assertEqual(self.request('/api/startup', {'similar': False})['status'], 200)
        with closing(sqlite3.connect(self.lib.db)) as c:
            self.assertEqual(c.execute("SELECT tag FROM asset_tags WHERE uuid='b' AND tag='opened-library-tag'")
                             .fetchall(), [('opened-library-tag',)])
        self.assertEqual(startup(self.lib.db)['similar'], False)
        self.assertEqual(untouched, {name: dump(other / name) for name in untouched})
        self.assertIn('opened-library-tag', self.request('/api/user-tags')['body'].decode())
        package = json.loads(self.request('/api/settings')['body'])['package']
        self.assertEqual((package['path'], package['name']), (str(alias), self.lib.name))
        # 再次打开这个别名是有意的切换：在另一个图库包上开新会话。
        old_token = self.token
        self.assertEqual(self.request('/api/libraries/open', {'path': str(alias)})['status'], 200)
        wait_human_backup(self, self.origin)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertNotEqual(self.token, old_token)
        self.assertEqual(self.request('/api/startup', {'similar': True}, token=old_token)['status'], 403)
        self.assertEqual(json.loads(self.request('/api/settings')['body'])['package']['name'], 'other')
        self.assertEqual(untouched, {name: dump(other / name) for name in untouched})
        self.assertFalse(self.sentinel.exists())

    def test_second_host_is_refused_while_the_first_owns_the_library(self):
        # #24：一个物理图库只有一个所属宿主；宿主崩溃时释放。
        before = dump(self.lib.db)
        paths = [self.lib.path]
        if os.name != 'nt':
            alias = self.root / 'alias.pilib'; alias.symlink_to(self.lib.path, target_is_directory=True)
            paths.append(alias)
        for path in paths:
            with self.subTest(path=path.name):
                result = subprocess.run([str(self.binary), 'http', '--no-auto-sync', '--library', str(path),
                                         '--port', str(free_port()), '--root', str(self.resources), '--no-names',
                                         '--no-index-check'], env=self.env, capture_output=True, timeout=WAIT)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('另一个 Illustash', result.stderr.decode(errors='replace'))
        self.assertEqual(before, dump(self.lib.db))
        self.assertEqual(self.request('/api/startup', {'similar': False})['status'], 200)
        # 所属宿主崩溃：系统释放它的锁，下一个宿主打开图库。
        self.process.kill(); self.process.wait()
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(startup(self.lib.db)['similar'], False)
        self.assertFalse(self.sentinel.exists())

    def test_stalled_connections_are_closed_and_their_number_is_bounded(self):
        # #25：停止发送的客户端在读取超时（这里缩短了）后失去连接，超出上限的连接立即被拒绝，
        # 所以卡住的客户端不能无限制地占用线程。
        body = json.dumps({'similar': False}).encode()

        def host(timeout_ms):
            self.stop(self.process)
            self.env['ILLUSTASH_HTTP_TIMEOUT_MS'] = timeout_ms
            self.process, self.log = self.start(self.lib.path)
            self.token = json.loads(self.request('/api/bootstrap')['body'])['token']

        def stalled(length):
            head = (f'POST /api/startup HTTP/1.1\r\nHost: 127.0.0.1:{self.port}\r\nOrigin: {self.origin}\r\n'
                    f'Cookie: {cookie(self.origin)}\r\n'
                    f'Content-Type: application/json\r\nContent-Length: {length}\r\nX-CSRF-Token: {self.token}\r\n\r\n')
            conn = socket.create_connection(('127.0.0.1', self.port), timeout=WAIT)
            self.addCleanup(conn.close)
            conn.sendall(head.encode() + body[:5])
            return conn

        def closed_within(conn, seconds):
            conn.settimeout(seconds)
            started = time.monotonic()
            try:
                while conn.recv(4096):
                    pass
            except ConnectionError:  # Unix 上是 reset，Windows 上是 aborted（10053）：总之都关闭了
                pass
            return time.monotonic() - started < seconds

        # 小的请求体由 HTTP 库自己读取，大的由请求处理函数读取：两种都会超时。
        host('3000')
        for length in (len(body), 4096):
            with self.subTest(length=length):
                self.assertTrue(closed_within(stalled(length), 10))
        # 占满所有连接槽：再来的连接立即被拒绝。这里的读取超时远长于繁忙机器上打开 64 个连接所需的时间，
        # 所以不会有槽提前空出来（#219）。
        host('600000')
        held = [stalled(len(body)) for _ in range(64)]
        refused = socket.create_connection(('127.0.0.1', self.port), timeout=WAIT)
        self.addCleanup(refused.close)
        refused.sendall(f'GET /api/bootstrap HTTP/1.1\r\nHost: 127.0.0.1:{self.port}\r\n\r\n'.encode())
        try:
            reply = refused.recv(4096)
        except ConnectionError:
            reply = b'reset'
        self.assertTrue(reply.startswith(b'HTTP/1.1 503') or reply in (b'', b'reset'), reply)
        # 占着的连接断开后释放它们的槽；之后宿主正常服务，而且什么都没写入。
        for conn in held:
            conn.close()
        wait_until(lambda: self.request('/api/bootstrap')['status'] == 200)
        self.assertEqual(startup(self.lib.db)['similar'], True)
        self.assertFalse(self.sentinel.exists())

    def test_slow_reader_old_snapshot_does_not_block_library_switch(self):
        target = self.package('second')
        ids = [f'old-{i:04}-' + 'x' * 24576 for i in range(200)]
        with closing(sqlite3.connect(self.lib.db)) as c:
            c.executemany('INSERT INTO assets VALUES (?,?,?,?,?,?,?)',
                          [(ident, 'synthetic.png', '', 120, 150, '2026-10-01', 'orig') for ident in ids])
            c.commit()
        # 小的接收窗口加上约 5 MiB 的 JSON，让第一个 socket 一直处于背压之下。
        sock = socket.socket()
        self.addCleanup(sock.close)
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
        sock.settimeout(WAIT)
        sock.connect(('127.0.0.1', self.port))
        sock.sendall((f'GET /api/assets?limit=200 HTTP/1.1\r\nHost: 127.0.0.1:{self.port}\r\n'
                      f'Origin: {self.origin}\r\nCookie: {cookie(self.origin)}\r\nAccept-Encoding: identity\r\n'
                      'Connection: close\r\n\r\n').encode())
        response = http.client.HTTPResponse(sock)
        self.addCleanup(response.close)
        response.begin()
        self.assertEqual(response.status, 200)
        self.assertGreater(int(response.getheader('Content-Length')), 4 * 1024 * 1024)
        before = dump(target.db)
        started = time.monotonic()
        switched = fetch(self.origin, {
            'method': 'POST', 'url': '/api/libraries/open',
            'body': json.dumps({'path': str(target.path)}).encode(),
            'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': self.token}},
            self.origin)
        self.assertEqual(switched['status'], 200)
        # 第一个连接要到 30 秒的写超时才会被宿主放弃；切换远短于它，就说明没有等那个连接（#508：对照阻塞时长）。
        self.assertLess(time.monotonic() - started, 15)
        self.assertEqual(before, dump(target.db))
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024 * 1024)
        value = json.loads(response.read())
        self.assertEqual({item['uuid'] for item in value['items']}, set(ids))
        current = json.loads(self.request('/api/assets')['body'])
        self.assertEqual({item['uuid'] for item in current['items']}, set('abc'))

    def test_upgrade_rejection_with_cold_wal_only_creates_sqlite_coordination_files(self):
        broken = self.package('cold-wal')  # 它录制的历史里有 'cold-failure-history'
        with closing(sqlite3.connect(broken.db)) as c:
            c.execute("UPDATE meta SET value='35' WHERE key='schema_version'")
            c.commit()
        before_sql = dump(broken.db)
        before = files(broken.path)
        self.assertNotIn('library.sqlite-wal', before)
        self.assertNotIn('library.sqlite-shm', before)
        result = subprocess.run([str(self.binary), 'http', '--no-auto-sync', '--library', str(broken.path),
                                 '--port', str(free_port())], env=self.env, capture_output=True, timeout=WAIT)
        self.assertNotEqual(result.returncode, 0)
        after = files(broken.path)
        added = set(after) - set(before)
        self.assertLessEqual(added, {'library.sqlite-wal', 'library.sqlite-shm'})
        self.assertEqual(before, {k: v for k, v in after.items() if k not in added})
        if 'library.sqlite-wal' in added:
            self.assertEqual((broken.path / 'library.sqlite-wal').stat().st_size, 0)
        if 'library.sqlite-shm' in added:
            self.assertEqual((broken.path / 'library.sqlite-shm').stat().st_size, 32768)
        self.assertEqual(before_sql, dump(broken.db))
        self.assertTrue(any('cold-failure-history' in line for line in before_sql))

    def test_upgrade_rejection_with_active_wal_preserves_database_wal_history(self):
        # 录制的图库包里有旧命令的历史；下面对 meta 的改动留在 WAL 里。
        broken = self.package('active-wal')  # 它录制的历史里有 'failure-wal-history'
        with closing(sqlite3.connect(broken.db)) as keeper:
            keeper.execute('PRAGMA journal_mode=WAL')
            keeper.execute('PRAGMA wal_autocheckpoint=0')
            keeper.execute("UPDATE meta SET value='35' WHERE key='schema_version'")
            keeper.commit()
            before_sql = dump(broken.db)
            before = files(broken.path)
            self.assertGreater(Path(str(broken.db) + '-wal').stat().st_size, 0)
            result = subprocess.run([str(self.binary), 'http', '--no-auto-sync', '--library', str(broken.path),
                                     '--port', str(free_port())], env=self.env, capture_output=True, timeout=WAIT)
            self.assertNotEqual(result.returncode, 0)
            after = files(broken.path)
            # 活动的 SQLite 读取方可能更新 SHM 的锁/读标记。应用数据库、WAL、source 和包里的所有其他文件逐字节不变；
            # 完整的 SQL 包含撤销历史。
            self.assertEqual({k: v for k, v in before.items() if not k.endswith('-shm')},
                             {k: v for k, v in after.items() if not k.endswith('-shm')})
            self.assertEqual(before_sql, dump(broken.db))
            self.assertTrue(any('failure-wal-history' in line for line in before_sql))

    def test_upgrade_missing_database_and_native_python_flags_reject_without_side_effects(self):
        broken = self.package('broken')
        for key, value in (('schema_version', '35'), ('terms_schema', '6'), ('terms_names', 'pending')):
            with self.subTest(key=key):
                with sqlite3.connect(broken.db) as c:
                    old = c.execute('SELECT value FROM meta WHERE key=?', (key,)).fetchone()[0]
                    c.execute('UPDATE meta SET value=? WHERE key=?', (value, key))
                    c.commit()
                    c.execute('PRAGMA journal_mode=DELETE')
                c.close()  # 静止的 DELETE 模式夹具：包里每个文件都逐字节不变。
                before = files(broken.path)
                result = subprocess.run([str(self.binary), 'http', '--no-auto-sync', '--library', str(broken.path),
                                         '--port', str(free_port())], env=self.env, capture_output=True, timeout=WAIT)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(before, files(broken.path))
                with sqlite3.connect(broken.db) as c:
                    c.execute('UPDATE meta SET value=? WHERE key=?', (old, key))
                c.close()
        broken.source.unlink()
        before = files(broken.path)
        result = subprocess.run([str(self.binary), 'http', '--no-auto-sync', '--library', str(broken.path),
                                 '--port', str(free_port())], env=self.env, capture_output=True, timeout=WAIT)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(before, files(broken.path))
        # 这里检验参数在开库前被拒绝；先停自有宿主，避免后台只读轮询改写 SHM 的协调字节。
        self.stop(self.process)
        for args in (['--python', str(self.bin / 'python')], ['--upstream', '41002']):
            before = files(self.lib.path)
            result = subprocess.run([str(self.binary), 'http', '--no-auto-sync', '--library', str(self.lib.path),
                                     '--port', str(free_port()), *args], env=self.env, capture_output=True, timeout=WAIT)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(before, files(self.lib.path))
        self.assertFalse(self.sentinel.exists())

    def test_retired_forward_route_is_rejected_and_exit_releases_port(self):
        self.stop(self.process)
        result = subprocess.run([str(self.binary), 'http', '--library', str(self.lib.path),
                                 '--port', str(self.port), '--route', 'GET:/api/fold=forward'],
                                env=self.env, capture_output=True, timeout=WAIT)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('已退役', result.stderr.decode())
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', self.port))
        self.assertFalse(self.sentinel.exists())

    def test_default_recent_library_and_native_alias(self):
        self.stop(self.process)
        log = tempfile.TemporaryFile(); self.addCleanup(log.close)
        self.process = subprocess.Popen([str(self.binary), 'http', '--root', str(self.resources),
            '--port', str(self.port), '--no-sync', '--no-names', '--no-index-check'],
            env=self.env, stdout=log, stderr=log, **SPAWN)
        self.addCleanup(self.stop, self.process)
        enter(self.origin, log, self.process)
        self.assertEqual(json.loads(self.request('/api/libraries')['body'])['current'], str(self.lib.path))
        self.assertFalse(self.sentinel.exists())
        self.stop(self.process)
        self.process, _ = self.start(self.lib.path, ('--native',))
        self.assertEqual(self.request('/api/bootstrap')['status'], 200)

    def test_all_retired_parameters_reject_before_side_effects(self):
        self.stop(self.process)
        before = files(self.lib.path)
        for arg in ('--python=unused', '--upstream=12345', '--route=GET:/api/assets=core',
                    '--refresh', '--source=unused', '--db=unused', '--cache=unused',
                    '--identity=hybrid', '--no-similar', '--'):
            result = subprocess.run([str(self.binary), 'http', arg], env=self.env,
                                    capture_output=True, timeout=WAIT)
            self.assertNotEqual(result.returncode, 0, arg)
            self.assertIn('已退役', result.stderr.decode(), arg)
        self.assertEqual(before, files(self.lib.path))
        help_result = subprocess.run([str(self.binary), 'http', '--help'], env=self.env,
                                     capture_output=True, timeout=WAIT)
        self.assertEqual(help_result.returncode, 0)
        self.assertNotIn('--upstream', help_result.stdout.decode())
        self.assertFalse(self.sentinel.exists())

    def test_production_python_has_no_compatibility_host(self):
        result = subprocess.run([__import__('sys').executable, '-c',
            "from pathlib import Path; assert not list(Path('web').glob('*.py')); assert not list(Path('tests/reference').rglob('*.py')); assert not list(Path('provenance').rglob('*.py'))"],
            cwd=self.resources, env=self.env, capture_output=True, timeout=WAIT)
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    @unittest.skipUnless(os.name == 'posix', 'macOS shell launcher acceptance')
    def test_launcher_directly_finds_rust_without_python(self):
        script = self.resources / 'start-web.command'
        self.stop(self.process)  # 同一时间一个图库只归一个宿主（#24）
        process, _ = self.start(self.lib.path, command=[str(script)])
        self.assertIsNone(process.poll())
        self.assertFalse(self.sentinel.exists())

    def test_diagnostics_http_failure_survives_restart_without_request_secrets(self):
        with closing(http.client.HTTPConnection('127.0.0.1', self.port)) as client:
            client.request('GET', '/api/diagnostics')
            denied = client.getresponse()
            self.assertEqual(denied.status, 403)
            self.assertNotIn(b'"records"', denied.read())
        backup = self.root / 'before-diagnostic.sqlite'
        with sqlite3.connect(self.lib.db) as source, sqlite3.connect(backup) as dest:
            source.backup(dest)
        with sqlite3.connect(self.lib.db) as db:
            kind = db.execute("SELECT type FROM sqlite_schema WHERE name='asset_tags'").fetchone()[0]
            self.assertIn(kind, ('table', 'view'))
            db.execute(f'DROP {kind} asset_tags')
        response = self.request('/api/user-tags?q=synthetic-private-filename.png&token=synthetic-secret')
        self.assertEqual(response['status'], 500, response['body'])
        error = json.loads(response['body'])['error']
        self.assertNotIn('终端', error)
        receipt = json.loads(response['headers']['X-Illustash-Diagnostic'])
        self.assertTrue(receipt['stored'])
        self.assertEqual(receipt['publication'], 'not_applicable')
        # WebView2 每次加载都会自己请求 /favicon.ico：照常 400，但不算用户操作的失败，不占诊断记录。
        noise = self.request('/favicon.ico')
        self.assertEqual(noise['status'], 400, noise['body'])
        self.assertNotIn('X-Illustash-Diagnostic', noise['headers'])
        report = json.loads(self.request('/api/diagnostics')['body'])
        self.assertEqual([r['id'] for r in report['records']], [receipt['id']])
        record = next(r for r in report['records'] if r['id'] == receipt['id'])
        self.assertEqual(record['cause'], 'database')
        self.assertEqual(record['stage'], 'read')
        for secret in ('synthetic-private-filename', 'synthetic-secret', str(self.root), self.token):
            self.assertNotIn(secret, json.dumps(report))
        # 修回合成数据库，重启宿主后仍能读到原来同一个编号。
        self.stop(self.process)
        with sqlite3.connect(backup) as source, sqlite3.connect(self.lib.db) as dest:
            source.backup(dest)
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertIn(receipt['id'], [r['id'] for r in json.loads(self.request('/api/diagnostics')['body'])['records']])

    def test_diagnostics_storage_failure_keeps_original_http_error_and_business_state(self):
        before = dump(self.lib.db)
        directory = self.support / 'diagnostics'
        shutil.rmtree(directory)
        directory.write_text('synthetic non-directory')
        response = self.request('/api/cache', {'action': 'synthetic-invalid-action'})
        self.assertEqual(response['status'], 400)
        self.assertEqual(json.loads(response['body']), {'error': '未知操作'})
        receipt = json.loads(response['headers']['X-Illustash-Diagnostic'])
        self.assertFalse(receipt['stored'])
        self.assertEqual(receipt['publication'], 'unknown')
        self.assertEqual(receipt['retry'], 'check_state')
        self.assertEqual(dump(self.lib.db), before)
        report = json.loads(self.request('/api/diagnostics')['body'])
        self.assertFalse(report['available'])
        self.assertEqual(report['records'], [])

    def test_diagnostics_background_import_records_failure_before_polling(self):
        invalid = self.root / 'synthetic-private-image.zip'
        invalid.write_bytes(b'not a zip: synthetic-secret')
        started = self.request('/api/libraries/import', {'path': str(invalid)})
        self.assertEqual(started['status'], 200, started['body'])
        # HTTP 诊断查询会拿独占锁，可能使后台写入返回 stored:false。
        # 先只读持久文件，不查任务状态，保证记录由后台主动生成。
        persisted = []
        def recorded():
            for path in (self.support / 'diagnostics').glob('D-*.json'):
                try:
                    record = json.loads(path.read_text(encoding='utf-8'))
                except json.JSONDecodeError:
                    continue  # 文件已创建，内容可能尚未写完；不写入、不删除、不加锁。
                if record['operation'] == 'archive':
                    persisted[:] = [record]
                    return True
            return False
        deadline = time.monotonic() + WAIT
        while time.monotonic() < deadline:
            if recorded():
                break
            time.sleep(.03)
        else:
            self.fail(f'后台导入未在 {WAIT:g} 秒内写出完整归档诊断')
        # 状态锁覆盖诊断写入和终态更新，读取返回后再查 HTTP 报告不会抢写入锁。
        job = json.loads(self.request('/api/imports')['body'])
        self.assertEqual(job['status'], 'error', job)
        self.assertTrue(job['diagnostic']['stored'], job)
        response = self.request('/api/diagnostics')
        self.assertEqual(response['status'], 200, response['body'])
        report = json.loads(response['body'])
        self.assertTrue(report['available'], report)
        reports = [r for r in report['records'] if r['operation'] == 'archive']
        self.assertEqual([r['id'] for r in reports], [persisted[0]['id']])
        self.assertEqual(job['diagnostic']['id'], reports[0]['id'])
        self.assertEqual(reports[0]['publication'], 'not_committed')
        for secret in (invalid.name, str(self.root), 'synthetic-secret'):
            self.assertNotIn(secret, json.dumps(reports))
