"""「可能认错」的原生 HTTP 契约；只用临时合成图库。"""
from contextlib import closing
import json
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest
from urllib.parse import urlencode

from tests import test_native_lifecycle as lifecycle
from tests.fixed_expectations import Library
from tests.synthetic_source import synthetic_reference
from tests.test_read_http import free_port
from tools.dev_preview import isolated_env
from tools.wait_limit import WAIT


class NativeAmbiguousClaimsTest(unittest.TestCase):
    setUpClass = lifecycle.NativeLifecycleTest.__dict__['setUpClass']
    start = lifecycle.NativeLifecycleTest.start
    stop = lifecycle.NativeLifecycleTest.stop
    request = lifecycle.NativeLifecycleTest.request

    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='illustash-ambiguous-')
        self.addCleanup(temp.cleanup)
        root = Path(temp.name).resolve()
        env = isolated_env(root)
        self.env = {**env, 'USERPROFILE': env['HOME'], 'PATH': ''}
        folder = root / 'images'
        folder.mkdir()
        reference = synthetic_reference(root / 'shared')
        dest = root / 'synthetic.pilib'
        result = subprocess.run(
            [str(self.binary), 'library-create', '--dest', str(dest), '--folder', str(folder),
             '--reference', str(reference)],
            env=self.env, capture_output=True, timeout=WAIT,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode(errors='replace'))
        self.lib = Library(dest)
        self.port = free_port()
        self.origin = f'http://127.0.0.1:{self.port}'
        self.start(dest)
        self.token = self.read('/api/bootstrap')['token']
        with self.connect() as c:
            ids = dict(c.execute("SELECT l.key,t.key FROM term_links l JOIN terms t ON t.id=l.term_id "
                                 "WHERE l.system='ccip' AND l.current=1"))
        self.alice, self.bob = ids['alice'], ids['bob']

    def connect(self):
        return closing(sqlite3.connect(self.lib.db))

    def read(self, url, body=None, status=200):
        reply = self.request(url, body)
        self.assertEqual(reply['status'], status, reply['body'])
        return json.loads(reply['body'])

    def seed(self, c, uuid, date, *, reverse=False):
        c.execute("INSERT INTO meta(key,value) VALUES ('terms_writer','1')")
        entity, rival = (self.bob, self.alice) if reverse else (self.alice, self.bob)
        c.execute("INSERT INTO assets(uuid,added_at) VALUES (?,?)", (uuid, date))
        c.execute("INSERT INTO single_assets VALUES (?)", (uuid,))
        c.executemany("INSERT INTO family_evidence VALUES (?,?,'WD',?,1,0,?,NULL,?)",
                      [(uuid, entity, .99, .99, 1), (uuid, rival, .98, .98, 2)])
        c.execute("INSERT INTO memberships VALUES (?,?,'machine',NULL,'WD',.99,1,0,'synthetic')", (uuid, entity))

        c.execute("DELETE FROM meta WHERE key='terms_writer'")

    def group(self, ids, reverse=False):
        if reverse:
            entity, name, rival, rival_name = self.bob, 'Bob', self.alice, 'Alice'
        else:
            entity, name, rival, rival_name = self.alice, 'Alice', self.bob, 'Bob'
        return {'a': {'id': entity, 'name': name}, 'b': {'id': rival, 'name': rival_name},
                'count': len(ids), 'items': ids}

    def assert_queue(self, groups, nsfw=''):
        self.assertEqual(self.read(f'/api/ambiguous?nsfw={nsfw}'), {'groups': groups})
        pools = self.read(f'/api/review-pools?nsfw={nsfw}')
        self.assertEqual([p for p in pools if p['kind'] == 'ambiguous'],
                         [{'kind': 'ambiguous', 'name': '可能认错',
                           'count': sum(g['count'] for g in groups), 'cover': None}])

    def test_grouped_visible_queue_ratings_and_per_claim_deferrals(self):
        with self.connect() as c, c:
            self.seed(c, 'old', '2026-01-01')
            self.seed(c, 'new', '2026-01-05')
            self.seed(c, 'r18', '2026-01-03')
            self.seed(c, 'reversed', '2026-01-04', reverse=True)
            self.seed(c, 'other-kind', '2026-01-02')
            self.seed(c, 'same-day', '2026-01-02')
            self.seed(c, 'later', '2026-01-07')
            self.seed(c, 'hidden', '2026-01-08')
            self.seed(c, 'excluded', '2026-01-09')
            c.execute("INSERT INTO term_assets(namespace,key) VALUES ('library','r18')")
            c.execute("INSERT INTO terms(key,kind,origin) VALUES ('synthetic-explicit','rating','fixture')")
            c.execute("INSERT OR IGNORE INTO term_links(system,key,term_id,source) "
                      "SELECT 'wd_rating','explicit',id,'fixture' FROM terms WHERE key='synthetic-explicit'")
            c.execute("INSERT INTO term_projection(asset_id,term_id,state) "
                      "SELECT a.id,l.term_id,'machine' FROM term_assets a,term_links l "
                      "WHERE a.namespace='library' AND a.key='r18' AND l.system='wd_rating' AND l.key='explicit'")
            c.execute("INSERT INTO duplicates VALUES ('hidden','g',2,2,'2026-01-08',0,1)")
            c.execute("INSERT INTO excluded VALUES ('excluded','manual','user')")
            c.execute("INSERT INTO asked VALUES ('old',?,'ambiguous','later',NULL,datetime('now','-8 days'))",
                      (self.alice,))
            c.execute("INSERT INTO asked(uuid,entity,kind,answer) VALUES ('other-kind',?,'seed','later')",
                      (self.alice,))
            c.execute("INSERT INTO asked(uuid,entity,kind,answer) VALUES ('new',?,'ambiguous','later')", (self.bob,))
        later = self.read('/api/ask/answer', {'kind': 'ambiguous', 'entity': self.alice, 'later': ['later']})
        self.assertEqual(later, {'undo': None})

        reverse = self.group(['reversed'], reverse=True)
        # 组内新加入的在前，同一入库时间按 uuid 升序。
        self.assert_queue([self.group(['new', 'r18', 'other-kind', 'same-day', 'old']), reverse])
        self.assert_queue([self.group(['new', 'other-kind', 'same-day', 'old']), reverse], 'hide')
        self.assert_queue([self.group(['r18'])], 'only')
        self.read('/api/ambiguous?nsfw=invalid', status=400)
        # 和 /api/assets 默认的「最新」排序同一个次序（那边不排除暂缓的图）。
        listed = self.read('/api/ids?' + urlencode({'entity': self.alice, 'status': 'ambiguous'}))
        queued = self.read('/api/ambiguous')['groups'][0]['items']
        self.assertEqual([uid for uid in listed if uid in queued], queued)

    def test_reject_both_then_undo_once(self):
        with self.connect() as c, c:
            self.seed(c, 'a', '2026-01-02')
            self.seed(c, 'a-copy', '2026-01-02')
            self.seed(c, 'b', '2026-01-01')
            self.seed(c, 'other', '2026-01-03', reverse=True)
            # a-copy 是 a 折叠起来的重复副本：不在清单里，回答时跟着 a 一起改。
            c.executemany("INSERT INTO duplicates VALUES (?,'g',2,?,'2026-01-02',?,?)",
                          [('a', 0, 1, 0), ('a-copy', 1, 0, 1)])
            # 这一步会覆盖的旧问答：A 侧一条过期的「以后再说」、一条别的问题还在暂缓期内的，B 侧一条还在暂缓期内的。
            c.execute("INSERT INTO asked VALUES ('b',?,'ambiguous','later',NULL,datetime('now','-8 days'))",
                      (self.alice,))
            c.execute("INSERT INTO asked(uuid,entity,kind,answer) VALUES ('a',?,'hint','later')", (self.alice,))
            c.execute("INSERT INTO asked(uuid,entity,kind,answer) VALUES ('a',?,'seed','later')", (self.bob,))
            before = self.history(c)
        reverse = self.group(['other'], reverse=True)
        self.assert_queue([self.group(['a', 'b']), reverse])
        body = {'kind': 'ambiguous', 'entity': self.alice, 'also_no': self.bob, 'no': ['a', 'b']}
        refused = [
            ({**body, 'no': ['a', 'other']}, '图片的备选归属已变更，请刷新'),
            ({**body, 'switch': self.bob}, '回答格式不正确'),
            ({**body, 'yes': ['other']}, '回答格式不正确'),
            ({**body, 'later': ['other']}, '回答格式不正确'),
            ({**body, 'kind': 'seed'}, '回答格式不正确'),
            ({**body, 'no': []}, '回答格式不正确'),
            ({**body, 'also_no': self.alice}, '请选择不同的父角色'),
            ({**body, 'also_no': 'missing'}, '角色不存在'),
        ]
        for refusal, error in refused:
            self.assertEqual(self.read('/api/ask/answer', refusal, status=400), {'error': error}, refusal)
        with self.connect() as c, c:
            c.execute(
                "CREATE TRIGGER fail_second_rejection BEFORE INSERT ON asked "
                "WHEN NEW.uuid='b' AND NEW.answer='no' AND EXISTS("
                "SELECT 1 FROM asked WHERE uuid=NEW.uuid AND action_id=NEW.action_id AND entity<>NEW.entity) "
                "BEGIN SELECT RAISE(ABORT,'synthetic rejection failure'); END"
            )
        self.read('/api/ask/answer', body, status=500)
        with self.connect() as c, c:
            c.execute("DROP TRIGGER fail_second_rejection")
            self.assertEqual(self.history(c), before)
            self.assertEqual(c.execute("SELECT count(*) FROM actions").fetchone()[0], 0)
        self.assert_queue([self.group(['a', 'b']), reverse])

        undo = self.read('/api/ask/answer', body)['undo']
        self.assertEqual(undo['label'], '已将 2 张移出 Alice、Bob')
        self.assertEqual(undo['removal'], {'count': '2', 'names': ['Alice', 'Bob']})
        self.assertEqual(self.read('/api/bootstrap')['undo'], undo)
        with self.connect() as c:
            # 隐藏副本跟着否定；问答只记提交的两张。
            rejected = sorted((uid, entity) for uid in ['a', 'a-copy', 'b'] for entity in (self.alice, self.bob))
            answered = sorted((uid, entity) for uid in ['a', 'b'] for entity in (self.alice, self.bob))
            self.assertEqual(c.execute("SELECT uuid,entity,status FROM assertions ORDER BY uuid,entity").fetchall(),
                             [(*pair, 'rejected') for pair in rejected])
            asked = c.execute("SELECT uuid,entity,kind,answer,action_id FROM asked ORDER BY uuid,entity").fetchall()
            self.assertEqual(asked, [(*pair, 'ambiguous', 'no', undo['id']) for pair in answered])
            verdicts = c.execute("SELECT a.key,t.key,v.verdict,v.action_id FROM term_verdicts v "
                                 "JOIN term_assets a ON a.id=v.asset_id JOIN terms t ON t.id=v.term_id "
                                 "ORDER BY a.key,t.key").fetchall()
            self.assertEqual(verdicts, [(*pair, 'no', undo['id']) for pair in rejected])
            self.assertEqual(c.execute("SELECT id,undone FROM actions").fetchall(), [(undo['id'], 0)])
        self.assert_queue([reverse])

        self.read('/api/undo', {'id': undo['id']})
        self.assert_queue([self.group(['a', 'b']), reverse])
        with self.connect() as c:
            # 两侧被覆盖的旧问答原样放回，含问的时间；原来没有问答的（b，B）不留行。
            self.assertEqual(self.history(c), before)
            self.assertEqual(c.execute("SELECT id,undone FROM actions").fetchall(), [(undo['id'], 1)])

    def test_ambiguous_later_does_not_rest_the_character_in_ask(self):
        # 「以后再说」只针对当前这一问：「可能认错」卡上的暂缓不让 A 在认一认里消失，认一认自己的暂缓照旧。
        with self.connect() as c, c:
            for day, uid in enumerate(['one', 'two', 'three', 'four'], 1):
                self.seed(c, uid, f'2026-01-0{day}')
        self.assertEqual(self.ask()['entity'], self.alice)
        self.read('/api/ask/answer', {'kind': 'ambiguous', 'entity': self.alice, 'later': ['one']})
        self.assert_queue([self.group(['four', 'three', 'two'])])

    def test_keep_apart_hides_both_directions_and_future_claims_until_undo(self):
        with self.connect() as c, c:
            self.seed(c, 'front', '2026-01-01')
            self.seed(c, 'back', '2026-01-02', reverse=True)
            before = self.history(c)
        groups = [self.group(['front']), self.group(['back'], reverse=True)]
        self.assert_queue(sorted(groups, key=lambda group: group['a']['id']))
        self.assertEqual(self.read('/api/detail?id=front')['question']['kind'], 'ambiguous')
        undo = self.read('/api/identity/pair', {'a': self.bob, 'b': self.alice, 'answer': 'apart'})['undo']
        self.assertEqual(self.read('/api/identity/pair', {'a': self.alice, 'b': self.bob, 'answer': 'apart'})['undo'],
                         undo, '同一无向角色对只写一步')
        with self.connect() as c, c:
            self.seed(c, 'future', '2026-01-03')
            self.assertEqual(self.history(c), before, '整理偏好不产生结论或训练样本')
        self.assert_queue([])
        for entity in (self.alice, self.bob):
            self.assertEqual(self.read('/api/ids?' + urlencode({'entity': entity, 'status': 'ambiguous'})), [])
        self.assertNotEqual(self.read('/api/detail?id=future')['question']['kind'], 'ambiguous')
        self.read('/api/undo', {'id': undo['id']})
        self.assert_queue([self.group(['future', 'front']), self.group(['back'], reverse=True)])
        self.assertEqual(self.read('/api/detail?id=future')['question']['kind'], 'ambiguous')

    def test_make_form_keeps_pictures_filterable_and_undo_restores_two_characters(self):
        with self.connect() as c, c:
            self.seed(c, 'front', '2026-01-01')
            self.seed(c, 'back', '2026-01-02', reverse=True)
            c.execute("INSERT INTO meta(key,value) VALUES ('terms_writer','1')")
            c.execute("INSERT INTO candidates SELECT uuid,entity,'WD',wd FROM family_evidence")
            c.execute("INSERT INTO assets(uuid,added_at) VALUES ('longtail','2026-01-03')")
            c.execute("INSERT INTO candidates VALUES ('longtail',?,'CCIP_LONGTAIL',.90)", (self.alice,))
            c.execute("DELETE FROM meta WHERE key='terms_writer'")
        undo = self.read('/api/identity/pair', {'a': self.alice, 'b': self.bob, 'answer': 'form'})['undo']
        self.assert_queue([])
        self.assertEqual(self.read('/api/ids?' + urlencode({'entity': self.bob})), ['longtail', 'back', 'front'])
        self.assertEqual(self.read('/api/ids?' + urlencode({'entity': self.bob, 'variant': self.alice})), ['longtail', 'back', 'front'])
        self.read('/api/undo', {'id': undo['id']})
        self.assertEqual(self.read('/api/ids?' + urlencode({'entity': self.bob})), ['back'])
        self.assertEqual(self.read('/api/ids?' + urlencode({'entity': self.alice})), ['longtail', 'front'])

    def test_catalog_pair_is_asked_in_direct_and_hybrid_modes(self):
        # 公共目录中的一对形态，角色和图片归属使用同一组合成夹具。
        with self.connect() as c, c:
            self.seed(c, 'front', '2026-01-01')
            self.seed(c, 'back', '2026-01-02', reverse=True)
            c.executemany("INSERT INTO term_links(system,key,term_id,source) "
                          "SELECT 'danbooru',?,id,'fixture' FROM terms WHERE key=?",
                          [('aemeath_(student)_(wuthering_waves)', self.alice),
                           ('aemeath_(wuthering_waves)', self.bob)])
            c.execute("INSERT INTO meta(key,value) VALUES ('terms_writer','1')")
            c.execute("INSERT INTO candidates SELECT uuid,entity,'WD',wd FROM family_evidence")
            c.execute("DELETE FROM meta WHERE key='terms_writer'")
        self.assert_catalog_pair()
        undo = self.read('/api/identity/pair', {'a': self.bob, 'b': self.alice, 'answer': 'apart'})['undo']
        self.assertEqual(self.read('/api/identity/count')['total'], 0)
        self.read('/api/undo', {'id': undo['id']})
        self.assert_catalog_pair()

        with self.connect() as c, c:
            c.execute("INSERT INTO meta(key,value) VALUES ('identity_policy','hybrid')")
        self.assert_catalog_pair()
        undo = self.read('/api/identity/pair', {'a': self.bob, 'b': self.alice, 'answer': 'form'})['undo']
        self.assertEqual(self.read('/api/identity/count')['total'], 0)
        with self.connect() as c:
            self.assertEqual(c.execute("SELECT character_id FROM identity_nodes WHERE id=?", (self.bob,)).fetchone(),
                             (self.alice,))
        self.read('/api/undo', {'id': undo['id']})
        self.assert_catalog_pair()

    def assert_catalog_pair(self):
        result = self.read('/api/identity/queue')
        self.assertEqual(result['total'], 1)
        [item] = result['items']
        self.assertEqual({item['a']['id'], item['b']['id']}, {self.alice, self.bob})
        self.assertEqual(item['catalog_kind'], 'form')
        batch = self.ask()
        self.assertIsNotNone(batch)
        self.assertEqual((batch['kind'], batch['entity']), ('seed', self.alice))
        # 问过的那一张不再出现在认一认里。
        self.assertEqual(sorted(item['uuid'] for item in batch['items']), ['four', 'three', 'two'])
        self.read('/api/ask/answer', {'kind': 'seed', 'entity': self.alice, 'later': ['two']})
        self.assertIsNone(self.ask())
        self.assert_queue([self.group(['four', 'three', 'two'])])

    def ask(self):
        return self.read('/api/ask')['batch']

    def history(self, c):
        return {table: c.execute(f'SELECT * FROM {table} ORDER BY 1,2').fetchall()
                for table in ('asked', 'assertions', 'term_effective_verdicts')}
