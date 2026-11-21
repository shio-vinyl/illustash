"""#41 原生本地查询：只用用完即弃的合成图库包和原有的字节契约。

原 Python 参照的响应和它建出的图库状态通过 tests/fixed_expectations.py 录制（#9）。
"""
from contextlib import closing
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import sqlite3
import unittest
from unittest.mock import patch
from urllib.parse import urlencode

from tests import test_native_human_labels as labels, test_native_human_curation as curation
from tests.fixed_expectations import unclocked
from tests.test_native_human_labels import difference
from tools.verify_read_http import cookie, fetch
from tools.wait_limit import WAIT


class NativeLocalReadsTest(unittest.TestCase):
    setUpClass = classmethod(curation.NativeHumanCurationTest.setUpClass.__func__)
    clone = labels.NativeHumanLabelsTest.clone
    stop = labels.NativeHumanLabelsTest.stop
    seed_both = labels.NativeHumanLabelsTest.seed_both
    pair = curation.NativeHumanCurationTest.pair
    latest_action = labels.NativeHumanLabelsTest.latest_action
    reference_response = labels.NativeHumanLabelsTest.reference_response

    def setUp(self):
        labels.NativeHumanLabelsTest.setUp(self)
        self.env.pop('ILLUSTASH_DESKTOP_BRIDGE', None)
        self.env.pop('ILLUSTASH_DESKTOP_TOKEN', None)
        self.seed_both()

    def start(self, lib):
        self.env['ILLUSTASH_NO_KEYCHAIN'] = '1'
        return labels.NativeHumanLabelsTest.start(self, lib)

    def get_pair(self,url,expected=200):
        before=[self.rec.state('before'),labels.states(self.right)]
        responses=[self.reference_response('response',{'url':url}),fetch(self.origin,{'url':url},self.origin)]
        for r in responses:self.assertEqual(r['status'],expected,(url,r['body']))
        self.assertEqual(responses[1]['route'],'core')
        if expected<400:
            if url=='/api/names':
                # #1 只扩展了 GET /api/names。冻结的参照字节保持原样，再补上这个夹具确切的未授权状态；
                # 比较完整的原生响应，包括新字段。
                self.assertTrue(responses[0]['body'].endswith(b'}'))
                responses[0]['body']=(responses[0]['body'][:-1]
                                      + b', "network": {"authorized": false, "granted_at": null, "this_run": false}}')
                responses[0]['headers']['Content-Length']=str(len(responses[0]['body']))
            self.assertIsNone(difference(*responses),(url,difference(*responses)))
            for h in labels.HEADERS:self.assertEqual(responses[0]['headers'].get(h),responses[1]['headers'].get(h),h)
        self.assertEqual(before,[self.rec.state('after'),labels.states(self.right)])
        self.assertFalse(self.sentinel.exists())
        return json.loads(responses[1]['body']) if expected<400 else None

    def test_retired_routes_are_unavailable_without_mutating_library(self):
        before=labels.states(self.right)
        for route in ('prefs','identity/status','identity/relations','identity/pairs','identity/assignments'):
            response=fetch(self.origin,{'url':'/api/'+route},self.origin)
            self.assertEqual(response['status'],400,(route,response['body']))
        for route in ('prefs','correct','character-work','identity/uncertain'):
            response=fetch(self.origin,{'url':'/api/'+route,'method':'POST','body':'{}','headers':{'Content-Type':'application/json','X-CSRF-Token':self.token}},self.origin)
            self.assertEqual(response['status'],502,(route,response['body']))
        self.assertEqual(labels.states(self.right),before)
        self.assertFalse(self.sentinel.exists())

    def ask(self,query='',kind=None,entity=None,allowed=None):
        before=labels.states(self.right)
        facts=self.rec.value('entity facts') if kind is not None else None
        item={'url':'/api/ask'+('?' + query if query else '')}
        for side in ('reference','native'):
            r=self.reference_response('ask',item) if side=='reference' else fetch(self.origin,item,self.origin)
            self.assertEqual(r['status'],200,r['body'])
            if side=='native':self.assertEqual(r['route'],'core')
            batch=json.loads(r['body'])['batch']
            if kind is None:self.assertIsNone(batch);continue
            self.assertEqual(batch['kind'],kind)
            self.assertEqual(batch['entity'],entity)
            self.assertEqual(batch['name'],facts['name'])
            self.assertEqual(batch['confirmed'],facts['confirmed'])
            valid={u:tuple(v) for u,v in facts['valid'].items()}
            ids=[x['uuid'] for x in batch['items']]
            self.assertEqual(len(ids),len(set(ids)));self.assertTrue(0<len(ids)<=6)
            self.assertLessEqual(set(ids),set(allowed))
            for x in batch['items']:self.assertEqual((x['width'],x['height']),valid[x['uuid']])
            if side=='reference':recorded=batch
        self.assertEqual(before,labels.states(self.right))
        # 每批都是随机抽样：测试接着用录制的参照那一批。
        return recorded if kind is not None else batch

    def test_seed_sample_skip_later_visibility_and_answer_loop(self):
        eligible={f'SYN{i:02}' for i in range(20)}
        batch=self.ask(kind='seed',entity=self.alice,allowed=eligible)
        self.ask(urlencode({'skip':self.alice}))
        self.get_pair('/api/ask?'+urlencode({'skip':','.join(['x']*51)}),400)
        self.get_pair('/api/ask?nsfw=wrong',400)
        chosen=batch['items'][0]['uuid']
        self.pair('/api/ask/answer',{'entity':self.alice,'yes':[chosen]})
        self.ask(kind='seed',entity=self.alice,allowed=eligible-{chosen})
        seeds={chosen,*sorted(eligible-{chosen})[:4]}
        self.seed_both()
        self.ask(kind='sample',entity=self.alice,allowed=eligible-seeds)
        later=sorted(eligible-seeds)[0];excluded=sorted(eligible-seeds)[1]
        self.seed_both()
        # 录制的「稍后」回答是相对时间的输入，不是固定日期。
        # 保持冻结的输出不变，同时测试还没到期的七天暂缓。
        with closing(sqlite3.connect(self.right.db)) as c, c:
            c.execute("UPDATE asked SET asked_at=datetime('now') WHERE answer='later'")
        self.ask()
        self.seed_both()
        with closing(sqlite3.connect(self.right.db)) as c, c:
            c.execute("UPDATE asked SET asked_at=datetime('now','-8 days') WHERE answer='later'")
        self.ask(kind='sample',entity=self.alice,allowed=eligible-seeds-{later,excluded})

    def test_hints_first_score_order_skip_and_nsfw(self):
        self.seed_both()
        batch=self.ask(kind='hint',entity=self.bob,allowed={'SYN00','SYN01','SYN02'})
        self.assertEqual([x['uuid'] for x in batch['items']],['SYN00','SYN01','SYN02'])
        self.ask(urlencode({'skip':self.bob}),kind='seed',entity=self.alice,allowed={f'SYN{i:02}' for i in range(20)})
        self.ask('nsfw=only')
        self.ask('nsfw=hide',kind='hint',entity=self.bob,allowed={'SYN00','SYN01','SYN02'})

    def test_suggest_ids_filters_rejections_order_and_errors(self):
        for body in ({'ids':['b']},{'ids':['a','a','b','c']},{'filter':{}},{'filter':{},'exclude':['a']},
                     {'ids':['SYN00']*2001},{'filter':{'entity':self.alice}},{'filter':{'q':'synthetic'}}):
            self.pair('/api/suggest',body)
        for body in ({},{'ids':[]},{'ids':['missing']},{'ids':[1]},{'ids':'b'},{'filter':[]},
                     {'filter':{'unknown':'x'}},{'ids':['b'],'filter':{}},{'ids':['b'],'exclude':['a']},{'ids':['b'],'exclude':[1]}):
            self.pair('/api/suggest',body,expected=400)
        self.seed_both()
        self.assertNotIn(self.bob,[r['id'] for r in self.pair('/api/suggest',{'ids':['b']})])

    def test_cached_groups_clusters_sync_and_worker_off(self):
        self.seed_both()
        for url in ['/api/similar','/api/similar?kind=duplicate','/api/clusters','/api/sync','/api/index',
                    '/api/index?progress=1','/api/provenance','/api/names','/api/names?progress=1']:
            if url=='/api/provenance':
                # #51 起即使没有计算 worker 也会提供协调器，这里和其他路由一样直接对照；
                # 原来为字节契约接入的真实空闲 Python 协调器随旧参考一起删除了（#9）。
                pass
            result=self.get_pair(url)
            if url=='/api/similar':self.assertTrue(result['groups'])
            if url=='/api/clusters':self.assertTrue(result['clusters'])
        for url in ['/api/similar?kind=wrong','/api/similar?nsfw=wrong','/api/clusters?nsfw=wrong','/api/oc/groups?entity='+self.alice]:self.get_pair(url,400)

    def test_names_configuration_coverage_and_accounts(self):
        self.seed_both()
        result=self.get_pair('/api/names');self.assertEqual(result['model_api']['hint'],'1234')
        with closing(sqlite3.connect(self.right.db)) as c:
            key_rows = dict(c.execute("SELECT key,value FROM name_settings WHERE key IN "
                                      "('deepseek_key','model_key','model_credential')"))
        self.assertEqual(set(key_rows), {'model_credential'})
        self.assertRegex(key_rows['model_credential'], r'^[0-9a-f]{32}$')
        support=self.root/'support';(support/'accounts').mkdir(exist_ok=True)
        (support/'accounts'/'saucenao').write_text('synthetic-placeholder')
        if os.name == 'nt':
            # 通过本地 API 写入原生的 DPAPI 格式；参照仍读现有的合成明文文件。
            response = fetch(self.origin, {
                'url': '/api/accounts/saucenao', 'method': 'POST',
                'body': json.dumps({'value': 'a' * 40}).encode('utf-8'),
                'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': self.token},
            }, self.origin)
            self.assertEqual(response['status'], 200, response['body'])
            self.assertTrue((support/'accounts'/'saucenao.dpapi').is_file())
        (support/'accounts.json').write_text(json.dumps({'pixiv':{'via':'app','name':'Synthetic User'},'saucenao':{'checked_at':'2026-10-01'}}))
        result=self.get_pair('/api/accounts');self.assertTrue(result['pixiv']['bound']);self.assertTrue(result['saucenao']['bound'])
        self.assertFalse(result['desktop'])

    def test_names_coverage_link_fallbacks(self):
        for system in ('photos_album', 'ccip', 'ccip_base'):
            with self.subTest(system=system):
                self.seed_both()
                self.get_pair('/api/names')

    def test_about_reports_compiled_host_without_python(self):
        r=fetch(self.origin,{'url':'/api/about'},self.origin)
        self.assertEqual((r['status'],r['route']),(200,'core'))
        info=json.loads(r['body'])
        # 版本号以 application.json 为准（#189），升版本号时不用改这里。
        application=json.loads((Path(__file__).resolve().parents[1]/'native/terms/application.json').read_text(encoding='utf-8'))
        self.assertEqual((info['name'],info['version'],info['channel'],info['schema']),('Illustash',application['version'],'预览版',36))
        self.assertIsNone(info['python']);self.assertTrue(info['runtime'].startswith('rustc '))
        self.assertRegex(info['build'],r'^[0-9a-f]{7,40}(?:-dirty)?$');self.assertIn('(',info['platform'])
        # #148：worker 会用到的识别组件，在 worker 查找的位置找到；只检查是否存在。
        library = 'onnxruntime.dll' if os.name == 'nt' else 'libonnxruntime.dylib'
        runtime = Path(os.environ.get('ILLUSTASH_ORT_DYLIB') or self.resources / 'onnxruntime' / library)
        worker = os.environ.get('ILLUSTASH_RECOGNITION_WORKER')
        self.assertEqual(info['libraries'], {'识别组件': '可用' if worker and Path(worker).is_file() else None,
                                             'ONNX Runtime': '已安装' if runtime.is_file() else None})
        self.assertFalse(self.sentinel.exists())

    def test_concurrent_cached_reads_and_suggest_then_switch(self):
        urls=['/api/similar','/api/clusters','/api/sync','/api/names','/api/about','/api/ask']*4
        before=labels.states(self.right)
        with ThreadPoolExecutor(max_workers=8) as pool:
            result=list(pool.map(lambda u:fetch(self.origin,{'url':u},self.origin),urls))
        self.assertTrue(all((r['status'],r['route'])==(200,'core') for r in result))
        self.assertEqual(before,labels.states(self.right))
        replacement=self.rec.package('replacement',self.root/'replacement.pilib')
        response=fetch(self.origin,{'url':'/api/libraries/open','method':'POST','body':json.dumps({'path':str(replacement.path)}),
                        'headers':{'Content-Type':'application/json','X-CSRF-Token':self.token}},self.origin)
        self.assertEqual(response['status'],200,response['body'])
        self.assertIsNone(json.loads(fetch(self.origin,{'url':'/api/ask'},self.origin)['body'])['batch'])
        old=fetch(self.origin,{'url':'/api/suggest','method':'POST','body':'{"ids":["b"]}',
                   'headers':{'Content-Type':'application/json','X-CSRF-Token':self.token}},self.origin)
        self.assertEqual(old['status'],403)


    def test_suggest_2000_cap_nine_limit_and_no_copy_expansion(self):
        # a/b 是合成的重复图片；只选 a 时不能带上 b 的备选。
        self.assertEqual(self.pair('/api/suggest',{'ids':['a']}),[])
        targets=[]
        self.seed_both()
        targets=self.rec.value('targets')
        ids=[f'SUG{i:04}' for i in range(2001)]
        result=self.pair('/api/suggest',{'ids':ids})
        self.assertEqual(len(result),9);self.assertEqual(result[0]['count'],2000)
        self.assertNotIn(targets[1],[r['id'] for r in result])
        result=self.pair('/api/suggest',{'ids':list(reversed(ids))})
        self.assertIn(targets[1],[r['id'] for r in result]);self.assertEqual(result[0]['count'],1999)

    def test_partial_suggest_body_cannot_cross_library_switch(self):
        import http.client
        connection=http.client.HTTPConnection('127.0.0.1',self.port,timeout=WAIT)
        body=b'{"ids":["b"]}'
        connection.putrequest('POST','/api/suggest');connection.putheader('Host',self.origin.split('://')[1])
        connection.putheader('Origin',self.origin);connection.putheader('Cookie',cookie(self.origin));connection.putheader('X-CSRF-Token',self.token)
        connection.putheader('Content-Type','application/json');connection.putheader('Content-Length',str(len(body)))
        connection.endheaders();connection.send(body[:3])
        replacement=self.clone(self.left,'partial-replacement');before=labels.states(replacement)
        try:
            opened=fetch(self.origin,{'url':'/api/libraries/open','method':'POST','body':json.dumps({'path':str(replacement.path)}),
                         'headers':{'Content-Type':'application/json','X-CSRF-Token':self.token}},self.origin)
            self.assertEqual(opened['status'],200,opened['body'])
            connection.send(body[3:]);response=connection.getresponse();response.read();self.assertEqual(response.status,403)
            self.assertEqual(before,labels.states(replacement))
        finally:connection.close()

    def test_hint_top_twelve_pool_after_skip(self):
        ids=[]
        self.seed_both()
        ids=self.rec.value('ids')
        for side in ('reference','native'):
            for _ in range(24):
                r=self.reference_response('ask',{'url':'/api/ask'}) if side=='reference' else fetch(self.origin,{'url':'/api/ask'},self.origin)
                self.assertEqual(r['status'],200)
                batch=json.loads(r['body'])['batch'];self.assertEqual(batch['kind'],'hint')
                self.assertIn(batch['entity'],ids[1:]);self.assertLessEqual(len(batch['items']),6)
        self.ask(urlencode({'skip':','.join(ids[1:])}),kind='hint',entity=ids[0],allowed={'SYN00','SYN01'})

    def test_read_sql_failures_status_only_and_no_partial_data(self):
        for lib in (self.left,self.right):
            with closing(sqlite3.connect(lib.db)) as c:c.execute('DROP TABLE sync_runs');c.commit()
        self.get_pair('/api/sync',500)
        self.pair('/api/suggest',{'ids':['b']},headers={'X-CSRF-Token':'bad'},expected=403)
        self.pair('/api/suggest',{'ids':['b']},headers={'Origin':'https://synthetic.invalid'},expected=403)
