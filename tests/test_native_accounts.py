"""#50 账号操作，只用合成的密钥、包和回环地址上的服务。

成功时的字节/元数据/数据库行与参考结果比较（经 tests/fixed_expectations.py 录制，#9；
精确到分钟的检查时间按 ``<clock>`` 比较）；错误只比较状态码。原生进程找不到 Python，
也读不到用户的凭据存储。
"""
from contextlib import closing
from concurrent.futures import ThreadPoolExecutor
import http.server
import json
import os
from pathlib import Path
import sqlite3
import stat
import threading
import time
import unittest
from tests.test_read_http import INTERRUPT
import urllib.parse
import zipfile

from tests import test_native_lifecycle as life, test_native_human_labels as labels
from tests.fixed_expectations import unclocked
from tools.wait_limit import WAIT


def difference(left, right):
    return life.difference(left, right)


def provenance_db(lib):
    return Path(lib.path) / 'provenance.sqlite'

class AccountService:
    def __init__(self):
        self.logged = True; self.login_open = False; self.rejected = False; self.code = 200
        self.fetch_status = None; self.failures_left=0
        self.calls = []; self.entered = threading.Event(); self.release = threading.Event(); self.block = False
        outer = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self,*args): pass
            def do_GET(self): self.reply()
            def do_POST(self): self.reply()
            def reply(self):
                outer.calls.append((self.command, self.path, self.headers.get('Cookie'), self.headers.get('X-Illustash-Token')))
                if outer.block and '/ajax/' in self.path:
                    outer.entered.set(); outer.release.wait(WAIT)
                route = urllib.parse.urlsplit(self.path).path
                code = outer.code
                if outer.failures_left and route.startswith("/ajax/"):
                    code=503;outer.failures_left-=1
                if route == '/pixiv/session': value = {'loggedIn':outer.logged,'userId':'123' if outer.logged else None,'loginOpen':outer.login_open}
                elif route in ('/pixiv/login','/pixiv/logout'):
                    if route.endswith('login'): outer.login_open=True
                    else: outer.logged=False
                    value={'ok':True}
                elif route == '/pixiv/fetch':
                    target=urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)['path'][0]
                    value={'status':outer.fetch_status if outer.fetch_status is not None else (401 if outer.rejected else 200),'body':json.dumps(outer.ajax(target),ensure_ascii=False),'cfMitigated':False}
                else: value=outer.ajax(self.path)
                data=json.dumps(value,ensure_ascii=False).encode()
                self.send_response(code); self.send_header('Content-Length',str(len(data))); self.end_headers()
                try:self.wfile.write(data)
                except BrokenPipeError:pass
        self.server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.base=f'http://127.0.0.1:{self.server.server_port}'
    def ajax(self,path):
        if self.rejected:return {'error':True,'message':'synthetic rejected'}
        return {'error':False,'body':{'following':7} if '/extra' in path else {'name':'合成账号'}}
    def close(self):self.release.set();self.server.shutdown();self.server.server_close();self.thread.join()

class NativeAccountsTest(unittest.TestCase):
    setUpClass=classmethod(life.NativeLifecycleTest.setUpClass.__func__)
    package=life.NativeLifecycleTest.package
    stop=life.NativeLifecycleTest.stop
    request=life.NativeLifecycleTest.request
    def start(self,path,extra=(),command=None):
        self.env.update(ILLUSTASH_NO_KEYCHAIN='1',ILLUSTASH_PIXIV_BASE=self.service.base)
        for key in ('ILLUSTASH_DESKTOP_BRIDGE','ILLUSTASH_DESKTOP_TOKEN'):self.env.pop(key,None)
        if getattr(self,'with_bridge',False):self.env.update(ILLUSTASH_DESKTOP_BRIDGE=self.service.base,ILLUSTASH_DESKTOP_TOKEN='synthetic-bridge-token')
        return life.NativeLifecycleTest.start(self,path,extra,command)
    def setUp(self):
        self.service=AccountService();self.addCleanup(self.service.close)
        life.NativeLifecycleTest.setUp(self)
        # 一份合成的溯源 schema，带冻结的夹具默认值（已录制）。
        self.rec.package('provenance', self.lib.path)
    def pair(self,action,body=None,expected=200):
        route='/api/accounts/'+action
        # 跨越分钟边界时保留原始时间戳，不对输出做规范化。
        left=life.NativeLifecycleTest.reference_response(self,route,body or {})
        right=self.request(route,body or {})
        self.assertEqual(left['status'],expected,left['body']);self.assertEqual(right['status'],expected,right['body'])
        self.assertEqual(right['route'],'core')
        if expected<400:
            self.assertIsNone(difference(left,right),(left['body'],right['body']))
            masked=lambda m:json.loads(unclocked(json.dumps(m,ensure_ascii=False)))
            self.assertEqual(masked(self.rec.value('reference metadata')),
                             masked(self.metadata(self.support)))
            if action == 'x/import':
                self.assertEqual(labels.states(self.lib),self.rec.state('reference state'))
        self.assertFalse(self.sentinel.exists())
        return json.loads(right['body']) if expected<400 else None
    def metadata(self,path):
        f=path/'accounts.json';return json.loads(f.read_text(encoding='utf-8')) if f.exists() else {}
    def status_pair(self):
        left=life.NativeLifecycleTest.reference_response(self,'/api/accounts');right=self.request('/api/accounts')
        self.assertIsNone(difference(left,right),(left['body'],right['body']));return json.loads(right['body'])
    def archive(self,items,name='synthetic.zip',broken=False):
        f=self.root/name
        with zipfile.ZipFile(f,'w') as z:
            z.writestr('synthetic/data/following.js','window.YTD.following.part0 = '+('broken' if broken else json.dumps([{'following':{'accountId':i}} for i in items])))
            z.writestr('synthetic/data/direct-messages.js','must never be opened')
        return f
    def test_cookie_bind_check_expire_reconnect_unbind(self):
        self.pair('pixiv',{'value':'other=x; PHPSESSID=123_AbC; more=y'})
        if os.name != 'nt':  # Windows 另测 DPAPI；POSIX 文件保持 0600
            self.assertEqual(stat.S_IMODE((self.support/'accounts/pixiv').stat().st_mode),0o600)
        self.status_pair();self.pair('pixiv/check')
        self.service.rejected=True;self.pair('pixiv/check');self.assertIn('expired',self.status_pair()['pixiv'])
        self.service.rejected=False;self.pair('pixiv/check');self.assertNotIn('expired',self.status_pair()['pixiv'])
        self.pair('pixiv/unbind');self.assertFalse(self.status_pair()['pixiv']['bound']);self.pair('pixiv/check',expected=400)
        self.log.seek(0);self.assertNotIn(b'123_AbC',self.log.read())
    def test_saucenao_global_secret_and_unbind(self):
        self.pair('saucenao',{'value':' A'+'b'*39+' '});self.status_pair()
        other=self.package('second');self.request('/api/libraries/open',{'path':str(other.path)})
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertTrue(json.loads(self.request('/api/accounts')['body'])['saucenao']['bound'])
        self.assertNotIn(('A'+'b'*39).encode(),self.request('/api/accounts')['body'])
        self.request('/api/accounts/saucenao/unbind',{});self.assertFalse((self.support/('accounts/saucenao.dpapi' if os.name == 'nt' else 'accounts/saucenao')).exists())
    @unittest.skipUnless(os.name == 'nt', 'Windows DPAPI unavailable on POSIX')
    def test_windows_dpapi_restart_replace_corruption_and_legacy_ignored(self):
        directory=self.support/'accounts';directory.mkdir(parents=True,exist_ok=True)
        legacy=directory/'saucenao';legacy.write_text('legacy-synthetic-only')
        self.assertFalse(json.loads(self.request('/api/accounts')['body'])['saucenao']['bound'])
        secret='a'*40
        self.assertEqual(self.request('/api/accounts/saucenao',{'value':secret})['status'],200)
        protected=directory/'saucenao.dpapi'
        self.assertNotIn(secret.encode(),protected.read_bytes())
        self.assertEqual(legacy.read_text(),'legacy-synthetic-only')
        self.stop(self.process);self.process,self.log=self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertTrue(json.loads(self.request('/api/accounts')['body'])['saucenao']['bound'])
        before=protected.read_bytes()
        self.assertEqual(self.request('/api/accounts/saucenao',{'value':'b'*40})['status'],200)
        self.assertNotEqual(before,protected.read_bytes())
        protected.write_bytes(b'synthetic damaged blob')
        self.assertEqual(self.request('/api/accounts')['status'],400)
        self.assertEqual(self.request('/api/accounts/saucenao/unbind',{})['status'],200)
        self.assertFalse(protected.exists());self.assertEqual(legacy.read_text(),'legacy-synthetic-only')
        self.assertEqual(self.service.calls,[])

    def test_invalid_branches_and_desktop_absent(self):
        for action,body in [('pixiv',{'value':'bad'}),('saucenao',{'value':'bad'}),('pixiv/login',{}),('pixiv/connect',{}),('pixiv/check',{}),('unknown',{})]:self.pair(action,body,400)
        self.assertFalse((self.support/'accounts.json').exists())
    def test_archive_duplicates_and_typed_database(self):
        f=self.archive(['2','1','2','3']);self.assertEqual(self.pair('x/import',{'path':str(f)})['new'],3)
        self.assertEqual(labels.snapshot(provenance_db(self.lib)),self.rec.value('reference provenance'))
        self.assertEqual(self.pair('x/import',{'path':str(f)})['new'],0);self.status_pair()
        directory=self.root/'unzipped';(directory/'data').mkdir(parents=True)
        (directory/'data/following.js').write_text('window.YTD.following.part0 = [{"following":{"accountId":"4"}}]')
        self.assertEqual(self.pair('x/import',{'path':str(directory)})['new'],1)
        self.assertEqual(labels.snapshot(provenance_db(self.lib)),self.rec.value('reference provenance'))

        before = labels.snapshot(provenance_db(self.lib))
        oversized = self.root / 'oversized-following.zip'
        with zipfile.ZipFile(oversized, 'w', compression=zipfile.ZIP_DEFLATED) as z:
            with z.open('data/following.js', 'w') as entry:
                entry.write(b'window.YTD.following.part0 = []')
                chunk = b' ' * (1 << 20)
                for _ in range(64):
                    entry.write(chunk)
        response = self.request('/api/accounts/x/import', {'path': str(oversized)})
        self.assertEqual(response['status'], 400, response)
        self.assertIn('关注列表超过限额', json.loads(response['body'])['error'])
        self.assertEqual(labels.snapshot(provenance_db(self.lib)), before)

    def test_archive_corruption_and_rollback(self):
        before=labels.snapshot(provenance_db(self.lib))
        self.pair('x/import',{'path':str(self.archive([],broken=True))},400)
        self.pair('x/import',{'path':str(self.root/'missing.zip')},400)
        f=self.root/'missing-following.zip'
        with zipfile.ZipFile(f,'w') as z:z.writestr('data/tweets.js','unused')
        self.pair('x/import',{'path':str(f)},400)
        with closing(sqlite3.connect(provenance_db(self.lib))) as c:c.execute("CREATE TRIGGER fail_lead BEFORE INSERT ON leads WHEN NEW.key='2' BEGIN SELECT RAISE(ABORT,'synthetic'); END")
        result=self.request('/api/accounts/x/import',{'path':str(self.archive(['1','2']))});self.assertEqual(result['status'],400)
        with closing(sqlite3.connect(provenance_db(self.lib))) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM leads').fetchone()[0],0);c.execute('DROP TRIGGER fail_lead')
        self.assertEqual(before,labels.snapshot(provenance_db(self.lib)))
    def test_archive_switch_package_isolation(self):
        f=self.archive(['1','2']);self.request('/api/accounts/x/import',{'path':str(f)})
        other=self.package('second')
        other.config['id']=self.lib.config['id'];(other.path/'library.json').write_text(json.dumps(other.config))
        self.request('/api/libraries/open',{'path':str(other.path)})
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token'];self.assertEqual(json.loads(self.request('/api/accounts')['body'])['x'],{'imported':0})
        self.assertEqual(json.loads(self.request('/api/accounts/x/import',{'path':str(f)})['body'])['new'],2)
        with closing(sqlite3.connect(other.path/'provenance.sqlite')) as c:self.assertEqual(c.execute('PRAGMA user_version').fetchone()[0],5)
        self.assertEqual(labels.snapshot(other.path/'provenance.sqlite')['schema'],self.rec.value('fresh schema'))
    def test_network_failure_does_not_mark_expired(self):
        self.pair('pixiv',{'value':'123_AbC'});before=self.metadata(self.support)
        self.service.code=503;self.pair('pixiv/check',expected=400)
        self.assertEqual(before,self.metadata(self.support));self.assertNotIn('expired',self.metadata(self.support)['pixiv'])
    def test_concurrent_repeated_import_once(self):
        f=self.archive(['1','2'])
        with ThreadPoolExecutor(4) as pool:results=list(pool.map(lambda _:self.request('/api/accounts/x/import',{'path':str(f)}),range(4)))
        self.assertEqual(sorted(json.loads(r['body'])['new'] for r in results),[0,0,0,2])
    def test_saving_key_does_not_query_network(self):
        self.pair('saucenao',{'value':'a'*40});self.assertEqual(self.service.calls,[])

    def test_synthetic_file_secret_absent_from_responses(self):
        # 原生钥匙串的兼容性在 platform_credentials_macos 里用显式隔离的句柄测试；
        # 这个 HTTP 测试保持 NO_KEYCHAIN=1。
        # SauceNAO 只接受 40 位十六进制数；把一个显式的合成标记编码进去。
        value=b'SYNTHETIC-119-ONLY'.hex().ljust(40,'0')
        self.pair('saucenao',{'value':value})
        self.assertNotIn(value.encode(),self.request('/api/accounts')['body'])
        self.pair('saucenao/unbind')
        self.assertFalse((self.support/'accounts/saucenao').exists())
        self.assertFalse((self.support/'accounts/saucenao.dpapi').exists())

    def test_validation_retries_transient_response_and_keeps_request_spacing(self):
        self.service.failures_left=1
        started=time.monotonic();reply=self.request('/api/accounts/pixiv',{'value':'123_AbC'})
        self.assertEqual(reply['status'],200,reply['body']);self.assertGreaterEqual(time.monotonic()-started,3.9)
        self.assertEqual(len([c for c in self.service.calls if c[1].startswith('/ajax/')]),3)
        if os.name == 'nt':
            self.assertNotIn(b'123_AbC',(self.support/'accounts/pixiv.dpapi').read_bytes())
            self.assertFalse((self.support/'accounts/pixiv').exists())
        else:
            self.assertEqual((self.support/'accounts/pixiv').read_text(),'123_AbC')

class NativeDesktopAccountsTest(NativeAccountsTest):
    with_bridge=True
    # 继承来的原生账号契约用例也在回环桥接下运行。
    def test_invalid_branches_and_desktop_absent(self):
        for action,body in [('pixiv',{'value':'bad'}),('saucenao',{'value':'bad'}),('pixiv/check',{}),('unknown',{})]:self.pair(action,body,400)
    def test_window_connect_logout_and_cancelled_login(self):
        self.service.logged=False;self.service.login_open=False
        self.pair('pixiv/login');self.pair('pixiv/connect');self.assertFalse(self.status_pair()['pixiv']['bound'])
        self.service.login_open=False;self.pair('pixiv/connect')
        self.service.logged=True;self.pair('pixiv/connect');self.assertEqual(self.status_pair()['pixiv']['via'],'app')
        self.pair('pixiv/check');self.pair('pixiv/unbind');self.assertFalse(self.status_pair()['pixiv']['bound'])
        for method,path,_,token in self.service.calls:
            if path.startswith('/pixiv/'):self.assertEqual(token,'synthetic-bridge-token')
    def test_window_expired_and_force_relogin(self):
        self.pair('pixiv/connect');self.service.logged=False
        self.pair('pixiv/check');self.status_pair();self.pair('pixiv/login')
        self.assertTrue(any('/pixiv/login?force=1' in c[1] for c in self.service.calls))
        self.service.logged=True;self.pair('pixiv/connect');self.assertNotIn('expired',self.status_pair()['pixiv'])
    def test_window_replaces_cookie(self):
        self.pair('pixiv',{'value':'123_AbC'});self.pair('pixiv/connect')
        self.assertFalse((self.support/'accounts/pixiv').exists());self.assertEqual(self.status_pair()['pixiv']['via'],'app')

    def test_window_expired_rejection_detail(self):
        self.pair('pixiv/connect');self.service.rejected=True
        self.pair('pixiv/check');self.status_pair()
    def test_window_open_fetch_failure_is_pending(self):
        self.service.login_open=True;self.service.fetch_status=0
        self.pair('pixiv/connect');self.assertFalse(self.status_pair()['pixiv']['bound'])
    def test_inflight_account_switch_waits_for_original_request(self):
        self.service.block=True;other=self.package('second')
        with ThreadPoolExecutor(2) as pool:
            action=pool.submit(self.request,'/api/accounts/pixiv',{'value':'123_AbC'})
            self.assertTrue(self.service.entered.wait(WAIT))
            switch=pool.submit(self.request,'/api/libraries/open',{'path':str(other.path)})
            time.sleep(.1);self.assertFalse(switch.done());self.service.release.set()
            self.assertEqual(action.result()['status'],200);self.assertEqual(switch.result()['status'],200)
        stale=self.request('/api/accounts/x/import',{'path':str(self.archive(['1']))})
        self.assertEqual(stale['status'],403);self.assertFalse((other.path/'provenance.sqlite').exists())
    def test_exit_waits_for_account_request_and_restarts(self):
        import signal
        self.service.block=True
        with ThreadPoolExecutor(1) as pool:
            action=pool.submit(self.request,'/api/accounts/pixiv',{'value':'123_AbC'})
            self.assertTrue(self.service.entered.wait(WAIT));self.process.send_signal(INTERRUPT)
            time.sleep(.1);self.assertIsNone(self.process.poll());self.service.release.set()
            self.assertEqual(action.result()['status'],200);self.process.wait(WAIT)
        self.process,self.log=self.start(self.lib.path)
        self.assertTrue(json.loads(self.request('/api/accounts')['body'])['pixiv']['bound'])

    def test_window_404_marks_expired_with_original_detail(self):
        self.pair('pixiv/connect');self.service.fetch_status=404
        self.pair('pixiv/check');self.status_pair()
