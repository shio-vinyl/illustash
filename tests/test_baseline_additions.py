"""v7 历史参考投影的边界：当前全状态、旧行值和未登记对象仍严格保留。"""
import copy
import json
from pathlib import Path
import re
import sqlite3
import tempfile
import unittest

from tests.fixed_expectations import _baseline_additions, _historical_projection, baseline_state


class HistoricalAdditions(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        with sqlite3.connect(self.root / 'library.sqlite') as c:
            c.executescript("""
                CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);
                INSERT INTO meta VALUES ('terms_schema','9'),('human_revision','3'),('preserved','yes');
                CREATE TABLE actions(id INTEGER PRIMARY KEY,label TEXT);
                INSERT INTO actions VALUES (1,'旧行为');
                CREATE TABLE human_state_marks(value TEXT);
                INSERT INTO human_state_marks VALUES ('reset');
                CREATE TABLE human_state_mark_undo(value TEXT);
                INSERT INTO human_state_mark_undo VALUES ('before-and-after');
            """)
        self.live = baseline_state(self.root)
        self.frozen = copy.deepcopy(self.live)
        for name in ('human_state_marks', 'human_state_mark_undo'):
            del self.frozen['app'][name]
        self.frozen['app']['meta']['rows'] = [row for row in self.frozen['app']['meta']['rows']
                                             if json.loads(row)[0] != 'human_revision']

    def test_current_state_keeps_additions_and_historical_projection_is_non_mutating(self):
        self.assertIn('human_state_marks', self.live['app'])
        self.assertIn('human_state_mark_undo', self.live['app'])
        self.assertTrue(any(json.loads(row)[0] == 'human_revision' for row in self.live['app']['meta']['rows']))
        original = copy.deepcopy(self.live)
        self.assertEqual(_historical_projection(self.live, self.frozen), self.frozen)
        self.assertEqual(self.live, original)
        with sqlite3.connect(self.root / 'library.sqlite') as c:
            c.execute("UPDATE human_state_marks SET value='changed'")
        self.assertNotEqual(baseline_state(self.root), original)

    def test_unlisted_tables_old_cells_types_and_multiplicity_remain_visible(self):
        for altered in (
            {'human_future': {'columns': ['v'], 'rows': ['[1]']}},
            {'actions': {'columns': ['id', 'label'], 'rows': ['[1,"改变"]']}},
            {'actions': {'columns': ['id', 'label'], 'rows': ['["1","旧行为"]']}},
            {'actions': {'columns': ['id', 'label'], 'rows': self.live['app']['actions']['rows'] * 2}},
        ):
            changed = copy.deepcopy(self.live)
            changed['app'].update(altered)
            self.assertNotEqual(_historical_projection(changed, self.frozen), self.frozen)

    def test_frozen_existing_table_or_key_cannot_be_hidden(self):
        for name in ('human_state_marks', 'human_state_mark_undo', 'meta'):
            old = copy.deepcopy(self.frozen)
            old['app'][name] = self.live['app'][name]
            with self.assertRaises(AssertionError):
                _historical_projection(self.live, old)

    def test_source_database_and_older_versions_are_not_projected(self):
        live = {**self.live, 'source': copy.deepcopy(self.live['app'])}
        projected = _historical_projection(live, self.frozen)
        self.assertEqual(projected['source'], live['source'])
        with sqlite3.connect(self.root / 'library.sqlite') as c:
            c.execute("UPDATE meta SET value='8' WHERE key='terms_schema'")
        old = baseline_state(self.root)
        self.assertEqual(_historical_projection(old, self.frozen), old)

    def test_every_manifest_entry_names_existing_semantic_tests(self):
        root = Path(__file__).resolve().parents[1]
        additions = _baseline_additions()
        self.assertEqual({e['name'] for e in additions['tables']}, {
            'galleries', 'gallery_members', 'similar_seen', 'human_state_marks', 'human_state_mark_undo',
            'human_object_contributions', 'human_entity_origins', 'human_relation_review_context',
            'human_asset_hashes', 'human_backup_state',
            'human_import_reports', 'human_import_report_rows'})
        self.assertEqual({e['name'] for e in additions['meta_keys']}, {'human_revision'})
        self.assertEqual({(e['physical'], e['view']) for e in additions['physical_views']}, {
            ('term_records', 'terms'), ('term_relation_records', 'term_relations')})
        introduced = {'galleries': 10, 'similar_seen': 10, 'gallery_members': 11}
        for entry in additions['tables'] + additions['meta_keys'] + additions['physical_views']:
            self.assertEqual(entry['introduced_in'], introduced.get(entry.get('name'), 9))
            for check in entry['checks']:
                path, function = check.rsplit('::', 1)
                self.assertRegex((root / path).read_text(), rf'fn {re.escape(function)}\(')

    def test_physical_view_mapping_compares_every_cell_and_rejects_hidden_history(self):
        with sqlite3.connect(self.root / 'library.sqlite') as c:
            c.executescript('CREATE TABLE term_records(id INTEGER PRIMARY KEY,key TEXT,attrs TEXT);'
                           'INSERT INTO term_records VALUES(1,\'synthetic\',\'{}\');'
                           'CREATE VIEW terms AS SELECT * FROM term_records;')
        live = baseline_state(self.root)
        frozen = copy.deepcopy(self.frozen)
        frozen['app']['terms'] = live['app']['terms']
        self.assertEqual(_historical_projection(live, frozen), frozen)
        for cells in ('[1,"different","{}"]', '["1","synthetic","{}"]'):
            changed = copy.deepcopy(live)
            changed['app']['term_records']['rows'] = [cells]
            with self.assertRaises(AssertionError):
                _historical_projection(changed, frozen)
        hidden = copy.deepcopy(live)
        hidden['app']['terms']['rows'] = []
        with self.assertRaises(AssertionError):
            _historical_projection(hidden, frozen)
        old = copy.deepcopy(frozen)
        old['app']['term_records'] = live['app']['term_records']
        with self.assertRaises(AssertionError):
            _historical_projection(live, old)
        self.assertIn('term_records', baseline_state(self.root)['app'])

    def test_raw_attrs_change_remains_visible_before_historical_projection(self):
        with sqlite3.connect(self.root / 'library.sqlite') as c:
            c.executescript("""CREATE TABLE term_records(id INTEGER PRIMARY KEY,key TEXT,attrs TEXT);
                INSERT INTO term_records VALUES(1,'synthetic','{"raw":"before"}');
                CREATE VIEW terms AS SELECT * FROM term_records;
                CREATE TABLE term_tree(term_id INTEGER,variant TEXT,artist TEXT,label_only INTEGER,
                    mascot INTEGER,mascot_origin TEXT,original INTEGER);
                INSERT INTO term_tree VALUES(1,'variant','artist',0,0,NULL,1);""")
        before = baseline_state(self.root)
        with sqlite3.connect(self.root / 'library.sqlite') as c:
            c.execute("UPDATE term_records SET attrs=?", ('{"raw":"after"}',))
        after = baseline_state(self.root)
        self.assertNotEqual(before['app']['term_records'], after['app']['term_records'])
        self.assertNotEqual(before['app']['terms'], after['app']['terms'])
        projected = _historical_projection(after, self.frozen)
        self.assertEqual(json.loads(projected['app']['terms']['rows'][0])[2],
                         '{"variant":"variant","artist":"artist","label_only":0,"mascot":0,"mascot_origin":null,"original":1}')
        self.assertEqual(after['app']['term_tree']['columns'], before['app']['term_tree']['columns'])
        with sqlite3.connect(self.root / 'library.sqlite') as c:
            c.executescript("DROP VIEW terms; CREATE VIEW terms AS SELECT id,key,'{}' AS attrs FROM term_records;")
        with self.assertRaises(AssertionError):
            _historical_projection(baseline_state(self.root), self.frozen)


if __name__ == '__main__':
    unittest.main()
