"""#35 原生角色 HTTP 验收：合成包、原始字节和三个数据库的带类型快照。"""
from contextlib import closing
import http.client
import json
import sqlite3
import unittest

from tests import test_native_human_labels as labels
from tests import test_native_human_curation as curation
from tests import test_native_human_identity as identity
from tests.test_native_human_labels import states
from tools.verify_read_http import cookie, fetch
from tests.fixed_expectations import unclocked
from tools.wait_limit import WAIT


class NativeHumanCharactersTest(unittest.TestCase):
    setUpClass = classmethod(labels.NativeHumanLabelsTest.setUpClass.__func__)
    setUp = labels.NativeHumanLabelsTest.setUp
    clone = labels.NativeHumanLabelsTest.clone
    stop = labels.NativeHumanLabelsTest.stop
    seed_both = labels.NativeHumanLabelsTest.seed_both
    undo_latest = labels.NativeHumanLabelsTest.undo_latest
    latest_action = labels.NativeHumanLabelsTest.latest_action
    reference_response = labels.NativeHumanLabelsTest.reference_response
    pair = curation.NativeHumanCurationTest.pair

    def start(self, lib):
        if self._testMethodName == 'test_change_all_actions_multi_copy_filter_and_variants':
            # 这条冻结用例的夏季造型按原选名规则物化，逐表断言仍覆盖它的投影。
            self.env['ILLUSTASH_TEST_LEGACY_SKIN_NAMES'] = '1'
        return labels.NativeHumanLabelsTest.start(self, lib)

    def test_change_all_actions_multi_copy_filter_and_variants(self):
        skin = 'synthetic-skin'
        self.seed_both()
        calls = [({'ids': ['a', 'a'], 'action': 'confirmed', 'entity': self.alice}),
                 ({'ids': ['c'], 'action': 'confirmed', 'entities': [self.alice, self.bob, self.alice]}),
                 ({'ids': ['c'], 'action': 'add_variant', 'entity': skin}),
                 ({'ids': ['c'], 'action': 'remove_variant', 'entity': skin}),
                 ({'ids': ['a'], 'action': 'rejected', 'entity': self.alice}),
                 ({'ids': ['a'], 'action': 'resume'}),
                 ({'filter': {}, 'exclude': ['c'], 'action': 'unknown'}),
                 ({'ids': ['c'] * 501, 'confirm': True, 'action': 'confirmed', 'entity': self.alice})]
        for body in calls:
            self.pair('/api/change', body)
        for _ in calls:
            self.undo_latest()

    def test_answer_yes_no_later_switch_and_kinds(self):
        for kind in ('seed', 'ambiguous', 'sample', 'audit', 'hint'):
            self.pair('/api/ask/answer', {'entity': self.alice, 'yes': ['a'], 'no': ['c'], 'kind': kind})
            self.undo_latest()
        self.pair('/api/ask/answer', {'entity': self.alice, 'no': ['a'], 'switch': self.bob, 'later': ['c']})
        self.undo_latest()
        prior = self.latest_action()
        result = self.pair('/api/ask/answer', {'entity': self.alice, 'later': ['missing', 'c'], 'yes': False, 'no': ''})
        self.assertEqual(result, {'undo': None})
        self.assertEqual(self.latest_action(), prior)

    def test_audit_correct_reassign_sweep_and_mixed_latest_undo(self):
        self.seed_both()
        self.pair('/api/audit/answer', {'yes': ['a'], 'no': ['c']})
        self.pair('/api/reassign', {'ids': ['a'], 'old': self.alice, 'new': self.bob})
        self.pair('/api/reassign', {'filter': {'entity': self.bob}, 'exclude': ['c'], 'old': self.bob, 'new': [self.alice]})
        self.pair('/api/sweep', {'entity': self.alice})
        self.pair('/api/sweep', {'entity': self.alice})  # 机器集合为空时仍记录一个操作
        self.pair('/api/tag', {'ids': ['c'], 'tag': '第二标签'})
        self.pair('/api/artist/verdict', {'ids': ['b'], 'artist': self.artist, 'verdict': 'yes'})
        self.pair('/api/curate', {'ids': ['c'], 'action': 'exclude'})
        self.pair('/api/change', {'ids': ['c'], 'action': 'resume'})
        for _ in range(9):
            self.undo_latest()

    def test_merged_entity_and_multi_replacement_preserve_other_memberships(self):
        self.seed_both()
        self.third, self.merged = self.rec.value('created ids')
        self.pair('/api/change', {'ids': ['c'], 'action': 'confirmed', 'entity': self.merged})
        self.pair('/api/reassign', {'ids': ['c'], 'old': self.merged, 'new': [self.bob, self.third, self.bob]})
        self.pair('/api/reassign', {'ids': ['c'], 'old': self.bob, 'new': self.alice})
        with closing(sqlite3.connect(self.right.db)) as c:
            self.assertIsNotNone(c.execute('SELECT 1 FROM memberships WHERE uuid=? AND entity=?', ('c', self.third)).fetchone())
        for _ in range(3): self.undo_latest()

    def test_hybrid_verdict_scope_and_undo(self):
        self.seed_both()
        self.pair('/api/change', {'ids': ['a'], 'action': 'confirmed', 'entity': self.alice})
        self.pair('/api/ask/answer', {'entity': self.alice, 'no': ['a'], 'switch': self.bob})
        self.pair('/api/reassign', {'ids': ['a'], 'old': self.bob, 'new': self.alice})
        self.pair('/api/sweep', {'entity': self.bob})
        for _ in range(4): self.undo_latest()

    def test_role_and_identity_commands_share_recent_undo(self):
        self.seed_both()
        self.third = self.rec.value('created id')
        pair = lambda url, body: identity.NativeHumanIdentityTest.pair(self, url, body)
        pair('/api/change', {'ids': ['c'], 'action': 'confirmed', 'entity': self.alice})
        pair('/api/identity/decision', {'a': self.alice, 'b': self.third, 'kind': 'duplicate_of',
                                      'status': 'rejected', 'reason': 'Synthetic refusal'})
        pair('/api/ask/answer', {'entity': self.alice, 'no': ['c'], 'switch': self.bob})
        pair('/api/change', {'ids': ['c'], 'action': 'resume'})
        pair('/api/merge', {'source': self.third, 'target': self.bob})
        for _ in range(5): self.undo_latest()

    def test_unused_extended_json_and_truthy_non_string_numbers(self):
        self.pair('/api/change', {'ids': ['c'], 'action': 'confirmed', 'entity': self.alice,
                                 'unused': [10 ** 120, float('nan'), float('inf')]})
        self.undo_latest()
        self.pair('/api/ask/answer', {'entity': self.alice, 'later': ['c'], 'yes': {}, 'no': 0.0})
        for invalid in (10 ** 120, float('nan'), float('inf')):
            self.pair('/api/ask/answer', {'entity': self.alice, 'later': ['c'], 'yes': invalid}, expected=400)

    def test_validation_errors_have_no_partial_writes(self):
        cases = [('/api/change', b) for b in (
            {}, {'action': 'bad'}, {'action': 'confirmed', 'entities': []},
            {'action': 'confirmed', 'entity': self.alice, 'entities': [self.bob]},
            {'action': 'unknown', 'ids': []}, {'action': 'unknown', 'ids': [1]},
            {'action': 'unknown', 'ids': ['missing']}, {'action': 'unknown', 'filter': []},
            {'action': 'unknown', 'filter': {'unknown': ''}},
            {'action': 'unknown', 'ids': ['a'], 'filter': {}},
            {'action': 'unknown', 'ids': ['a'], 'exclude': ['c']},
            {'action': 'unknown', 'ids': ['a'], 'exclude': [1]},
            {'action': 'unknown', 'ids': ['a'] * 501, 'confirm': 1},
            {'action': 'add_variant', 'entity': self.alice, 'ids': ['a']})]
        cases += [('/api/ask/answer', b) for b in (
            {}, {'entity': self.alice, 'yes': [1]}, {'entity': self.alice, 'yes': ['a'], 'no': ['a']},
            {'entity': self.alice, 'yes': ['a'], 'switch': self.bob},
            {'entity': self.alice, 'yes': ['a'], 'kind': None},
            {'entity': self.alice, 'yes': ['a'], 'kind': 'bad'},
            {'entity': self.alice, 'yes': ['missing']},
            {'entity': self.alice, 'no': ['a'], 'switch': self.alice})]
        cases += [('/api/audit/answer', {}), ('/api/audit/answer', {'yes': ['missing']}),
                  ('/api/reassign', {'ids': ['a'], 'old': self.alice, 'new': []}),
                  ('/api/reassign', {'ids': ['a'], 'old': self.alice, 'new': ['missing']}),
                  ('/api/sweep', {'entity': 'missing'})]
        for url, body in cases:
            with self.subTest(url=url, body=body):
                self.pair(url, body, expected=400)

    def test_transaction_faults_rollback(self):
        self.seed_both()
        commands = [('/api/change', {'ids': ['a'], 'action': 'confirmed', 'entity': self.alice}),
                    ('/api/ask/answer', {'entity': self.alice, 'yes': ['a']}),
                    ('/api/reassign', {'ids': ['a'], 'old': self.alice, 'new': self.bob}),
                    ('/api/sweep', {'entity': self.alice})]
        commands.append(('/api/audit/answer', {'yes': ['a']}))
        for table in ('actions', 'term_verdicts', 'backend_effects', 'assertions'):
            for lib in (self.left, self.right):
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute(f"CREATE TRIGGER fail_character BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'synthetic failure'); END"); c.commit()
            for url, body in commands:
                self.pair(url, body, expected=500)
            for lib in (self.left, self.right):
                with closing(sqlite3.connect(lib.db)) as c:
                    c.execute('DROP TRIGGER fail_character'); c.commit()

    def test_postcommit_event_failure_survives_and_recovers(self):
        for lib in (self.left, self.right):
            with closing(sqlite3.connect(lib.db)) as c:
                c.execute("CREATE TRIGGER fail_ack BEFORE UPDATE ON backend_effects BEGIN SELECT RAISE(ABORT,'synthetic ack failure'); END"); c.commit()
        item = {'method': 'POST', 'url': '/api/change',
                'body': json.dumps({'ids': ['c'], 'action': 'confirmed', 'entity': self.alice}),
                'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': self.token}}
        # 旧参考实现在改动已提交的情况下仍回答 500；按用户 2026-10-11 的决定（#40），
        # 已提交的改动回答 200，它的事件保持 pending，留待恢复。
        responses = [self.reference_response('response', item), fetch(self.origin, item, self.origin)]
        self.assertEqual([r['status'] for r in responses], [500, 200])
        self.assertIn('undo', json.loads(responses[1]['body']))
        self.assertEqual(self.rec.state('after'), states(self.right))
        with closing(sqlite3.connect(self.right.db)) as c:
            self.assertEqual(c.execute("SELECT status FROM backend_effects ORDER BY id DESC LIMIT 1").fetchone()[0], 'pending')
        for lib in (self.left, self.right):
            with closing(sqlite3.connect(lib.db)) as c:
                c.execute('DROP TRIGGER fail_ack'); c.commit()
        self.stop(self.process)
        self.process = self.start(self.right)
        self.token = json.loads(fetch(self.origin, {'url': '/api/bootstrap'}, self.origin)['body'])['token']
        self.rec.alias(self.token, '@TOKEN@')
        self.assertEqual(self.rec.state('drained'), states(self.right))
        self.undo_latest()

    def test_security_and_inflight_request_switch_isolation(self):
        commands = [('/api/change', {'ids': ['a'], 'action': 'unknown'}),
                    ('/api/ask/answer', {'entity': self.alice, 'later': ['a']}),
                    ('/api/audit/answer', {'yes': ['a']}),
                    ('/api/reassign', {'ids': ['a'], 'old': self.alice, 'new': self.bob}),
                    ('/api/sweep', {'entity': self.alice})]
        for url, body in commands:
            self.pair(url, body, expected=403, headers={'X-CSRF-Token': 'bad'})
        newer = self.clone(self.right, 'switched')
        for url, body in commands:
            token = self.token
            before_old, before_new = states(self.right), states(newer)
            encoded = json.dumps(body).encode()
            conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=WAIT)
            conn.putrequest('POST', url, skip_host=True)
            for key, value in {'Host': self.origin.split('://')[1], 'Origin': self.origin, 'Cookie': cookie(self.origin),
                               'Content-Type': 'application/json', 'X-CSRF-Token': token,
                               'Content-Length': str(len(encoded))}.items(): conn.putheader(key, value)
            conn.endheaders(); conn.send(encoded[:1])
            switched = fetch(self.origin, {'method': 'POST', 'url': '/api/libraries/open',
                         'body': json.dumps({'path': str(newer.path)}),
                         'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': token}}, self.origin)
            self.assertEqual(switched['status'], 200)
            conn.send(encoded[1:]); response = conn.getresponse(); response.read(); conn.close()
            self.assertEqual(response.status, 403)
            self.assertEqual(states(self.right), before_old); self.assertEqual(states(newer), before_new)
            self.token = json.loads(fetch(self.origin, {'url': '/api/bootstrap'}, self.origin)['body'])['token']
            restored = fetch(self.origin, {'method': 'POST', 'url': '/api/libraries/open',
                         'body': json.dumps({'path': str(self.right.path)}),
                         'headers': {'Content-Type': 'application/json', 'X-CSRF-Token': self.token}}, self.origin)
            self.assertEqual(restored['status'], 200)
            self.token = json.loads(fetch(self.origin, {'url': '/api/bootstrap'}, self.origin)['body'])['token']
            self.rec.alias(self.token, '@TOKEN@')
