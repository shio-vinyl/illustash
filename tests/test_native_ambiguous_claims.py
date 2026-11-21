"""「可能认错」的原生 HTTP 契约；只用临时合成图库。"""
from contextlib import closing
import json
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest

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
        self.assert_queue([self.group(['new', 'r18', 'other-kind', 'old']), reverse])
        self.assert_queue([self.group(['new', 'other-kind', 'old']), reverse], 'hide')
        self.assert_queue([self.group(['r18'])], 'only')
        self.read('/api/ambiguous?nsfw=invalid', status=400)

    def test_reject_both_then_undo_once(self):
        with self.connect() as c, c:
            self.seed(c, 'a', '2026-01-02')
            self.seed(c, 'b', '2026-01-01')
            self.seed(c, 'other', '2026-01-03', reverse=True)
            c.execute("INSERT INTO asked VALUES ('b',?,'ambiguous','later',NULL,datetime('now','-8 days'))",
                      (self.alice,))
            before = self.history(c)
        reverse = self.group(['other'], reverse=True)
        self.assert_queue([self.group(['a', 'b']), reverse])
        body = {'kind': 'ambiguous', 'entity': self.alice, 'also_no': self.bob, 'no': ['a', 'b']}
        self.read('/api/ask/answer', {**body, 'no': ['a', 'other']}, status=400)
        self.read('/api/ask/answer', {**body, 'switch': self.bob}, status=400)
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
        with self.connect() as c:
            pairs = sorted((uid, entity) for uid in ['a', 'b'] for entity in (self.alice, self.bob))
            self.assertEqual(c.execute("SELECT uuid,entity,status FROM assertions ORDER BY uuid,entity").fetchall(),
                             [(*pair, 'rejected') for pair in pairs])
            self.assertEqual(c.execute("SELECT uuid,entity,kind,answer,action_id FROM asked ORDER BY uuid,entity").fetchall(),
                             [(*pair, 'ambiguous', 'no', undo['id']) for pair in pairs])
            verdicts = c.execute("SELECT a.key,t.key,v.verdict,v.action_id FROM term_verdicts v "
                                 "JOIN term_assets a ON a.id=v.asset_id JOIN terms t ON t.id=v.term_id "
                                 "ORDER BY a.key,t.key").fetchall()
            self.assertEqual(verdicts, [(*pair, 'no', undo['id']) for pair in pairs])
            self.assertEqual(c.execute("SELECT id,undone FROM actions").fetchall(), [(undo['id'], 0)])
        self.assert_queue([reverse])

        self.read('/api/undo', {'id': undo['id']})
        self.assert_queue([self.group(['a', 'b']), reverse])
        with self.connect() as c:
            self.assertEqual(self.history(c), before)
            self.assertEqual(c.execute("SELECT id,undone FROM actions").fetchall(), [(undo['id'], 1)])

    def history(self, c):
        return {table: c.execute(f'SELECT * FROM {table} ORDER BY 1,2').fetchall()
                for table in ('asked', 'assertions', 'term_effective_verdicts')}
