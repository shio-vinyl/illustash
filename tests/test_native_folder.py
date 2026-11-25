"""M3 文件夹扫描：只用测试自己持有的临时合成文件夹和包。

原 Python 扫描器和导入器在同样输入上的结果经 tests/fixed_expectations.py 录制（#9）。
生成的文件用由内容推出的修改时间，原生的键可以复现，所以录制的状态依然成立。
"""
from tests.native_worker_fixture import command as worker_command, source as worker_source, exists as worker_exists

from contextlib import closing
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from PIL import Image, ImageDraw
from PIL.PngImagePlugin import PngInfo
from tests.folder_fixture import FolderSource, image, settle
from tests.fixed_expectations import Expectations, unclocked
from tests.test_native_human_labels import snapshot
from tests import test_native_lifecycle as lifecycle
from tests.test_native_lifecycle import NativeLifecycleTest
from tests.test_read_http import wait_open_tasks, wait_until
from tools.wait_limit import WAIT


def assert_folder_source(test, source, before_scan, recorded):
    """逐表比较历史录制，变更过的文件链接按扫描账目独立核验。"""
    actual = snapshot(source)
    test.assertEqual({k: v for k, v in actual.items() if k not in ('schema', 'asset_links')},
                     {k: v for k, v in recorded.items() if k not in ('schema', 'asset_links')})
    with closing(sqlite3.connect(source)) as c, closing(sqlite3.connect(before_scan)) as previous:
        ddl = """SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE tbl_name<>'asset_links'
                 AND name<>'files_asset' ORDER BY type,name"""
        test.assertEqual(c.execute(ddl).fetchall(), previous.execute(ddl).fetchall())
        keys = {r[1]: r[5] for r in c.execute('PRAGMA table_info(asset_links)')}
        test.assertEqual(keys, {'asset': 4, 'system': 1, 'key': 2, 'source_id': 3, 'current': 0})
        rows = c.execute('SELECT asset,system,key,source_id,current FROM asset_links ORDER BY 1,2,3,4').fetchall()
        expected = c.execute("""SELECT asset,'file',path,source_id,1 FROM files UNION ALL
                                SELECT asset,'md5',md5,source_id,1 FROM files ORDER BY 1,2,3,4""").fetchall()
        test.assertEqual(rows, expected)


class NativeFolderScanTest(unittest.TestCase):
    tearDown = FolderSource.tearDown
    rows = FolderSource.rows

    def setUp(self):
        FolderSource.setUp(self)
        self.rec = Expectations(self)
        self.addCleanup(self.rec.finish, self)
        self.rec.alias(self.root, '@ROOT@')
        # 最初构建出的包（它的资源键是随机的）：已录制，重放时还原。
        self.rec.package('package', self.lib.path)
        self.env = {**os.environ, 'ILLUSTASH_TEST_KEY_COUNTER': str(self.root / 'key-counter')}

    @classmethod
    def setUpClass(cls):
        path = os.environ.get('ILLUSTASH_TERMS_BIN')
        if not path:
            raise unittest.SkipTest('explicit native binary required')
        cls.binary = Path(path).resolve()

    def scan_pair(self):
        settle(self.pics)
        old = self.root / 'old.sqlite'
        with sqlite3.connect(self.lib.source) as src, sqlite3.connect(old) as dest:
            src.backup(dest)
        before = set(u for u, in self.rows('SELECT uuid FROM assets'))
        result = subprocess.run([str(self.binary), 'folder-scan', '--db', str(self.lib.source)],
                                input=json.dumps(self.source).encode(), capture_output=True, timeout=WAIT, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        new = json.loads(result.stdout)
        ids = [u for u, in self.rows('SELECT uuid FROM assets ORDER BY rowid') if u not in before]
        expected = self.rec.value('old scan')
        self.assertEqual(new, expected['result'])
        assert_folder_source(self, self.lib.source, old, expected['source'])
        return new

    @unittest.skipIf(os.name == 'nt', '固定期望录自 macOS：资产键序与文件元数据随平台不同')
    def test_complete_source_typed_rowid_ddl_comparison(self):
        shutil.copyfile(self.pics / 'a.png', self.pics / 'copy.png')
        (self.pics / 'bad.gif').write_bytes(b'synthetic corrupt image')
        image(self.pics / '.hide' / 'invisible.png', 'red')
        image(self.pics / 'skip.pilib' / 'invisible.png', 'red')
        r = self.scan_pair()
        self.assertEqual(r['new'], 4)
        self.assertEqual(self.rows("SELECT width,height FROM assets WHERE filename='bad.gif'"), [(0, 0)])
        self.assertEqual(self.scan_pair()['changed'], 0)
        with sqlite3.connect(self.lib.source) as c:
            c.execute("INSERT INTO labels(uuid,dim,value,conf) SELECT uuid,'rating','general',1 FROM assets")
            c.execute("INSERT INTO embeddings(uuid,model,vec) SELECT uuid,'synthetic',x'0102' FROM assets")
        (self.pics / 'a.png').rename(self.pics / 'sub' / 'moved.png')
        image(self.pics / 'copy.png', 'yellow', (50, 20))
        image(self.pics / 'z-new.webp', 'blue')
        r = self.scan_pair()
        self.assertEqual((r['new'], r['changed'], r['moved']), (1, 1, 1))
        (self.pics / 'sub' / 'b.jpg').rename(self.root / 'saved.jpg')
        self.assertEqual(self.scan_pair()['gone'], 1)
        self.assertEqual(self.scan_pair()['gone'], 1)  # 现有的 Python 统计会把消失的路径再算一次
        (self.root / 'saved.jpg').rename(self.pics / 'returned.jpg')
        r = self.scan_pair()
        self.assertEqual((r['returned'], r['moved']), (1, 1))
        p = self.pics / 'returned.jpg'
        os.utime(p, ns=(-1234567890, -1234567890))
        # 消费但不改写旧录制；它把 mtime-only 误报为内容变化，#364 明确修正这一处。
        self.assertEqual(self.rec.value('old scan')['result']['changed'], 1)
        # #364：mtime-only 不再使有效标签/向量失效；这是修复后的契约，不沿用旧录制中的错误行为。
        before = {table: snapshot(self.lib.source)[table] for table in ('labels', 'embeddings')}
        result = subprocess.run([str(self.binary), 'folder-scan', '--db', str(self.lib.source)],
                                input=json.dumps(self.source).encode(), capture_output=True, timeout=WAIT, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['changed'], 0)
        self.assertEqual({table: snapshot(self.lib.source)[table] for table in before}, before)

    def test_source_commit_failure_rolls_back_and_retry(self):
        before = snapshot(self.lib.source)
        with sqlite3.connect(self.lib.source) as c:
            c.execute("CREATE TRIGGER fail_file BEFORE INSERT ON files BEGIN SELECT RAISE(ABORT,'synthetic source failure'); END")
        failed = snapshot(self.lib.source)
        result = subprocess.run([str(self.binary), 'folder-scan', '--db', str(self.lib.source)],
                                input=json.dumps(self.source).encode(), capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(snapshot(self.lib.source), failed)
        with sqlite3.connect(self.lib.source) as c:
            c.execute('DROP TRIGGER fail_file')
        self.assertEqual(snapshot(self.lib.source), before)
        self.assertEqual(self.scan_pair()['new'], 2)


class NativeFolderHTTPTest(unittest.TestCase):
    setUpClass = classmethod(NativeLifecycleTest.setUpClass.__func__)
    _fixture_setup = NativeLifecycleTest.setUp

    def setUp(self):
        self._fixture_setup()
        if self._testMethodName.startswith('test_native_library_new'):
            return  # 工厂现在会在发布之前校验完整的合成参考。
        with sqlite3.connect(self.lib.source) as c:
            for column in ('hidden INTEGER', 'stable_hash TEXT', 'orig_size INTEGER'):
                c.execute('ALTER TABLE assets ADD COLUMN ' + column)
            c.execute('CREATE TABLE embeddings(uuid TEXT NOT NULL,model TEXT NOT NULL,vec BLOB NOT NULL,PRIMARY KEY(uuid,model))')

    package = NativeLifecycleTest.package
    stop = NativeLifecycleTest.stop
    start = NativeLifecycleTest.start
    request = NativeLifecycleTest.request

    def finish(self):
        for _ in range(150):
            response = self.request('/api/sync')
            self.assertEqual(response['status'], 200)
            state = json.loads(response['body'])
            if state['state']['status'] not in ('scanning', 'importing'):
                return state
            time.sleep(.02)
        self.fail('sync did not finish')

    def sync(self):
        settle(*[f for f in (getattr(self, 'new_folder', None), self.root / 'second-source') if f])
        response = self.request('/api/sync', {})
        self.assertEqual(response['status'], 200, response['body'])
        self.assertEqual(response['body'], b'{"started": true}')
        return self.finish()

    def test_native_library_new_folder_inventory_paths_verdict_undo(self):
        settings = json.loads(self.request('/api/settings')['body'])
        cleared = json.loads(self.request('/api/cache', {'action': 'clear'})['body'])
        self.assertEqual(cleared.pop('removed'), 0)
        self.assertEqual(cleared, settings)
        self.assertNotIn('sync', settings['forced_off'])
        folder_path = self.new_folder
        image(folder_path / 'a.png', 'red')
        shutil.copyfile(folder_path / 'a.png', folder_path / 'copy.png')
        state = self.sync()
        self.assertEqual(state['state']['status'], 'needs_recognition', state)
        self.assertEqual(state['state']['total'], 3)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM assets').fetchone()[0], 3)
            self.assertEqual(c.execute('SELECT count(*) FROM duplicates').fetchone()[0], 2)
            self.assertEqual(c.execute("SELECT kind,status FROM backend_effects").fetchall(), [('imported', 'pending')])
            asset = c.execute("SELECT uuid FROM assets WHERE filename='a.png'").fetchone()[0]
        tagged = self.request('/api/tag', {'ids': [asset], 'tag': '合成标签'})
        self.assertEqual(tagged['status'], 200, tagged['body'])
        undo = json.loads(tagged['body'])['undo']
        before = snapshot(self.lib.db)['actions']
        (folder_path / 'a.png').rename(folder_path / 'moved.png')
        self.sync()
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT src_path FROM assets WHERE uuid=?', (asset,)).fetchone()[0], str(folder_path / 'moved.png'))
            self.assertEqual(c.execute('SELECT tag FROM asset_tags WHERE uuid=?', (asset,)).fetchall(), [('合成标签',)])
        self.assertEqual(snapshot(self.lib.db)['actions'], before)
        detail = self.request('/api/detail?id=' + asset)
        self.assertEqual(detail['status'], 200)
        # 资源的添加时间来自文件的创建时间，只有 macOS 允许 settle() 固定它。
        self.assertEqual(unclocked(detail['body']), unclocked(lifecycle.NativeLifecycleTest.reference_response(self, '/api/detail?id=' + asset)['body']))
        self.assertEqual(self.request('/api/undo', {'id': undo['id']})['status'], 200)
        (folder_path / 'moved.png').rename(self.root / 'saved.png')
        state = self.sync()
        self.assertEqual(state['state']['removed'], 1)
        (self.root / 'saved.png').rename(folder_path / 'back.png')
        self.sync()
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute("SELECT uuid FROM assets WHERE filename='back.png'").fetchone()[0], asset)
        self.assertFalse(self.sentinel.exists())

    def source_action(self, action, **args):
        response = self.request('/api/libraries/sources/' + action, args)
        self.assertEqual(response['status'], 200, response['body'])
        return json.loads(response['body'])

    def file_ids(self):
        with closing(sqlite3.connect(self.lib.source)) as c:
            return {(s, p): u for s, p, u in c.execute('SELECT source_id,path,asset FROM files')}

    def test_native_library_new_move_from_old_file_records_and_undo_offline(self):
        folder = self.new_folder
        image(folder / 'first.png', 'red')
        shutil.copyfile(folder / 'first.png', folder / 'copy.png')
        image(folder / 'sub' / 'unique.png', 'green')
        self.sync()
        original = self.file_ids()
        tagged = original['s1', 'first.png']
        self.assertEqual(self.request('/api/tag', {'ids': [tagged], 'tag': '整理过'})['status'], 200)
        # 模拟在已有数据的旧文件表上首次使用新版，旧磁盘下线前没有机会重新扫描。
        with closing(sqlite3.connect(self.lib.source)) as c, c:
            c.executescript("""ALTER TABLE asset_links RENAME TO saved_links;
                CREATE TABLE asset_links(asset TEXT NOT NULL,system TEXT NOT NULL,key TEXT NOT NULL,
                    source_id TEXT NOT NULL,current INTEGER NOT NULL DEFAULT 1,PRIMARY KEY(system,key,source_id));
                INSERT INTO asset_links SELECT * FROM saved_links WHERE system<>'md5';
                DROP TABLE saved_links; CREATE INDEX asset_links_asset ON asset_links(asset);""")
        target = self.root / 'relocated'
        folder.rename(target)
        (target / 'sub' / 'unique.png').rename(target / 'renamed.png')
        self.assertTrue(self.source_action('move', source='s1', path=str(target))['sync'])
        self.assertNotEqual(self.finish()['state']['status'], 'error')
        moved = self.file_ids()
        expected = dict(original)
        expected['s1', 'renamed.png'] = expected.pop(('s1', 'sub/unique.png'))
        self.assertEqual(moved, expected)
        with closing(sqlite3.connect(self.lib.db)) as c:
            self.assertEqual(c.execute('SELECT src_path FROM assets WHERE uuid=?', (tagged,)).fetchone(), (str(target / 'first.png'),))
            self.assertEqual(c.execute('SELECT tag FROM asset_tags WHERE uuid=?', (tagged,)).fetchone(), ('整理过',))
        self.stop(self.process)
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        change, = json.loads(self.request('/api/libraries')['body'])['folder_changes']
        self.assertEqual((change['kept'], change['new']), (len(original), 0))
        self.source_action('undo', source='s1', token=change['token'])
        self.assertNotEqual(self.finish()['state']['status'], 'error')
        self.assertEqual(self.file_ids(), original)
        with closing(sqlite3.connect(self.lib.db)) as c:
            self.assertEqual(c.execute('SELECT src_path FROM assets WHERE uuid=?', (tagged,)).fetchone(), (str(folder / 'first.png'),))
            self.assertEqual(c.execute('SELECT tag FROM asset_tags WHERE uuid=?', (tagged,)).fetchone(), ('整理过',))
        self.assertFalse(folder.exists())
        self.assertTrue(target.is_dir())

    def test_native_library_new_move_rejects_ambiguous_or_replaced_content(self):
        folder = self.new_folder
        for name in ('one.png', 'two.png'):
            image(folder / name, 'red')
        same = folder / 'same.gif'
        same.write_bytes(b'GIF89a-first')
        self.sync()
        original = self.file_ids()
        target = self.root / 'elsewhere'
        folder.rename(target)
        for old, new in (('one.png', 'three.png'), ('two.png', 'four.png')):
            (target / old).rename(target / new)
        replacement = target / 'same.gif'
        stamp = replacement.stat()
        replacement.write_bytes(b'GIF89a-other')
        os.utime(replacement, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
        self.source_action('move', source='s1', path=str(target))
        self.assertNotEqual(self.finish()['state']['status'], 'error')
        current = self.file_ids()
        self.assertTrue({current['s1', n] for n in ('three.png', 'four.png', 'same.gif')}.isdisjoint(original.values()))
        change, = json.loads(self.request('/api/libraries')['body'])['folder_changes']
        self.assertEqual((change['kept'], change['new']), (1, 3))
        (target / 'same.gif').rename(target / 'edited.gif')
        self.sync()
        self.assertNotEqual(self.request('/api/libraries/sources/undo', {'source': 's1', 'token': change['token']})['status'], 200)
        self.assertEqual(json.loads((self.lib.path / 'library.json').read_text())['sources'][0]['path'], str(target))

    def test_native_library_new_confirm_cross_source_and_undo(self):
        old = self.new_folder / 'one.png'
        image(old, 'red')
        self.sync()
        old_id = self.file_ids()['s1', 'one.png']
        self.assertEqual(self.request('/api/tag', {'ids': [old_id], 'tag': '原位置'})['status'], 200)
        dest = self.root / 'copy'
        dest.mkdir()
        shutil.copy2(old, dest / 'copy.png')
        sid = self.source_action('add', kind='folder', path=str(dest), preset='x')['id']
        self.finish()
        original = self.file_ids()
        self.assertNotEqual(original[sid, 'copy.png'], old_id)
        self.assertEqual(self.source_action('recognize', source=sid)['files'], [])
        self.new_folder.rename(self.root / 'offline')
        ask = self.source_action('recognize', source=sid)
        self.assertEqual(ask['files'], [{'old': str(old), 'new': str(dest / 'copy.png')}])
        self.assertEqual(self.file_ids(), original)
        # 用户确认时，原件又上线：旧确认已失效，两份 ID 都保留。
        (self.root / 'offline').rename(self.new_folder)
        stale = self.request('/api/libraries/sources/recognize', {'source': sid, 'digest': ask['digest']})
        self.assertNotEqual(stale['status'], 200)
        self.assertEqual(self.file_ids(), original)
        self.new_folder.rename(self.root / 'offline')
        answer = self.source_action('recognize', source=sid, digest=ask['digest'])
        self.assertEqual(answer['pending'], 1)
        self.finish()
        self.assertEqual(self.file_ids()[sid, 'copy.png'], old_id)
        with closing(sqlite3.connect(self.lib.db)) as c:
            self.assertEqual(c.execute('SELECT tag FROM asset_tags WHERE uuid=?', (old_id,)).fetchone(), ('原位置',))
        self.source_action('undo', source=sid, token=answer['token'])
        self.finish()
        self.assertEqual(self.file_ids(), original)

    def test_native_library_new_folder_written_back_file_returns_in_one_sync(self):
        # #44：写回原先被移走路径的文件，第一次同步后就再次显示。
        path = self.new_folder / 'a.png'
        image(path, 'red')
        self.sync()
        with sqlite3.connect(self.lib.db) as c:
            asset = c.execute("SELECT uuid FROM assets WHERE filename='a.png'").fetchone()[0]
        self.assertEqual(self.request('/api/tag', {'ids': [asset], 'tag': '人工标签'})['status'], 200)

        def shown():
            with sqlite3.connect(self.lib.db) as c:
                return (c.execute('SELECT uuid,width FROM assets WHERE uuid=?', (asset,)).fetchone(),
                        c.execute('SELECT tag FROM asset_tags WHERE uuid=?', (asset,)).fetchall())

        for case, write in (('same bytes, new time', lambda saved: path.write_bytes(saved)),
                            ('other bytes', lambda saved: image(path, 'yellow', (51, 31)))):
            with self.subTest(case):
                saved = path.read_bytes()
                path.unlink()
                self.sync()
                self.assertIsNone(shown()[0])
                write(saved)
                settle(self.new_folder)
                stat = path.stat()
                os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns + 7_000_000_000))
                self.assertEqual(self.request('/api/sync', {})['status'], 200)
                state = self.finish()['state']
                self.assertNotEqual(state['status'], 'error', state)
                row, tags = shown()
                self.assertEqual(row, (asset, 51 if case == 'other bytes' else 40))
                self.assertEqual(tags, [('人工标签',)])
                with sqlite3.connect(self.lib.source) as c:
                    self.assertEqual(c.execute('SELECT trashed FROM assets WHERE uuid=?', (asset,)).fetchone(), (0,))
        self.assertFalse(self.sentinel.exists())

    @unittest.skipIf(os.name == 'nt', 'POSIX 权限位制造读取失败；Windows ACL 另行验证')
    def test_native_library_new_folder_unreadable_paths_keep_their_pictures(self):
        # #43：扫描读不到的文件夹或文件不算消失；真的删除了才算。
        sub = self.new_folder / 'sub'; sub.mkdir()
        image(sub / 'a.png', 'red')
        image(self.new_folder / 'b.png', 'blue')
        image(self.new_folder / 'c.png', 'green')
        self.sync()
        with sqlite3.connect(self.lib.db) as c:
            ids = dict(c.execute('SELECT filename,uuid FROM assets').fetchall())
        tagged = self.request('/api/tag', {'ids': [ids['a.png']], 'tag': '人工标签'})
        self.assertEqual(tagged['status'], 200, tagged)

        def visible():
            with sqlite3.connect(self.lib.db) as c:
                return {f for f, in c.execute('SELECT filename FROM assets')} & {'a.png', 'b.png', 'c.png'}

        (self.new_folder / 'b.png').unlink()
        sub.chmod(0)
        self.addCleanup(sub.chmod, 0o755)
        state = self.sync()['state']
        sub.chmod(0o755)
        self.assertEqual(visible(), {'a.png', 'c.png'})
        self.assertEqual((state['removed'], state.get('unreadable')), (1, 1), state)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT tag FROM asset_tags WHERE uuid=?', (ids['a.png'],)).fetchall(), [('人工标签',)])
        # 已知文件改了内容但算不了哈希时，图片也保留。
        image(self.new_folder / 'c.png', 'yellow', (51, 31))
        settle(self.new_folder)
        (self.new_folder / 'c.png').chmod(0)
        self.addCleanup((self.new_folder / 'c.png').chmod, 0o644)
        self.assertEqual(self.request('/api/sync', {})['status'], 200)
        state = self.finish()['state']
        (self.new_folder / 'c.png').chmod(0o644)
        self.assertEqual(visible(), {'a.png', 'c.png'})
        self.assertEqual((state['removed'], state.get('unreadable')), (0, 1), state)
        # 又能读了：一次普通的同步，没有保留未处理的项，图片和它的标签都不变。
        state = self.sync()['state']
        self.assertNotIn('unreadable', state)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute("SELECT uuid FROM assets WHERE filename='a.png'").fetchone()[0], ids['a.png'])
            self.assertEqual(c.execute("SELECT width FROM assets WHERE filename='c.png'").fetchone()[0], 51)
            self.assertEqual(c.execute('SELECT tag FROM asset_tags WHERE uuid=?', (ids['a.png'],)).fetchall(), [('人工标签',)])
        self.assertFalse(self.sentinel.exists())

    def test_native_library_new_folder_changed_content_is_compared_again(self):
        # #27：同一个 UUID 下图片的字节一旦变了，由旧字节得出的相似结果就去掉。
        worker = self.root / 'research-worker'
        worker.write_text('#!' + sys.executable + '\nimport os,subprocess,sys\n'
                          "subprocess.run([os.environ['ILLUSTASH_RESEARCH_COMPUTE_BIN'],sys.argv[1],sys.argv[2]],check=True)\n")
        worker.chmod(0o755)
        self.env.update(ILLUSTASH_RESEARCH_WORKER=worker_command(worker), ILLUSTASH_RESEARCH_TIMEOUT_MS=str(int(WAIT * 1000)),
                        ILLUSTASH_RESEARCH_RETRY_MS='300')
        self.stop(self.process)
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        # 像素相同但字节不同的会比较；字节完全相同的副本归为一组并跳过。
        first = Image.linear_gradient('L').resize((64, 48)).convert('RGB')
        first.save(self.new_folder / 'a.png')
        note = PngInfo(); note.add_text('synthetic', 'second copy')
        first.save(self.new_folder / 'b.png', pnginfo=note)

        def compared():
            self.assertNotEqual(self.sync()['state']['status'], 'error')
            wait_until(lambda: json.loads(self.request('/api/similar')['body'])['state']['status'] == 'idle'
                       and not self.pending())
            with sqlite3.connect(self.lib.db) as c:
                a, b = (c.execute('SELECT uuid FROM assets WHERE filename=?', (n,)).fetchone()[0] for n in ('a.png', 'b.png'))
                pair = tuple(sorted((a, b)))
                kinds = c.execute('SELECT kind FROM relations WHERE a=? AND b=?', pair).fetchall()
                grid = c.execute('SELECT grid FROM image_sigs WHERE uuid=?', (b,)).fetchone()
                tags = c.execute('SELECT tag FROM asset_tags WHERE uuid=?', (b,)).fetchall()
            return b, kinds, grid, tags

        b, kinds, grid, _ = compared()
        self.assertEqual(kinds, [('identical',)])
        tag = self.request('/api/tag', {'ids': [b], 'tag': '人工标签'})
        self.assertEqual(tag['status'], 200, tag)
        edited = first.copy(); ImageDraw.Draw(edited).rectangle((28, 20, 35, 25), fill='black')
        edited.save(self.new_folder / 'b.png')
        again, kinds, changed_grid, tags = compared()
        self.assertEqual(again, b)
        self.assertEqual(len(kinds), 1)
        self.assertNotEqual(kinds, [('identical',)])
        self.assertNotEqual(changed_grid, grid)
        groups = json.loads(self.request('/api/similar')['body'])['groups']
        self.assertEqual([list(g['kinds']) for g in groups], [[kinds[0][0]]])  # 分组跟随新的结果
        self.assertEqual(tags, [('人工标签',)])
        self.assertFalse(self.sentinel.exists())

    def pending(self):
        with sqlite3.connect(self.lib.db) as c:
            return c.execute("SELECT count(*) FROM backend_effects WHERE status='pending' AND kind IN ('verdict','imported','index_changed')").fetchone()[0]

    def test_native_library_new_folder_application_failure_retry_and_changed_evidence(self):
        image(self.new_folder / 'a.png', 'red')
        state=self.sync()
        self.assertNotEqual(state['state']['status'],'error',state)
        with sqlite3.connect(self.lib.db) as c:
            asset = c.execute("SELECT uuid FROM assets WHERE filename='a.png'").fetchone()[0]
        with sqlite3.connect(self.lib.source) as c:
            c.execute("INSERT INTO labels(uuid,dim,value,conf) VALUES (?,'rating','general',1)", (asset,))
        # 把现有的合成识别结果经同一条原生证据路径交付。
        result = subprocess.run([str(self.binary), 'library-scan', '--library', str(self.lib.path)], env=self.env, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        with sqlite3.connect(self.lib.db) as c:
            c.execute("CREATE TRIGGER fail_import BEFORE INSERT ON assets BEGIN SELECT RAISE(ABORT,'synthetic app failure'); END")
        image(self.new_folder / 'a.png', 'yellow', (51, 31))
        state = self.sync()
        self.assertEqual(state['state']['status'], 'error')
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT width FROM assets WHERE uuid=?', (asset,)).fetchone()[0], 51)
            self.assertEqual(c.execute('SELECT count(*) FROM labels WHERE uuid=?', (asset,)).fetchone()[0], 0)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT width FROM assets WHERE uuid=?', (asset,)).fetchone()[0], 40)
            c.execute('DROP TRIGGER fail_import')
        state = self.sync()
        self.assertEqual(state['state']['status'], 'needs_recognition', state)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT width FROM assets WHERE uuid=?', (asset,)).fetchone()[0], 51)
            self.assertEqual(c.execute('SELECT count(*) FROM asset_rating WHERE uuid=?', (asset,)).fetchone()[0], 0)
            self.assertGreater(c.execute("SELECT count(*) FROM term_evidence e JOIN terms t ON t.id=e.term_id WHERE t.kind='rating'").fetchone()[0], 0)
            self.assertGreater(c.execute("SELECT count(*) FROM meta WHERE key LIKE 'folder_stale_wd:%'").fetchone()[0], 0)
        # 输出相同的识别也算一次新的成功交付；不可变的旧证据可以复用。
        with sqlite3.connect(self.lib.source) as c:
            c.execute("INSERT INTO labels(uuid,dim,value,conf) VALUES (?,'rating','general',1)", (asset,))
        self.sync()
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT rating FROM asset_rating WHERE uuid=?', (asset,)).fetchall(), [('general',)])
        self.assertFalse(self.lib.source.with_suffix('.scan-pending.json').exists())
        self.assertFalse(self.sentinel.exists())

    def test_native_library_new_folder_retries_content_change_with_journal_but_no_app_hash(self):
        image(self.new_folder / 'a.png', 'red')
        self.assertEqual(self.sync()['state']['status'], 'needs_recognition')
        with sqlite3.connect(self.lib.db) as c:
            asset = c.execute("SELECT uuid FROM assets WHERE filename='a.png'").fetchone()[0]
            key = f'folder_hash:{asset}'
            c.execute('DELETE FROM meta WHERE key=?', (key,))
            c.execute("""CREATE TRIGGER fail_import BEFORE INSERT ON assets WHEN NEW.filename='a.png'
                         BEGIN SELECT RAISE(ABORT,'synthetic content projection failure'); END""")
        image(self.new_folder / 'a.png', 'yellow', (51, 31))
        self.assertEqual(self.sync()['state']['status'], 'error')
        with sqlite3.connect(self.lib.source) as c:
            source_hash = c.execute('SELECT stable_hash FROM assets WHERE uuid=?', (asset,)).fetchone()[0]
        journal = self.lib.source.with_suffix('.scan-pending.json')
        self.assertEqual(json.loads(journal.read_text())[asset], source_hash)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT width FROM assets WHERE uuid=?', (asset,)).fetchone()[0], 40)
            self.assertIsNone(c.execute('SELECT value FROM meta WHERE key=?', (key,)).fetchone())
            c.execute('DROP TRIGGER fail_import')
        retried = self.sync()['state']
        self.assertEqual((retried['status'], retried['found'], retried['imported']), ('needs_recognition', 0, 1))
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT width FROM assets WHERE uuid=?', (asset,)).fetchone()[0], 51)
            self.assertEqual(c.execute('SELECT value FROM meta WHERE key=?', (key,)).fetchone()[0], source_hash)
        self.assertFalse(journal.exists())

    @unittest.skipIf(os.name == 'nt', '固定期望录自 macOS：资产键序与文件元数据随平台不同')
    def test_native_library_new_folder_full_application_typed_rowid_projection_comparison(self):
        clone = self.root / 'reference-clone.pilib'
        clone.mkdir()
        with closing(sqlite3.connect(self.lib.source)) as src, closing(sqlite3.connect(clone / 'source.sqlite')) as dest:
            src.backup(dest)
        image(self.new_folder / 'a.png', 'red')
        image(self.new_folder / 'b.webp', 'blue')
        state=self.sync()
        self.assertEqual(state['state']['status'],'needs_recognition',state)
        expected = self.rec.value('old import')
        assert_folder_source(self, self.lib.source, clone / 'source.sqlite', expected['source'])
        # #201 的完整逻辑行对照仍包含旧文件链接，只从测试副本里移去上面已核验的新增内容链接。
        for name in ('source.sqlite', 'library.sqlite', 'provenance.sqlite'):
            if (self.lib.path / name).exists():
                with closing(sqlite3.connect(self.lib.path / name)) as src, closing(sqlite3.connect(clone / name)) as dest:
                    src.backup(dest)
        with closing(sqlite3.connect(clone / 'source.sqlite')) as c, c:
            c.execute("DELETE FROM asset_links WHERE system='md5'")
        self.rec.expect_empty_character_catalog()
        self.assertTrue(self.rec.baseline('old import', clone))
        for url in ('/api/assets','/api/ids','/api/home','/api/assets/count'):
            self.assertEqual(self.request(url)['body'],lifecycle.NativeLifecycleTest.reference_response(self, url)['body'])
        self.assertFalse(self.sentinel.exists())


    @unittest.skipIf(os.name == 'nt', '固定期望录自 macOS：资产键序与文件元数据随平台不同')
    def test_native_library_new_folder_multiple_sources_keep_configuration_order(self):
        self.stop(self.process)
        self.lib.config['sources'][0]['id']='z'
        second=self.root / 'second-source'
        image(second / 'copy.png','red')
        self.lib.config['sources'].append({'id':'a','kind':'folder','path':str(second),'preset':'other'})
        (self.lib.path / 'library.json').write_text(json.dumps(self.lib.config,ensure_ascii=False))
        self.process,self.log=self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.test_native_library_new_folder_full_application_typed_rowid_projection_comparison()
        state=json.loads(self.request('/api/sync')['body'])
        self.assertEqual(list(state['history'][0]['sources']),['z','a'])
        self.assertEqual((state['state']['scanned'],state['state']['scan_total']),(4,4))

    def test_native_library_new_folder_shutdown_completed_and_restart_retries(self):
        # 先提交再关闭是显式的。提交前的取消/回滚和 join 由同一个应用服务上的
        # app_sync::tests::scan_boundary_fixture 把关。
        image(self.new_folder / 'a.png', 'red')
        self.assertEqual(self.sync()['state']['status'], 'needs_recognition')
        source, app = snapshot(self.lib.source), snapshot(self.lib.db)
        self.stop(self.process)
        self.assertEqual(snapshot(self.lib.source), source)
        self.assertEqual(snapshot(self.lib.db), app)
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(self.sync()['state']['status'], 'needs_recognition')
        self.assertFalse(self.sentinel.exists())

    def test_native_library_new_folder_security_concurrent_switch_and_shutdown(self):
        # 文件大小无法决定扫描完成与包的新建/切换孰先孰后。
        # 提交前的并发取消和重复启动，由 Application 持有的真实 run_with 任务
        # 把关（app_sync::tests::scan_boundary_fixture）。
        image(self.new_folder / 'a.png', 'red')
        self.assertEqual(self.request('/api/sync', {}, token='wrong')['status'], 403)
        self.assertEqual(self.request('/api/sync', {})['status'], 200)
        self.assertEqual(self.finish()['state']['status'], 'needs_recognition')
        source, app = snapshot(self.lib.source), snapshot(self.lib.db)
        old = self.lib
        other = self.package('replacement')
        response = self.request('/api/libraries/open', {'path': str(other.path)})
        self.assertEqual(response['status'], 200, response['body'])
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        state = json.loads(self.request('/api/sync')['body'])
        self.assertEqual(state['state']['status'], 'off')
        self.assertEqual(self.request('/api/sync', {})['status'], 400)
        wait_open_tasks(self, self.origin, other.path)
        before=snapshot(other.db)
        refused=subprocess.run([str(self.binary),'library-scan','--library',str(other.path)],env=self.env,capture_output=True)
        self.assertNotEqual(refused.returncode,0)
        self.assertEqual(snapshot(other.db),before)
        # 已完成的扫描原样保留它提交的数据和最终历史记录。
        self.assertEqual(snapshot(old.db), app)
        self.assertEqual(snapshot(old.source), source)
        self.assertFalse(self.sentinel.exists())

# 复用夹具，但不再发现它们那些无关的测试。
del NativeLifecycleTest
