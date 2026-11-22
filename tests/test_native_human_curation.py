"""M2 画师/整理/折叠/偏好设置的验收，使用各自独立的合成包。

原来的 Python 参考实现（seed 和 Server）从固定录制中重放
（tests/fixed_expectations.py，#9）。
必须显式提供新的 release 构建；缺少二进制而跳过不算验收。
成功的响应比较原始状态码/字节/响应头；错误只比较状态码。
harness 还在每一步之后比较 app/source/provenance 的原始 DDL、全部带类型的单元格
和物理 rowid。不继承旧测试，不规范化任何输出，也不打开真实图库或图库副本。
"""
from contextlib import closing
import http.client
import json
import sqlite3
import unittest

from tests import test_native_human_labels as labels
from tests.test_native_human_labels import snapshot, states
from tools.verify_read_http import cookie, fetch
from tests.fixed_expectations import unclocked
from tools.wait_limit import WAIT


class NativeHumanCurationTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        labels.NativeHumanLabelsTest.setUpClass()
        cls.binary, cls.resources = labels.NativeHumanLabelsTest.binary, labels.NativeHumanLabelsTest.resources

    # 用组合而不是继承 Harness，免得把它的十个旧测试再算一遍。
    clone = labels.NativeHumanLabelsTest.clone
    start = labels.NativeHumanLabelsTest.start
    stop = labels.NativeHumanLabelsTest.stop
    seed_both = labels.NativeHumanLabelsTest.seed_both
    undo_latest = labels.NativeHumanLabelsTest.undo_latest
    latest_action = labels.NativeHumanLabelsTest.latest_action
    reference_response = labels.NativeHumanLabelsTest.reference_response

    def pair(self, url, body, *, expected=200, headers=None):
        item = {'url': url, 'method': 'POST', 'body': json.dumps(body, ensure_ascii=False).encode('utf-8'),
                'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': self.token, **(headers or {})}}
        before = [self.rec.state('before'), states(self.right)]
        responses = [self.reference_response('response', item), fetch(self.origin, item, self.origin)]
        after = [self.rec.state('after'), states(self.right)]
        for old, new, response in zip(before, after, responses):
            self.assertEqual(response['status'], expected, (url, body, response['body']))
            if response['status'] >= 400:
                self.assertEqual(old, new, ('failed POST partially committed', url, body))
        self.assertEqual(responses[1]['route'], 'core')
        if expected < 400:
            self.assertIsNone(labels.difference(*responses), (url, body, labels.difference(*responses)))
            for header in labels.HEADERS:
                self.assertEqual(responses[0]['headers'].get(header), responses[1]['headers'].get(header), header)
        self.assertEqual(after[0], after[1], (url, body))
        self.latency.append((url, responses[0]['ms'], responses[1]['ms']))
        self.assertFalse(self.sentinel.exists())
        return json.loads(responses[1]['body']) if expected < 400 else None

    def setUp(self):
        labels.NativeHumanLabelsTest.setUp(self)
        self.relations = [('e', 'f', 'crop'), ('f', 'g', 'edit'), ('c', 'd', 'variant'),
                          ('h', 'a', 'subtle'), ('b', 'd', 'similar')]
        self.seed_both()
        self.work, self.merged = self.rec.value('augmented ids')
        with closing(sqlite3.connect(self.right.db)) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM assets').fetchone()[0], 9)
            self.assertEqual(c.execute('PRAGMA foreign_key_check').fetchall(), [])

    def test_artist_filters_existing_ids_without_expanding_copies(self):
        answer = self.pair('/api/artist/verdict', {'ids': ['missing', 'b', 'b'], 'artist': self.artist,
                                                 'verdict': 'yes', 'name': ' 合成画师\n'})
        self.assertEqual(answer['undo']['label'], '已确认 1 张是  合成画师\n 画的')
        with closing(sqlite3.connect(self.right.db)) as c:
            verdicts = c.execute("SELECT a.key,v.verdict FROM term_effective_verdicts v JOIN term_assets a ON a.id=v.asset_id JOIN terms t ON t.id=v.term_id WHERE t.key=?", (self.artist,)).fetchall()
            self.assertEqual([tuple(row) for row in verdicts], [('b', 'yes')])
        self.pair('/api/artist/verdict', {'ids': ['a'], 'artist': self.artist, 'verdict': 'no', 'name': 3})
        self.pair('/api/artist/verdict', {'ids': ['b'], 'artist': self.artist, 'verdict': None})
        for _ in range(3):
            self.undo_latest()

    def test_curate_ascii_history_order_and_bulk_excluded_rowids_on_undo(self):
        self.seed_both()
        before_rows = self.excluded()
        self.assertEqual(before_rows, [(1, 'a'), (2, 'b')])
        action = self.pair('/api/curate', {'ids': ['b', 'a'], 'action': 'include'})['undo']
        self.assertEqual(self.excluded(), [])
        self.undo_latest()
        self.assertEqual(self.excluded(), before_rows)
        ids = ['图甲', 'b', 'a', 'c', 'b']
        ordered = ['图甲', 'b', 'a', 'c']
        action = self.pair('/api/curate', {'ids': ids, 'action': 'exclude'})['undo']
        with closing(sqlite3.connect(self.right.db)) as c:
            encoded = c.execute('SELECT before_json FROM actions WHERE id=?', (action['id'],)).fetchone()[0]
        self.assertEqual(encoded, json.dumps([{'uuid': u, 'curation': None} for u in ordered],
                                            ensure_ascii=False, sort_keys=True, separators=(',', ':')))
        self.assertIn('图甲', encoded)
        self.assertEqual(self.excluded(), [(1, 'a'), (2, 'b'), (3, 'c'), (4, '图甲')])
        self.pair('/api/curate', {'ids': ids, 'action': 'reset'})
        self.assertEqual(self.excluded(), before_rows)
        self.undo_latest()
        self.assertEqual(self.excluded(), [(1, 'a'), (2, 'b'), (3, 'c'), (4, '图甲')])
        self.undo_latest()
        self.assertEqual(self.excluded(), before_rows)

    def excluded(self):
        with closing(sqlite3.connect(self.right.db)) as c:
            return c.execute('SELECT rowid,uuid FROM excluded ORDER BY rowid').fetchall()

    def test_fold_hash_union_relationship_order_rep_ties_and_mixed_memberships(self):
        history = snapshot(self.right.db)['actions']
        self.pair('/api/fold', {'kinds': []})
        self.assert_duplicates(['d', 'e'], 'hash-two', 'e', hidden={'d'})
        self.assert_duplicates(['g', 'h'], 'hash-three', 'g', hidden={'h'})
        response = self.pair('/api/fold', {'kinds': ['edit', 'crop', 'edit']})
        self.assertEqual(response, {'kinds': ['crop', 'edit']})
        self.assert_duplicates(['d', 'e', 'f', 'g', 'h'], 'hash-three', 'e', hidden={'d', 'f', 'g', 'h'})
        self.seed_both()
        self.pair('/api/fold', {'kinds': ['edit', 'crop']})
        self.assert_duplicates(['d', 'e', 'f', 'g', 'h'], 'hash-three', 'e', hidden={'d', 'f', 'g', 'h'})
        self.assertEqual(snapshot(self.right.db)['actions'], history)
        # 没有 stable_hash 的合法旧 source 只按关系处理，不写入 source。
        self.seed_both()
        self.pair('/api/fold', {'kinds': ['crop', 'edit']})
        self.assert_duplicates(['e', 'f', 'g'], 'sim:e', 'e', hidden={'f', 'g'})
        self.assertEqual(snapshot(self.right.db)['actions'], history)

    def assert_duplicates(self, ids, group, representative, hidden):
        with closing(sqlite3.connect(self.right.db)) as c:
            rows = c.execute('SELECT uuid,grp,size,pos,rep,hidden FROM duplicates WHERE grp=? ORDER BY pos', (group,)).fetchall()
        self.assertEqual({row[0] for row in rows}, set(ids))
        self.assertEqual({row[2] for row in rows}, {len(ids)})
        self.assertEqual([row[3] for row in rows], list(range(1, len(ids) + 1)))
        self.assertEqual({row[0] for row in rows if row[4]}, {representative})
        self.assertEqual({row[0] for row in rows if row[5]}, hidden)

    def test_fold_has_no_action_or_event_history(self):
        before = snapshot(self.right.db)
        self.pair('/api/fold', {'kinds': ['identical', 'duplicate', 'identical']})
        after = snapshot(self.right.db)
        for table in ('actions', 'backend_effects'):
            self.assertEqual(before[table], after[table])

    def test_cross_label_artist_curate_latest_undo_stack(self):
        first = self.pair('/api/tag', {'ids': ['c'], 'tag': '第二标签'})['undo']
        self.pair('/api/artist/verdict', {'ids': ['b'], 'artist': self.artist, 'verdict': 'yes'})
        self.pair('/api/curate', {'ids': ['c', '图甲'], 'action': 'exclude'})
        self.seed_both()
        self.pair('/api/fold', {'kinds': ['crop']})
        self.pair('/api/undo', {'id': first['id']}, expected=400)
        for _ in range(3):
            self.undo_latest()
        prefs, kinds = self.rec.value('reference preferences')
        self.assertEqual(prefs, {'pins': [self.bob]})
        self.assertEqual(kinds, ['crop'])

    def test_parameter_validation_selection_confirmation_and_error_order(self):
        artist = {'artist': self.artist, 'verdict': 'yes'}
        for body in ({}, artist | {'ids': []}, artist | {'ids': ['missing']}, artist | {'ids': [3]},
                     artist | {'ids': ['b'] * 5001}, artist | {'ids': ['a'], 'verdict': True},
                     artist | {'ids': ['a'], 'artist': None}, artist | {'ids': ['a'], 'artist': 'missing'}):
            with self.subTest(artist=body):
                self.pair('/api/artist/verdict', body, expected=400)
        for kinds in (None, 'crop', [None], ['missing'], [[]]):
            with self.subTest(kinds=kinds):
                self.pair('/api/fold', {'kinds': kinds}, expected=400)
        base = {'action': 'exclude'}
        for body in ({'action': 'bad', 'ids': []}, base | {'ids': []}, base | {'ids': ['missing']},
                     base | {'ids': ['a'], 'exclude': [1]}, base | {'ids': ['a'], 'filter': {}},
                     base | {'filter': {'q': 'missing'}}, base | {'filter': {'bad': ''}},
                     base | {'ids': ['c'] * 501}, base | {'ids': ['c'] * 501, 'confirm': 1}):
            with self.subTest(curate=body):
                self.pair('/api/curate', body, expected=400)
        self.pair('/api/curate', base | {'ids': ['c'] * 501, 'confirm': True})
        self.undo_latest()
        self.pair('/api/curate', {'filter': {}, 'exclude': ['b'], 'action': 'include'})
        self.undo_latest()

    def test_committed_undo_is_reported_even_when_event_acknowledgement_fails(self):
        # #40：提交之后的失败（确认裁决事件时）不报告为回滚。
        def post(url, body):
            item = {'url': url, 'method': 'POST', 'body': json.dumps(body).encode(),
                    'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': self.token}}
            response = fetch(self.origin, item, self.origin)
            return response['status'], json.loads(response['body'])
        def sql(query):
            with closing(sqlite3.connect(self.right.db)) as c:
                return c.execute(query).fetchall()
        status, first = post('/api/curate', {'ids': ['a'], 'action': 'exclude'}); self.assertEqual(status, 200, first)
        status, second = post('/api/curate', {'ids': ['a'], 'action': 'include'}); self.assertEqual(status, 200, second)
        with closing(sqlite3.connect(self.right.db)) as c:
            c.execute("CREATE TRIGGER fail_ack BEFORE UPDATE ON backend_effects BEGIN SELECT RAISE(ABORT,'synthetic ack failure'); END")
            c.commit()
        status, undone = post('/api/undo', {'id': second['undo']['id']})
        self.assertEqual(status, 200, undone)
        self.assertEqual(undone['undo']['id'], first['undo']['id'])
        self.assertEqual(sql(f"SELECT undone FROM actions WHERE id={second['undo']['id']}"), [(1,)])
        self.assertGreater(sql("SELECT count(*) FROM backend_effects WHERE kind='verdict' AND status='pending'")[0][0], 0)
        # 已提交的那一步不会被撤销两次，更早的一步仍可撤销。
        status, refused = post('/api/undo', {'id': second['undo']['id']}); self.assertEqual(status, 400, refused)
        with closing(sqlite3.connect(self.right.db)) as c:
            c.execute('DROP TRIGGER fail_ack'); c.commit()
        status, last = post('/api/undo', {'id': first['undo']['id']}); self.assertEqual(status, 200, last)
        self.assertEqual(sql("SELECT count(*) FROM backend_effects WHERE kind='verdict' AND status='pending'"), [(0,)])
        self.assertFalse(self.sentinel.exists())

    def test_injected_writes_roll_back_every_declared_database(self):
        for table, url, body in (
            ('term_verdicts', '/api/artist/verdict', {'ids': ['b'], 'artist': self.artist, 'verdict': 'yes'}),
            ('actions', '/api/curate', {'ids': ['b', 'a'], 'action': 'exclude'}),
            ('duplicates', '/api/fold', {'kinds': ['crop', 'edit']}),

        ):
            for lib in (self.left, self.right):
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute(f"CREATE TRIGGER fail_curation BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'synthetic write failure'); END")
                    c.commit()
            self.pair(url, body, expected=500)
            for lib in (self.left, self.right):
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute('DROP TRIGGER fail_curation'); c.commit()

    def test_security_and_inflight_curation_cannot_write_into_new_session(self):
        commands = (('/api/artist/verdict', {'ids': ['b'], 'artist': self.artist, 'verdict': 'yes'}),
                    ('/api/curate', {'ids': ['a'], 'action': 'exclude'}),
                    ('/api/fold', {'kinds': []}), )
        for url, body in commands:
            self.pair(url, body, expected=403, headers={'X-CSRF-Token': 'bad'})
        self.stop(self.process)
        newer = self.clone(self.right, 'switched')
        self.process = self.start(self.right)
        token = json.loads(fetch(self.origin, {'url': '/api/bootstrap'}, self.origin)['body'])['token']
        before_old, before_new = states(self.right), states(newer)
        body = json.dumps({'ids': ['a'], 'action': 'exclude'}).encode()
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=WAIT)
        self.addCleanup(conn.close)
        conn.putrequest('POST', '/api/curate', skip_host=True)
        for name, value in {'Host': self.origin.split('://')[1], 'Origin': self.origin, 'Cookie': cookie(self.origin),
                            'Content-Type': 'application/json', 'X-CSRF-Token': token,
                            'Content-Length': str(len(body))}.items():
            conn.putheader(name, value)
        conn.endheaders(); conn.send(body[:1])
        switched = fetch(self.origin, {'method': 'POST', 'url': '/api/libraries/open',
                                      'body': json.dumps({'path': str(newer.path)}).encode(),
                                      'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': token}}, self.origin)
        self.assertEqual(switched['status'], 200, switched['body'])
        conn.send(body[1:]); stale = conn.getresponse()
        self.assertEqual(stale.status, 403, stale.read())
        self.assertEqual(states(self.right), before_old)
        self.assertEqual(states(newer), before_new)
        self.assertFalse(self.sentinel.exists())
