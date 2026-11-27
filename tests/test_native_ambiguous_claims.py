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

    def read(self, url, body=None, status=200, *, headers=None):
        reply = self.request(url, body, headers=headers)
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
        batch = self.ask()
        self.assertIsNotNone(batch)
        self.assertEqual((batch['kind'], batch['entity']), ('seed', self.alice))
        # 问过的那一张不再出现在认一认里。
        self.assertEqual(sorted(item['uuid'] for item in batch['items']), ['four', 'three', 'two'])
        self.read('/api/ask/answer', {'kind': 'seed', 'entity': self.alice, 'later': ['two']})
        self.assertIsNone(self.ask())
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

    def test_saved_pair_can_resume_after_other_edits_in_direct_and_hybrid(self):
        with self.connect() as c, c:
            self.seed(c, 'front', '2026-01-01')
            self.seed(c, 'back', '2026-01-02', reverse=True)
        unrelated = ['other-a', 'other-b']
        pair = sorted([self.alice, self.bob])
        order = [self.group(['front']), self.group(['back'], reverse=True)]
        order.sort(key=lambda g: g['a']['id'])
        query = lambda e: self.read('/api/character/profile?' + urlencode({'entity': e}))
        for hybrid in (False, True):
            with self.subTest(hybrid=hybrid):
                with self.connect() as c, c:
                    if hybrid:
                        c.execute("INSERT INTO meta(key,value) VALUES ('identity_policy','hybrid')")
                    c.execute("INSERT OR REPLACE INTO prefs VALUES ('identity_kept_apart',?)",
                              (json.dumps([pair[::-1], unrelated]),))
                self.assert_queue([])
                for entity in pair:
                    [saved] = query(entity)['kept_apart']
                    self.assertEqual([saved['a']['id'], saved['b']['id']], pair)
                    self.assertEqual({saved['a']['name'], saved['b']['name']}, {'Alice', 'Bob'})
                self.read('/api/favorite', {'ids': ['front'], 'add': not hybrid})
                before = self.history_snapshot()
                body = {'a': pair[1], 'b': pair[0], 'answer': 'resume'}
                undo = self.read('/api/identity/pair', body)['undo']
                self.assertEqual(undo['label'], '已允许再次询问这对角色')
                self.read('/api/identity/pair', body, status=400)
                self.assert_queue(order)
                self.assertEqual(self.history_snapshot(), before)
                self.assertNotIn('kept_apart', query(self.bob))
                with self.connect() as c:
                    prefs = c.execute("SELECT value FROM prefs WHERE key='identity_kept_apart'").fetchone()[0]
                    self.assertEqual(json.loads(prefs), [unrelated])
                self.read('/api/undo', {'id': undo['id']})
                self.assert_queue([])
                self.assertIn('kept_apart', query(self.alice))
                self.assertEqual(self.history_snapshot(), before)

    def test_catalog_group_cannot_enter_or_answer_a_person_pair(self):
        with self.connect() as c, c:
            self.seed(c, 'front', '2026-01-01')
            self.seed(c, 'back', '2026-01-02', reverse=True)
        self.assertEqual(self.read('/api/detail?id=front')['question']['kind'], 'ambiguous')
        with self.connect() as c, c:
            c.execute("INSERT INTO meta(key,value) VALUES ('terms_writer','1')")
            c.execute("UPDATE term_tree SET label_only=1 WHERE term_id=(SELECT id FROM terms WHERE key=?)",
                      (self.bob,))
            c.execute("INSERT INTO tag_memberships SELECT * FROM memberships WHERE entity=?", (self.bob,))
            c.execute("DELETE FROM memberships WHERE entity=?", (self.bob,))
            c.execute("DELETE FROM meta WHERE key='terms_writer'")
        self.assert_queue([])
        for entity in (self.alice, self.bob):
            self.assertEqual(self.read('/api/ids?' + urlencode({'entity': entity, 'status': 'ambiguous'})), [])
        self.assertNotEqual(self.read('/api/detail?id=front')['question']['kind'], 'ambiguous')
        refusal = self.read('/api/ask/answer', {'kind': 'ambiguous', 'entity': self.alice,
                                               'also_no': self.bob, 'no': ['front']}, status=400)
        self.assertEqual(refusal['error'], '图片的备选归属已变更，请刷新')
        [group] = self.read('/api/characters?' + urlencode({'id': self.bob}))
        self.assertIs(group['group_tag'], True)
        self.assertTrue(any(x['id'] == self.bob and x['group_tag'] for x in self.read('/api/tagged')))
        filters = urlencode({'entity': self.bob})
        self.assertEqual(self.read('/api/ids?' + filters), ['back'])
        self.assertEqual(self.read('/api/assets/count?' + filters), {'total': 1})
        with self.connect() as c, c:
            c.execute("INSERT INTO meta(key,value) VALUES ('terms_writer','1')")
            c.execute("UPDATE term_tree SET original=1 WHERE term_id=(SELECT id FROM terms WHERE key=?)", (self.bob,))
            c.execute("DELETE FROM meta WHERE key='terms_writer'")
        [original] = self.read('/api/characters?' + urlencode({'id': self.bob}))
        self.assertNotIn('group_tag', original)
        self.assertFalse(any(x['id'] == self.bob for x in self.read('/api/tagged')))

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

    def test_reviewed_forms_without_implications_reach_the_identity_queue(self):
        with self.connect() as c, c:
            self.seed(c, 'front', '2026-01-01')
            self.seed(c, 'back', '2026-01-02', reverse=True)
        for left, right, relation in [
            ('firefly_(honkai:_star_rail)', 'sam_(honkai:_star_rail)', 'form'),
            ('sailor_moon', 'tsukino_usagi', 'form'),
            ('hoshino_ai', 'hoshino_ai_(oshi_no_ko)', 'unknown'),
            ('shorekeeper_(wuthering_waves)', 'the_shorekeeper_(wuthering_waves)', 'unknown'),
            ('kujo_jotaro', 'star_platinum', None),
        ]:
            with self.subTest(left=left, right=right):
                with self.connect() as c, c:
                    c.execute("DELETE FROM term_links WHERE system='danbooru' AND source='reviewed-form-fixture'")
                    c.executemany("INSERT INTO term_links(system,key,term_id,source) "
                                  "SELECT 'danbooru',?,id,'reviewed-form-fixture' FROM terms WHERE key=?",
                                  [(left, self.alice), (right, self.bob)])
                if relation:
                    self.assert_catalog_pair(relation)
                else:
                    self.assertEqual(self.read('/api/identity/queue')['items'], [])

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

    def assert_catalog_pair(self, relation='form'):
        result = self.read('/api/identity/queue')
        self.assertEqual(result['total'], 1)
        [item] = result['items']
        self.assertEqual({item['a']['id'], item['b']['id']}, {self.alice, self.bob})
        self.assertEqual(item['catalog_kind'], relation)

    def test_same_work_role_labels_after_keep_apart(self):
        # 同一个 Bangumi 角色的两种目录形态，使用合成图片和离线选名数据。
        guard, base = self.alice, self.bob
        tags = {guard: 'amiya_(guard)_(arknights)', base: 'amiya_(arknights)'}
        name = '合成阿米娅'
        with self.connect() as c, c:
            self.seed(c, 'front', '2026-01-01')
            self.seed(c, 'back', '2026-01-02', reverse=True)
            c.execute("INSERT INTO meta(key,value) VALUES ('terms_writer','1')")
            for entity, tag in tags.items():
                # 模拟旧索引已经有当前链接，随后索引更新追加目录规范标签。
                c.execute("UPDATE term_links SET added_at='1970-01-01' WHERE system='danbooru' "
                          "AND term_id=(SELECT id FROM terms WHERE key=?)", (entity,))
                c.execute("INSERT INTO term_links(system,key,term_id,source) "
                          "SELECT 'danbooru',?,id,'fixture' FROM terms WHERE key=?", (tag, entity))
                c.executemany("INSERT INTO term_names(term_id,lang,value,source,role,seq) "
                              "SELECT id,?,?,'bangumi','display',(SELECT coalesce(max(seq),0)+1 FROM term_names) "
                              "FROM terms WHERE key=?", [('zh', name, entity), ('en', 'Synthetic Amiya', entity)])
            c.execute("DELETE FROM meta WHERE key='terms_writer'")
        work = self.read('/api/works', {'action': 'from_tag', 'tag': 'arknights'})['id']
        self.read('/api/character/profile', {'entity': base, 'work': work})

        def labels(rows):
            return {r['id']: (r['name'], r.get('name_disambiguation')) for r in rows}

        def endpoints(pair):
            return labels([pair['a'], pair['b']])

        def short(entity, value, shown=name):
            return shown, {'qualifier': value, 'tag': tags[entity]}

        expected = {base: short(base, '基本形态'), guard: short(guard, '近卫')}
        # 一端来自有作品归属的索引，另一端只有已经确认作品的角色目录 tag。
        for entity in tags:
            path = '/api/characters?' + urlencode({'id': entity})
            [row] = self.read(path)
            self.assertEqual(row['work_id'], work if entity == base else None)
            self.assertEqual(row['tag'], 'alice' if entity == guard else 'bob')
            self.assertEqual(labels([row]), {entity: expected[entity]})

        # 若旧身份还有另一个互相矛盾的目录链接，不从候选标签推断作品归属。
        guard_path = '/api/characters?' + urlencode({'id': guard})
        other_tag = 'acheron_(honkai:_star_rail)'
        with self.connect() as c, c:
            c.execute("INSERT INTO term_links(system,key,term_id,source) "
                      "SELECT 'wd_character',?,id,'fixture' FROM terms WHERE key=?", (other_tag, guard))
        self.assertEqual(labels(self.read(guard_path)), {guard: (name, None)})
        with self.connect() as c, c:
            c.execute("DELETE FROM term_links WHERE system='wd_character' AND key=?", (other_tag,))
        self.read('/api/character/profile', {'entity': guard, 'work': work})

        [pair] = self.read('/api/identity/queue?limit=1&offset=0')['items']
        self.assertEqual(endpoints(pair), expected)
        groups = self.read('/api/ambiguous')['groups']
        self.assertEqual(len(groups), 2)
        for group in groups:
            self.assertEqual(endpoints(group), expected)

        self.read('/api/identity/pair', {'a': base, 'b': guard, 'answer': 'apart'})
        self.assertEqual(self.read('/api/identity/queue')['items'], [])
        self.assertEqual(self.read('/api/ambiguous')['groups'], [])
        profile_url = '/api/character/profile?' + urlencode({'entity': guard})
        [kept] = self.read(profile_url)['kept_apart']
        self.assertEqual(endpoints(kept), expected)

        self.assertEqual(labels(self.read('/api/characters?summary=1&min=1&nsfw=hide')), expected)
        find = urlencode({'q': name})
        self.assertEqual(labels(self.read('/api/entities?' + find)), expected)
        self.assertEqual(labels(self.read('/api/entities?q=guard')), {guard: expected[guard]})
        self.assertEqual(labels(self.read('/api/suggest', {'ids': ['front', 'back']})), expected)
        only_guard = '/api/characters?' + urlencode({'id': guard})
        self.assertEqual(labels(self.read(only_guard)), {guard: expected[guard]})
        en = self.read(only_guard, headers={'X-Illustash-Language': 'en-US'})
        self.assertEqual(labels(en), {guard: short(guard, 'Guard', 'Synthetic Amiya')})

        manual = self.read('/api/character/profile', {'entity': base, 'name': name})['undo']
        self.assertEqual(labels(self.read('/api/entities?' + find)), {base: (name, None), guard: expected[guard]})
        self.read('/api/undo', {'id': manual['id']})
        self.assertEqual(labels(self.read('/api/entities?' + find)), expected)

        other = self.read('/api/works', {'action': 'create', 'name': '另一合成作品'})['id']
        self.read('/api/character/profile', {'entity': base, 'work': other})
        self.assertEqual(labels(self.read('/api/entities?' + find)), {base: (name, None), guard: (name, None)})

    def test_profile_only_lists_live_catalog_edges_and_displays_original_tags(self):
        owner, child = self.alice, self.bob
        source_tag, owner_tag = 'hiyori_(swimsuit)_(blue_archive)', 'hiyori_(blue_archive)'
        with self.connect() as c, c:
            c.executemany("INSERT INTO term_links(system,key,term_id,source) "
                          "SELECT 'danbooru',?,id,'fixture' FROM terms WHERE key=?",
                          [(source_tag, child), (owner_tag, owner)])
            c.execute("INSERT INTO meta(key,value) VALUES ('terms_writer','1')")
            c.execute("UPDATE term_tree SET parent_id=?, variant='swimsuit' WHERE term_id=(SELECT id FROM terms WHERE key=?)",
                      (owner, child))
            c.execute("UPDATE terms SET kind='skin' WHERE key=?", (child,))
            c.execute("DELETE FROM meta WHERE key='terms_writer'")
            saved = {'action': None, 'before': {'kind': 'character', 'work': None, 'variant': ''}, 'variant': 'swimsuit'}
            c.execute("INSERT OR REPLACE INTO meta VALUES ('character_catalog',?)", (json.dumps({
                'revision': 1, 'attached': [[child, owner, saved], ['stale-child', owner]],
            }),))
        url = '/api/character/profile?' + urlencode({'entity': owner})
        [edge] = self.read(url)['catalog']
        self.assertEqual((edge['id'], edge['kind']), (child, 'attached'))
        self.assertIn(source_tag, edge['tags'])
        self.assertIn(owner_tag, edge['owner_tags'])
        request = {'entity': child, 'owner': owner, 'kind': 'attached'}
        self.read('/api/catalog/split', {**request, 'owner': 'unrelated'}, status=400)
        undo = self.read('/api/catalog/split', request)['undo']
        self.assertNotIn('catalog', self.read(url))
        self.read('/api/undo', {'id': undo['id']})
        [restored] = self.read(url)['catalog']
        for key in ('id', 'kind', 'tags', 'owner_tags'):
            self.assertEqual(restored[key], edge[key])
        self.assertTrue(restored['name'])

    def ask(self):
        return self.read('/api/ask')['batch']

    def history_snapshot(self):
        with self.connect() as c:
            return self.history(c)

    def history(self, c):
        return {table: c.execute(f'SELECT * FROM {table} ORDER BY 1,2').fetchall()
                for table in ('asked', 'assertions', 'term_effective_verdicts')}
