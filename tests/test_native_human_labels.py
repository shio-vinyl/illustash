"""M2 用户标签/撤销验收，只用各自独立、用完即弃的包副本。

必须显式给出 ILLUSTASH_TERMS_BIN；缺少 release 构建不算验收。原生宿主经固定录制与原 Python 参考处理程序
比较（tests/fixed_expectations.py，#9：种子包、参考实现的每个响应和每一步之后参考图库的状态，
都是在参考实现还在时录制的）。两边都不启动研究 worker。每个 HTTP 步骤之后比较 app/source/provenance 的
全部表、原始 DDL、带类型的单元格和 rowid（以摘要形式），包括历史、撤销和持久化的事件。成功响应逐字节比较；
错误沿用状态码对照，另验各自真实长度及当前 500 安全文案/诊断回执。
只有会话 token、回环 origin 和临时根目录是占位符。录制的种子固定了时间戳的 DEFAULT 并预置了用户词条，
所以原生结果是确定的。快照各自在一个事务里取，跨数据库不是原子的。
"""
from contextlib import closing
import datetime
import http.client
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import subprocess
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from tests.fixed_expectations import Library, Expectations, library_digests, unclocked
from tests.test_read_http import INTERRUPT, SPAWN, free_port, wait_until
from tests.synthetic_source import source_fixture as fixture
from tools import verify_read_http
from tools.verify_read_http import cookie, enter, fetch, temporary_path

HEADERS = ('Content-Type', 'Content-Length', 'Content-Encoding', 'Cache-Control', 'Vary',
           'X-Content-Type-Options', 'Referrer-Policy', 'Content-Security-Policy')


def quote(name):
    return '"' + name.replace('"', '""') + '"'


def difference(left, right):
    """两个响应的字节差异；形似时钟的文本按 `<clock>` 比较。"""
    masked = [dict(r, body=unclocked(r['body'])) if 'body' in r else r for r in (left, right)]
    return verify_read_http.difference(*masked)


def assert_response_headers(test, reference, response):
    """错误沿用状态码对照；长度各自核实，500 另验当前安全文案及诊断回执。"""
    if response['status'] >= 400:
        for value in (reference, response):
            test.assertEqual(value['headers'].get('Content-Length'), str(len(value['body'])))
    if response['status'] == 500:
        def unique_object(pairs):
            test.assertEqual(len(pairs), len(dict(pairs)), 'duplicate diagnostic fields')
            return dict(pairs)
        test.assertEqual(json.loads(response['body'], object_pairs_hook=unique_object), {
            'error': '保存失败，请先核对操作结果；可在设置的关于页复制诊断信息'})
        diagnostic = json.loads(response['headers']['X-Illustash-Diagnostic'], object_pairs_hook=unique_object)
        test.assertEqual(set(diagnostic), {'id', 'stored', 'publication', 'retry'})
        test.assertIsNotNone(re.fullmatch(r'D-[0-9a-f]{32}', diagnostic['id']))
        test.assertIs(diagnostic['stored'], True)
        test.assertEqual(diagnostic['publication'], 'unknown')
        test.assertEqual(diagnostic['retry'], 'check_state')
    for header in HEADERS:
        if response['status'] >= 400 and header == 'Content-Length':
            continue  # 已按各自的真实字节数核实，不能拿旧提示文案的长度要求新文案。
        test.assertEqual(reference['headers'].get(header), response['headers'].get(header), header)


def snapshot(path):
    """schema 和每张表（带类型的单元格和物理 rowid），以逐表摘要的形式给出。"""
    from tests.fixed_expectations import table_digests
    return table_digests(temporary_path(path))


def states(lib):
    return library_digests(lib)


def freeze_seed_defaults(path):
    """只用于新构造的临时种子：改一次 DEFAULT，从不用于比较的输出。"""
    path = temporary_path(path)
    with closing(sqlite3.connect(path)) as c:
        current, storage = c.execute('SELECT CURRENT_TIMESTAMP,typeof(CURRENT_TIMESTAMP)').fetchone()
        assert storage == 'text' and re.fullmatch(r'\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}', current)
        stamp = datetime.datetime.strptime(current, '%Y-%m-%d %H:%M:%S').replace(tzinfo=datetime.timezone.utc)
        assert abs((datetime.datetime.now(datetime.timezone.utc) - stamp).total_seconds()) < 2
        version = c.execute('PRAGMA schema_version').fetchone()[0]
        rows = c.execute("SELECT name,sql FROM sqlite_schema WHERE type='table' AND sql IS NOT NULL").fetchall()
        c.execute('PRAGMA writable_schema=ON')
        for name, sql in rows:
            fixed = re.sub(r'\bDEFAULT\s+CURRENT_TIMESTAMP\b', "DEFAULT '2026-10-01 00:00:00'", sql, flags=re.I)
            if fixed != sql:
                c.execute("UPDATE sqlite_schema SET sql=? WHERE type='table' AND name=?", (fixed, name))
        c.execute('PRAGMA writable_schema=OFF')
        c.execute(f'PRAGMA schema_version={version + 1}')
        c.commit()
    with closing(sqlite3.connect(path)) as c:
        assert c.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'


class SnapshotTest(unittest.TestCase):
    def test_error_headers_reject_invalid_diagnostics_lengths_and_security_changes(self):
        def response(message, diagnostic=None):
            body = json.dumps({'error': message}, ensure_ascii=False).encode()
            headers = {'Content-Length': str(len(body)), 'Content-Type': 'application/json'}
            if diagnostic is not None:
                headers['X-Illustash-Diagnostic'] = json.dumps(diagnostic)
            return {'status': 500, 'body': body, 'headers': headers}
        reference = response('保存失败，未提交的操作已回滚；请查看启动终端')
        receipt = {'id': 'D-' + 'a' * 32, 'stored': True, 'publication': 'unknown', 'retry': 'check_state'}
        message = '保存失败，请先核对操作结果；可在设置的关于页复制诊断信息'
        current = response(message, receipt)
        assert_response_headers(self, reference, current)
        for invalid in ({**receipt, 'id': '/synthetic/private/file.png'}, {**receipt, 'id': receipt['id'] + '\n'},
                        {**receipt, 'stored': 1},
                        {**receipt, 'publication': 'committed'}, {**receipt, 'retry': 'retry'},
                        {**receipt, 'path': '/synthetic/private/file.png'}):
            with self.subTest(invalid=invalid), self.assertRaises(AssertionError):
                assert_response_headers(self, reference, response(message, invalid))
        for header, value in (('Content-Length', '1'), ('Content-Type', 'text/plain'),
                              ('Cache-Control', 'public'), ('X-Illustash-Diagnostic', '{}')):
            bad = {**current, 'headers': {**current['headers'], header: value}}
            with self.subTest(header=header), self.assertRaises(AssertionError):
                assert_response_headers(self, reference, bad)
        with self.assertRaises(AssertionError):
            assert_response_headers(self, {**reference, 'headers': {**reference['headers'], 'Content-Length': '1'}}, current)
        with self.assertRaises(AssertionError):
            assert_response_headers(self, reference, response('保存成功', receipt))

    def test_detects_typed_cells_rowids_ddl_history_events_and_auxiliary_changes(self):
        with tempfile.TemporaryDirectory() as tmp:
            a, b = Path(tmp) / 'a.sqlite', Path(tmp) / 'b.sqlite'
            with closing(sqlite3.connect(a)) as c:
                c.executescript("CREATE TABLE data(value); INSERT INTO data VALUES (1); "
                                "CREATE TABLE actions(id INTEGER PRIMARY KEY,before_json TEXT);"
                                "INSERT INTO actions VALUES (1,'[]');"
                                "CREATE TABLE backend_effects(id INTEGER PRIMARY KEY,status TEXT);"
                                "INSERT INTO backend_effects VALUES (1,'pending');")
            for sql, table in (("UPDATE data SET value=1.0", 'data'),
                               ("UPDATE data SET rowid=3", 'data'),
                               ("ALTER TABLE data ADD COLUMN extra", 'schema'),
                               ("UPDATE actions SET before_json='[1]'", 'actions'),
                               ("UPDATE backend_effects SET status='done'", 'backend_effects')):
                shutil.copyfile(a, b)
                self.assertEqual(snapshot(a), snapshot(b))
                with closing(sqlite3.connect(b)) as c:
                    c.execute(sql); c.commit()
                self.assertNotEqual(snapshot(a)[table], snapshot(b)[table], sql)
            # 同一个快照函数也用于指名的 source/provenance 数据库，不只是 app。
            self.assertIsNotNone(difference({'status': 200, 'body': b'{"x": 1}'},
                                            {'status': 200, 'body': b'{"x":1}'}))


    def test_http_pair_rejects_same_failed_partial_commit_and_aux_mutation(self):
        # 不用原生 release 构建、也不用活的 HTTP 服务器，走一遍真实的 pair 把关。
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            left, right = SimpleNamespace(path=root / 'left'), SimpleNamespace(path=root / 'right')
            for lib in (left, right):
                lib.path.mkdir()
                for filename in ('library.sqlite', 'source.sqlite', 'provenance.sqlite'):
                    with closing(sqlite3.connect(lib.path / filename)) as c:
                        c.executescript('CREATE TABLE data(value); INSERT INTO data VALUES (1);')
            harness = NativeHumanLabelsTest('test_tag_add_remove_filter_duplicates_and_literal_false')
            harness.left, harness.right = left, right
            harness.direct, harness.origin = 'left', 'right'
            harness.token, harness.latency, harness.sentinel = 'token', [], root / 'sentinel'
            harness.rec = Live(harness)
            def partial(base, item, origin):
                lib = left if base == 'left' else right
                with closing(sqlite3.connect(lib.path / 'library.sqlite')) as c:
                    c.execute('UPDATE data SET value=2'); c.commit()
                return {'status': 500, 'body': b'{"error":"same"}', 'route': 'core', 'headers': {}, 'ms': 1}
            with patch(__name__ + '.fetch', partial), self.assertRaisesRegex(AssertionError, 'failed POST partially committed'):
                harness.pair('/api/tag', {'ids': ['a'], 'tag': 'synthetic'}, expected=500)
            for filename in ('source.sqlite', 'provenance.sqlite'):
                shutil.copyfile(left.path / 'source.sqlite', right.path / 'source.sqlite')
                shutil.copyfile(left.path / 'provenance.sqlite', right.path / 'provenance.sqlite')
                self.assertEqual(states(left), states(right))
                def auxiliary(base, item, origin):
                    if base == 'right':
                        with closing(sqlite3.connect(right.path / filename)) as c:
                            c.execute('UPDATE data SET value=3'); c.commit()
                    return {'status': 200, 'body': b'{}', 'route': 'core', 'headers': {}, 'ms': 1}
                with patch(__name__ + '.fetch', auxiliary), self.assertRaises(AssertionError):
                    harness.pair('/api/tag', {'ids': ['a'], 'tag': 'synthetic'})


class Live:
    """比较器自测的假响应与状态输入，独立于产品实现。"""
    def __init__(self, harness):
        self.harness = harness
    def state(self, label):
        return states(self.harness.left)
    def response(self, item):
        return fetch(self.harness.direct,item,self.harness.direct)


class NativeHumanLabelsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        configured = os.environ.get('ILLUSTASH_TERMS_BIN')
        if not configured or not Path(configured).is_file():
            raise unittest.SkipTest('set ILLUSTASH_TERMS_BIN explicitly to the M2 release; skip is not acceptance')
        cls.binary = Path(configured).resolve()
        cls.resources = Path(os.environ.get('ILLUSTASH_NATIVE_ROOT') or cls.binary.parents[4]).resolve()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='illustash-m2-human-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.rec = Expectations(self)
        self.addCleanup(self.rec.finish, self)
        self.rec.alias(self.root, '@ROOT@')
        support, shared = self.root / 'support', self.root / 'shared'
        shared.mkdir()
        sentinel_bin = self.root / 'bin'; sentinel_bin.mkdir()
        self.sentinel = self.root / 'python-was-launched'
        for command in ('python', 'python3', 'python3.12'):
            executable = sentinel_bin / command
            executable.write_text('#!/bin/sh\n/bin/echo forbidden > "$ILLUSTASH_PYTHON_SENTINEL"\nexit 97\n')
            executable.chmod(0o755)
        self.env = {**os.environ, 'PATH': str(sentinel_bin), 'HOME': str(self.root / 'home'), 'USERPROFILE': str(self.root / 'home'),
                    'ILLUSTASH_APP_SUPPORT': str(support), 'ILLUSTASH_SHARED': str(shared),
                    'ILLUSTASH_PYTHON_SENTINEL': str(self.sentinel),
                    # 可复现的原生键，录制的期望才能成立（store::new_key 的测试钩子）。
                    'ILLUSTASH_TEST_KEY_COUNTER': str(self.root / 'key-counter')}
        for name in ('pristine', 'left', 'right'):
            lib = self.rec.package(name, self.root / f'{name}.pilib')
            setattr(self, name, lib)
        self.alice, self.bob, self.artist = self.rec.value('ids')
        self.port = free_port(); self.origin = f'http://127.0.0.1:{self.port}'
        self.rec.alias(self.origin, '@ORIGIN@')
        self.process = self.start(self.right)
        self.token = json.loads(fetch(self.origin, {'url': '/api/bootstrap'}, self.origin)['body'])['token']
        self.rec.alias(self.token, '@TOKEN@')
        self.assertEqual(self.rec.state('start'), states(self.right))
        self.latency = []



    def clone(self, source, name):
        path = self.root / (name + '.pilib'); path.mkdir()
        shutil.copyfile(source.path / 'library.json', path / 'library.json')
        shutil.copyfile(source.path / 'album-char-map.json', path / 'album-char-map.json')
        for filename in ('library.sqlite', 'source.sqlite', 'provenance.sqlite'):
            with closing(sqlite3.connect(source.path / filename)) as src, closing(sqlite3.connect(path / filename)) as dst:
                src.backup(dst)
        for directory in ('thumbnails', 'features', 'backups'):
            (path / directory).mkdir()
        return Library(path)

    def stop(self, process):
        if process.poll() is None:
            process.send_signal(INTERRUPT)
            try:
                process.wait(5)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait(3)

    def start(self, lib):
        log = tempfile.TemporaryFile(); self.addCleanup(log.close)
        process = subprocess.Popen([str(self.binary), 'http', '--library', str(lib.path),
                                    '--port', str(self.port), '--root', str(self.resources), '--no-names', '--no-index-check'],
                                   env=self.env, stdout=log, stderr=log, **SPAWN)
        self.addCleanup(self.stop, process)
        try:
            enter(self.origin, log, process, timeout=5)
        except AssertionError:
            log.seek(0); self.fail(f'native startup failed: {log.read().decode(errors="replace")}')
        return process

    def reference_response(self, label, item):
        """原参考实现对同一请求的响应（已录制）。"""
        return self.rec.response(item) if isinstance(self.rec, Live) else self.rec.value(label)

    def pair(self, url, body, *, expected=200, headers=None):
        item = {'url': url, 'method': 'POST', 'body': json.dumps(body, ensure_ascii=False).encode('utf-8'),
                'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': self.token, **(headers or {})}}
        before = [self.rec.state('before'), states(self.right)]
        responses = [self.reference_response('response', item), fetch(self.origin, item, self.origin)]
        after = [self.rec.state('after'), states(self.right)]
        # 即使两边的命令以相同方式失败/部分提交，验收也必须判为失败。
        for old, new, response in zip(before, after, responses):
            if response['status'] >= 400:
                self.assertEqual(old, new, ('failed POST partially committed', url, body))
        self.assertIsNone(difference(*responses), (url, body, difference(*responses)))
        self.assertEqual(responses[1]['status'], expected, (url, body, responses[1]['body']))
        self.assertEqual(responses[1]['route'], 'core')
        assert_response_headers(self, *responses)
        self.assertEqual(after[0], after[1], (url, body))
        self.latency.append((url, responses[0]['ms'], responses[1]['ms']))
        self.assertFalse(self.sentinel.exists())
        return json.loads(responses[1]['body'])

    def latest_action(self):
        """最新的未撤销操作；两边的历史相同（每一步之后都检查）。"""
        with closing(sqlite3.connect(self.right.db)) as c:
            action = c.execute('SELECT id FROM actions WHERE undone=0 ORDER BY id DESC LIMIT 1').fetchone()
        return action[0] if action else None

    def undo_latest(self):
        action = self.latest_action()
        self.assertIsNotNone(action)
        return self.pair('/api/undo', {'id': action})

    def seed_both(self):
        # 用真实的旧命令播种一次历史（已录制），再复制完整的已提交输入。
        self.stop(self.process)
        self.rec.package('seeded', self.left.path)
        for filename in ('library.sqlite', 'source.sqlite', 'provenance.sqlite'):
            with closing(sqlite3.connect(self.left.path / filename)) as src, closing(sqlite3.connect(self.right.path / filename)) as dst:
                src.backup(dst)
        self.process = self.start(self.right)
        self.token = json.loads(fetch(self.origin, {'url': '/api/bootstrap'}, self.origin)['body'])['token']
        self.rec.alias(self.token, '@TOKEN@')
        self.assertEqual(self.rec.state('seeded'), states(self.right))

    def test_tag_add_remove_filter_duplicates_and_literal_false(self):
        first = self.pair('/api/tag', {'ids': ['a', 'a'], 'tag': ' 合成\t标签\n'})['undo']
        self.assertEqual(first['label'], '已给 1 张加上标签 合成 标签')
        for value in (0, None, 'false', False, True):
            self.pair('/api/tag', {'ids': ['a'], 'tag': '合成 标签', 'add': value})
        self.pair('/api/tag', {'filter': {}, 'exclude': ['b'], 'tag': '第二标签'})
        self.pair('/api/tag', {'filter': {'artist': self.artist}, 'tag': '第二标签'})
        self.pair('/api/tag', {'ids': ['c'] * 501, 'tag': '第二标签', 'confirm': True})
        for _ in range(9):
            self.undo_latest()

    def test_validation_error_order_and_no_partial_commit(self):
        cases = [({}, 400), ({'tag': None, 'ids': ['missing']}, 400),
                 ({'tag': 'x' * 61, 'ids': []}, 400), ({'tag': ' ' * 61}, 400)]
        base = {'tag': '合成 标签'}
        cases += [(base | body, 400) for body in (
            {'ids': None}, {'ids': []}, {'ids': 'a'}, {'ids': [1]}, {'ids': ['a', 'missing']},
            {'ids': ['a'], 'exclude': 'b'}, {'ids': ['a'], 'exclude': [1]},
            {'ids': ['a'], 'exclude': ['b']}, {'ids': ['a'], 'filter': {}},
            {'filter': []}, {'filter': {'unknown': ''}}, {'filter': {'entity': 1}},
            {'filter': {'q': 'missing-synthetic'}}, {'filter': {}, 'exclude': ['a', 'b', 'c']},
            {'ids': ['a'] * 501}, {'ids': ['a'] * 501, 'confirm': 1})]
        for body, status in cases:
            with self.subTest(body=body):
                self.pair('/api/tag', body, expected=status)
        for value in (None, 999999, 0, '999999', 1.25, [], {}):
            with self.subTest(undo=value):
                self.pair('/api/undo', {'id': value}, expected=400 if value not in ([], {}) else 500)

    def test_cross_type_latest_undo_and_committed_effects(self):
        self.seed_both()
        self.pair('/api/tag', {'ids': ['c'], 'tag': '第二标签'})
        with closing(sqlite3.connect(self.right.db)) as c:
            oldest = c.execute('SELECT min(id) FROM actions WHERE undone=0').fetchone()[0]
        self.pair('/api/undo', {'id': oldest}, expected=400)
        for _ in range(5):
            self.undo_latest()
        with closing(sqlite3.connect(self.right.db)) as c:
            self.assertEqual(c.execute("SELECT kind,status,error FROM backend_effects").fetchall(),
                             [('verdict', 'done', '')] * 4)
        self.pair('/api/undo', {'id': oldest}, expected=400)

    def test_injected_writes_and_unknown_history_are_atomic(self):
        for table in ('actions', 'term_verdicts'):
            for lib in (self.left, self.right):
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute(f"CREATE TRIGGER fail_write BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'synthetic write failure'); END")
                    c.commit()
            self.pair('/api/tag', {'ids': ['a'], 'tag': '合成 标签'}, expected=500)
            for lib in (self.left, self.right):
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute('DROP TRIGGER fail_write'); c.commit()
        for owner in ('character', 'curation'):
            self.seed_both()
            for lib in (self.left, self.right):
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute("CREATE TRIGGER fail_event BEFORE INSERT ON backend_effects BEGIN SELECT RAISE(ABORT,'synthetic event failure'); END")
                    c.commit()
            with closing(sqlite3.connect(self.right.db)) as c:
                action = c.execute('SELECT max(id) FROM actions WHERE undone=0').fetchone()[0]
            self.pair('/api/undo', {'id': action}, expected=500)
            for lib in (self.left, self.right):
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute('DROP TRIGGER fail_event'); c.commit()
            self.undo_latest()
        for lib in (self.left, self.right):
            with closing(sqlite3.connect(lib.db)) as c:
                c.execute("INSERT INTO actions(label,before_json) VALUES ('unknown synthetic','[{\"unknown\":true}]')")
                c.commit()
        self.pair('/api/undo', {'id': self.rec.value('reference undo id')}, expected=500)

    def test_csrf_host_and_origin_refuse_without_database_writes(self):
        for headers in ({'X-CSRF-Token': 'bad'}, {'Host': 'evil.example'}, {'Origin': 'https://evil.example'}):
            self.pair('/api/tag', {'ids': ['a'], 'tag': '合成 标签'}, expected=403, headers=headers)
        self.pair('/api/undo', {'id': 1}, expected=403, headers={'X-CSRF-Token': 'bad'})

    def test_new_concept_failure_is_fully_rolled_back_native(self):
        for lib in (self.left, self.right):
            with closing(sqlite3.connect(lib.db)) as c:
                c.execute("CREATE TRIGGER fail_action BEFORE INSERT ON actions BEGIN SELECT RAISE(ABORT,'synthetic action failure'); END")
                c.commit()
        # 注入的 schema 本身是共同输入的一部分，不是输出上的例外。
        before = [self.rec.state('before'), states(self.right)]
        item = {'method': 'POST', 'url': '/api/tag', 'body': json.dumps({'ids': ['a'], 'tag': '全新故障标签'}),
                'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': self.token}}
        old = self.reference_response('response', item)
        native = fetch(self.origin, item, self.origin)
        self.assertIsNone(difference(old, native), difference(old, native))
        self.assertEqual(native['status'], 500)
        self.assertEqual(states(self.right), before[1])
        # 旧 CLI 在裁决事务之外创建 concept。把旧实现真实的部分提交单独录制；
        # 这个例外从不放宽普通的 pair()。
        after_old = self.rec.state('after old')
        self.assertNotEqual(after_old, before[0], 'the legacy partial-create bug must be evidenced')
        self.assertEqual(after_old['source'], before[0]['source'])
        self.assertEqual(after_old['provenance'], before[0]['provenance'])
        self.assertEqual(after_old['app']['actions'], before[0]['app']['actions'])
        self.assertEqual(after_old['app']['backend_effects'], before[0]['app']['backend_effects'])
        self.assertEqual(self.rec.value('old concepts'), 1)
        with closing(sqlite3.connect(self.right.db)) as c:
            self.assertEqual(c.execute("SELECT count(*) FROM terms t JOIN term_names n ON n.term_id=t.id WHERE t.kind='concept' AND n.value='全新故障标签'").fetchone()[0], 0)
        # 成功的新建只看原生一侧：两个引擎各自生成独立的 UUID。
        # 不映射它们，这个单独阶段也不声称两边状态完全相同。
        with closing(sqlite3.connect(self.right.db)) as c:
            c.execute('DROP TRIGGER fail_action'); c.commit()
        retry_before = states(self.right)
        created = fetch(self.origin, item, self.origin)
        self.assertEqual(created['status'], 200, created['body'])
        self.assertEqual(created['route'], 'core')
        action = json.loads(created['body'])['undo']
        self.assertEqual(action['label'], '已给 1 张加上标签 全新故障标签')
        expected_tags = self.rec.value('reference reading of the native tags')
        tags = fetch(self.origin, {'url': '/api/user-tags'}, self.origin)
        self.assertEqual(tags['status'], 200)
        self.assertEqual(tags['body'], json.dumps(expected_tags, ensure_ascii=False).encode())
        self.assertEqual([entry['tag'] for entry in expected_tags], ['全新故障标签'])
        with closing(sqlite3.connect(self.right.db)) as c:
            self.assertEqual(c.execute('SELECT uuid,tag FROM asset_tags ORDER BY uuid,tag').fetchall(),
                             [('a', '全新故障标签'), ('b', '全新故障标签')])
            history = c.execute('SELECT id,label,ts,before_json,undone FROM actions WHERE id=?', (action['id'],)).fetchone()
            self.assertEqual(history[:3], (action['id'], action['label'], action['ts']))
            self.assertEqual(json.loads(history[3]), [{'terms': 'verdicts'}])
            self.assertEqual(history[4], 0)
            self.assertEqual(c.execute('SELECT max(id) FROM actions WHERE undone=0').fetchone()[0], action['id'])
            self.assertEqual(c.execute('PRAGMA foreign_key_check').fetchall(), [])
        for name in ('source', 'provenance'):
            self.assertEqual(states(self.right)[name], retry_before[name])
        undone = fetch(self.origin, {'method': 'POST', 'url': '/api/undo', 'body': json.dumps({'id': action['id']}).encode(),
                                     'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': self.token}}, self.origin)
        self.assertEqual(undone['status'], 200, undone['body'])
        self.assertEqual(undone['route'], 'core')
        self.assertEqual(undone['body'], b'{"undo": null}')
        self.assertEqual(fetch(self.origin, {'url': '/api/user-tags'}, self.origin)['body'], b'[]')
        with closing(sqlite3.connect(self.right.db)) as c:
            self.assertEqual(c.execute('SELECT uuid,tag FROM asset_tags').fetchall(), [])
            self.assertEqual(c.execute('SELECT undone FROM actions WHERE id=?', (action['id'],)).fetchone()[0], 1)
            self.assertEqual(c.execute('SELECT max(id) FROM actions WHERE undone=0').fetchone()[0], None)
            self.assertEqual(c.execute('PRAGMA foreign_key_check').fetchall(), [])
            # 撤销去掉裁决，新建的身份和完整的历史仍然保留。
            self.assertEqual(c.execute("SELECT count(*) FROM terms t JOIN term_names n ON n.term_id=t.id WHERE t.kind='concept' AND n.value='全新故障标签'").fetchone()[0], 1)
        retry_after = states(self.right)
        for name in ('source', 'provenance'):
            self.assertEqual(retry_after[name], retry_before[name])
        self.assertEqual(retry_after['app']['schema'], retry_before['app']['schema'])
        self.assertEqual(retry_after['app']['backend_effects'], retry_before['app']['backend_effects'])
        self.assertFalse(self.sentinel.exists())

    def test_undo_input_types_keep_owner_specific_legacy_contract(self):
        for owner in ('tag', 'artist', 'character', 'curation', 'identity_skip'):
            for value in (True, 1.0, '1'):
                with self.subTest(owner=owner, id=value):
                    self.seed_both()
                    rejected = owner == 'character' or owner in ('curation', 'identity_skip') and isinstance(value, str)
                    self.pair('/api/undo', {'id': value}, expected=400 if rejected else 200)

    def test_write_lock_contention_preserves_exact_error_and_all_data(self):
        with closing(sqlite3.connect(self.left.db)) as left, closing(sqlite3.connect(self.right.db)) as right:
            left.execute('BEGIN IMMEDIATE'); right.execute('BEGIN IMMEDIATE')
            self.pair('/api/tag', {'ids': ['a'], 'tag': '合成 标签'}, expected=500)
            left.rollback(); right.rollback()
        self.pair('/api/tag', {'ids': ['a'], 'tag': '合成 标签'})
        self.undo_latest()

    def test_inflight_old_token_cannot_write_after_library_switch(self):
        self.stop(self.process)
        newer = self.clone(self.right, 'new-session')
        self.process = self.start(self.right)
        old_token = json.loads(fetch(self.origin, {'url': '/api/bootstrap'}, self.origin)['body'])['token']
        before_old, before_new = states(self.right), states(newer)
        body = json.dumps({'ids': ['a'], 'tag': '合成 标签'}).encode()
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=5)
        self.addCleanup(conn.close)
        conn.putrequest('POST', '/api/tag', skip_host=True)
        for name, value in {'Host': self.origin.split('://')[1], 'Origin': self.origin, 'Cookie': cookie(self.origin),
                            'Content-Type': 'application/json', 'X-CSRF-Token': old_token,
                            'Content-Length': str(len(body))}.items():
            conn.putheader(name, value)
        conn.endheaders(); conn.send(body[:1])
        # 旧请求还在接收请求体时，第二个连接切换了图库。
        response = fetch(self.origin, {'method': 'POST', 'url': '/api/libraries/open',
                                      'body': json.dumps({'path': str(newer.path)}),
                                      'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': old_token}}, self.origin)
        self.assertEqual(response['status'], 200, response['body'])
        current_token = json.loads(fetch(self.origin, {'url': '/api/bootstrap'}, self.origin)['body'])['token']
        self.assertNotEqual(current_token, old_token)
        conn.send(body[1:]); stale = conn.getresponse()
        self.assertEqual(stale.status, 403, stale.read())
        self.assertEqual(states(self.right), before_old)
        self.assertEqual(states(newer), before_new)
        stale_again = fetch(self.origin, {'method': 'POST', 'url': '/api/tag', 'body': body,
                                         'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': old_token}}, self.origin)
        self.assertEqual(stale_again['status'], 403)
        self.assertEqual(states(newer), before_new)
        self.assertFalse(self.sentinel.exists())

    def test_pending_verdict_startup_and_failed_ack_resume_without_http_trigger(self):
        # 分发器的确认失败发生在人工事务已经提交之后。旧参考实现回答 HTTP 500 "rolled back"；
        # 按用户 2026-10-11 的决定（#40），已提交的撤销现在回答 200 并带上最新可撤销的一步，
        # 事件保持 pending，留待恢复。数据库状态仍与录制一致。
        # 这个显式的已提交阶段与 pair() 的提交前故障把关分开。
        for owner in ('character', 'curation'):
            self.seed_both()
            for lib in (self.left, self.right):
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute("CREATE TRIGGER fail_ack BEFORE UPDATE ON backend_effects WHEN OLD.kind='verdict' "
                              "BEGIN SELECT RAISE(ABORT,'synthetic acknowledgement failure'); END")
                    c.commit()
            with closing(sqlite3.connect(self.right.db)) as c:
                action = c.execute('SELECT max(id) FROM actions WHERE undone=0').fetchone()[0]
            before = [self.rec.state('before'), states(self.right)]
            item = {'method': 'POST', 'url': '/api/undo', 'body': json.dumps({'id': action}).encode(),
                    'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': self.token}}
            old, native = self.reference_response('response', item), fetch(self.origin, item, self.origin)
            self.assertEqual(old['status'], 500)  # 录制的旧响应，已不再是契约
            self.assertEqual(native['status'], 200, native['body'])
            self.assertIn('undo', json.loads(native['body']))
            after = [self.rec.state('after'), states(self.right)]
            self.assertEqual(after[0], after[1])
            def committed(lib):
                with closing(sqlite3.connect(lib.db)) as c:
                    return (c.execute('SELECT undone FROM actions WHERE id=?', (action,)).fetchone()[0],
                            c.execute("SELECT kind,status,error FROM backend_effects ORDER BY id DESC LIMIT 1").fetchone())
            for lib, previous, current in zip((self.left, self.right), before, after):
                self.assertEqual(previous['source'], current['source'])
                self.assertEqual(previous['provenance'], current['provenance'])
            for seen in (self.rec.value('old side committed'), committed(self.right)):
                self.assertEqual(tuple(seen[0:1]) + (tuple(seen[1]),), (1, ('verdict', 'pending', '')))
            for lib in (self.left, self.right):
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute('DROP TRIGGER fail_ack'); c.commit()
            drained = self.rec.state('drained')
            # 没有 HTTP 请求：原生循环必须自行恢复已提交的 pending 事件。
            wait_until(lambda: states(self.right) == drained, timeout=5)
            self.assertFalse(self.sentinel.exists())
        self.stop(self.process)
        with closing(sqlite3.connect(self.right.db)) as c:
            existing = c.execute('SELECT kind,status,error FROM backend_effects ORDER BY id').fetchall()
            c.execute("INSERT INTO backend_effects(kind) VALUES ('verdict')")
            c.execute("INSERT INTO backend_effects(kind) VALUES ('unknown-synthetic')")
            c.execute("CREATE TRIGGER fail_ack BEFORE UPDATE ON backend_effects WHEN OLD.kind='verdict' "
                      "BEGIN SELECT RAISE(ABORT,'synthetic acknowledgement failure'); END")
            c.commit()
        self.process = self.start(self.right)
        with closing(sqlite3.connect(self.right.db)) as c:
            self.assertEqual(c.execute('SELECT kind,status,error FROM backend_effects ORDER BY id').fetchall(),
                             existing + [('verdict', 'pending', ''), ('unknown-synthetic', 'pending', '')])
            c.execute('DROP TRIGGER fail_ack'); c.commit()
        def consumed():
            with closing(sqlite3.connect(self.right.db)) as c:
                return c.execute("SELECT count(*) FROM backend_effects WHERE kind='verdict' AND status='pending'").fetchone()[0] == 0
        wait_until(consumed, timeout=5)
        with closing(sqlite3.connect(self.right.db)) as c:
            self.assertEqual(c.execute('SELECT kind,status,error FROM backend_effects ORDER BY id').fetchall(),
                             existing + [('verdict', 'done', ''), ('unknown-synthetic', 'pending', '')])
        # 第二个已发布的 pending 裁决在重启时也必须被处理掉。
        self.stop(self.process)
        with closing(sqlite3.connect(self.right.db)) as c:
            c.execute("INSERT INTO backend_effects(kind) VALUES ('verdict')"); c.commit()
        self.process = self.start(self.right)
        self.assertTrue(consumed())
