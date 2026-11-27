"""R4 状态/原始 HTTP 验收；所有可变输入都是临时的合成夹具。"""
import json
import os
import subprocess
import unittest
from urllib.parse import urlencode
from tests.read_http_harness import HTTPHarness

CORE = {'characters', 'tagged', 'character/profile', 'character/activity', 'character/stats',
        'identity/count', 'identity/queue',
        }

class CharacterReadHTTPTest(HTTPHarness):
    # 本类复用同一个只读宿主，接口字节按扩充造型译名前的录制比较。
    host_env = {'ILLUSTASH_TEST_LEGACY_SKIN_NAMES': '1'}

    def test_directory_and_identity_corpus(self):
        requests = self.corpus(sample=6, query_limit=30,
                               keep=lambda r: r['stage'] == 'R4' and r['url'].split('?', 1)[0].removeprefix('/api/') in CORE)
        report = self.compare(requests, require_core=['R4'])
        self.assertEqual(report['differ'], 0, report['differences'])
        self.assertEqual(report['compared'], report['core_equal'])
        self.assertGreater(report['core_equal'], 100)
        self.assertTrue(json.loads(self.pair('/api/characters')['body']))

    def test_error_and_query_parameters(self):
        for path, key in [('characters', 'min'),
                          ('identity/queue', 'offset')]:
            for n in ['bad', '1.2', '-1', '0', '201', ' １_０ ', '\x1c2\x1f', '9'*100]:
                self.pair('/api/' + path + '?' + urlencode({key:n}))
        for path in ['characters', 'tagged', 'character/activity', 'character/stats', 'identity/count', 'identity/queue']:
            self.pair('/api/' + path + '?nsfw=bad')
        for query in ['min=bad&min=', 'min=bad&min=0', 'q=Alice&q=&q=雪', 'q=%FF%FE', 'q=+%1C+']:
            self.pair('/api/characters?' + query)
        self.pair('/api/character/profile?entity=no-such')
        self.pair('/api/characters?work=no-such')

    def test_eager_errors_do_not_read_db(self):
        cases = ['/api/characters?min=-1',
                  '/api/identity/queue?limit=0',
                 '/api/identity/queue?nsfw=bad', '/api/identity/count?nsfw=bad',
                  '/api/character/stats',
                 '/api/character/stats?id=a&nsfw=bad']
        missing = self.root/'never-created.sqlite'
        self.publish()
        for path in cases:
            self.assertEqual(self.pair(path)['status'], 400)
        self.assertFalse(missing.exists())
    def test_nonempty_identity_queue_first_hit_paging_and_detail(self):
        root = self.root/'r4-hybrid'; root.mkdir(exist_ok=True)
        self.sync('hybrid'); self.publish()
        with self.connect() as c:
            ids = [r[0] for r in c.execute("SELECT entity,count(*) FROM memberships GROUP BY entity ORDER BY count(*) DESC")]
        self.assertGreaterEqual(len(ids), 2)
        a, b = ids[:2]
        self.sync('proposed')
        for path in ['identity/queue', 'identity/count', 'detail?id=a']:
            self.pair('/api/' + path)
            for query in ['limit=1&offset=0', 'limit=1&offset=1', 'limit=1&offset=1000']:
                self.pair('/api/' + path + '?' + query)
        for nsfw in ['', 'hide', 'only']:
            self.pair('/api/identity/queue?' + urlencode({'nsfw':nsfw}))
            self.pair('/api/identity/count?' + urlencode({'nsfw':nsfw}))
        queue = json.loads(self.pair('/api/identity/queue')['body'])
        self.assertEqual(queue['total'], 1)
        self.assertEqual(queue['items'][0]['distance'], 0.11)
        self.assertEqual(queue['items'][0]['hint']['subject'], 'first_tag')
        self.assertEqual(queue['items'][0]['a']['id'], a)
        for e in (a,b):
            for summary in ('0','1'):
                self.pair('/api/characters?' + urlencode({'id':e,'summary':summary}))
            self.pair('/api/character/profile?' + urlencode({'entity':e}))
            self.pair('/api/character/stats?' + urlencode({'id':e,'exclude':'a'}))
        covers={r['id']:r['cover'] for r in json.loads(self.pair('/api/characters')['body'])}
        self.assertEqual(covers[a], 'a')  # 单人且已确认的图片优先于共用的机器识别图片。
        self.assertEqual(covers[b], 'b')
        self.assertNotEqual(covers[a], covers[b])
        self.sync('mascot')
        tagged=json.loads(self.pair('/api/tagged')['body'])
        self.assertTrue(any(r['id']==b and r['count'] for r in tagged))
        self.pair('/api/characters?'+urlencode({'id':b}))
        self.sync('profile')
        profile=json.loads(self.pair('/api/character/profile?'+urlencode({'entity':a}))['body'])
        self.assertEqual(profile['user_name'], '合成用户名字')
        self.assertEqual(profile['aliases'], ['后插入','先排序'])
        for uuid in ('a','b','c'):
            self.pair('/api/detail?id='+uuid)
        self.sync('skipped')
        self.assertEqual(json.loads(self.pair('/api/identity/count')['body'])['total'],0)
        self.sync('decided')
        self.pair('/api/identity/count')
        self.sync('uncertain')
        response = json.loads(self.pair('/api/detail?id=a')['body'])['identity']
        self.assertTrue(response['undetermined'])

    def test_unset_policy_legacy_names_stable_ties_and_cover_reuse(self):
        # 在 r4-legacy 上录制，它是切换到 core 之前的图库（没有 `meta`；从 core v8 起被拒绝，#201，
        # 见 `HTTPHarness.after_switch`）。同一个包的根图库就是切换后的这个图库：两者共有的每张表里行都相同，
        # 按当前 core 版本恢复。所以 r4-legacy 上录制的回复与根图库的实时回复比较。
        (self.root/'r4-legacy').mkdir(exist_ok=True)
        self.sync('legacy'); self.publish(db=str(self.root/'app.sqlite'), source=str(self.root/'source.db'))
        live = lambda path: self.live(self.rec.value('response'), '/api/' + path)
        for path in ['characters', 'characters?summary=1', 'tagged', 'identity/queue', 'identity/count', 'detail?id=a']:
            live(path)
        # 绕过 core 写入名称，与录制时在 r4-legacy 上写的一样：按 id 排在前三个的角色（以前的查询经
        # `entities_parent` 索引读到它们）。
        with self.connect() as c:
            ids=[r[0] for r in c.execute("SELECT t.key FROM term_tree r JOIN terms t ON t.id=r.term_id WHERE t.kind='character' AND r.parent_id IS NULL AND r.merged_into IS NULL ORDER BY t.key LIMIT 3")]
            for e in ids:
                rename(c, e, '平局')
            c.execute("DELETE FROM meta WHERE key='identity_policy'")
            c.commit()
        # 切换前的图库没有显示名投影：读取方每次读取时自己挑名称。切换后的图库，读取方读投影，core 在每次写入时
        # 维护它；绕过 core 写入的名称，要等 core 从名称表重建投影（`names-project` 所做的）才会进去。
        terms = os.environ.get('ILLUSTASH_TERMS_BIN')
        if not terms:
            raise unittest.SkipTest('set ILLUSTASH_TERMS_BIN: the projection is rebuilt with the core command')
        subprocess.run([terms, 'names-project', '--db', str(self.db)], check=True, capture_output=True,
                       env={**os.environ, **self.host_env})
        for path in ['characters', 'characters?summary=1', 'detail?id=a']:
            live(path)
        for e in ids:
            live('characters?' + urlencode({'id':e}))
            live('character/profile?' + urlencode({'entity':e}))


def rename(c, entity, name):
    """不经过 core，把一个身份的名称换成一个首选的用户名称。"""
    c.execute("DELETE FROM term_names WHERE term_id=(SELECT id FROM terms WHERE key=?)",(entity,))
    c.execute("INSERT INTO term_names(term_id,lang,value,source,role,preferred,position,seq) VALUES "
              "((SELECT id FROM terms WHERE key=?),'zh',?,'user','display',1,0,(SELECT coalesce(max(seq),0)+1 FROM term_names))",(entity,name))


if __name__ == '__main__':
    unittest.main()
