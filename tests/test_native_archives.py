"""#42 验收。所有路径和包都是临时的合成夹具；没有真实图库输入。"""
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
import json
import os
from pathlib import Path
import shutil
import sqlite3
import threading
import time
import unittest
from unittest.mock import patch
import zipfile

from tests import test_native_lifecycle as lifecycle
from tests.test_native_human_labels import snapshot
from tests.test_read_http import wait_until
from tests import library_fixture as library
from tools.wait_limit import WAIT


class NativeArchivesTest(unittest.TestCase):
    setUpClass = classmethod(lifecycle.NativeLifecycleTest.setUpClass.__func__)
    setUp = lifecycle.NativeLifecycleTest.setUp
    package = lifecycle.NativeLifecycleTest.package
    stop = lifecycle.NativeLifecycleTest.stop
    clone_closed_library = lifecycle.NativeLifecycleTest.clone_closed_library
    request = lifecycle.NativeLifecycleTest.request
    pair = lifecycle.NativeLifecycleTest.pair

    def start(self, path, extra=(), command=None):
        self.env['ILLUSTASH_NO_KEYCHAIN'] = '1'
        return lifecycle.NativeLifecycleTest.start(self, path, extra, command)

    def reference_status(self, url, body=None, **kwargs):
        """原参考服务器对同一请求给出的状态码（已录制）。"""
        return lifecycle.NativeLifecycleTest.reference_response(self, url, body, **kwargs)['status']

    def state(self):
        return json.loads(self.request('/api/libraries')['body'])['export']

    def archive_import(self, body, **kwargs):
        return lifecycle.import_task(self, '/api/libraries/import', body, **kwargs)

    def begin(self):
        response = self.request('/api/libraries/export', {})
        self.assertEqual(response['status'], 200, response)
        self.assertEqual(response['route'], 'core')
        return json.loads(response['body'])

    def done(self):
        wait_until(lambda: self.state()['status'] in ('done', 'error'))
        result = self.state()
        self.assertEqual(result['status'], 'done', result)
        return Path(result['path'])

    def test_share_export_and_legacy_import_remove_secrets_and_require_new_consent(self):
        markers = [f'archive-synthetic-secret-{kind}' for kind in ('current', 'legacy', 'future', 'retired')]
        response = self.request('/api/names', {'action': 'model_api', 'key': markers[0],
                                             'model': 'synthetic-model', 'base': 'https://example.invalid'})
        self.assertEqual(response['status'], 200, response)
        response = self.request('/api/names', {'action': 'network', 'allow': True})
        self.assertEqual(response['status'], 200, response)
        with closing(sqlite3.connect(self.lib.db)) as c:
            c.execute('PRAGMA secure_delete=OFF')
            c.executemany('INSERT OR REPLACE INTO name_settings VALUES (?,?)',
                          [('deepseek_key', markers[1]), ('future_credential', markers[2]),
                           ('traffic_reserved', '2000000'), ('retired_key', markers[3] + 'x' * 16000)])
            c.commit()
            c.execute("DELETE FROM name_settings WHERE key='retired_key'")
            c.execute("INSERT OR REPLACE INTO prefs VALUES ('startup',?)", ('{"names":true,"sync":false}',))
            # #116：发送方的首次使用引导（回答过的问题、勾选项）也属于个人数据。
            c.execute("INSERT OR REPLACE INTO prefs VALUES ('onboarding',?)",
                      ('{"status":"skipped","step":"home","ticked":["undo"],"seen":["names"],"hidden":false}',))
            c.execute("INSERT INTO sync_runs(status) VALUES ('done')")
            # 没有索引的历史表在压缩整理后必须保留稀疏的物理 rowid。
            c.execute('CREATE TABLE archive_sparse(value TEXT)')
            c.execute("INSERT INTO archive_sparse(rowid,value) VALUES (37,'synthetic human record')")
            c.commit()
            c.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        self.assertIn(markers[3].encode(), self.lib.db.read_bytes(), 'fixture must contain deleted secret bytes')
        before = snapshot(self.lib.db)
        source_before = snapshot(self.lib.source)
        # snapshot() 是逐表的摘要：在普通副本上构建期望状态，rowid 不动。
        expected_db = self.root / 'expected-sanitized.sqlite'
        with closing(sqlite3.connect(self.lib.db)) as c, closing(sqlite3.connect(expected_db)) as e:
            c.backup(e)
            e.execute('DELETE FROM name_settings')
            e.execute("DELETE FROM prefs WHERE key IN ('names_network','onboarding')")
            e.commit()
        expected = snapshot(expected_db)
        self.assertNotEqual(expected['prefs'], before['prefs'], 'fixture must grant names_network')
        legacy = library.export_library(self.lib, self.root / 'unsafe-legacy.zip')
        legacy_bytes = legacy.read_bytes()
        self.begin()
        exported = self.done()
        unpacked = self.root / 'sanitized'
        with zipfile.ZipFile(exported) as z:
            self.assertEqual(set(z.namelist()), set(library.EXPORTED) &
                             {p.relative_to(self.lib.path).as_posix() for p in self.lib.path.rglob('*') if p.is_file()})
            z.extractall(unpacked)
            for name in z.namelist():
                data = z.read(name)
                for marker in markers:
                    self.assertNotIn(marker.encode(), data, name)
                if name.endswith('.sqlite'):
                    # snapshot 遍历每张表/视图、每个带类型的单元格、rowid 和 DDL。
                    state = snapshot(unpacked / name)
                    for marker in markers:
                        self.assertNotIn(marker, repr(state), name)
        self.assertEqual(snapshot(unpacked / 'library.sqlite'), expected)
        self.assertEqual(snapshot(unpacked / 'source.sqlite'), source_before)
        self.assertEqual(snapshot(self.lib.db), before)
        self.assertEqual(snapshot(self.lib.source), source_before)
        for archive in (exported, legacy):
            response = self.archive_import({'path': str(archive)})
            self.assertEqual(response['status'], 200, response)
            path = Path(json.loads(response['body'])['path'])
            self.assertEqual(snapshot(path / 'library.sqlite'), expected)
            self.assertEqual(snapshot(path / 'source.sqlite'), source_before)
            for file in path.rglob('*'):
                if file.is_file():
                    for marker in markers:
                        self.assertNotIn(marker.encode(), file.read_bytes(), file.name)
            response = self.request('/api/libraries/open', {'path': str(path)})
            self.assertEqual(response['status'], 200, response)
            self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
            names = json.loads(self.request('/api/names')['body'])
            self.assertFalse(names['model_api']['configured'])
            self.assertEqual(names['network'], {'authorized': False, 'this_run': False, 'granted_at': None})
            guide = json.loads(self.request('/api/onboarding')['body'])['guide']
            # 没有发送方的记录时，完成过同步的图库算作 off：没有引导，也没有清单。
            self.assertEqual((guide['status'], guide['seen'], guide['ticked']), ('off', [], []))
        self.assertEqual(legacy.read_bytes(), legacy_bytes)
        self.assertEqual(snapshot(self.lib.db), before)
        self.assertFalse(self.sentinel.exists())

    def test_roundtrip_wal_archive_contents_database_projection_and_same_id(self):
        self.lib.config['sources'] = [library.folder_source(self.root / 'Pixiv')]
        self.lib.save()
        for rel, data in (('features/full-features.npy', b'synthetic-features\0\xff'),
                          ('features/full-manifest.csv', b'id,synthetic\n'), ('album-char-map.json', b'{"synthetic":1}')):
            (self.lib.path / rel).write_bytes(data)
        (self.lib.thumbnails / 'excluded').write_bytes(b'excluded')
        (self.lib.backups / 'excluded').write_bytes(b'excluded')
        connections = []
        for db in (self.lib.source, self.lib.db):
            c = sqlite3.connect(db); connections.append(c); self.addCleanup(c.close)
            c.execute('PRAGMA journal_mode=WAL'); c.execute('PRAGMA wal_autocheckpoint=0')
            c.execute('CREATE TABLE archive_probe(v BLOB, n INTEGER, r REAL, t TEXT)')
            c.execute("INSERT INTO archive_probe(rowid,v,n,r,t) VALUES (37,?,7,1.5,'合成 WAL')", (b'\0\xff',)); c.commit()
            self.assertGreater(Path(str(db) + '-wal').stat().st_size, 0)
        legacy = library.export_library(self.lib, self.root / 'legacy.zip')
        initial = self.begin()
        expected = {'status':'running', 'path':str(Path(self.env['HOME']) / 'Downloads' / f'{self.lib.name}-{time.strftime("%Y%m%d")}.pilib.zip')}
        self.assertEqual(initial, expected)
        native = self.done()
        self.assertEqual(self.state()['bytes'], native.stat().st_size)
        left, right = self.root / 'legacy-unpacked', self.root / 'native-unpacked'
        with zipfile.ZipFile(legacy) as a, zipfile.ZipFile(native) as b:
            self.assertEqual(set(a.namelist()), set(b.namelist()))
            self.assertEqual(set(a.namelist()), set(library.EXPORTED))
            a.extractall(left); b.extractall(right)
        for rel in library.EXPORTED:
            if rel.endswith('.sqlite'):
                self.assertEqual(snapshot(left / rel), snapshot(right / rel), rel)
                self.assertEqual(snapshot(self.lib.path / rel), snapshot(right / rel), rel)
            else:
                self.assertEqual((left / rel).read_bytes(), (right / rel).read_bytes(), rel)
        expected_path = library.unique_path(library.HOME, self.lib.name)
        expected_bytes = json.dumps({'path':str(expected_path), 'name':self.lib.name}, ensure_ascii=False).encode()
        response = self.archive_import({'path':str(native)})
        self.assertEqual(response['status'], 200, response)
        self.assertEqual(response['body'], expected_bytes)
        imported = library.open_library(expected_path)
        self.assertEqual(imported.config['id'], self.lib.config['id'])
        self.assertEqual(imported.config['imported_from'], native.name)
        for rel in ('source.sqlite', 'library.sqlite'):
            self.assertEqual(snapshot(self.lib.path / rel), snapshot(imported.path / rel))
        old_token = self.token
        response = self.request('/api/libraries/open', {'path':str(imported.path)})
        self.assertEqual(response['body'], expected_bytes)
        self.assertEqual(self.state()['status'], 'idle')
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertNotEqual(self.token, old_token)
        self.assertEqual(self.request('/api/libraries/export', {}, token=old_token)['status'], 403)
        self.begin(); second = self.done()
        self.assertEqual(json.loads(self.request('/api/libraries')['body'])['current'], str(imported.path))
        self.assertTrue(second.exists())
        self.assertFalse(self.sentinel.exists())

    def test_sanitization_failure_does_not_publish_export_or_import(self):
        self.begin()
        dest = self.done()
        published = dest.read_bytes()
        with closing(sqlite3.connect(self.lib.db)) as c:
            c.execute("INSERT INTO name_settings VALUES ('model_key','archive-synthetic-failure-secret')")
            c.execute("CREATE TRIGGER archive_fail BEFORE DELETE ON name_settings BEGIN SELECT RAISE(ABORT,'synthetic failure'); END")
            c.commit()
        before = snapshot(self.lib.db)
        legacy = library.export_library(self.lib, self.root / 'failure-legacy.zip')
        self.begin()
        wait_until(lambda: self.state()['status'] == 'error')
        self.assertEqual(dest.read_bytes(), published)
        self.assertEqual(list(dest.parent.glob('.illustash-export-*')), [])
        imports = set(library.HOME.glob('*')) if library.HOME.exists() else set()
        response = self.archive_import({'path': str(legacy)})
        self.assertNotEqual(response['status'], 200, response)
        self.assertEqual(set(library.HOME.glob('*')) if library.HOME.exists() else set(), imports)
        self.assertEqual(snapshot(self.lib.db), before)
        with closing(sqlite3.connect(self.lib.db)) as c:
            c.execute('DROP TRIGGER archive_fail')
            c.commit()
        self.begin()
        self.done()
        with zipfile.ZipFile(dest) as z:
            self.assertNotIn(b'archive-synthetic-failure-secret', z.read('library.sqlite'))

    def test_preset_success_bytes_database_and_forget_preserves_package(self):
        source = library.folder_source(self.root / 'folder')
        self.lib.config['sources'] = [source]; self.lib.save()
        with closing(sqlite3.connect(self.lib.source)) as c:
            library.ensure_source_schema(c)
            c.execute("UPDATE assets SET source_id='s1',preset='other'"); c.commit()
        expected_config = self.root / 'expected.pilib'; self.clone_closed_library(expected_config)
        expected = library.open_library(expected_config)
        result = library.set_preset(expected, 's1', 'pixiv')
        response = self.request('/api/libraries/preset', {'source':'s1', 'preset':'pixiv'})
        self.assertEqual(response['status'], 200, response)
        self.assertEqual(response['body'], json.dumps(result, ensure_ascii=False).encode())
        self.assertEqual(self.lib.path.joinpath('library.json').read_bytes(), expected.path.joinpath('library.json').read_bytes())
        self.assertEqual(snapshot(self.lib.source), snapshot(expected.source))
        self.assertEqual(json.loads(self.request('/api/libraries')['body'])['info']['sources'][0], result)
        for body in ({'source':'absent','preset':'x'}, {'source':'s1','preset':'bad'}, {'source':None, 'preset':[]}):
            self.assertEqual(self.request('/api/libraries/preset', body)['status'], self.reference_status('/api/libraries/preset', body))
        other = self.package('other'); library.remember(other, front=False)
        before = snapshot(other.db)
        response = self.pair('/api/libraries/forget', {'path':str(other.path)})
        self.assertEqual(response['body'], b'{"ok": true}')
        self.assertEqual(snapshot(other.db), before)
        self.assertEqual(self.request('/api/libraries/forget', {'path':str(self.lib.path)})['status'], 400)
        self.assertTrue(other.path.is_dir())

    def test_invalid_archives_status_paths_crc_and_cleanup(self):
        good = library.export_library(self.lib, self.root / 'good.zip')
        with zipfile.ZipFile(good) as z:
            files = {name:z.read(name) for name in z.namelist()}
        variants = [b'not zip', good.read_bytes()[:100]]
        # Windows 上 zipfile 写入时会把反斜杠换成 `/`，构造不出反斜杠条目；该情形由 app_archives 的单元测试覆盖。
        names = ('../escape', '/absolute', 'features/../library.json', 'features\\full-features.npy', 'unexpected', 'features/')
        for name in names:
            if os.name == 'nt' and '\\' in name:
                variants.append(None)   # 录制于 POSIX；这里跳过（见上文）
                continue
            out = self.root / f'bad-{len(variants)}.zip'
            with zipfile.ZipFile(out, 'w') as z:
                for n,b in files.items(): z.writestr(n,b)
                z.writestr(name,b'bad')
            variants.append(out.read_bytes())
        for config in ('{', '[]', '{"format":2,"name":"future"}', '{"format":"1","name":"invalid"}', '{"format":1.0,"name":"float"}'):
            out = self.root / f'bad-{len(variants)}.zip'
            with zipfile.ZipFile(out, 'w') as z:
                for n,b in files.items(): z.writestr(n,config if n == 'library.json' else b)
            variants.append(out.read_bytes())
        out = self.root / 'bad-crc.zip'
        with zipfile.ZipFile(out,'w',compression=zipfile.ZIP_STORED) as z:
            for n,b in files.items(): z.writestr(n,b)
            z.writestr('features/full-features.npy', b'synthetic-crc-needle')
        data = bytearray(out.read_bytes()); data[data.index(b'synthetic-crc-needle')] ^= 1; variants.append(data)
        for i,data in enumerate(variants):
            if data is None:
                self.rec.skip('response')
                continue
            archive = self.root / f'case-{i}.zip'; archive.write_bytes(data)
            before = set(library.HOME.glob('*')) if library.HOME.exists() else set()
            with self.subTest(i=i):
                self.assertEqual(self.archive_import({'path':str(archive)})['status'], self.reference_status('/api/libraries/import', {'path':str(archive)}))
                self.assertEqual(set(library.HOME.glob('*')) if library.HOME.exists() else set(), before)
        self.assertFalse((self.root / 'escape').exists())
        self.assertFalse(self.sentinel.exists())

    def test_import_refuses_a_package_that_cannot_be_opened(self):
        # #42：归档的结构合法，但里面的包打不开。
        good = library.export_library(self.lib, self.root / 'good.zip')
        with zipfile.ZipFile(good) as z:
            files = {name: z.read(name) for name in z.namelist()}
        config = json.loads(files['library.json'])
        cases = {
            'damaged-source': {**files, 'source.sqlite': b'not a database ' * 512},
            'config-without-id': {**files, 'library.json': json.dumps({k: v for k, v in config.items() if k != 'id'}).encode()},
            'config-without-name': {**files, 'library.json': json.dumps({k: v for k, v in config.items() if k != 'name'}).encode()},
        }
        registry = self.support / 'libraries.json'
        listing = lambda: set(library.HOME.glob('*')) if library.HOME.exists() else set()
        for case, content in cases.items():
            with self.subTest(case):
                archive = self.root / f'{case}.zip'
                with zipfile.ZipFile(archive, 'w') as z:
                    for name, data in content.items():
                        z.writestr(name, data)
                before, recent = listing(), registry.read_bytes() if registry.exists() else None
                response = self.archive_import({'path': str(archive)})
                self.assertEqual(response['status'], 400, response)
                self.assertIn('导入的图库无法打开', json.loads(response['body'])['error'])
                self.assertEqual(listing(), before)
                self.assertEqual(registry.read_bytes() if registry.exists() else None, recent)
        imported = self.archive_import({'path': str(good)})
        self.assertEqual(imported['status'], 200, imported)
        self.assertTrue(Path(json.loads(imported['body'])['path']).is_dir())
        self.assertFalse(self.sentinel.exists())

    def test_import_limits_refuse_before_anything_is_created(self):
        # #26：配置有大小上限，声明的大小必须放得进磁盘（条目限于导出的那些）。
        good = library.export_library(self.lib, self.root / 'good.zip')
        with zipfile.ZipFile(good) as z:
            files = {name: z.read(name) for name in z.namelist()}
        registry = self.support / 'libraries.json'
        listing = lambda: set(library.HOME.glob('*')) if library.HOME.exists() else set()

        def archive(case, content=files, declare=None, compression=zipfile.ZIP_STORED):
            out = self.root / f'{case}.zip'
            with zipfile.ZipFile(out, 'w', compression=compression) as z:
                for name, data in content:
                    z.writestr(name, data)
                for info in z.filelist:  # 中央目录在关闭时写入，带上这些大小
                    if declare and info.filename == declare[0]:
                        info.file_size = declare[1]
            return out

        oversized = json.dumps({**json.loads(files['library.json']), 'padding': ' ' * (1 << 20)}).encode()
        npy = 'features/full-features.npy'
        cases = {
            'config-over-limit': (archive('config-over-limit', [(n, oversized if n == 'library.json' else b) for n, b in files.items()]), 400, '图库配置过大'),
            'beyond-free-space': (archive('beyond-free-space', [*files.items(), (npy, b'small')], declare=(npy, 1 << 50)), 400, '磁盘空间不足'),
            'more-than-declared': (archive('more-than-declared', [*files.items(), (npy, b'x' * 4096)], declare=(npy, 16)), None, None),
        }
        for case, (path, status, message) in cases.items():
            with self.subTest(case):
                before, recent = listing(), registry.read_bytes() if registry.exists() else None
                response = self.archive_import({'path': str(path)})
                if status:
                    self.assertEqual(response['status'], status, response)
                    self.assertIn(message, json.loads(response['body'])['error'])
                else:
                    self.assertGreaterEqual(response['status'], 400, response)
                self.assertEqual(listing(), before)
                self.assertEqual(registry.read_bytes() if registry.exists() else None, recent)
        self.assertEqual(self.archive_import({'path': str(good)})['status'], 200)

    LARGE = 256 << 20

    def large_archive(self):
        """一个合法的导出，其中特征文件是 LARGE 个零字节、经 LZMA 压缩：解包要花几秒。"""
        if shutil.disk_usage(self.root).free < 2 * self.LARGE + (512 << 20):
            self.skipTest('not enough free space for the synthetic archive')
        good = library.export_library(self.lib, self.root / 'good.zip')
        archive = self.root / 'large.zip'
        with zipfile.ZipFile(good) as source, zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_LZMA) as z:
            for name in source.namelist():
                if name != 'features/full-features.npy':
                    z.writestr(name, source.read(name))
            with z.open('features/full-features.npy', 'w', force_zip64=True) as out:
                for _ in range(self.LARGE >> 20):
                    out.write(bytes(1 << 20))
        with zipfile.ZipFile(archive) as z:
            declared = sum(info.file_size for info in z.infolist())
        return archive, declared

    def imports(self):
        return json.loads(self.request('/api/imports')['body'])

    def test_archive_import_polls_its_real_stages_and_unpacked_bytes(self):
        # #88：请求立即应答；轮询按顺序跟随各阶段，带上真实的已解包字节数。
        archive, declared = self.large_archive()
        registry = self.support / 'libraries.json'
        seen = []
        response = self.archive_import({'path': str(archive)}, seen=seen, timeout=WAIT * 10)
        self.assertEqual(response['status'], 200, response)
        order = ['reading', 'unpacking', 'cleaning', 'checking', 'publishing']
        stages = [s['stage'] for s in seen if s['status'] == 'running' and 'stage' in s]
        self.assertEqual(stages, sorted(stages, key=order.index), 'stages only move forward')
        unpacked = [(s['done'], s['total']) for s in seen if s.get('stage') == 'unpacking' and 'done' in s]
        self.assertGreaterEqual(len({done for done, _ in unpacked}), 2, 'the unpacked bytes are seen growing')
        self.assertEqual({total for _, total in unpacked}, {declared})
        self.assertEqual([done for done, _ in unpacked], sorted(done for done, _ in unpacked))
        self.assertTrue(all(done <= declared for done, _ in unpacked))
        end = seen[-1]
        self.assertEqual((end['status'], end['kind'], end['file']), ('done', 'archive', archive.name))
        path = Path(end['result']['path'])
        self.assertEqual((path / 'features' / 'full-features.npy').stat().st_size, self.LARGE)
        self.assertIn(str(path), [r['path'] for r in json.loads(registry.read_text())['recent']])

    def test_archive_import_cancel_switch_and_quit_remove_what_it_unpacked(self):
        # #88（#26）：取消按钮、切换图库和退出都会停止解包、删除新目录，不动登记表。停没停在解包阶段由
        # 终态的 stage 和 done < total 判定，退出则由图库目录没有多出新包判定；不另设耗时上界（#508）。
        archive, _ = self.large_archive()
        other = self.root / 'other.pilib'
        self.clone_closed_library(other)
        registry = self.support / 'libraries.json'
        listing = lambda: set(library.HOME.glob('*')) if library.HOME.exists() else set()
        for how in ('cancel', 'switch', 'quit'):
            with self.subTest(how):
                before, recent = listing(), registry.read_bytes() if registry.exists() else None
                r = self.request('/api/libraries/import', {'path': str(archive)})
                self.assertEqual(r['status'], 200, r)
                task = json.loads(r['body'])['task']
                wait_until(lambda: self.imports().get('done', 0) > 0)
                # 解包期间读取请求照常应答（要是等解包完，下面的终态就不会停在解包阶段）。
                self.assertEqual(self.request('/api/bootstrap')['status'], 200)
                if how == 'quit':
                    self.stop(self.process)
                    self.process, self.log = self.start(self.lib.path)
                    self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
                else:
                    if how == 'cancel':
                        self.assertEqual(self.request('/api/imports/cancel', {})['status'], 200)
                        reason = '导入已取消'
                    else:
                        self.assertEqual(self.request('/api/libraries/open', {'path': str(other)})['status'], 200)
                        reason = '图库已切换，导入已取消'
                    wait_until(lambda: self.imports()['status'] in lifecycle.IMPORT_ENDED)
                    end = self.imports()
                    self.assertEqual((end['id'], end['status'], end['reason'], end['stage']),
                                     (task['id'], 'cancelled', reason, 'unpacking'), end)
                    self.assertLess(end['done'], end['total'])
                    if how == 'switch':
                        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
                        self.assertEqual(self.request('/api/libraries/open', {'path': str(self.lib.path)})['status'], 200)
                        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
                self.assertEqual(listing(), before)
                if how != 'cancel':  # 打开图库（切换、重启）会更新登记表；导入不添加任何条目
                    self.assertFalse([r for r in json.loads(registry.read_text())['recent'] if Path(r['path']).resolve().parent == library.HOME.resolve()])
                else:
                    self.assertEqual(registry.read_bytes() if registry.exists() else None, recent)

    def test_damaged_archive_reports_why_and_publishes_nothing(self):
        good = library.export_library(self.lib, self.root / 'good.zip')
        with zipfile.ZipFile(good) as z:
            files = {name: z.read(name) for name in z.namelist()}
        damaged = self.root / 'damaged.zip'
        with zipfile.ZipFile(damaged, 'w') as z:
            for name, data in files.items():
                z.writestr(name, b'not a database ' * 512 if name == 'source.sqlite' else data)
        registry = self.support / 'libraries.json'
        listing = lambda: set(library.HOME.glob('*')) if library.HOME.exists() else set()
        before, recent = listing(), registry.read_bytes() if registry.exists() else None
        seen = []
        response = self.archive_import({'path': str(damaged)}, seen=seen)
        self.assertEqual(response['status'], 400, response)
        end = seen[-1]
        self.assertEqual((end['status'], end['code'], end['stage']), ('error', 400, 'checking'), end)
        self.assertIn('导入的图库无法打开', end['error'])
        self.assertEqual(listing(), before)
        self.assertEqual(registry.read_bytes() if registry.exists() else None, recent)

    def test_repeated_export_busy_switch_cancel_same_id_and_exit_cleanup(self):
        # 持有 rollback journal 的写锁，不用大量数据就能确定地让备份一直处于进行中。
        other = self.package('other')
        other.config['id'] = self.lib.config['id']; other.save()
        lock = sqlite3.connect(self.lib.source); self.addCleanup(lock.close)
        lock.execute('BEGIN EXCLUSIVE')
        initial = self.begin()
        self.assertEqual(self.request('/api/libraries/export', {})['status'], 400)
        self.assertEqual(self.state(), initial)
        # 写锁一直不放：切换要是等备份，就会一直卡到等待上限而失败。
        opened = self.request('/api/libraries/open', {'path':str(other.path)})
        self.assertEqual(opened['status'], 200, opened)
        self.assertEqual(self.state()['status'], 'idle')
        self.assertFalse(Path(initial['path']).exists())
        self.assertEqual(list(Path(initial['path']).parent.glob('.illustash-export-*')), [])
        lock.rollback()
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.begin(); self.done()
        lock2 = sqlite3.connect(other.source); self.addCleanup(lock2.close); lock2.execute('BEGIN EXCLUSIVE')
        self.begin(); self.stop(self.process)
        self.assertEqual(list(Path(self.env['HOME']).joinpath('Downloads').glob('.illustash-export-*')), [])
        self.assertFalse(self.sentinel.exists())

    def test_export_error_atomic_existing_destination_and_retry(self):
        initial = self.begin(); dest = self.done(); before = dest.read_bytes()
        feature = self.lib.features / 'full-features.npy'; feature.mkdir()
        self.begin(); wait_until(lambda:self.state()['status']=='error')
        self.assertEqual(dest.read_bytes(), before)
        self.assertEqual(list(dest.parent.glob('.illustash-export-*')), [])
        feature.rmdir(); feature.write_bytes(b'retry')
        self.begin(); self.done()
        with zipfile.ZipFile(dest) as z: self.assertEqual(z.read('features/full-features.npy'), b'retry')

    def test_choose_invalid_kind_status_and_security(self):
        for body in ({}, {'kind':'bad'}, {'kind':[]}, {'kind':None}):
            self.assertEqual(self.request('/api/libraries/choose', body)['status'], self.reference_status('/api/libraries/choose', body))
        self.assertEqual(self.request('/api/libraries/choose', {'kind':'bad'}, token='bad')['status'],403)

    def test_parallel_export_one_start_and_failed_switch_keeps_job(self):
        lock = sqlite3.connect(self.lib.source); self.addCleanup(lock.close); lock.execute('BEGIN EXCLUSIVE')
        responses = []
        barrier = threading.Barrier(8)
        def start():
            barrier.wait(); responses.append(self.request('/api/libraries/export', {})['status'])
        threads = [threading.Thread(target=start) for _ in range(8)]
        for t in threads: t.start()
        for t in threads: t.join(WAIT)
        self.assertEqual(sorted(responses), [200] + [400]*7)
        initial = self.state()
        self.assertEqual(self.request('/api/libraries/open', {'path':str(self.root / 'missing.pilib')})['status'], 400)
        self.assertEqual(self.state(), initial)
        lock.rollback(); self.done()

    def test_import_unique_names_config_bytes_registry_failure_and_existing_paths(self):
        archive = library.export_library(self.lib, self.root / 'synthetic.zip')
        legacy = library.import_library(archive, self.root / 'legacy-imports')
        library.HOME.mkdir(parents=True, exist_ok=True)
        taken = library.HOME / (self.lib.name + '.pilib'); taken.write_bytes(b'keep-existing')
        response = self.archive_import({'path':str(archive)})
        self.assertEqual(response['status'], 200, response)
        path = Path(json.loads(response['body'])['path'])
        self.assertEqual(path.name, self.lib.name + ' 2.pilib')
        self.assertEqual((path / 'library.json').read_bytes(), (legacy.path / 'library.json').read_bytes())
        self.assertEqual(taken.read_bytes(), b'keep-existing')
        tmp = self.support / 'libraries.tmp'; tmp.mkdir()
        registry = self.support / 'libraries.json'
        recent = registry.read_bytes() if registry.exists() else None
        response = self.archive_import({'path':str(archive)})
        # #111（与新建相同，#97）：报告已发布的包并说明它为什么没有登记，而不是录制里的 400；
        # 再导入一次只会生成第二个带编号的包。
        self.assertEqual(response['status'], 200, response)
        published = library.HOME / (self.lib.name + ' 3.pilib')
        made = json.loads(response['body'])
        self.assertEqual((Path(made['path']), made['name'], made['listed']), (published, self.lib.name, False))
        self.assertTrue(made['error'])
        self.assertEqual(registry.read_bytes() if registry.exists() else None, recent)
        self.assertEqual((published / 'library.json').read_bytes(), (legacy.path / 'library.json').read_bytes())
        self.assertEqual(snapshot(published / 'library.sqlite'), snapshot(self.lib.db))
        self.assertEqual(json.loads(self.request('/api/libraries')['body'])['current'], str(self.lib.path))

    def test_management_filesystem_failures_status_and_source_database_boundary(self):
        other = self.package('other'); library.remember(other, front=False)
        tmp = self.support / 'libraries.tmp'; tmp.mkdir()
        body = {'path':str(other.path)}
        self.assertEqual(self.request('/api/libraries/forget',body)['status'], self.reference_status('/api/libraries/forget',body))
        source = library.folder_source(self.root / 'folder')
        self.lib.config['sources']=[source];self.lib.save()
        config_tmp = self.lib.path / 'library.json.tmp';config_tmp.mkdir()
        before = snapshot(self.lib.source)
        body={'source':'s1','preset':'x'}
        self.assertEqual(self.request('/api/libraries/preset',body)['status'], self.reference_status('/api/libraries/preset',body))
        self.assertEqual(snapshot(self.lib.source),before)

    def test_import_legacy_zip_compressions(self):
        exported = library.export_library(self.lib, self.root / 'exported.zip')
        with zipfile.ZipFile(exported) as z: files = {name:z.read(name) for name in z.namelist()}
        for compression in (zipfile.ZIP_STORED,zipfile.ZIP_DEFLATED,zipfile.ZIP_BZIP2,zipfile.ZIP_LZMA):
            with self.subTest(compression=compression):
                archive=self.root / f'compression-{compression}.zip'
                with zipfile.ZipFile(archive,'w',compression=compression) as z:
                    for name,data in files.items(): z.writestr(name,data)
                expected=library.import_library(archive,self.root / 'legacy-compression')
                path=library.unique_path(library.HOME,self.lib.name)
                result=self.archive_import({'path':str(archive)})
                self.assertEqual(result['status'],200,result)
                self.assertEqual(result['body'],json.dumps({'path':str(path),'name':self.lib.name},ensure_ascii=False).encode())
                self.assertEqual((path / 'library.json').read_bytes(),(expected.path / 'library.json').read_bytes())
                for name in ('source.sqlite','library.sqlite'): self.assertEqual(snapshot(path / name),snapshot(expected.path / name))

                if compression == zipfile.ZIP_LZMA:
                    corrupted = bytearray(archive.read_bytes())
                    central = corrupted.rfind(b'PK\x01\x02')
                    corrupted[central+16] ^= 1
                    bad = self.root / 'lzma-crc.zip'; bad.write_bytes(corrupted)
                    before = set(library.HOME.iterdir())
                    body = {'path':str(bad)}
                    self.assertEqual(self.archive_import(body)['status'],self.reference_status('/api/libraries/import',body))
                    self.assertEqual(set(library.HOME.iterdir()),before)
