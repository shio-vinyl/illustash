"""数据包的签名目录（#602）走原生 HTTP：跟着检查更新读目录、下载新版本、换包、换回、撤回，离线包导入。

GitHub 的版本列表和 Release 下载都由本机替身服务答复（ILLUSTASH_UPDATE_BASE），目录用测试密钥签名
（ILLUSTASH_TEST_PACKS_PUBKEY，只在指向本机测试服务时才认）。包、图片和模型身份都是合成的，夹具见
tests/fixtures/pack_catalog/。
"""
from contextlib import closing, contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hashlib
import json
import shutil
import socket
from pathlib import Path
import sqlite3
import struct
import subprocess
import tempfile
from threading import Event, Thread
import time
import unittest
from urllib.parse import urlsplit
import zipfile

from tests.folder_fixture import image
from tests.fixed_expectations import Library
from tests.pack_shards_fixture import invalid_assembly, shard_fixture
from tests import test_native_lifecycle as lifecycle
from tests.test_read_http import free_port, wait_until
from tools import pack_catalog
from tools.dev_preview import isolated_env
from tools.wait_limit import WAIT

FIXTURES = Path(__file__).resolve().parent / 'fixtures' / 'pack_catalog'
DOWNLOAD = '/shio-vinyl/illustash/releases/download/'
WD_MODEL = 'ae25f5f12b2d5891728fb7768fa63d65bf13a83d6d4823e3a1bf7b42875c74ff'
WD_TAGS = '298633d94d0031d2081c0893f29c82eab7f0df00b08483ba8f29d1e979441217'
RELEASES = '/repositories/1407079068/releases?per_page=30'
CCIP_MODEL = '4ea118d16496274f4f6e08d3afc768cc592389e8f7f32f8732ce2215c228ac5f'


def sha(data):
    return hashlib.sha256(data).hexdigest()


@contextmanager
def github(files, activity=None):
    """`files` 是「路径 → 字节」，随时可以改；`seen` 记下每个请求的路径。

    它也能当 `HTTPS_PROXY` 用：经代理发来的本机请求（请求行是完整地址）照常答复，去别处的 HTTPS（`CONNECT`）一律拒绝，
    测试不会连到外网。"""
    seen = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            path = self.path
            if path.startswith('http://'):
                parts = urlsplit(path)
                path = parts.path + (f'?{parts.query}' if parts.query else '')
            seen.append(path)
            body = b'[]' if path == RELEASES else files.get(path)
            if callable(body):
                body = body()
            if body is False:
                # 模拟服务器在给出回应前中断连接。
                self.connection.shutdown(socket.SHUT_RDWR)
                return
            self.send_response(200 if body is not None else 404)
            body = body if body is not None else b'{"message": "Not Found"}'
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            if activity is None or self.path != '/platform/activity':
                self.send_error(404)
                return
            activity.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
            self.send_response(200)
            self.send_header('Content-Length', '2')
            self.end_headers()
            self.wfile.write(b'{}')

        def do_CONNECT(self):
            self.send_response(403)
            self.send_header('Content-Length', '0')
            self.end_headers()

        def log_message(self, *_):
            pass

    with ThreadingHTTPServer(('127.0.0.1', 0), Handler) as server:
        thread = Thread(target=server.serve_forever)
        thread.start()
        try:
            yield f'http://127.0.0.1:{server.server_port}', seen
        finally:
            server.shutdown()
            thread.join()


def builtin_v1(directory, origin):
    """内置包的走查替身：WD 部分一个头看第 0 维（ILLUSTASH_TEST_WD_EXTENSION），CCIP 部分是夹具里的
    ccip-longtail-v1.zip（ILLUSTASH_TEST_CCIP_LONGTAIL）。目录里的 v2 原样带着这个 CCIP 文件。"""
    header = {'format': 'illustash-wd-extension/1', 'wd_model_sha256': WD_MODEL, 'wd_tags_sha256': WD_TAGS,
              'threshold': 0.8008725047111511, 'labels': ['packone_(synthetic)']}
    weights = struct.pack('<1024f', 2., *([0.] * 1023))
    contents = json.dumps(header).encode() + b'\n' + weights + struct.pack('<f', 0.)
    spec = {'revision': 1, 'format': 'illustash-wd-extension/1', 'heads': 1, 'license': 'Apache-2.0',
            'base_sha256': WD_MODEL,
            'file': {'path': 'wd-extension-v1.bin', 'size': len(contents), 'sha256': sha(contents),
                     'url': f'{origin}/builtin/wd-extension-v1.bin'}}
    ccip = (FIXTURES / 'ccip-longtail-v1.zip').read_bytes()
    longtail = {'sha256': sha(ccip), 'size': len(ccip), 'ccip_sha256': CCIP_MODEL, 'roles': 2,
                'url': f'{origin}/builtin/A-longtail-p75.zip'}
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'wd-extension.json').write_text(json.dumps(spec), encoding='utf-8')
    (directory / 'ccip-longtail.json').write_text(json.dumps(longtail), encoding='utf-8')
    return directory / 'wd-extension.json', directory / 'ccip-longtail.json', contents, ccip


class NativePackCatalogTest(unittest.TestCase):
    setUpClass = classmethod(lifecycle.NativeLifecycleTest.setUpClass.__func__)
    start = lifecycle.NativeLifecycleTest.start
    stop = lifecycle.NativeLifecycleTest.stop
    request = lifecycle.NativeLifecycleTest.request

    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='illustash-pack-catalog-')
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        checkout = self.resources
        self.resources = self.root / 'resources'
        (self.resources / 'tags').mkdir(parents=True)
        (self.resources / 'tags' / 'tags_v3.csv').write_bytes((checkout / 'tags' / 'tags_v3.csv').read_bytes())
        self.env = {**isolated_env(self.root), 'USERPROFILE': str(self.root / 'home'),
                    'ILLUSTASH_TEST_PACKS_PUBKEY': (FIXTURES / 'key-a.pub').read_text().strip()}
        self.env.pop('ILLUSTASH_RECOGNITION_WORKER', None)
        folder = self.root / 'images'
        folder.mkdir()
        for name, color in (('a.png', 'blue'), ('b.png', 'orange'), ('c.png', 'green')):
            image(folder / name, color)
        path = self.root / 'first.pilib'
        self.cli('library-create', '--dest', str(path), '--folder', str(folder))
        self.lib = Library(path)
        self.cli('library-scan', '--library', str(self.lib.path))
        self.seed_wd_vectors(3)
        self.port = free_port()
        self.origin = f'http://127.0.0.1:{self.port}'

    def seed_wd_vectors(self, count):
        """在没有宿主连接图库时写入合成识别结果。"""
        # 第 i 张图（按 uuid 排）的向量只有第 i 维是 1：合成包的第 i 个头只认它。
        with closing(sqlite3.connect(self.lib.source)) as source, source:
            source.execute('CREATE TABLE IF NOT EXISTS embeddings(uuid TEXT,model TEXT,vec BLOB,PRIMARY KEY(uuid,model))')
            self.ids = [u for u, in source.execute('SELECT uuid FROM assets WHERE in_scope=1 ORDER BY uuid')]
            self.assertEqual(len(self.ids), count)
            for i, uuid in enumerate(self.ids):
                vector = [0.] * 1024
                vector[i] = 1.
                source.execute("INSERT INTO embeddings VALUES (?,'wd',?)", (uuid, struct.pack('<1024f', *vector)))

    def cli(self, *arguments):
        result = subprocess.run([str(self.binary), *arguments], env=self.env, capture_output=True, timeout=WAIT)
        self.assertEqual(result.returncode, 0, result.stderr.decode(errors='replace'))

    def restart(self):
        if hasattr(self, 'process'):
            self.stop(self.process)
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']

    def get(self, route):
        response = self.request(route)
        self.assertEqual(response['status'], 200, response['body'])
        return json.loads(response['body'])

    def post(self, route, body, status=200):
        response = self.request(route, body)
        self.assertEqual(response['status'], status, response['body'])
        return json.loads(response['body'])

    def finish(self, route, kind, key='task'):
        observed = None

        def ended():
            nonlocal observed
            observed = self.get(route)
            job = observed[key]
            return job and job['kind'] == kind and job['status'] != 'running'

        wait_until(ended)
        return observed

    def members(self):
        with sqlite3.connect(self.lib.db) as app:
            assigned = [u for u, in app.execute("SELECT uuid FROM memberships WHERE sources='WD_EXT' ORDER BY uuid")]
            candidates = [u for u, in app.execute("SELECT DISTINCT uuid FROM candidates WHERE source='WD_EXT' ORDER BY uuid")]
        return assigned, candidates

    def serve(self, origin):
        files = {DOWNLOAD + 'packs/packs.json': (FIXTURES / 'packs.json').read_bytes(),
                 DOWNLOAD + 'packs/packs.json.sig': (FIXTURES / 'packs.json.sig').read_bytes()}
        for path, name in (('character-pack-1.0/wd-extension-v3.bin', 'wd-extension-v3.bin'),
                           ('character-pack-1.0/ccip-longtail-v2.zip', 'ccip-longtail-v2.zip'),
                           ('character-pack-0.2/wd-extension-v2.bin', 'wd-extension-v2.bin')):
            files[DOWNLOAD + path] = (FIXTURES / name).read_bytes()
        wd, ccip, wd_bytes, ccip_bytes = builtin_v1(self.root / 'builtin', origin)
        files['/builtin/wd-extension-v1.bin'] = wd_bytes
        files['/builtin/A-longtail-p75.zip'] = ccip_bytes
        self.env['ILLUSTASH_TEST_WD_EXTENSION'] = str(wd)
        self.env['ILLUSTASH_TEST_CCIP_LONGTAIL'] = str(ccip)
        self.env['ILLUSTASH_UPDATE_BASE'] = origin + '/'
        return files

    def packs(self, body=None, status=200):
        response = self.request('/api/packs', body)
        self.assertEqual(response['status'], status, response['body'])
        return json.loads(response['body'])

    def settle(self, what):
        """等 `GET /api/packs` 里的下载（`download`）或任务（`task`）结束。"""
        observed = None

        def ended():
            nonlocal observed
            observed = self.packs()
            job = observed[what]
            return job and job['status'] != 'running'

        wait_until(ended)
        return observed

    def sync_to_pending_recognition(self, count):
        self.post('/api/sync', {})

        def finished():
            state = self.get('/api/sync')['state']
            return state if state.get('finished_at') else None

        state = wait_until(finished)
        self.assertEqual((state['status'], state['total']), ('needs_recognition', count), state)

    def test_character_catalog_retries_and_only_activates_on_next_start(self):
        files = {}

        def applied():
            with closing(sqlite3.connect(self.lib.db)) as db:
                row = db.execute("SELECT json_extract(value,'$.revision') FROM meta WHERE key='character_catalog'").fetchone()
            return row[0] if row else None

        with github(files) as (origin, seen):
            files.update(self.serve(origin))
            for suffix in ('', '.sig'):
                files[DOWNLOAD + 'packs/packs.json' + suffix] = (FIXTURES / ('packs-roles.json' + suffix)).read_bytes()
            route = DOWNLOAD + 'character-catalog-2/roles.json'
            files[route] = b'partial'
            self.restart()
            wait_until(lambda: applied() == 1)
            self.post('/api/updates', {'action': 'check'})
            self.assertTrue(self.packs()['catalog']['error'])
            files[route] = (FIXTURES / 'roles.json').read_bytes()
            self.post('/api/updates', {'action': 'check'})
            self.assertIsNone(self.packs()['catalog']['error'])
            self.assertEqual(seen.count(route), 2)
            self.assertEqual(applied(), 1)
            self.restart()
            wait_until(lambda: applied() == 2)

    def test_offline_character_catalog_verifies_each_member_and_activates_after_restart(self):
        files = {}
        roles = (FIXTURES / 'roles.json').read_bytes()
        descriptor = FIXTURES / 'packs-roles.json'

        def applied():
            with closing(sqlite3.connect(self.lib.db)) as db:
                row = db.execute("SELECT json_extract(value,'$.revision') FROM meta WHERE key='character_catalog'").fetchone()
            return row[0] if row else None

        with github(files) as (origin, seen):
            files.update(self.serve(origin))
            self.restart()
            wait_until(lambda: applied() == 1)
            installed = self.root / 'shared/extensions' / f'character-catalog-{sha(roles)}.json'
            bad = self.root / 'untrusted.zip'
            with zipfile.ZipFile(bad, 'w') as archive:
                archive.writestr('packs.json', descriptor.read_bytes())
                archive.writestr('packs.json.sig', (FIXTURES / 'packs.json.sig-b').read_bytes())
                archive.writestr('catalog-2/roles.json', roles)
            self.packs({'action': 'import', 'paths': [str(bad)]})
            results = self.settle('download')['download']['results']
            self.assertEqual({r['name']: r['result'] for r in results}, {
                'untrusted.zip/packs.json': 'catalog_invalid',
                'untrusted.zip/packs.json.sig': 'catalog_invalid',
                'untrusted.zip/catalog-2/roles.json': 'unknown',
            })
            self.assertFalse(installed.exists())

            partial = self.root / 'damaged.zip'
            with zipfile.ZipFile(partial, 'w') as archive:
                archive.writestr('packs.json', descriptor.read_bytes())
                archive.writestr('packs.json.sig', (FIXTURES / 'packs-roles.json.sig').read_bytes())
                # 正确签名、同样文件大小，但内容与 SHA-256 不符；兼容放在 ZIP 根上的词表文件。
                archive.writestr('roles.json', b'x' + roles[1:])
            good = self.root / 'verified.zip'
            pack_catalog.offline(descriptor, [FIXTURES / 'roles.json'], good)
            self.packs({'action': 'import', 'paths': [str(partial), str(good)]})
            download = self.settle('download')['download']
            self.assertEqual(download['status'], 'done')
            results = {r['name']: r['result'] for r in download['results']}
            self.assertEqual(results['damaged.zip/roles.json'], 'mismatch')
            self.assertEqual(results['verified.zip/catalog-2/roles.json'], 'catalog_file_installed')
            self.assertEqual(installed.read_bytes(), roles)
            self.assertEqual(applied(), 1)
            self.restart()
            wait_until(lambda: applied() == 2)
            self.assertEqual(seen, [])

    def test_shell_receives_pack_download_enable_and_withdraw(self):
        files, activity = {}, []
        release = Event()

        def reported(key, kind):
            return any(item['kind'] == 'pack' and item['variant'] == kind
                       for message in activity for item in message[key])

        with github(files, activity) as (origin, _):
            files.update(self.serve(origin))
            self.env.update(ILLUSTASH_DESKTOP_BRIDGE=origin, ILLUSTASH_DESKTOP_TOKEN='synthetic-token')
            self.restart()
            url = '/builtin/wd-extension-v1.bin'
            body = files[url]

            def held():
                release.wait(WAIT)
                return body

            files[url] = held
            try:
                self.packs({'action': 'download'})
                wait_until(lambda: reported('busy', 'download'))
            finally:
                release.set()
            self.assertEqual(self.settle('download')['current']['status'], 'ready')
            wait_until(lambda: reported('ended', 'download'))
            for action in ('enable', 'withdraw'):
                # 持有应用库写锁，让真实包任务在取锁阶段等候一次活动上报。
                with closing(sqlite3.connect(self.lib.db)) as blocker:
                    blocker.execute('BEGIN IMMEDIATE')
                    self.packs({'action': action})
                    wait_until(lambda: reported('busy', action))
                self.assertEqual(self.settle('task')['task']['status'], 'done')
                wait_until(lambda: reported('ended', action))

    def test_one_pack_follows_update_check_then_download_switch_back_and_withdraw(self):
        files = {}
        with github(files) as (origin, seen):
            files.update(self.serve(origin))
            self.restart()
            packs = self.packs()
            self.assertEqual((packs['catalog']['configured'], packs['catalog']['serial']), (True, None))
            self.assertEqual((packs['current']['version'], packs['current']['stage'], packs['current']['status'], packs['offer']),
                             ('0.1', 'preview', 'missing', None))
            self.assertEqual(packs['current']['heads'], 3)
            # 已有识别向量、还没决定过：提示一次，不自动启用。
            self.assertEqual((packs['library']['enabled'], packs['library']['prompt']), (False, 'ask'))
            # 没允许自动检查时，页面的自动检查什么都不读，也不读目录。
            self.post('/api/updates', {'action': 'check', 'auto': True})
            self.assertEqual(seen, [])

            # 一个包：两个文件一起下载，一个开关一起启用。没有 CCIP 向量的组成部分只记下启用了哪个文件。
            self.packs({'action': 'download'})
            self.assertEqual(self.settle('download')['current']['status'], 'ready')
            self.packs({'action': 'enable'})
            enabled = self.settle('task')
            self.assertEqual(enabled['task']['status'], 'done', enabled)
            self.assertLessEqual({'enabled': True, 'version': '0.1', 'wd': True, 'ccip': True,
                                  'prompt': None, 'images': 1, 'assigned': 1}.items(), enabled['library'].items())
            self.assertEqual(self.members(), (self.ids[:1], self.ids[:1]))
            before = {route: self.request(route)['body'] for route in ('/api/wd-extension', '/api/ccip-longtail')}
            del seen[:]

            # 手动检查更新时顺带读目录；不下载任何包。
            self.post('/api/updates', {'action': 'check'})
            self.assertEqual(seen, [RELEASES, DOWNLOAD + 'packs/packs.json', DOWNLOAD + 'packs/packs.json.sig'])
            packs = self.packs()
            self.assertEqual((packs['catalog']['serial'], packs['catalog']['error']), (1, None))
            self.assertEqual(packs['current']['version'], '0.1')
            offer = packs['offer']
            self.assertEqual((offer['version'], offer['stage'], offer['heads'], offer['candidate_only'], offer['status']),
                             ('1.0', 'release', 5, 1, 'missing'))
            self.assertEqual(offer['note'], {'zh-CN': '合成测试包 3', 'en': 'Synthetic test pack 3'})
            # 读到目录以后、用户点之前，两个组成部分的旧接口逐字节不变。
            for route, body in before.items():
                self.assertEqual(self.request(route)['body'], body, route)

            # 目录里没有的版本、格式不对的请求都不认。
            self.packs({'action': 'enable', 'version': '1.1'}, 409)
            self.packs({'action': 'enable', 'version': 3}, 400)
            self.packs({'action': 'withdraw', 'version': '1.0'}, 400)
            self.packs({'action': 'download', 'version': '9.9'}, 409)
            self.packs({'action': 'delete', 'version': '1.0'}, 400)
            self.packs({'action': 'nope'}, 400)
            self.packs({'action': 'enable', 'version': '1.0'}, 409)  # 还没下载
            self.assertEqual(self.members(), (self.ids[:1], self.ids[:1]))

            # 下载新版本：两个文件都下，卡片上仍是已启用的 v1。
            self.packs({'action': 'download', 'version': '1.0'})
            done = self.settle('download')
            self.assertEqual(done['download']['status'], 'done', done)
            self.assertEqual((done['current']['version'], done['offer']['status']), ('0.1', 'ready'))

            # 换成新版本：两个组成部分都换；二级头（第三张图）只出候选，不补归属。
            self.packs({'action': 'enable', 'version': '1.0'})
            switched = self.settle('task')
            self.assertEqual(switched['task']['status'], 'done', switched)
            self.assertEqual(switched['library']['version'], '1.0')
            self.assertEqual(switched['wd']['library']['package_sha256'], sha((FIXTURES / 'wd-extension-v3.bin').read_bytes()))
            self.assertEqual(switched['ccip']['library']['package_sha256'], sha((FIXTURES / 'ccip-longtail-v2.zip').read_bytes()))
            self.assertEqual(self.members(), (self.ids[:2], self.ids))
            self.assertEqual((switched['library']['images'], switched['library']['assigned']), (3, 2))
            self.assertIsNone(switched['offer'])
            alternatives = json.loads(self.request('/api/detail?id=' + self.ids[2])['body'])['alternatives']
            self.assertIn(['WD_EXT'], [alt['evidence']['sources'] for alt in alternatives])

            # 重启以后照样记得用的是哪个版本。
            self.restart()
            self.assertEqual(self.packs()['library']['version'], '1.0')

            # 换回 v1，再撤回；撤回以后不再提示，也不自动启用。
            self.packs({'action': 'enable', 'version': '0.1'})
            back = self.settle('task')
            self.assertEqual((back['task']['status'], back['library']['version']), ('done', '0.1'), back)
            self.assertEqual(self.members(), (self.ids[:1], self.ids[:1]))
            self.assertEqual(back['offer']['status'], 'ready')
            self.packs({'action': 'withdraw'})
            withdrawn = self.settle('task')
            self.assertEqual(withdrawn['task']['status'], 'done', withdrawn)
            self.assertEqual(self.members(), ([], []))
            self.assertLessEqual({'enabled': False, 'version': None, 'wd': False, 'ccip': False, 'prompt': 'declined',
                                  'images': 0, 'assigned': 0}.items(), withdrawn['library'].items())
            # 没启用时卡片上是本机已有的最新版本。
            self.assertEqual(withdrawn['current']['version'], '1.0')

            # 目录读不到时只记一句话，已有的缓存照用；签名对不上的目录整份不用。
            files.pop(DOWNLOAD + 'packs/packs.json.sig')
            self.post('/api/updates', {'action': 'check'})
            packs = self.packs()
            self.assertEqual(packs['catalog']['serial'], 1)
            self.assertIn('HTTP 404', packs['catalog']['error'])
            files[DOWNLOAD + 'packs/packs.json'] = (FIXTURES / 'packs-2.json').read_bytes()
            files[DOWNLOAD + 'packs/packs.json.sig'] = (FIXTURES / 'packs.json.sig').read_bytes()
            self.post('/api/updates', {'action': 'check'})
            packs = self.packs()
            self.assertEqual(packs['catalog']['serial'], 1)
            self.assertIn('签名', packs['catalog']['error'])

    def test_bad_zip_reports_the_file_and_reason(self):
        files = {}
        with github(files) as (origin, _):
            files.update(self.serve(origin))
            self.restart()
            invalid = self.root / 'bad.zip'
            invalid.write_bytes(b'not an archive')
            self.packs({'action': 'import', 'paths': [str(invalid)]})
            result = self.settle('download')['download']
            self.assertEqual((result['kind'], result['status']), ('import', 'failed'))
            self.assertEqual(result['message'], 'bad.zip 不是有效的 ZIP 文件')

    def test_failed_download_and_offline_import_remove_empty_partials(self):
        files = {}
        with github(files) as (origin, _):
            files.update(self.serve(origin))
            names = ('wd-extension-v1.bin', 'A-longtail-p75.zip')
            for name in names:
                del files[f'/builtin/{name}']
            self.restart()
            extensions = self.root / 'shared' / 'extensions'
            partials = [extensions / f'{name}.partial' for name in ('wd-extension-v1', 'ccip-longtail-v1')]

            self.packs({'action': 'download'})
            failed = self.settle('download')
            self.assertEqual(failed['download']['status'], 'failed')
            self.assertEqual(failed['current']['status'], 'missing')
            self.assertTrue(all(not partial.exists() for partial in partials))

            # 第一份留的是旧版下载残留，第二份仍有可续传的字节。
            for partial, name, data in zip(partials, names, [b'', b'partial-download']):
                partial.mkdir(parents=True)
                (partial / f'{name}.part').write_bytes(data)
            archive = FIXTURES / 'character-pack-1.0-offline.zip'
            self.packs({'action': 'import', 'paths': [str(archive)]})
            imported = self.settle('download')
            self.assertEqual(imported['download']['status'], 'done')
            self.assertEqual(imported['current']['status'], 'ready')
            self.assertFalse(partials[0].exists())
            self.assertEqual((partials[1] / f'{names[1]}.part').read_bytes(), b'partial-download')

    def test_offline_bundle_carries_the_signed_catalog(self):
        files = {}
        with github(files) as (origin, seen):
            files.update(self.serve(origin))
            self.restart()
            # 签名不对的离线包：目录不采用，包文件也就不认。
            bad = self.root / 'character-pack-1.0-offline.zip'
            with zipfile.ZipFile(bad, 'w') as archive:
                archive.writestr('packs.json', (FIXTURES / 'packs.json').read_bytes())
                archive.writestr('packs.json.sig', (FIXTURES / 'packs.json.sig-b').read_bytes())
                for name in ('wd-extension-v3.bin', 'ccip-longtail-v2.zip'):
                    archive.writestr(name, (FIXTURES / name).read_bytes())
                archive.writestr('NOTICE.txt', b'synthetic notice\n')
            self.packs({'action': 'import', 'paths': [str(bad)]})
            results = self.settle('download')['download']['results']
            self.assertEqual({r['name']: r['result'] for r in results}, {
                'character-pack-1.0-offline.zip/packs.json': 'catalog_invalid',
                'character-pack-1.0-offline.zip/packs.json.sig': 'catalog_invalid',
                'character-pack-1.0-offline.zip/wd-extension-v3.bin': 'unknown',
                'character-pack-1.0-offline.zip/ccip-longtail-v2.zip': 'unknown',
                'character-pack-1.0-offline.zip/NOTICE.txt': 'unknown'})
            self.assertIsNone(self.packs()['catalog']['serial'])

            # 官方离线包附带许可和说明（#643）：验过签的目录没列出的成员是 extra，不算问题；单独选的别的文件照旧认不出。
            good = self.root / 'offline' / 'character-pack-1.0-offline.zip'
            good.parent.mkdir()
            with zipfile.ZipFile(FIXTURES / 'character-pack-1.0-offline.zip') as source, \
                    zipfile.ZipFile(good, 'w') as archive:
                for item in source.infolist():
                    archive.writestr(item, source.read(item))
                archive.writestr('NOTICE.txt', b'synthetic notice\n')
                archive.writestr('licenses/LICENSE-2.0.txt', b'synthetic license\n')
            stray = self.root / 'offline' / 'notes.txt'
            stray.write_bytes(b'not a pack\n')
            self.packs({'action': 'import', 'paths': [str(good), str(stray)]})
            packs = self.settle('download')
            self.assertEqual({r['name']: r['result'] for r in packs['download']['results']}, {
                'character-pack-1.0-offline.zip/packs.json': 'catalog',
                'character-pack-1.0-offline.zip/packs.json.sig': 'catalog',
                'character-pack-1.0-offline.zip/wd-extension-v3.bin': 'accepted',
                'character-pack-1.0-offline.zip/ccip-longtail-v2.zip': 'accepted',
                'character-pack-1.0-offline.zip/NOTICE.txt': 'extra',
                'character-pack-1.0-offline.zip/licenses/LICENSE-2.0.txt': 'extra',
                'notes.txt': 'unknown'})
            self.assertEqual(packs['catalog']['serial'], 1)
            # 没启用：卡片上是本机已有的 v3；已有识别向量，提示一次。
            self.assertEqual((packs['current']['version'], packs['current']['status'], packs['offer']),
                             ('1.0', 'ready', None))
            self.assertEqual(packs['library']['prompt'], 'ask')
            self.assertEqual(self.packs({'action': 'decline'})['library']['prompt'], 'declined')
            self.packs({'action': 'enable'})
            enabled = self.settle('task')
            self.assertEqual(enabled['task']['status'], 'done', enabled)
            self.assertEqual((enabled['library']['version'], enabled['library']['prompt']), ('1.0', None))
            self.assertEqual(self.members(), (self.ids[:2], self.ids))
            self.assertTrue((self.root / 'shared/extensions' / ('ccip-longtail-' + sha(
                (FIXTURES / 'ccip-longtail-v2.zip').read_bytes())[:12]) / 'ccip-longtail-v2.zip').is_file())
            # 整个过程不联网。
            self.assertEqual(seen, [])

            # 只缺一个组成部分时再下载（#643）：进度只算这次要下的文件，不把上次导入的总量合进来，种类是下载。
            wd = (FIXTURES / 'wd-extension-v3.bin').read_bytes()
            shutil.rmtree(self.root / 'shared/extensions' / ('wd-extension-' + sha(wd)[:12]))
            self.assertEqual(self.packs()['current']['status'], 'missing')
            self.packs({'action': 'download'})
            again = self.settle('download')
            self.assertEqual((again['download']['kind'], again['download']['status']), ('download', 'done'), again)
            self.assertEqual((again['download']['done'], again['download']['total']), (len(wd), len(wd)))
            self.assertEqual(again['download']['results'], [])
            self.assertEqual(seen, [DOWNLOAD + 'character-pack-1.0/wd-extension-v3.bin'])

    def test_new_library_is_enabled_before_its_first_sync(self):
        files = {}
        with github(files) as (origin, seen):
            files.update(self.serve(origin))
            folder = self.root / 'fresh-images'
            folder.mkdir()
            image(folder / 'x.png', 'red')
            fresh = self.root / 'fresh.pilib'
            self.cli('library-create', '--dest', str(fresh), '--folder', str(folder))
            self.lib = Library(fresh)
            self.restart()
            # 还没有任何识别向量的图库等于新图库：包就位以后，第一次同步之前记成启用。
            self.assertEqual(self.packs()['library']['prompt'], 'auto')
            self.packs({'action': 'download'})
            self.assertEqual(self.settle('download')['current']['status'], 'ready')
            self.sync_to_pending_recognition(1)
            packs = self.packs()
            self.assertLessEqual({'enabled': True, 'version': '0.1', 'wd': True, 'ccip': True,
                                  'prompt': None, 'images': 0}.items(), packs['library'].items())
            # 只记下启用了哪个文件：没有证据，也没有归属。
            with sqlite3.connect(self.lib.db) as app:
                self.assertEqual(app.execute("SELECT count(*) FROM candidates WHERE source IN ('WD_EXT','CCIP_LONGTAIL')").fetchone(), (0,))

    def fresh_library_missing_the_pack(self, files):
        """新图库，第一次准备时角色资料包的 CCIP 文件没下载下来，第一次同步就过去了（#630）。返回那个文件的字节。"""
        ccip = files.pop('/builtin/A-longtail-p75.zip')
        folder = self.root / 'fresh-images'
        folder.mkdir()
        for name, color in (('x.png', 'red'), ('y.png', 'purple')):
            image(folder / name, color)
        fresh = self.root / 'fresh.pilib'
        self.cli('library-create', '--dest', str(fresh), '--folder', str(folder))
        self.lib = Library(fresh)
        self.restart()
        self.assertEqual(self.packs()['library']['prompt'], 'auto')
        self.packs({'action': 'download'})
        failed = self.settle('download')
        self.assertEqual(failed['download']['status'], 'failed', failed)
        self.assertNotEqual(failed['current']['status'], 'ready')
        self.sync_to_pending_recognition(2)
        self.assertEqual(self.packs()['library']['prompt'], 'pending')
        # 等宿主退出，再写入模拟的首次识别结果。重开前先关闭 SQLite 连接。
        self.stop(self.process)
        self.seed_wd_vectors(2)
        self.restart()
        # 已有识别结果，但没当成升级上来的图库去问：还是等包就位后默认启用。
        packs = self.packs()
        self.assertLessEqual({'enabled': False, 'prompt': 'pending'}.items(), packs['library'].items())
        return ccip

    def test_new_library_enables_the_pack_once_a_late_download_arrives(self):
        files = {}
        with github(files) as (origin, seen):
            files.update(self.serve(origin))
            ccip = self.fresh_library_missing_the_pack(files)
            # 点「重试」补下载：下完以后宿主自己启用，像用户点了启用一样检查已有的识别结果。
            files['/builtin/A-longtail-p75.zip'] = ccip
            self.packs({'action': 'download'})
            self.assertEqual(self.settle('download')['current']['status'], 'ready')
            observed = None

            def enabled():
                nonlocal observed
                observed = self.packs()
                task = observed['task']
                return task and task['kind'] == 'enable' and task['status'] != 'running'

            wait_until(enabled)
            self.assertEqual(observed['task']['status'], 'done', observed)
            self.assertLessEqual({'enabled': True, 'version': '0.1', 'wd': True, 'ccip': True, 'prompt': None,
                                  'images': 1, 'assigned': 1}.items(), observed['library'].items())
            self.assertEqual(self.members(), (self.ids[:1], self.ids[:1]))

            # 用户撤回以后不再默认启用：重启后也不会。
            self.packs({'action': 'withdraw'})
            withdrawn = self.settle('task')
            self.assertEqual(withdrawn['task']['status'], 'done', withdrawn)
            self.assertEqual(withdrawn['library']['prompt'], 'declined')
            self.restart()
            time.sleep(3)  # 宿主每秒检查一次要不要默认启用。
            packs = self.packs()
            self.assertIsNone(packs['task'])
            self.assertLessEqual({'enabled': False, 'prompt': 'declined'}.items(), packs['library'].items())
            self.assertEqual(self.members(), ([], []))

    def test_declining_before_a_late_download_keeps_the_pack_off(self):
        files = {}
        with github(files) as (origin, seen):
            files.update(self.serve(origin))
            ccip = self.fresh_library_missing_the_pack(files)
            # 包还没到时用户选了不用：下完以后也不启用。
            self.assertEqual(self.packs({'action': 'decline'})['library']['prompt'], 'declined')
            files['/builtin/A-longtail-p75.zip'] = ccip
            self.packs({'action': 'download'})
            self.assertEqual(self.settle('download')['current']['status'], 'ready')
            time.sleep(3)  # 宿主每秒检查一次要不要默认启用。
            packs = self.packs()
            self.assertIsNone(packs['task'])
            self.assertLessEqual({'enabled': False, 'prompt': 'declined'}.items(), packs['library'].items())
            self.assertEqual(self.members(), ([], []))

    def first_prepare(self, origin):
        """首次准备：识别模型还没就位时点「下载识别模型」。模型的来源都在外网，经测试代理全部失败，不影响资料包。"""
        self.env.update(ILLUSTASH_MODELS=str(self.root / 'models'), HTTPS_PROXY=origin)
        self.restart()
        self.assertEqual(self.packs()['current']['version'], '0.1')
        self.post('/api/models', {'action': 'download'})
        observed = None

        def ended():
            nonlocal observed
            observed = self.packs()
            job = observed['download']
            return job and job['kind'] != 'catalog' and job['status'] != 'running'

        wait_until(ended)
        return observed

    def test_first_prepare_reads_the_catalog_and_downloads_the_newest_pack(self):
        files = {}
        with github(files) as (origin, seen):
            files.update(self.serve(origin))
            packs = self.first_prepare(origin)
            # 先读目录（不读版本列表），再按目录下载最新的、这个版本能用的包。
            self.assertIn(DOWNLOAD + 'packs/packs.json.sig', seen)
            self.assertNotIn(RELEASES, seen)
            self.assertLess(seen.index(DOWNLOAD + 'packs/packs.json'), seen.index(DOWNLOAD + 'character-pack-1.0/wd-extension-v3.bin'))
            self.assertEqual((packs['catalog']['serial'], packs['catalog']['error']), (1, None))
            self.assertIsNotNone(packs['catalog']['checked_at'])
            self.assertEqual(packs['download']['status'], 'done', packs['download'])
            self.assertEqual((packs['current']['version'], packs['current']['status'], packs['offer']), ('1.0', 'ready', None))
            self.assertNotIn('/builtin/wd-extension-v1.bin', seen)

    def test_first_prepare_falls_back_to_the_builtin_pack_without_a_catalog(self):
        files = {}
        with github(files) as (origin, seen):
            files.update(self.serve(origin))
            files.pop(DOWNLOAD + 'packs/packs.json.sig')
            packs = self.first_prepare(origin)
            # 读不到目录：记下原因，照常下载内置的 0.1。
            self.assertIn('HTTP 404', packs['catalog']['error'])
            self.assertIsNone(packs['catalog']['serial'])
            self.assertEqual(packs['download']['status'], 'done', packs['download'])
            self.assertEqual((packs['current']['version'], packs['current']['status']), ('0.1', 'ready'))
            self.assertNotIn(DOWNLOAD + 'character-pack-1.0/wd-extension-v3.bin', seen)

    def test_incremental_shards_keep_the_active_pack_until_the_update_is_verified(self):
        files = {}
        ccip = FIXTURES / 'ccip-longtail-v1.zip'
        (old_wd, new_wd), (older, newer) = shard_fixture(self.root)
        old_files = pack_catalog.shard_files(older['packs'][0]['components']['wd']['shards'])
        new_files = pack_catalog.shard_files(newer['packs'][1]['components']['wd']['shards'])
        self.assertEqual(old_files[:-1], new_files[:-1])

        def publish(code, catalog):
            name = f'packs-shards-{code}.json'
            payload = (FIXTURES / name).read_bytes()
            self.assertEqual(payload, pack_catalog.dumps(catalog))
            files[DOWNLOAD + 'packs/packs.json'] = payload
            files[DOWNLOAD + 'packs/packs.json.sig'] = (FIXTURES / (name + '.sig')).read_bytes()
            self.post('/api/updates', {'action': 'check'})
            self.assertEqual(self.packs()['catalog']['serial'], catalog['serial'])

        def route(file):
            return urlsplit(file['url']).path

        def assert_old_unchanged():
            self.assertEqual(self.packs()['library']['version'], '2.0')
            self.assertEqual(self.packs()['wd']['library']['package_sha256'], sha(old_wd.read_bytes()))
            self.assertEqual(self.members(), (self.ids[:2], self.ids))
            self.packs({'action': 'enable', 'version': '2.1'}, 409)

        with github(files) as (origin, seen):
            files.update(self.serve(origin))
            self.restart()
            for part in old_files:
                files[route(part)] = (self.root / 'first' / part['name']).read_bytes()
            publish('a', older)
            seen.clear()
            self.packs({'action': 'download', 'version': '2.0'})
            installed = self.settle('download')
            self.assertEqual(installed['download']['status'], 'done', installed)
            self.assertCountEqual(seen, [route(f) for f in old_files] + ['/builtin/A-longtail-p75.zip'])
            self.packs({'action': 'enable', 'version': '2.0'})
            self.assertEqual(self.settle('task')['task']['status'], 'done')

            publish('b', newer)
            changed = route(new_files[-1])
            payload = (self.root / 'second' / new_files[-1]['name']).read_bytes()
            seen.clear()
            files[changed] = False
            self.packs({'action': 'download', 'version': '2.1'})
            failed = self.settle('download')
            self.assertEqual(failed['download']['status'], 'failed', failed)
            self.assertNotEqual(failed['offer']['status'], 'ready')
            self.assertEqual(seen, [changed] * 4)
            assert_old_unchanged()

            seen.clear()
            files[changed] = bytes([payload[0] ^ 1]) + payload[1:]
            self.packs({'action': 'download', 'version': '2.1'})
            failed = self.settle('download')
            self.assertEqual(failed['download']['status'], 'failed', failed)
            self.assertIn('SHA-256', failed['download']['message'])
            self.assertEqual(seen, [changed])
            assert_old_unchanged()

            seen.clear()
            files[changed] = payload
            self.packs({'action': 'download', 'version': '2.1'})
            state = self.settle('download')
            self.assertEqual((state['download']['status'], state['offer']['status']), ('done', 'ready'), state)
            self.assertEqual((state['download']['done'], state['download']['total']), (len(payload), len(payload)))
            self.assertEqual(seen, [changed])
            installed_path = self.root / 'shared' / 'extensions' / ('wd-extension-' + sha(new_wd.read_bytes())[:12])
            self.assertEqual((installed_path / new_wd.name).read_bytes(), new_wd.read_bytes())
            self.packs({'action': 'enable', 'version': '2.1'})
            self.assertEqual(self.settle('task')['library']['version'], '2.1')
            self.packs({'action': 'withdraw'})
            self.assertFalse(self.settle('task')['library']['enabled'])
            self.assertEqual(self.members(), ([], []))

            # 所有单片都合法，但完整包摘要不符也不能就位。
            publish('invalid', invalid_assembly(newer))
            seen.clear()
            self.packs({'action': 'download', 'version': '2.2'})
            invalid = self.settle('download')
            self.assertEqual(invalid['download']['status'], 'failed', invalid)
            self.assertEqual(invalid['download']['message'], 'WD 扩展包校验失败')
            self.assertEqual(seen, [])
            self.packs({'action': 'enable', 'version': '2.2'}, 409)

            # 本机仍有等长的旧版，从完整离线包导入，不能把新版误认成旧版。
            self.packs({'action': 'delete'})
            offline = self.root / 'shards-offline.zip'
            pack_catalog.offline(FIXTURES / 'packs-shards-b.json', [new_wd, ccip], offline)
            seen.clear()
            self.packs({'action': 'import', 'paths': [str(offline)]})
            imported = self.settle('download')
            self.assertEqual((imported['download']['status'], imported['current']['version']), ('done', '2.1'))
            self.assertEqual((installed_path / new_wd.name).read_bytes(), new_wd.read_bytes())
            self.assertEqual(seen, [])


if __name__ == '__main__':
    unittest.main()
