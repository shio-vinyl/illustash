"""在真正由历史版本产出的合成图库上，测试显式的、只涉及 terms 的启动升级。

把 ILLUSTASH_TERMS_BIN 设为被测的 release。tests/fixtures/library_upgrade 里的 v5/v6 图库由历史产出者
5e7e7f7 / d9c49b4 一次性产出，这两个提交早于公开仓库；每个测试都从逐字节相同的副本开始。要重新生成，
把 ILLUSTASH_TERMS_V5_BIN / ILLUSTASH_TERMS_V6_BIN 设为这两个产出者，然后运行
`python -m tests.test_native_library_upgrade --write-fixtures`（这需要原来的 Python 应用，所以只能在 #9
删掉它之前的检出上运行）。原 Python 参照的读取和启动事件消费者已通过 tests/fixed_expectations.py 录制
（#9）。测试从不构建、不查找默认图库，也不改动现有的黄金数据；原生宿主的 PATH 里只有 Python 哨兵。
"""
from contextlib import closing
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import sqlite3
import stat
import subprocess
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from PIL import Image

from tests import test_native_lifecycle as lifecycle
from tests.fixed_expectations import Library, Expectations, TERMS_VERSION, baseline_state, current_review_pools
from tests.synthetic_source import synthetic_reference
from tests.test_read_http import INTERRUPT, SPAWN, free_port, wait_until
from tools.build_db import SCHEMA as SOURCE_SCHEMA
from tools.verify_read_http import enter, fetch
from tools.wait_limit import WAIT

difference = lifecycle.difference

# 真实存在但未使用的混合表，取自 d9c49b4:web/identity.py:45-49。当前的 Python 构造函数已不再建它们；
# 保留它们的历史定义作为 v7 输入。
OLD_IDENTITY = '''
CREATE TABLE IF NOT EXISTS identity_claims(uuid TEXT PRIMARY KEY,character_id TEXT,variant_id TEXT,
 status TEXT NOT NULL CHECK(status IN ('assigned','undetermined')),options_json TEXT NOT NULL,
 d1 REAL,d2 REAL,reason TEXT NOT NULL,bank_version TEXT NOT NULL,scope_json TEXT NOT NULL DEFAULT '[]');
CREATE TABLE IF NOT EXISTS identity_calibration(id TEXT PRIMARY KEY,payload TEXT NOT NULL,
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
INSERT INTO meta VALUES ('identity_calibration_active','synthetic-old'),('identity_bank_error','synthetic-old');
'''

# tools/build_db.py:SCHEMA 在两个历史产出者提交上完全相同。把它钉住，免得将来 source 结构的改动悄悄改写旧输入。
HISTORICAL_SOURCE_SHA256 = '8b2c4aae7adb8e413bbe4ec309cfd523e8ae3f9bcacc6c4c6e91269aaee36c05'

FIXTURES = Path(__file__).resolve().parent / 'fixtures' / 'library_upgrade'
PRODUCERS = {5: '5e7e7f7e585f1cf8a17a8703e900a0f8fe99edef', 6: 'd9c49b42eed12c5cc810215be4acf70688617679'}
FOLDER = '<test folder>'
LOG_DIRECTORY = 'support/diagnostics/logs'
ACTIVE_LOG = 'file:' + LOG_DIRECTORY + '/runtime.log'
LOG_LOCK = 'file:' + LOG_DIRECTORY + '/.lock'
LOG_LIMIT = 2 * 1024 * 1024
EMPTY_SHA256 = hashlib.sha256(b'').hexdigest()
STARTUP_REASONS = {
    'invalid_input': '输入或请求无效',
    'database': '数据库操作失败',
    'schema': '数据库版本不兼容',
    'permission_denied': '系统拒绝访问',
    'not_found': '系统找不到文件或程序',
    'storage_full': '磁盘空间不足',
    'io': '系统读写或进程操作失败',
    'conflict': '操作与当前状态冲突',
    'network': '网络请求失败',
    'timeout': '等待服务或计算超时',
}


def tree(root):
    state = {}
    for p in root.rglob('*'):
        name = p.relative_to(root).as_posix()
        if p.is_symlink():
            state['link:' + name] = str(p.readlink())
        elif p.is_file():
            state['file:' + name] = hashlib.sha256(p.read_bytes()).hexdigest()
        elif p.is_dir():
            state['dir:' + name] = None
    return state


def assert_startup_log_appended(test, root, before, after, record):
    directory = root / LOG_DIRECTORY
    active = directory / 'runtime.log'
    lock = directory / '.lock'
    test.assertIn('dir:' + LOG_DIRECTORY, after)
    test.assertEqual(after.get(LOG_LOCK), EMPTY_SHA256)
    test.assertIn(ACTIVE_LOG, after)
    if os.name == 'posix':
        for path, mode in ((directory, 0o700), (active, 0o600), (lock, 0o600)):
            test.assertEqual(stat.S_IMODE(path.stat().st_mode), mode, path.name)
    test.assertLessEqual(active.stat().st_size, LOG_LIMIT)
    contents = active.read_bytes()
    test.assertTrue(contents.endswith(b'\n'))
    prefix, newline, line = contents[:-1].rpartition(b'\n')
    test.assertEqual(hashlib.sha256(prefix + newline).hexdigest(), before.get(ACTIVE_LOG, EMPTY_SHA256),
                     'previous log bytes must stay unchanged')
    stamp, separator, reason = line.partition(b' [host] ')
    test.assertTrue(separator, 'startup log must identify the host')
    test.assertRegex(stamp, rb'\A[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z\Z')
    expected = STARTUP_REASONS[record['cause']]
    if record['code'] is not None:
        expected += f"（代码 {record['code']}）"
    test.assertEqual(reason, expected.encode('utf-8'))


def assert_only_startup_diagnostics_written(test, root, before):
    """一次拒绝只写一条结构化诊断，并向远未达到滚动阈值的日志追加一行。"""
    after = tree(root)
    added = set(after) - set(before)
    records = []
    for key in added - {'dir:' + LOG_DIRECTORY, LOG_LOCK, ACTIVE_LOG}:
        if key in ('dir:support', 'dir:support/diagnostics'):
            test.assertIsNone(after[key])
        elif key == 'file:support/diagnostics/.lock':
            test.assertEqual(after[key], EMPTY_SHA256)
        else:
            match = re.fullmatch(r'file:support/diagnostics/(D-[0-9a-f]{32})\.json', key)
            test.assertIsNotNone(match, ('unexpected startup output', key))
            data = (root / key.removeprefix('file:')).read_bytes()
            test.assertLessEqual(len(data), 2048)
            def unique_object(pairs):
                test.assertEqual(len(pairs), len(dict(pairs)), 'duplicate diagnostic fields')
                return dict(pairs)
            record = json.loads(data, object_pairs_hook=unique_object)
            test.assertEqual(set(record), {'id', 'at', 'operation', 'stage', 'cause', 'code', 'publication', 'retry'})
            test.assertEqual(record['id'], match[1])
            test.assertIs(type(record['at']), int)
            test.assertLessEqual(0, int(time.time()) - record['at'])
            test.assertLessEqual(int(time.time()) - record['at'], 14 * 86400)
            test.assertEqual(record['operation'], 'host')
            test.assertEqual(record['stage'], 'startup')
            test.assertIs(type(record['cause']), str)
            test.assertIn(record['cause'], STARTUP_REASONS)
            if record['code'] is not None:
                test.assertIs(type(record['code']), int)
                test.assertGreaterEqual(record['code'], -(2 ** 31))
                test.assertLess(record['code'], 2 ** 31)
            test.assertEqual(record['publication'], 'unknown')
            test.assertEqual(record['retry'], 'check_state')
            records.append(record)
    test.assertEqual(len(records), 1, 'each rejected startup must persist exactly one diagnostic')
    test.assertEqual({key: value for key, value in after.items() if key not in added and key != ACTIVE_LOG},
                     {key: value for key, value in before.items() if key != ACTIVE_LOG})
    assert_startup_log_appended(test, root, before, after, records[0])


class StartupDiagnosticGuardTest(unittest.TestCase):
    def test_rejects_business_mutations_and_unvalidated_diagnostic_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            paths = ('library.pilib/library.sqlite', 'library.pilib/library.json',
                     'library.pilib/backups/old.sqlite', 'support/settings.json',
                     'support/diagnostics/D-' + 'b' * 32 + '.json')
            for name in paths:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b'unchanged')
            before = tree(root)
            directory = root / 'support/diagnostics'
            record = {'id': 'D-' + 'a' * 32, 'at': int(time.time()), 'operation': 'host', 'stage': 'startup',
                      'cause': 'schema', 'code': None, 'publication': 'unknown', 'retry': 'check_state'}
            output = directory / (record['id'] + '.json')
            output.write_text(json.dumps(record))
            (directory / '.lock').touch()
            logs = root / LOG_DIRECTORY
            logs.mkdir(mode=0o700)
            lock, active = logs / '.lock', logs / 'runtime.log'
            lock.touch(mode=0o600)
            line = '2026-01-01T00:00:00Z [host] 数据库版本不兼容\n'.encode('utf-8')
            active.write_bytes(line)
            active.chmod(0o600)
            assert_only_startup_diagnostics_written(self, root, before)
            for name in paths:
                path = root / name
                path.write_bytes(b'changed')
                with self.subTest(changed=name), self.assertRaises(AssertionError):
                    assert_only_startup_diagnostics_written(self, root, before)
                path.write_bytes(b'unchanged')
            for name in ('support/unexpected.txt', 'support/diagnostics/raw.log', 'library.pilib/new.sqlite',
                         LOG_DIRECTORY + '/raw.log', LOG_DIRECTORY + '/runtime.1.log'):
                extra = root / name
                extra.write_bytes(b'unexpected')
                with self.subTest(added=name), self.assertRaises(AssertionError):
                    assert_only_startup_diagnostics_written(self, root, before)
                extra.unlink()
            for invalid in ({**record, 'path': '/synthetic/private/file.png'},
                            {**record, 'cause': '/synthetic/private/file.png'}, {**record, 'at': True},
                            {**record, 'id': 'D-' + 'c' * 32}, {**record, 'stage': 'write'}):
                output.write_text(json.dumps(invalid))
                with self.subTest(invalid=invalid), self.assertRaises(AssertionError):
                    assert_only_startup_diagnostics_written(self, root, before)
            output.write_text(json.dumps(record))
            for invalid in (line.rstrip(b'\n') + b' /synthetic/private/art.png Cookie: mock-secret\n',
                            line.replace(b'[host]', b'[worker]'), line + line, line[:-1], b'x' * (LOG_LIMIT + 1)):
                active.write_bytes(invalid)
                with self.assertRaises(AssertionError):
                    assert_only_startup_diagnostics_written(self, root, before)
            active.write_bytes(line)
            lock.write_bytes(b'secret')
            with self.assertRaises(AssertionError):
                assert_only_startup_diagnostics_written(self, root, before)
            lock.write_bytes(b'')
            if os.name == 'posix':
                active.chmod(0o644)
                with self.assertRaises(AssertionError):
                    assert_only_startup_diagnostics_written(self, root, before)
                active.chmod(0o600)
            (directory / '.lock').write_bytes(b'secret')
            with self.assertRaises(AssertionError):
                assert_only_startup_diagnostics_written(self, root, before)
            (directory / '.lock').write_bytes(b'')
            output.unlink()
            before = tree(root)
            output.write_text(json.dumps(record))
            active.write_bytes(line + line)
            assert_only_startup_diagnostics_written(self, root, before)
            active.write_bytes(line.replace(b'2026-', b'2025-') + line)
            with self.assertRaises(AssertionError):
                assert_only_startup_diagnostics_written(self, root, before)


def database(path):
    """精确的 DDL 和每个带类型的单元格/rowid；克隆出来的输入不需要映射 UUID 或时钟。"""
    with closing(sqlite3.connect(path)) as c:
        schema = c.execute('SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name').fetchall()
        rows = {}
        for kind, name, _, sql in schema:
            if kind != 'table':
                continue
            quoted = '"' + name.replace('"', '""') + '"'
            selection = '*' if 'WITHOUT ROWID' in (sql or '').upper() else 'rowid,*'
            rows[name] = sorted([tuple((type(v).__name__, v) for v in r)
                                 for r in c.execute('SELECT ' + selection + ' FROM ' + quoted)], key=repr)
        return schema, rows


class NativeLibraryUpgradeTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls, producers=False):
        cls.binaries = {}
        for version, variable in (('current', 'ILLUSTASH_TERMS_BIN'), (5, 'ILLUSTASH_TERMS_V5_BIN'),
                                  (6, 'ILLUSTASH_TERMS_V6_BIN'))[:3 if producers else 1]:
            configured = os.environ.get(variable)
            if not configured or not Path(configured).is_file():
                raise unittest.SkipTest(f'set {variable} explicitly; absence is not upgrade acceptance')
            cls.binaries[version] = Path(configured).resolve()
        cls.resources = Path(os.environ.get('ILLUSTASH_NATIVE_ROOT') or cls.binaries['current'].parents[4]).resolve()

    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='illustash-native-upgrade-')
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        self.shared, self.support, self.home = [self.root / name for name in ('shared', 'support', 'home')]
        self.shared.mkdir()
        self.reference = synthetic_reference(self.shared)
        self.assertEqual(hashlib.sha256(SOURCE_SCHEMA.encode()).hexdigest(), HISTORICAL_SOURCE_SHA256)
        # 共用的读取夹具省略了扫描相关的列。历史上的文件夹图库照搬了 build_db 完整的 source 结构，
        # 包括可为空的 stable_hash。
        with closing(sqlite3.connect(self.reference)) as c:
            for table in ('assets', 'labels', 'decisions', 'runs'):
                c.execute('DROP TABLE ' + table)
            c.executescript(SOURCE_SCHEMA)
            c.commit()
        self.folder = self.root / 'folder'; self.folder.mkdir()
        self.rec = Expectations(self)
        self.addCleanup(self.rec.finish, self)
        self.rec.alias(self.root, '@ROOT@')
        if False:
            from tests import library_fixture as library
            for name, value in (('HOME', self.home / 'Pictures' / 'Illustash'), ('APP_SUPPORT', self.support),
                                ('SHARED', self.shared), ('REFERENCE', self.reference)):
                scoped = patch.object(library, name, value)
                scoped.start(); self.addCleanup(scoped.stop)
        self.sentinel = self.root / 'python-was-launched'
        bin_dir = self.root / 'bin'; bin_dir.mkdir()
        for name in ('python', 'python3', 'python3.12'):
            path = bin_dir / name
            path.write_text('#!/bin/sh\n/bin/echo forbidden > "$ILLUSTASH_PYTHON_SENTINEL"\nexit 97\n')
            path.chmod(0o755)
        self.env = {**os.environ, 'PATH': str(bin_dir), 'HOME': str(self.home), 'USERPROFILE': str(self.home),
                    'ILLUSTASH_APP_SUPPORT': str(self.support), 'ILLUSTASH_SHARED': str(self.shared),
                    'ILLUSTASH_PYTHON_SENTINEL': str(self.sentinel)}

    def package(self, version, name):
        """历史 v5/v6 图库的一份新副本，它的 source 指向本测试的文件夹。"""
        fixture = FIXTURES / f'v{version}'
        produced = json.loads((fixture / 'fixture.json').read_text(encoding='utf-8'))
        self.assertEqual(produced['producer'], PRODUCERS[version])
        path = self.root / (name + '.pilib')
        for sub in ('', 'features', 'thumbnails', 'backups'):
            (path / sub).mkdir()
        for database_name in ('library.sqlite', 'source.sqlite'):
            (path / database_name).write_bytes(gzip.decompress((fixture / (database_name + '.gz')).read_bytes()))
        config = json.loads((fixture / 'library.json').read_text(encoding='utf-8'))
        self.assertEqual(config['sources'][0]['path'], FOLDER)
        config['name'], config['sources'][0]['path'] = name, str(self.folder)
        lib = Library(path)
        lib.save(config)
        return lib, produced['action']


    def args(self, lib, *, upgrade=True, port=None, sync=False):
        # 迁移/历史/撤销的比较不含独立的启动任务。
        # 默认的自动同步用真实的 source DDL 另外测试。
        return [str(self.binaries['current']), 'http', '--no-names', *([] if sync else ['--no-sync']), '--library', str(lib.path),
                '--port', str(port or free_port()), '--root', str(self.resources), *(['--upgrade'] if upgrade else [])]

    def failed(self, lib, *, upgrade=True, port=None):
        result = subprocess.run(self.args(lib, upgrade=upgrade, port=port), env=self.env,
                                capture_output=True, timeout=WAIT)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(self.sentinel.exists())
        return result

    def stop(self, process):
        if process.poll() is None:
            process.send_signal(INTERRUPT)
            try:
                process.wait(WAIT)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait(WAIT)

    def start(self, lib, *, upgrade=True, sync=False, backup_gate=None):
        port = free_port(); origin = f'http://127.0.0.1:{port}'
        log = tempfile.TemporaryFile(); self.addCleanup(log.close)
        env = self.env if backup_gate is None else {**self.env, 'ILLUSTASH_TEST_HUMAN_BACKUP_GATE': str(backup_gate)}
        process = subprocess.Popen(self.args(lib, upgrade=upgrade, port=port, sync=sync), env=env, stdout=log, stderr=log, **SPAWN)
        self.addCleanup(self.stop, process)
        try:
            enter(origin, log, process)
        except AssertionError:
            log.seek(0)
            self.fail(f'upgrade startup failed ({process.poll()}): {log.read().decode(errors="replace")}')
        self.assertFalse(self.sentinel.exists())
        return process, origin

    def sync_finished(self, origin):
        result = None

        def terminal():
            nonlocal result
            response = fetch(origin, {'url': '/api/sync'}, origin)
            self.assertEqual(response['status'], 200, response['body'])
            result = json.loads(response['body'])
            return bool(result['history']) and result['history'][0]['status'] != 'running' \
                and result['state']['status'] not in ('idle', 'scanning', 'recognising', 'importing')

        wait_until(terminal)
        return result

    def backups(self, lib, before):
        return [p for p in lib.path.rglob('*.sqlite') if p.relative_to(lib.path).as_posix() not in before]

    def reads(self, lib, origin):
        token = json.loads(fetch(origin, {'url': '/api/bootstrap'}, origin)['body'])['token']
        self.rec.alias(origin, '@ORIGIN@')
        self.rec.alias(token, '@TOKEN@')
        server = thread = None
        try:
            reference = f'http://127.0.0.1:{server.server_port}' if server else None
            for url in lifecycle.CORE_READS:
                with self.subTest(url=url):
                    # 对照录制的响应；整理队列按 #786、#836 的接口变更逐字段修正。
                    left = current_review_pools(url, self.rec.value('response'))
                    right = fetch(origin, {'url': url}, origin)
                    self.assertIsNone(difference(left, right), (url, difference(left, right)))
                    self.assertEqual(right['route'], 'core')
                    for header in lifecycle.HEADERS:
                        self.assertEqual(left['headers'].get(header), right['headers'].get(header), (url, header))
            before = database(lib.db)
            for token, status in ((token, 400), ('not-current', 403)):
                out = fetch(origin, {'url': '/api/startup', 'method': 'POST', 'body': b'{}',
                                     'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': token}}, origin)
                self.assertEqual(out['status'], status)
            self.assertEqual(database(lib.db), before)
        finally:
            if server:
                server.shutdown(); thread.join(); server.server_close()

    def test_true_v5_v6_upgrade_matches_existing_migrate_history_undo_and_27_reads(self):
        for version in (5, 6):
            with self.subTest(version=version):
                # 原完整状态比较只覆盖迁移与撤销；首备份含随机 ID 和发布时间，等比较完成再释放并验收。
                backup_gate = self.root / f'backup-v{version}.gate'
                backup_gate.touch()
                lib, action = self.package(version, f'v{version}')
                before, source = database(lib.db), lib.source.read_bytes()
                existing = {p.relative_to(lib.path).as_posix() for p in lib.path.rglob('*.sqlite')}
                baseline = self.root / f'baseline-v{version}.pilib'; shutil.copytree(lib.path, baseline)
                expected = subprocess.run([str(self.binaries['current']), 'migrate', '--db', str(baseline / 'library.sqlite')],
                                          env=self.env, capture_output=True, timeout=WAIT)
                self.assertEqual(expected.returncode, 0, expected.stderr)
                # 比较等价的启动消费者：单独的 migrate CLI 让裁决事件留在待处理，而原生 HTTP 宿主会执行不带学习器的
                # 分派。原 Python 消费者的结果已录制（tests/fixed_expectations.py）。
                self.rec.package(f'drained-v{version}', baseline)
                process, origin = self.start(lib, backup_gate=backup_gate)
                # 按 #201 的规则：录制的已排空图库包处于 core 第 7 版，恢复时做过升级，所以它的物理行所在的 rowid
                # 可能与直接升级一次后的不同。
                self.assertEqual(baseline_state(lib.path)['app'], baseline_state(baseline)['app'])
                upgraded = database(lib.db)
                self.assertEqual(lib.source.read_bytes(), source)
                backups = self.backups(lib, existing)
                self.assertEqual(len(backups), 1)
                self.assertEqual(database(backups[0]), before)
                self.reads(lib, origin)
                with closing(sqlite3.connect(lib.db)) as c:
                    self.assertEqual(c.execute("SELECT value FROM meta WHERE key='terms_schema'").fetchone()[0], str(TERMS_VERSION))
                    self.assertEqual(c.execute("SELECT count(*) FROM sqlite_schema WHERE name IN ('identity_claims','identity_calibration')").fetchone()[0], 0)
                self.stop(process)
                self.assertEqual(process.returncode, 0, '首备份闸门等待必须允许宿主正常退出')
                results = []
                for db in (lib.db, baseline / 'library.sqlite'):
                    out = subprocess.run([str(self.binaries['current']), 'undo', '--db', str(db), '--action', str(action)],
                                         env=self.env, capture_output=True, timeout=WAIT)
                    self.assertEqual(out.returncode, 0, out.stderr)
                    results.append(out.stdout)
                self.assertEqual(*results)
                # 按 #201 的规则：录制的已排空图库包处于 core 第 7 版，恢复时做过升级，所以它的物理行所在的 rowid
                # 可能与直接升级一次后的不同。
                self.assertEqual(baseline_state(lib.path)['app'], baseline_state(baseline)['app'])
                process, origin = self.start(lib, upgrade=False, backup_gate=backup_gate)
                self.reads(lib, origin)
                self.stop(process)
                self.assertEqual(process.returncode, 0, '撤销后的闸门等待必须允许宿主正常退出')
                # 只在上面所有 HTTP/SQLite 连接都关闭之后，恢复一份新的、静止的、测试自有的克隆。
                # 绝不对 current7 运行旧的产出者。
                restored = self.root / f'restored-v{version}.pilib'
                self.assertTrue(restored.is_relative_to(self.root))
                shutil.copytree(lib.path, restored)
                restored_db = restored / 'library.sqlite'
                for suffix in ('-wal', '-shm'):
                    Path(str(restored_db) + suffix).unlink(missing_ok=True)
                shutil.copyfile(backups[0], restored_db)
                self.assertEqual(database(restored_db), before)
                with closing(sqlite3.connect(restored_db)) as c:
                    self.assertEqual(c.execute("SELECT value FROM meta WHERE key='terms_schema'").fetchone()[0], str(version))
                    self.assertEqual(c.execute('SELECT undone FROM actions WHERE id=?', (action,)).fetchone()[0], 0)
                recovered = Library(restored)
                process, origin = self.start(recovered, backup_gate=backup_gate)
                self.assertEqual(database(recovered.db), upgraded)
                self.assertEqual(recovered.source.read_bytes(), source)
                self.reads(recovered, origin)
                backup_gate.unlink()
                automatic = lifecycle.wait_human_backup(self, origin)
                self.assertIsNotNone(automatic['protected_revision'], automatic)
                self.assertEqual(len(automatic['snapshots']), 1, automatic)
                self.stop(process)
                self.assertEqual(process.returncode, 0)


    def test_plain_startup_rejects_true_old_schemas_without_changes(self):
        for version in (5, 6):
            with self.subTest(version=version):
                lib, _ = self.package(version, f'plain-v{version}')
                before = tree(self.root)
                self.failed(lib, upgrade=False)
                assert_only_startup_diagnostics_written(self, self.root, before)

    def test_desktop_upgrade_prompt_and_success_are_not_errors(self):
        lib, _ = self.package(6, 'desktop-upgrade')
        with closing(sqlite3.connect(lib.db)) as c, c:
            c.executescript("""
                INSERT INTO assets VALUES ('manual','manual.png','manual.png',10,10,'2026-10-01','orig'),
                    ('large','large.png','large.png',20,20,'2026-10-02','orig');
                INSERT INTO duplicates VALUES ('manual','pair',2,1,'2026-10-02',0,0),
                    ('large','pair',2,2,'2026-10-02',1,0);
                INSERT INTO meta VALUES ('terms_writer','1');
                INSERT INTO assertions SELECT 'manual',id,'confirmed','review' FROM entities ORDER BY id LIMIT 1;
                DELETE FROM meta WHERE key='terms_writer';
            """)
            decisions = c.execute('SELECT * FROM assertions').fetchall()
            self.assertEqual(len(decisions), 1)
        env = {**self.env, 'ILLUSTASH_DESKTOP_STDIN': '1'}
        before = tree(self.root)
        result = subprocess.run(self.args(lib, upgrade=False), env=env, input=b'\x01',
                                capture_output=True, timeout=WAIT)
        self.assertEqual(result.returncode, 6, result.stderr)
        self.assertEqual(result.stderr, b'')
        self.assertEqual(tree(self.root), {**before, 'dir:support': None, 'dir:support/diagnostics': None,
                                           'file:support/diagnostics/.lock': EMPTY_SHA256})

        port = free_port()
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            process = subprocess.Popen(self.args(lib, port=port), env=env, stdin=subprocess.PIPE,
                                       stdout=stdout, stderr=stderr, **SPAWN)
            try:
                with process.stdin:
                    process.stdin.write(b'\x01')
                    process.stdin.flush()
                    enter(f'http://127.0.0.1:{port}', stdout, process)
                self.assertEqual(process.wait(WAIT), 0)
                stderr.seek(0)
                self.assertEqual(stderr.read(), b'')
            finally:
                self.stop(process)
        with closing(sqlite3.connect(lib.db)) as c:
            self.assertEqual(c.execute("SELECT value FROM meta WHERE key='terms_schema'").fetchone()[0],
                             str(TERMS_VERSION))
            self.assertEqual(c.execute("SELECT value FROM meta WHERE key='fold_projection'").fetchone(), ('2',))
            self.assertEqual(c.execute('SELECT uuid,rep,hidden FROM duplicates ORDER BY pos').fetchall(),
                             [('manual', 1, 0), ('large', 0, 1)])
            self.assertEqual(c.execute('SELECT * FROM assertions').fetchall(), decisions)
        self.assertFalse(list((self.support / 'diagnostics').glob('D-*.json')))
        self.assertFalse((self.root / LOG_DIRECTORY / 'runtime.log').exists())

    def test_desktop_incompatible_schema_still_records_a_failure(self):
        lib, _ = self.package(6, 'unsupported-schema')
        with closing(sqlite3.connect(lib.db)) as c:
            c.execute("UPDATE meta SET value=? WHERE key='terms_schema'", (str(TERMS_VERSION + 1),))
            c.commit()
        before = tree(self.root)
        result = subprocess.run(self.args(lib, upgrade=False), env={**self.env, 'ILLUSTASH_DESKTOP_STDIN': '1'},
                                input=b'\x01', capture_output=True, timeout=WAIT)
        self.assertEqual(result.returncode, 4, result.stderr)
        self.assertTrue(result.stderr)
        assert_only_startup_diagnostics_written(self, self.root, before)

    def test_historical_source_default_auto_sync_records_each_start_and_imports_new_file(self):
        for version in (5, 6):
            with self.subTest(version=version):
                lib, action = self.package(version, f'auto-v{version}')
                before, source = database(lib.db), database(lib.source)
                existing = {p.relative_to(lib.path).as_posix() for p in lib.path.rglob('*.sqlite')}
                process, origin = self.start(lib, sync=True)
                state = self.sync_finished(origin)
                self.assertEqual(state['state']['status'], 'done', state)
                self.assertEqual(len(state['history']), 1)
                run = state['history'][0]
                self.assertEqual(run['status'], 'done')
                self.assertEqual(run['message'], '')
                self.assertEqual(tuple(run[k] for k in ('found', 'removed', 'imported', 'failed')), (0, 0, 0, 0))
                self.assertEqual(database(lib.source), source)  # 不隐式修复 source 结构。
                backups = self.backups(lib, existing)
                self.assertEqual(len(backups), 1)
                self.assertEqual(database(backups[0]), before)
                self.stop(process)

                # 在 current7 上即使带 --upgrade，普通重启也会再建一次同步运行；迁移什么都不做，启动同步却不是。
                process, origin = self.start(lib, sync=True)
                state = self.sync_finished(origin)
                self.assertEqual(state['state']['status'], 'done', state)
                self.assertEqual([r['status'] for r in state['history']], ['done', 'done'])
                self.assertEqual(database(lib.source), source)
                self.assertEqual(self.backups(lib, existing), backups)
                self.stop(process)

                # 测试真实的文件夹写入，而不只是没有图片的 SELECT 路径。
                image = self.folder / f'synthetic-v{version}.png'
                Image.new('RGB', (4, 3), 'red').save(image)
                process, origin = self.start(lib, upgrade=False, sync=True)
                state = self.sync_finished(origin)
                self.assertEqual(state['state']['status'], 'needs_recognition', state)
                self.assertEqual([r['status'] for r in state['history']], ['needs_recognition', 'done', 'done'])
                run = state['history'][0]
                self.assertEqual(tuple(run[k] for k in ('found', 'removed', 'imported', 'failed')), (1, 0, 1, 0))
                self.assertIn('本次启动没有识别 worker', run['message'])
                with closing(sqlite3.connect(lib.source)) as c:
                    self.assertEqual(c.execute("SELECT key,asset FROM asset_links WHERE system='md5'").fetchall(),
                                     c.execute('SELECT md5,asset FROM files').fetchall())
                    self.assertEqual(c.execute('SELECT stable_hash,width,height,src_path FROM assets').fetchall(),
                                     [(hashlib.md5(image.read_bytes()).hexdigest(), 4, 3, str(image))])
                with closing(sqlite3.connect(lib.db)) as c:
                    self.assertEqual(c.execute('SELECT count(*) FROM assets').fetchone()[0], 1)
                    self.assertEqual(c.execute("SELECT seq FROM sqlite_sequence WHERE name='sync_runs'").fetchone()[0], 3)
                    self.assertEqual(c.execute('SELECT undone FROM actions WHERE id=?', (action,)).fetchone()[0], 0)
                self.assertFalse(self.sentinel.exists())
                self.stop(process)
                image.unlink()

    def test_reduced_source_missing_hash_is_sync_error_after_successful_upgrade(self):
        # 这个故意不完整的只读夹具不是历史产出者的结构。独立于迁移等价检查，复现原来的根本原因，
        # 而不是用 --no-sync 把它藏起来。
        for version in (5, 6):
            with self.subTest(version=version):
                lib, _ = self.package(version, f'missing-hash-v{version}')
                with closing(sqlite3.connect(lib.source)) as c:
                    c.execute('ALTER TABLE assets DROP COLUMN stable_hash')
                    c.commit()
                before, source = database(lib.db), database(lib.source)
                existing = {p.relative_to(lib.path).as_posix() for p in lib.path.rglob('*.sqlite')}
                process, origin = self.start(lib, sync=True)
                state = self.sync_finished(origin)
                self.assertEqual(state['state']['status'], 'error', state)
                self.assertEqual(len(state['history']), 1)
                self.assertEqual(state['history'][0]['status'], 'error')
                for message in (state['state']['message'], state['history'][0]['message']):
                    self.assertIn('数据库错误：no such column: stable_hash', message)
                    self.assertIn('SELECT uuid,stable_hash FROM assets', message)
                with closing(sqlite3.connect(lib.db)) as c:
                    self.assertEqual(c.execute("SELECT value FROM meta WHERE key='terms_schema'").fetchone()[0], str(TERMS_VERSION))
                self.assertEqual(database(lib.source), source)
                backups = self.backups(lib, existing)
                self.assertEqual(len(backups), 1)
                self.assertEqual(database(backups[0]), before)
                self.stop(process)

    def test_occupied_port_and_invalid_preflight_do_not_write_or_backup(self):
        lib, _ = self.package(5, 'preflight')
        before = tree(self.root)
        result = subprocess.run(self.args(lib) + ['--backup'], env=self.env, capture_output=True, timeout=WAIT)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stderr.decode().strip(), '--upgrade 不可与 --backup 同用')
        assert_only_startup_diagnostics_written(self, self.root, before)
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0)); listener.listen()
            before = tree(self.root)
            self.failed(lib, port=listener.getsockname()[1])
            assert_only_startup_diagnostics_written(self, self.root, before)
        for key, value in (('schema_version', '35'), ('terms_names', 'pending')):
            with self.subTest(key=key):
                with closing(sqlite3.connect(lib.db)) as c:
                    old = c.execute('SELECT value FROM meta WHERE key=?', (key,)).fetchone()[0]
                    c.execute('UPDATE meta SET value=? WHERE key=?', (value, key)); c.commit()
                before = tree(self.root)
                self.failed(lib)
                assert_only_startup_diagnostics_written(self, self.root, before)
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute('UPDATE meta SET value=? WHERE key=?', (old, key)); c.commit()
        lib.source.rename(lib.path / 'saved-source.sqlite')
        before = tree(self.root)
        self.failed(lib)
        assert_only_startup_diagnostics_written(self, self.root, before)

    def test_injected_migration_failure_rolls_back_and_retains_preupgrade_backup(self):
        lib, _ = self.package(5, 'rollback')
        with closing(sqlite3.connect(lib.db)) as c:
            c.execute("CREATE TRIGGER fail_upgrade BEFORE INSERT ON meta WHEN NEW.key='terms_schema' AND NEW.value='" + str(TERMS_VERSION) + "' "
                      "BEGIN SELECT RAISE(ABORT,'synthetic-upgrade-failure'); END")
            c.commit()
        before, source = database(lib.db), lib.source.read_bytes()
        existing = {p.relative_to(lib.path).as_posix() for p in lib.path.rglob('*.sqlite')}
        result = self.failed(lib)
        self.assertIn(b'synthetic-upgrade-failure', result.stderr)
        self.assertEqual(database(lib.db), before)
        self.assertEqual(lib.source.read_bytes(), source)
        backups = self.backups(lib, existing)
        self.assertEqual(len(backups), 1)
        self.assertEqual(database(backups[0]), before)
        saved = backups[0].read_bytes()
        with closing(sqlite3.connect(lib.db)) as c:
            c.execute('DROP TRIGGER fail_upgrade'); c.commit()
        process, origin = self.start(lib)
        self.assertEqual(backups[0].read_bytes(), saved)
        self.reads(lib, origin)
        self.stop(process)

    def test_desktop_upgrade_reports_its_stages_and_stays_alive_through_steps_that_cannot_report(self):
        # #267：服务在就绪之前 30 秒没有任何输出，桌面外壳就会停掉它（desktop/src/server.rs，STALL）。检查边读边
        # 报告；备份、迁移和空间回收做不到，所以宿主重复最后一条进度。这里的暂停代替大图库的备份：没有这些重复，
        # 光是它就会留下 PAUSE 秒的空档。
        PAUSE = 16
        lib, _ = self.package(6, 'slow')
        existing = {p.relative_to(lib.path).as_posix() for p in lib.path.rglob('*.sqlite')}
        env = {**self.env, 'ILLUSTASH_DESKTOP_STDIN': '1', 'ILLUSTASH_TEST_UPGRADE_PAUSE_MS': str(PAUSE * 1000),
               'ILLUSTASH_TEST_TRACE_STARTUP_OUTPUT': '1'}
        trace = tempfile.TemporaryFile()
        self.addCleanup(trace.close)
        process = subprocess.Popen(self.args(lib), env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=trace, **SPAWN)
        self.addCleanup(process.stdin.close)
        self.addCleanup(process.stdout.close)
        # 读线程若卡在管道上，先退出宿主让管道到达 EOF，再关闭读端。
        self.addCleanup(self.stop, process)
        lines = []
        process.stdin.write(b'\x01')
        process.stdin.flush()

        def read():
            for raw in process.stdout:
                line = raw.decode().rstrip('\r\n')
                if line.startswith(('启动进度：', '图库已启动：')):
                    lines.append(line)
                if line.startswith('图库已启动：'):
                    return

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        reader.join(PAUSE + WAIT)
        self.assertFalse(reader.is_alive(), lines)
        self.assertTrue(lines and lines[-1].startswith('图库已启动：'), lines)
        process.stdin.close()
        self.assertEqual(process.wait(WAIT), 0)
        trace.seek(0)
        events = []
        for line in trace:
            if line.startswith(b'ILLUSTASH_TEST_STARTUP_OUTPUT_NS '):
                _, emitted_ns, label = line.decode().rstrip('\r\n').split(' ', 2)
                events.append((int(emitted_ns), label))
        expected = ['<host start>'] + [line.removeprefix('启动进度：') for line in lines[:-1]] + ['<ready>']
        self.assertEqual([label for _, label in events], expected)
        gaps = ((b_at - a_at, a_line, b_line) for (a_at, a_line), (b_at, b_line) in zip(events, events[1:]))
        worst = max(gaps, key=lambda item: item[0])
        self.assertLess(worst[0] / 1e9, PAUSE * 0.75, (worst, events))
        progress = [line.split('：', 1)[1].split(' ', 1) for line in lines if line.startswith('启动进度：')]
        permille = [int(p) for p, _ in progress]
        self.assertEqual(permille, sorted(permille))
        stages = [stage for _, stage in progress]
        firsts = [stages.index(stage) for stage in ('正在打开图库', '正在检查图库数据（library.sqlite）',
                                                    '正在检查图库数据（source.sqlite）', '正在备份图库', '正在升级图库',
                                                    '正在核对升级后的图库数据')]
        self.assertEqual(firsts, sorted(firsts), stages)
        self.assertGreaterEqual(stages.count('正在备份图库'), 2, stages)
        self.assertEqual(len(self.backups(lib, existing)), 1)

    def test_switching_to_an_old_library_asks_first_and_a_failed_upgrade_changes_nothing(self):
        # #193：页面上的切换图库，而不只是启动时的 --upgrade。
        current, _ = self.package(6, 'current')
        process, origin = self.start(current)
        old, _ = self.package(5, 'old')
        needed = '这个图库需要升级到新格式才能打开。升级后旧版本的 Illustash 打不开它；升级前会自动备份，升级没有完成时图库保持原样。'

        def post(url, body):
            token = json.loads(fetch(origin, {'url': '/api/bootstrap'}, origin)['body'])['token']
            out = fetch(origin, {'url': url, 'method': 'POST', 'body': json.dumps(body).encode(),
                                 'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': token}}, origin)
            return out['status'], json.loads(out['body'])

        def opened():
            return json.loads(fetch(origin, {'url': '/api/settings'}, origin)['body'])['package']['path']

        existing = {p.relative_to(old.path).as_posix() for p in old.path.rglob('*.sqlite')}
        before, source = database(old.db), old.source.read_bytes()
        # 先询问：什么都不写，不备份，已打开的图库保持不变。
        self.assertEqual(post('/api/libraries/open', {'path': str(old.path)}), (409, {'error': needed}))
        self.assertEqual((database(old.db), old.source.read_bytes(), self.backups(old, existing)), (before, source, []))
        self.assertEqual(opened(), str(current.path))
        # 升级失败时如实说明，图库包保持原样，当前图库保持打开。
        with closing(sqlite3.connect(old.db)) as c:
            c.execute("CREATE TRIGGER fail_upgrade BEFORE INSERT ON meta WHEN NEW.key='terms_schema' AND NEW.value='" + str(TERMS_VERSION) + "' "
                      "BEGIN SELECT RAISE(ABORT,'synthetic-upgrade-failure'); END")
            c.commit()
        failing = database(old.db)
        status, body = post('/api/libraries/open', {'path': str(old.path), 'upgrade': True})
        self.assertEqual(status, 400)
        self.assertTrue(body['error'].startswith('升级未提交：'), body)
        self.assertIn('synthetic-upgrade-failure', body['error'])
        self.assertEqual((database(old.db), old.source.read_bytes()), (failing, source))
        self.assertEqual(len(self.backups(old, existing)), 1)
        self.assertEqual(opened(), str(current.path))
        with closing(sqlite3.connect(old.db)) as c:
            c.execute('DROP TRIGGER fail_upgrade'); c.commit()
        self.assertEqual(database(old.db), before)
        # 同意后：备份、升级并打开；页面得知备份的位置。
        existing = {p.relative_to(old.path).as_posix() for p in old.path.rglob('*.sqlite')}
        status, body = post('/api/libraries/open', {'path': str(old.path), 'upgrade': True})
        self.assertEqual(status, 200, body)
        backup = Path(body['upgraded']['backup'])
        self.assertEqual(self.backups(old, existing), [backup])
        self.assertEqual(database(backup), before)
        wait_until(lambda: opened() == str(old.path))
        listed = json.loads(fetch(origin, {'url': '/api/libraries'}, origin)['body'])
        self.assertEqual(listed['upgraded'], {'backup': str(backup)})
        # 现在已是当前格式：来回切换不再询问。
        self.assertEqual(post('/api/libraries/open', {'path': str(current.path)})[0], 200)
        wait_until(lambda: opened() == str(current.path))
        self.assertNotIn('upgraded', json.loads(fetch(origin, {'url': '/api/libraries'}, origin)['body']))
        self.assertEqual(post('/api/libraries/open', {'path': str(old.path)})[0], 200)
        self.stop(process)
        self.assertFalse(self.sentinel.exists())

    def test_committed_upgrade_registry_failure_recovers_with_plain_start_and_repeated_flag_is_noop(self):
        lib, _ = self.package(6, 'registration-recovery')
        before, source, config = database(lib.db), lib.source.read_bytes(), (lib.path / 'library.json').read_bytes()
        existing = {p.relative_to(lib.path).as_posix() for p in lib.path.rglob('*.sqlite')}
        self.support.mkdir(parents=True, exist_ok=True)
        registry = self.support / 'libraries.json'
        registry_before = registry.read_bytes() if registry.exists() else None
        blocked = self.support / 'libraries.tmp'; blocked.mkdir()
        # #176：没能把图库记进最近图库，不妨碍打开它；页面会得知原因和升级前的备份，
        # 升级保持已提交（绝不报告为回滚）。
        process, origin = self.start(lib)
        libraries = json.loads(fetch(origin, {'url': '/api/libraries'}, origin)['body'])
        self.assertIn('升级已提交，没能记进最近图库', libraries['unlisted'])
        backups = self.backups(lib, existing)
        self.assertEqual(len(backups), 1)
        self.assertIn(str(backups[0]), libraries['unlisted'])
        self.assertEqual(database(backups[0]), before)
        self.assertEqual(lib.source.read_bytes(), source)
        self.assertEqual((lib.path / 'library.json').read_bytes(), config)
        with closing(sqlite3.connect(lib.db)) as c:
            self.assertEqual(c.execute("SELECT value FROM meta WHERE key='terms_schema'").fetchone()[0], str(TERMS_VERSION))
        self.assertEqual(registry.read_bytes() if registry.exists() else None, registry_before)
        self.stop(process)
        blocked.rmdir()
        process, origin = self.start(lib, upgrade=False)
        self.assertNotIn('unlisted', json.loads(fetch(origin, {'url': '/api/libraries'}, origin)['body']))
        self.reads(lib, origin)
        self.stop(process)
        current = database(lib.db)
        backup_state = {p.relative_to(lib.path).as_posix(): p.read_bytes() for p in lib.path.rglob('*.sqlite')
                        if p not in (lib.source, lib.db)}
        process, origin = self.start(lib)  # 在 current7 上显式带 --upgrade，只是普通的空操作。
        self.reads(lib, origin)
        self.stop(process)
        self.assertEqual(database(lib.db), current)
        self.assertEqual(lib.source.read_bytes(), source)
        self.assertEqual((lib.path / 'library.json').read_bytes(), config)
        self.assertEqual({p.relative_to(lib.path).as_posix(): p.read_bytes() for p in lib.path.rglob('*.sqlite')
                          if p not in (lib.source, lib.db)}, backup_state)
        self.assertFalse(self.sentinel.exists())




if __name__ == '__main__':
    unittest.main()
