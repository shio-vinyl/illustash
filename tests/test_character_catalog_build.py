"""角色词表的公共 ID 和自动挂载审核边界。"""
import json
import unittest
from unittest import mock

from types import SimpleNamespace

from tools.character_catalog import build, forms, reviewed, wikidata_labels
from tools.character_catalog.match_bangumi import Matcher


class CharacterCatalogBuildTest(unittest.TestCase):
    def test_series_lookup_uses_exact_names_without_widening_a_subwork(self):
        subjects = {
            10: {'name': 'ファイアーエムブレム 紋章の謎', 'name_cn': 'First Subwork', 'type': 4},
            12: {'name': 'ファイアーエムブレム 外伝', 'name_cn': 'Second Subwork', 'type': 4},
            13: {'name': 'A Fire Emblem Mystery', 'name_cn': '', 'type': 4},
        }
        characters = {
            1: {'name': 'ターニャ', 'zh': '', 'names': {'Tanya'}},
            2: {'name': 'ダナ', 'zh': '', 'names': {'Dana'}},
            3: {'name': 'ドーラ', 'zh': '', 'names': {'Dora'}},
        }
        wiki = {
            'tanya_(fire_emblem)': {'other_names': ['ターニャ']},
            'dana_(fire_emblem)': {'other_names': ['ラジュルネ']},
            'dora_(fire_emblem)': {'other_names': ['ドーラ']},
        }
        copyrights = {'fire_emblem', 'first_subwork', 'second_subwork'}
        matcher = Matcher(characters, subjects, {1: [(12, 1)], 2: [(10, 1)], 3: [(13, 1)]},
                          wiki, {}, copyrights, {}, {}, {})
        self.assertEqual(matcher.series_subjects['fire_emblem'], {10, 12})
        match = matcher.match(['tanya_(fire_emblem)'])
        self.assertEqual((match['id'], match['how'], match['subjects']), (1, 'series', [12]))
        for tag in ['dana_(fire_emblem)', 'dora_(fire_emblem)', 'tanya_(first_subwork)']:
            self.assertIsNone(matcher.match([tag]), tag)

    def test_archive_form_candidates_need_review_and_cannot_cross_works(self):
        columns = ['id', 'tag', 'parent', 'relation', 'action', 'bangumi']
        rows = [
            [1, 'base', None, None, None, 1],
            [2, 'checked', None, None, None, 1],
            [3, 'cross_work', None, None, None, 1],
            [4, 'archive', None, None, None, 2],
            [5, 'unreviewed', None, None, None, 3],
            [6, 'renamed', None, None, None, 1],
        ]
        matcher = SimpleNamespace(qualified_works=lambda tag: {'other'} if tag == 'cross_work' else {'same'})
        review = set()
        with (mock.patch.dict(reviewed.FORMS, checked='base', cross_work='base', archive='base'),
              mock.patch.dict(reviewed.RENAMED_TAGS, renamed='base')):
            forms.add(rows, columns, matcher, [(2, forms.BANGUMI_FORM, 1), (3, forms.BANGUMI_FORM, 1)], review)
        self.assertEqual(rows[1][2:5], [1, 'form', 'ask'])
        self.assertEqual(rows[2][2:5], [None, None, None])
        self.assertEqual(rows[3][2:5], [1, 'form', 'ask'])
        self.assertEqual(rows[4][2:5], [None, None, None])
        self.assertEqual(rows[5][2:5], [1, 'unknown', 'ask'])
        self.assertIn(('Bangumi 形态', 'base', 'unreviewed'), review)

    def test_bundled_renamed_tags_keep_both_public_ids_and_ask(self):
        catalog = json.loads((build.ROOT / 'tags/character_catalog.json').read_bytes())
        rows = (dict(zip(catalog['columns'], raw, strict=True)) for raw in catalog['characters'])
        entries = {row['tag']: row for row in rows}
        for old, old_id, current, current_id in (
                ('the_shorekeeper_(wuthering_waves)', 13321, 'shorekeeper_(wuthering_waves)', 12257),
                ('hoshino_ai_(oshi_no_ko)', 4905, 'hoshino_ai', 4904)):
            with self.subTest(old=old):
                former = entries[old]
                self.assertEqual((former['id'], former['parent'], former['relation'], former['action']),
                                 (old_id, current_id, 'unknown', 'ask'))
                self.assertEqual(entries[current]['id'], current_id)

    def test_only_unambiguous_original_wikidata_labels_with_external_evidence_are_shipped(self):
        columns = ['tag', 'tags', 'wikidata', 'bangumi', 'group', 'action']
        rows = [
            ['original', [], 'Q1', None, False, None],
            ['conflict', [], 'Q2', None, False, None],
            ['no_evidence', [], 'Q3', None, False, None],
            ['has_bangumi', [], 'Q4', 123, False, None],
            ['first', [], 'Q5', None, False, None],
            ['second', [], 'Q5', None, False, None],
            ['costume', [], 'Q6', None, False, 'auto'],
        ]
        records = {
            tag: [{'item': f'http://www.wikidata.org/entity/{qid}', 'zh_hans': '林・晓'}]
            for tag, _, qid, *_ in rows
        }
        records['conflict'].append({'item': 'http://www.wikidata.org/entity/Q2', 'zh_hans': '同名异写'})
        records['bad_qid'] = [{'item': 'http://www.wikidata.org/entity/Q0'}]
        self.assertIsNone(wikidata_labels.qid_for(['original', 'has_bangumi'], records))
        self.assertIsNone(wikidata_labels.qid_for(['bad_qid'], records))
        translations = {tag: {'zh': '林·晓（作品）'} for tag in records if tag != 'no_evidence'}
        self.assertEqual(wikidata_labels.reviewed_labels(rows, columns, records, translations), {'Q1': '林・晓'})
    def test_reassigned_alias_and_removed_highest_id_do_not_reuse_ids(self):
        previous = {'columns': ['id', 'tag', 'tags'], 'id_high_water': 7,
                    'characters': [[3, 'b', ['a']], [5, 'old', []]]}
        ids, limit = build.assign_ids({'a': [], 'b': [], 'renamed': ['old']}, previous)
        self.assertEqual((ids, limit), ({'a': 8, 'b': 3, 'renamed': 5}, 8))

    def test_costume_requires_evidence_for_this_character(self):
        self.assertTrue(reviewed.appearance('a_(red)_(w)', 'a_(w)', 'Official costume of [[A (W)|]].'))
        for child, parent, paragraph in (
            ('arcane_vi', 'vi_(league)', 'As she appears in a show. Also released as a skin of [[Vi (League)|]].'),
            ('a_(armor)', 'a', 'A playable [[official alternate costume|battlesuit]] of [[A]], her transformed form.'),
            ('a_(swimsuit_archer)', 'a', 'The Swimsuit Archer variant of [[A]].'),
            ('a_(maid)', 'a', 'Official costume of [[B]].'),
            ('a_(uniform)', 'a', ''),
        ):
            self.assertFalse(reviewed.appearance(child, parent, paragraph), child)
