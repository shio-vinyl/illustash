"""#48 真实的原生名称事件消费。只用新生成的合成图库包和服务。

比较确定的成功字节、正式名称/投影/历史行和事件确认；
出错时比较状态码。不读账号存储、真实的照片图库包或图库副本。
"""
from contextlib import closing
from concurrent.futures import ThreadPoolExecutor
import http.server
import json
import os
import shutil
from pathlib import Path
import signal
import sqlite3
import subprocess
import tempfile
import threading
import unittest
import urllib.parse
from unittest.mock import patch

from tests import test_native_lifecycle as lifecycle
from tests.fixed_expectations import RETIRED, SKIP_COLUMNS, unclocked
from tests.test_read_http import INTERRUPT, SPAWN, free_port, wait_until
from tests.name_providers import Providers
from tools.verify_read_http import enter, output
from tools.wait_limit import WAIT


class TextService:
    def __init__(self):
        self.provider = Providers()
        self.calls = []
        self.entered, self.release = threading.Event(), threading.Event()
        self.block = False
        self.block_tag = None
        self.hold_deadline = WAIT
        self.failure = None
        self.failures_left = 0
        self.bad_json = False
        self.alice_zh = "爱丽丝"
        outer = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args): pass
            def do_GET(self): self.reply()
            def do_POST(self): self.reply()
            def reply(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length']))) if 'Content-Length' in self.headers else None
                outer.calls.append((self.path, body))
                tag = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query).get('search[title]', [''])[0]
                if outer.block or (outer.block_tag and outer.block_tag in tag):
                    outer.entered.set()
                    if not outer.release.wait(outer.hold_deadline): raise AssertionError('synthetic hold deadline')
                if outer.failure or outer.failures_left:
                    code = outer.failure or 503
                    outer.failures_left = max(0, outer.failures_left - 1)
                    data = b'private provider response must never appear'
                else:
                    code = 200
                    q = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
                    if 'wiki_pages.json' in self.path:
                        tag = q.get('search[title]', [''])[0]
                        if 'search[title]' in q:
                            values = ['アリス', '爱丽丝'] if 'alice' in tag else ['ボブ', '鲍勃'] if 'bob' in tag else ['エグザンプル']
                            value = [{'title': tag, 'other_names': values}]
                        else: value = [{'title': 'new_character_(Example)', 'tag': {'post_count': 9}}]
                    elif '/tags.json' in self.path:
                        if 'search[name]' in q:
                            value = [{'name': q['search[name]'][0], 'post_count': 12}]
                        else: value = [{'name': 'new_character_(Example)', 'post_count': 9}, {'name':'alice_tag','post_count':2}]
                    elif '/related_tag.json' in self.path:
                        value = {'related_tags': [{'tag': {'name': 'Example', 'post_count': 12}, 'frequency': .95}]}
                    elif '/search/subjects' in self.path:
                        value = {'data': [{'id': 1, 'name':'エグザンプル','name_cn':'示例作品'}]}
                    elif '/search/characters' in self.path:
                        value = {'data': [{'id': 10, 'name': body['keyword'], 'infobox': [{'key':'简体中文名','value':outer.alice_zh if 'リス' in body['keyword'] else '鲍勃'}]}]}
                    elif '/characters/' in self.path:
                        value = [{'name':'エグザンプル','name_cn':'示例作品'}]
                    elif '/chat/completions' in self.path:
                        value = {'choices':[{'message':{'content':'{"zh":"合成译名"}'}}]}
                    else: raise AssertionError(self.path)
                    data = b'{' if outer.bad_json else json.dumps(value, ensure_ascii=False).encode()
                self.send_response(code); self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(data))); self.end_headers()
                try: self.wfile.write(data)
                except BrokenPipeError: pass
        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.base = f'http://127.0.0.1:{self.server.server_port}'
    def close(self):
        self.release.set(); self.server.shutdown(); self.server.server_close(); self.thread.join()
    def transport(self, url, body=None, key=None):
        import urllib.request
        parsed = urllib.parse.urlsplit(url)
        path = parsed.path + ('?' + parsed.query if parsed.query else '')
        data = json.dumps(body).encode() if body is not None else None
        with urllib.request.urlopen(urllib.request.Request(self.base + path, data=data), timeout=WAIT) as response:
            return json.load(response)


class NativeNamesTest(unittest.TestCase):
    setUpClass = classmethod(lifecycle.NativeLifecycleTest.setUpClass.__func__)
    package = lifecycle.NativeLifecycleTest.package
    stop = lifecycle.NativeLifecycleTest.stop
    request = lifecycle.NativeLifecycleTest.request
    def start(self, path, extra=(), command=None):
        log = tempfile.TemporaryFile(); self.addCleanup(log.close)
        args = [str(self.binary), 'http', '--no-auto-sync', '--library', str(path), '--port', str(self.port), '--root', str(self.resources), *extra]
        if not getattr(self, 'enabled', False): args += ['--no-names']
        env = {**self.env, 'ILLUSTASH_NO_KEYCHAIN': '1'}
        process = subprocess.Popen(args, env=env, stdout=log, stderr=log, **SPAWN)
        self.addCleanup(self.stop, process)
        enter(self.origin, log, process)
        lifecycle.wait_human_backup(self, self.origin)
        return process, log
    def setUp(self):
        self.enabled = False
        lifecycle.NativeLifecycleTest.setUp(self)
        self.service = TextService(); self.addCleanup(self.service.close)
        self.rec.alias(self.service.base, '@SERVICE@')
        self.env.update(ILLUSTASH_COPYRIGHT_BASE=self.service.base, ILLUSTASH_BANGUMI_BASE=self.service.base)
        self.alice = self.rec.value('alice')
    def enable(self):
        self.stop(self.process); self.enabled=True
        self.process, self.log = self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.rec.alias(self.token,'@TOKEN@')
        self.finish()
    def finish(self, status='done'):
        def complete():
            value = json.loads(self.request('/api/names?progress=1')['body'])['state']
            return value if value['status']==status else False
        # 后续名称事件可以立即开始，断言轮询命中时的状态。
        return wait_until(complete)
    def events(self):
        with closing(sqlite3.connect(self.lib.db)) as c:
            return c.execute("SELECT id,status,error FROM backend_effects WHERE kind LIKE 'names:%' ORDER BY id").fetchall()
    def test_complete_refresh_matches_reference_formal_names_and_history(self):
        expected=self.rec.value('reference refresh')
        self.enable()
        self.assertEqual(self.request('/api/names',{'action':'refresh'})['status'],200)
        self.finish()
        # 全部正式/core 名称表和未改动的历史；不含不确定的计时、预算和缓存记账。
        with closing(sqlite3.connect(self.lib.db)) as new:
            actual=name_tables(new)
            # core v8 删掉的旧表（#201）：其内容在取代它们的 term_* 表里比较。
            self.assertEqual({t:rows for t,rows in expected.items() if t in actual or t not in RETIRED},actual)
            self.assertEqual(new.execute("SELECT value FROM meta WHERE key='names_projected'").fetchone(),new.execute("SELECT value FROM meta WHERE key='names_version'").fetchone())
        self.assertTrue(all(status=='done' and not error for _,status,error in self.events()))
        self.assertFalse(self.sentinel.exists())
    def test_single_entity_duplicate_events_and_warm_restart(self):
        self.enable()
        ident=self.alice
        self.assertEqual(self.request('/api/names',{'action':'refresh','entity':ident})['status'],200)
        self.finish()
        with sqlite3.connect(self.lib.db) as c:
            event='names:'+json.dumps({'entity':ident,'refresh':False})
            c.executemany('INSERT INTO backend_effects(kind) VALUES (?)',[(event,),(event,)])
        self.stop(self.process); self.process,self.log=self.start(self.lib.path); self.finish()
        wait_until(lambda:all(s=='done' for _,s,_ in self.events()))
        count=len(self.service.calls)
        self.stop(self.process);self.process,self.log=self.start(self.lib.path);self.finish()
        self.assertEqual(len(self.service.calls),count)
    def test_configuration_success_bytes_clear_legacy_and_secret_redaction(self):
        for body in ({'action':'model_api','key':'synthetic-secret-key','model':'synthetic-model','base':self.service.base}, {'action':'model_api','key':None}, {'action':'model_api','model':None}):
            left=lifecycle.NativeLifecycleTest.reference_response(self, '/api/names',body);right=self.request('/api/names',body)
            self.assertEqual(left['status'],200);self.assertEqual(right['status'],200)
            self.assertEqual(left['body'],right['body'])
            self.assertNotIn(b'synthetic-secret-key',right['body'])
        self.assertEqual(json.loads(self.request('/api/names',{'action':'model_api_test'})['body']),{'ok':False,'error':'尚未设置 API key'})
        self.request('/api/names',{'action':'model_api','key':'synthetic-secret-key','base':self.service.base})
        self.assertEqual(self.request('/api/names',{'action':'model_api_test'})['body'],b'{"ok": true}')
        self.service.failure=401
        response=self.request('/api/names',{'action':'model_api_test'})
        self.assertEqual(response['status'],200)
        self.assertNotIn(b'private provider',response['body']);self.assertNotIn(b'synthetic-secret',response['body'])
    def test_legacy_model_key_migrates_after_storage_failure_and_stays_local(self):
        self.stop(self.process)
        active = 'synthetic-active-model-key-4829'
        legacy = 'synthetic-obsolete-model-key-2930'
        with closing(sqlite3.connect(self.lib.db)) as c:
            c.execute('PRAGMA journal_mode=WAL')
            c.executemany('INSERT OR REPLACE INTO name_settings VALUES (?,?)',
                          [('model_key', active), ('deepseek_key', legacy),
                           ('deepseek_model', 'synthetic-legacy-model'), ('model_base', self.service.base)])
            c.commit()
        accounts = self.support / 'accounts'
        accounts.write_bytes(b'synthetic obstacle')
        self.process, self.log = self.start(self.lib.path)

        def rows(db=None):
            with closing(sqlite3.connect(db or self.lib.db)) as c:
                return dict(c.execute('SELECT key,value FROM name_settings'))

        def model():
            response = self.request('/api/names')
            self.assertEqual(response['status'], 200, response['body'])
            return json.loads(response['body'])['model_api']

        self.assertEqual(rows()['model_key'], active)
        self.assertEqual(rows()['deepseek_key'], legacy)
        self.assertFalse(model()['configured'])
        self.assertIn('尚未迁移', model()['credential_error'])
        self.stop(self.process)
        accounts.unlink()
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(model(), {'configured': True, 'hint': '4829', 'model': 'synthetic-legacy-model',
                                   'base': self.service.base})
        self.assertFalse({'model_key', 'deepseek_key'} & rows().keys())
        files = [self.lib.db, self.lib.db.with_suffix('.sqlite-wal')]
        for path in files:
            data = path.read_bytes() if path.is_file() else b''
            self.assertNotIn(active.encode(), data)
            self.assertNotIn(legacy.encode(), data)
        stored, = accounts.glob('model-api-*')
        if os.name == 'nt':
            self.assertEqual(stored.suffix, '.dpapi')
            self.assertNotIn(active.encode(), stored.read_bytes())
        self.assertEqual(json.loads(self.request('/api/names', {'action': 'model_api_test'})['body']), {'ok': True})
        self.stop(self.process)

        # 若本机条目存在但读失败，仍保留旧版重填的密钥，避免把读取失败当作缺少条目。
        replacement = 'synthetic-legacy-replacement-7364'
        with closing(sqlite3.connect(self.lib.db)) as c:
            c.execute("INSERT INTO name_settings VALUES ('model_key',?)", (replacement,))
            c.commit()
        saved = accounts / 'held-synthetic-credential'
        stored.rename(saved)
        stored.mkdir()
        self.process, self.log = self.start(self.lib.path)
        self.assertEqual(rows()['model_key'], replacement)
        self.stop(self.process)
        stored.rmdir()
        saved.rename(stored)
        self.process, self.log = self.start(self.lib.path)
        self.assertNotIn('model_key', rows())
        self.assertEqual(model()['hint'], '4829')
        self.stop(self.process)

        # 同 ID 的原样复制只得到配置。若旧版在副本重填过 key，引用虽在但本机条目确实缺失，应迁移明文。
        copy = self.root / 'model-copy.pilib'
        shutil.copytree(self.lib.path, copy)
        self.process, self.log = self.start(copy)
        self.assertFalse(model()['configured'])
        self.stop(self.process)
        copy_db = copy / 'library.sqlite'
        with closing(sqlite3.connect(copy_db)) as c:
            c.execute("INSERT INTO name_settings VALUES ('model_key',?)", (replacement,))
            c.commit()
        self.process, self.log = self.start(copy)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(model()['hint'], '7364')
        self.assertNotIn('model_key', rows(copy_db))
        self.assertEqual(self.request('/api/names', {'action': 'model_api', 'key': None})['status'], 200)
        self.stop(self.process)
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(model()['hint'], '4829')
        self.assertEqual(self.request('/api/names', {'action': 'model_api', 'key': None})['status'], 200)
        self.assertFalse(model()['configured'])
        self.assertFalse(stored.exists())

    def test_model_settings_clear_failed_migration_and_replace_unreadable_key(self):
        self.stop(self.process)
        accounts = self.support / 'accounts'
        accounts.write_bytes(b'synthetic obstacle')
        old_key = 'synthetic-unmigrated-key-2351'
        with closing(sqlite3.connect(self.lib.db)) as c:
            c.execute("INSERT OR REPLACE INTO name_settings VALUES ('deepseek_key',?)", (old_key,))
            c.commit()
        self.process, self.log = self.start(self.lib.path)
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']

        def save(**fields):
            response = self.request('/api/names', {'action': 'model_api', **fields})
            self.assertEqual(response['status'], 200, response['body'])
            return json.loads(response['body'])['model_api']

        broken = self.request('/api/names')
        self.assertIn('尚未迁移', json.loads(broken['body'])['model_api']['credential_error'])
        self.assertNotIn(old_key.encode(), broken['body'])
        self.assertNotIn('credential_error', save(key=None))
        with closing(sqlite3.connect(self.lib.db)) as c:
            self.assertIsNone(c.execute("SELECT value FROM name_settings WHERE key='deepseek_key'").fetchone())
        accounts.unlink()
        self.assertTrue(save(key='synthetic-initial-key-2469', base=self.service.base)['configured'])
        stored, = accounts.glob('model-api-*')
        stored.write_bytes(b'\xff\xfeinvalid-synthetic-credential')
        broken = self.request('/api/names')
        self.assertIn('无法读取模型 API key', json.loads(broken['body'])['model_api']['credential_error'])
        self.assertNotIn(b'invalid-synthetic-credential', broken['body'])
        self.assertNotEqual(self.request('/api/names', {'action': 'model_api_test'})['status'], 200)
        # 系统删除失败时，设置保留引用并返回错误；解除故障后同一清除动作能完成。
        def reference():
            with closing(sqlite3.connect(self.lib.db)) as c:
                row = c.execute("SELECT value FROM name_settings WHERE key='model_credential'").fetchone()
                return row[0] if row else None

        original = reference()
        moved = accounts / 'held-credential'
        stored.rename(moved)
        stored.mkdir()
        error = self.request('/api/names', {'action': 'model_api', 'key': None})
        self.assertNotEqual(error['status'], 200, error['body'])
        self.assertIn('无法清除模型 API key', json.loads(error['body'])['error'])
        self.assertEqual(reference(), original)
        stored.rmdir()
        moved.rename(stored)
        self.assertFalse(save(key=None)['configured'])
        self.assertIsNone(reference())
        self.assertFalse(stored.exists())

        self.assertTrue(save(key='synthetic-new-initial-key-4829')['configured'])
        stored, = accounts.glob('model-api-*')
        stored.write_bytes(b'\xff\xfeinvalid-synthetic-credential')
        corrected = save(key='synthetic-replacement-key-2673')
        self.assertEqual(corrected['hint'], '2673')
        self.assertNotIn('credential_error', corrected)
        self.assertFalse(stored.exists())

    def test_bangumi_name_is_published_with_unreadable_model_key(self):
        self.enable()
        result = self.request('/api/names', {'action': 'model_api', 'key': 'synthetic-key-4928',
                                             'base': self.service.base})
        self.assertEqual(result['status'], 200, result['body'])
        stored, = (self.support / 'accounts').glob('model-api-*')
        stored.write_bytes(b'\xff\xfeinvalid-synthetic-credential')
        self.service.alice_zh = '爱丽丝新译'
        result = self.request('/api/names', {'action': 'refresh', 'entity': self.alice})
        self.assertEqual(result['status'], 200, result['body'])

        def published():
            with closing(sqlite3.connect(self.lib.db)) as c:
                return c.execute(
                    "SELECT n.value FROM term_names n JOIN terms t ON t.id=n.term_id "
                    "WHERE t.key=? AND n.lang='zh' AND n.source='bangumi' AND n.value=?",
                    (self.alice, self.service.alice_zh)).fetchone()
        wait_until(published)
        self.assertFalse(any('/chat/completions' in path for path, _ in self.service.calls))

    def test_validation_and_disabled_refresh_error_status(self):
        for body in ({'action':'refresh'},{'action':'refresh','entity':3},{'action':'model_api','key':'bad'}, {'action':'model_api','model':'bad space'},{'action':'model_api','base':'http://example.invalid'},{'action':'model_api','base':self.service.base+'?x=1'},{'action':'unknown'}):
            self.assertEqual(self.request('/api/names',body)['status'],400)
        self.assertEqual(len(self.service.calls),0)
    def test_library_consent_to_name_queries_persists_and_revokes(self):
        network=lambda:json.loads(self.request('/api/names')['body'])['network']
        prefs=lambda:sqlite3.connect(self.lib.db).execute("SELECT count(*) FROM prefs WHERE key='names_network'").fetchone()[0]
        self.assertEqual(network(),{'authorized':False,'this_run':False,'granted_at':None})
        for body in ({'action':'network'},{'action':'network','allow':'yes'},{'action':'network','allow':1}):
            self.assertEqual(self.request('/api/names',body)['status'],400)
        self.assertEqual(prefs(),0)
        response=self.request('/api/names',{'action':'network','allow':True})
        self.assertEqual(response['status'],200,response['body'])
        granted=json.loads(response['body'])['network']
        self.assertTrue(granted['authorized']);self.assertFalse(granted['this_run']);self.assertRegex(granted['granted_at'],r'^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d$')
        self.assertEqual(prefs(),1)
        # 同意属于图库：重启后仍在，也不是启动偏好设置。
        self.stop(self.process);self.process,self.log=self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(network(),granted)
        self.assertNotIn('names_network',json.loads(self.request('/api/settings')['body'])['startup'])
        revoked=self.request('/api/names',{'action':'network','allow':False})
        self.assertEqual(revoked['status'],200,revoked['body'])
        self.assertEqual(json.loads(revoked['body'])['network'],
                         {'authorized':False,'this_run':False,'granted_at':None})
        self.assertEqual(prefs(),0)
        # --names-network 只授权本次运行，不记录任何东西。
        self.stop(self.process);self.process,self.log=self.start(self.lib.path,extra=('--names-network',))
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(network(),{'authorized':True,'this_run':True,'granted_at':None})
        self.assertEqual(prefs(),0)
        self.assertEqual(len(self.service.calls),0)
    def test_remote_picker_success_bytes_and_persistent_query_cache(self):
        self.stop(self.process);self.process,self.log=self.start(self.lib.path)
        for url in ('/api/entities?remote=1&q=new','/api/entities?remote=1&q=%E5%90%88%E6%88%90','/api/works?remote=1&q=new','/api/works?remote=1&q=%E5%90%88%E6%88%90','/api/local-links'):
            old=lifecycle.NativeLifecycleTest.reference_response(self, url);new=self.request(url)
            self.assertEqual(new['status'],200,new['body']);self.assertEqual(old['body'],new['body'],url)
        count=len(self.service.calls)
        self.request('/api/entities?remote=1&q=new')
        self.assertEqual(len(self.service.calls),count)
    def test_failure_no_negative_cache_then_restart_consumes_pending(self):
        self.service.failure=503
        self.enable_error()
        self.assertTrue(any(s=='pending' and e for _,s,e in self.events()))
        with closing(sqlite3.connect(self.lib.db)) as c:
            self.assertEqual(c.execute("SELECT count(*) FROM name_cache WHERE key LIKE '%wiki_pages%' OR key LIKE '[\"done\", 1, \"character\",%'").fetchone()[0],0)
        self.service.failure=None
        self.stop(self.process);self.process,self.log=self.start(self.lib.path);self.finish()
        wait_until(lambda:all(s=='done' for _,s,_ in self.events()))
    def enable_error(self):
        self.stop(self.process);self.enabled=True;self.process,self.log=self.start(self.lib.path);return self.finish('error')

    def start_blocked_refresh(self):
        self.env['ILLUSTASH_TEST_TRACE_NAMES_CANCEL'] = '1'
        self.enable()
        previous_ids = {event_id for event_id, _, _ in self.events()}
        self.service.block = True
        response = self.request('/api/names', {'action': 'refresh'})
        self.assertEqual(response['status'], 200, response['body'])
        self.assertTrue(self.service.entered.wait(WAIT))
        new_events = [row for row in self.events() if row[0] not in previous_ids]
        self.assertEqual(len(new_events), 1, new_events)
        event_id, status, error = new_events[0]
        self.assertEqual((status, error), ('pending', ''))
        return event_id

    def release_after_cancel(self):
        try:
            wait_until(lambda: b'ILLUSTASH_TEST_NAMES_CANCELLED' in output(self.log))
        finally:
            self.service.release.set()

    def test_busy_refresh_cancel_switch_same_id_and_late_result(self):
        second = self.package('second')
        second.config['id'] = self.lib.config['id']
        second.save()
        event_id = self.start_blocked_refresh()
        self.assertEqual(self.request('/api/names', {'action': 'refresh'})['status'], 400)
        with closing(sqlite3.connect(self.lib.db)) as c:
            before = c.execute(NAMES + ' ORDER BY t.key,n.value').fetchall()
        with ThreadPoolExecutor() as pool:
            switched = pool.submit(self.request, '/api/libraries/open', {'path': str(second.path)})
            self.release_after_cancel()
            opened = switched.result(timeout=WAIT)
        self.assertEqual(opened['status'], 200, opened['body'])
        self.assertTrue(Path(json.loads(opened['body'])['path']).samefile(second.path))
        with closing(sqlite3.connect(self.lib.db)) as c:
            self.assertEqual(before, c.execute(NAMES + ' ORDER BY t.key,n.value').fetchall())
        self.assertIn((event_id, 'pending', '名称任务已取消'), self.events())
        self.assertFalse(self.sentinel.exists())

    def test_exit_waits_cancellation_and_keeps_pending_for_recovery(self):
        event_id = self.start_blocked_refresh()
        self.process.send_signal(INTERRUPT)
        self.release_after_cancel()
        self.assertEqual(self.process.wait(WAIT), 0)
        self.assertIn((event_id, 'pending', '名称任务已取消'), self.events())

    def test_transient_retry(self):
        self.service.failures_left=2;self.enable()
        self.assertGreaterEqual(len(self.service.calls),3)
    def test_profile_lookup_and_durable_event_actual_consumption(self):
        self.enable();ident=self.alice
        response=self.request('/api/character/profile',{'entity':ident,'tag':'synthetic_(Example)'})
        self.assertEqual(response['status'],200,response['body'])
        wait_until(lambda:all(s=='done' for _,s,_ in self.events()))
        self.assertTrue(any('/related_tag.json' in path for path,_ in self.service.calls))
        self.assertTrue(any('synthetic_' in path for path,_ in self.service.calls))

    def test_error_statuses_and_no_negative_remote_cache(self):
        self.stop(self.process);self.process,self.log=self.start(self.lib.path)
        self.service.failure=401
        old=lifecycle.NativeLifecycleTest.reference_response(self, '/api/works?remote=1&q=uncached')
        new=self.request('/api/works?remote=1&q=uncached')
        self.assertEqual(old['status'],502);self.assertEqual(new['status'],old['status'])
        self.assertNotIn(b'private provider',new['body'])
        self.service.failure=None
        self.assertEqual(self.request('/api/works?remote=1&q=uncached')['status'],200)

    def test_ack_failure_replays_without_false_completion(self):
        with sqlite3.connect(self.lib.db) as c:
            c.execute("CREATE TRIGGER synthetic_ack_failure BEFORE UPDATE OF status ON backend_effects WHEN NEW.kind LIKE 'names:%' BEGIN SELECT RAISE(ABORT,'synthetic ack failure'); END")
        state = self.enable_error()
        self.assertEqual(state['diagnostic']['publication'], 'may_be_partial')
        self.assertTrue(any(s=='pending' for _,s,_ in self.events()))
        with closing(sqlite3.connect(self.lib.db)) as c:
            formal=c.execute(NAMES+' ORDER BY t.key,n.role,n.source,n.value').fetchall()
        self.stop(self.process)
        with sqlite3.connect(self.lib.db) as c:c.execute('DROP TRIGGER synthetic_ack_failure')
        self.process,self.log=self.start(self.lib.path);self.finish()
        wait_until(lambda:all(s=='done' for _,s,_ in self.events()))
        with closing(sqlite3.connect(self.lib.db)) as c:self.assertEqual(formal,c.execute(NAMES+' ORDER BY t.key,n.role,n.source,n.value').fetchall())

    def test_query_source_changed_while_pending_rejects_old_name(self):
        self.enable();ident=self.alice
        self.service.block_tag = "alice"
        refresh=self.request('/api/names',{'action':'refresh','entity':ident})
        self.assertEqual(refresh['status'],200,refresh['body'])
        self.assertTrue(self.service.entered.wait(WAIT))
        event_id, _, _ = self.events()[-1]
        # 这期间保存了一个用户名称（以前经过旧 Store，现在走应用自己的路由）。
        edited=self.request('/api/character/profile',{'entity':ident,'name':'人工保留名'})
        self.assertEqual(edited['status'],200,edited['body'])
        self.service.release.set();state=self.finish('error')
        self.assertIn('名称来源已改变',state['error'])
        profile=json.loads(self.request('/api/character/profile?entity='+urllib.parse.quote(ident))['body'])
        self.assertEqual(profile['user_name'],'人工保留名')
        wait_until(lambda:(event_id,'pending',state['error']) in self.events())

    def test_malformed_event_does_not_block_new_valid_request(self):
        self.enable()
        with sqlite3.connect(self.lib.db) as c:
            c.execute("INSERT INTO backend_effects(kind) VALUES ('names:malformed')")
        state = self.finish('error')
        self.assertEqual(state['diagnostic']['publication'], 'not_committed')
        ident=self.alice
        self.assertEqual(self.request('/api/names',{'action':'refresh','entity':ident})['status'],200)
        wait_until(lambda:self.events()[-1][1]=='done')
        self.assertTrue(any(s=='pending' and e for _,s,e in self.events()))

    def test_diagnostic_includes_names_committed_after_an_earlier_failure(self):
        # 禁用名称时先排好两个事件，重启后由同一轮恢复按顺序处理。
        learned = NAMES + " WHERE t.key=? AND n.source='danbooru' AND n.value='爱丽丝'"
        with closing(sqlite3.connect(self.lib.db)) as c, c:
            c.execute("UPDATE backend_effects SET status='done' WHERE kind LIKE 'names:%'")
            failed = c.execute("INSERT INTO backend_effects(kind) VALUES ('names:malformed')").lastrowid
            request = 'names:' + json.dumps({'entity': self.alice, 'refresh': True})
            succeeded = c.execute("INSERT INTO backend_effects(kind) VALUES (?)", [request]).lastrowid
            before = c.execute(learned, [self.alice]).fetchall()
        self.assertEqual(before, [])

        state = self.enable_error()
        self.assertEqual(state['error'], '名称事件格式不正确')
        self.assertEqual(state['diagnostic']['publication'], 'may_be_partial')
        events = {event: (status, error) for event, status, error in self.events()}
        self.assertEqual(events[failed], ('pending', state['error']))
        self.assertEqual(events[succeeded], ('done', ''))
        with closing(sqlite3.connect(self.lib.db)) as c:
            self.assertTrue(c.execute(learned, [self.alice]).fetchall())

    def test_explicit_no_network_and_key_save_cannot_authorize(self):
        self.request('/api/names',{'action':'model_api','key':'synthetic-secret'})
        self.assertEqual(self.request('/api/names',{'action':'model_api_test'})['status'],200)
        self.assertIn('尚未授权',json.loads(self.request('/api/names',{'action':'model_api_test'})['body'])['error'])
        self.assertEqual(len(self.service.calls),0)

    def test_model_request_switch_rejects_late_response(self):
        self.stop(self.process);self.process,self.log=self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.request('/api/names',{'action':'model_api','key':'synthetic-secret','base':self.service.base})
        second = self.package('model-second')
        self.service.block=True
        with ThreadPoolExecutor() as pool:
            pending=pool.submit(self.request,'/api/names',{'action':'model_api_test'})
            self.assertTrue(self.service.entered.wait(WAIT))
            self.assertEqual(self.request('/api/libraries/open',{'path':str(second.path)})['status'],200)
            self.service.release.set()
            self.assertEqual(pending.result(timeout=WAIT)['status'],403)
        self.assertFalse(self.sentinel.exists())


    def test_process_killed_while_query_pending_restarts_actual_consumer(self):
        self.enable();self.service.block=True
        self.assertEqual(self.request('/api/names',{'action':'refresh'})['status'],200)
        self.assertTrue(self.service.entered.wait(WAIT))
        self.process.kill();self.process.wait(WAIT)
        self.assertTrue(any(s=='pending' for _,s,_ in self.events()))
        self.service.release.set();self.service.block=False
        self.process,self.log=self.start(self.lib.path);self.finish()
        wait_until(lambda:all(s=='done' for _,s,_ in self.events()))
        self.assertFalse(self.sentinel.exists())

    def test_inflight_transport_timeout_retries_and_commits_after_recovery(self):
        self.enable();self.service.block=True;self.service.hold_deadline=32
        before=len(self.service.calls)
        self.assertEqual(self.request('/api/names',{'action':'refresh'})['status'],200)
        self.assertTrue(self.service.entered.wait(WAIT))
        timer=threading.Timer(26,self.service.release.set);timer.start();self.addCleanup(timer.cancel)
        wait_until(lambda:json.loads(self.request('/api/names?progress=1')['body'])['state']['status']=='done',timeout=26+WAIT)
        wait_until(lambda:all(s=='done' for _,s,_ in self.events()))
        attempted=[path for path,_ in self.service.calls[before:]]
        self.assertGreaterEqual(attempted.count(attempted[0]),2)



# 按身份键列出角色、皮肤和作品的名称（原来的 `names` 表，core v8 把它们放在 term_names 里）。
NAMES="SELECT t.key,n.lang,n.value,n.source,n.role,n.preferred,n.position FROM term_names n JOIN terms t ON t.id=n.term_id JOIN term_tree r ON r.term_id=n.term_id"


def name_tables(c):
    """正式/core 名称表和历史，行已排序；类似时钟的文本记为 <clock>。core v8 新增的列不取，
    因为对照基准也不含它们（SKIP_COLUMNS）。"""
    tables=[r[0] for r in c.execute("SELECT name FROM sqlite_master WHERE type='table' AND (name IN ('names','display_names','name_aliases','actions') OR name LIKE 'term_name%' OR name IN ('term_key_names','term_key_aliases'))")]
    def columns(table):
        return ','.join(f'"{name}"' for _,name,*_ in c.execute(f'PRAGMA table_info("{table}")') if (table,name) not in SKIP_COLUMNS)
    return {table:sorted([[unclocked(v) for v in row] for row in c.execute(f'SELECT {columns(table)} FROM "{table}"')],key=repr) for table in tables}


if __name__=='__main__':unittest.main()
