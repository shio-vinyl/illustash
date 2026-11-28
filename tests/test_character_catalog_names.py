"""合成角色、作品和别名上的 Bangumi 目录对应。"""
import unittest

from tools.character_catalog import build
from tools.character_catalog.match_bangumi import Matcher


def character(name, chinese, **labelled):
    return {'name': name, 'zh': chinese, 'raw_zh': chinese,
            'names': {name, *labelled.values()}, 'labelled': labelled}


class CatalogNamesTest(unittest.TestCase):
    def matcher(self, characters, wiki):
        return Matcher(
            characters, {1: {'name': 'テスト', 'name_cn': '样例作品'}}, {cid: [(1, 1)] for cid in characters},
            {'example': {'other_names': ['テスト']},
             **{tag: {'other_names': names} for tag, names in wiki.items()}},
            {}, {'example': 2}, dict.fromkeys(wiki), {}, {},
        )

    def test_shared_aliases_do_not_match_a_group_to_a_member(self):
        alice, bob = 'alice_(example)', 'bob_(example)'
        matcher = self.matcher(
            {1: character('雛コンビ', '合成双子'), 2: character('アリス', '爱丽丝'),
             3: character('同僚', '同学'), 4: character('ボブ', '鲍勃')},
            {alice: ['雛コンビ', '同僚', 'アリス'], bob: ['同僚（样例作品）', 'ボブ']},
        )
        self.assertEqual(matcher.match([alice])['id'], 2)
        self.assertEqual(matcher.match([bob])['id'], 4)

    def test_segment_matches_the_tag_or_remains_unresolved(self):
        tag = 'ghostblade_(example)'
        matcher = self.matcher(
            {1: character('王一 / 默杀', '王一 / 默杀', 英文名='Wang Yi / GHOSTBLADE'),
             2: character('田中明 / 変身', '田中明 / 变身')},
            {tag: ['王一', '默杀'], 'other_(example)': ['田中明', '変身']},
        )
        self.assertEqual(matcher.match([tag])['id'], 1)
        self.assertEqual(matcher.pick(1, [tag]), 'p1')
        self.assertEqual(matcher.pick(2, ['other_(example)']), '?')

    def test_han_japanese_original_keeps_the_explicit_chinese_name(self):
        tag = 'red_blood_cell_(example)'
        matcher = self.matcher({1: character('赤血球', '红血球')}, {tag: ['赤血球']})
        self.assertEqual(matcher.pick(1, [tag]), '')


    def test_global_rejects_known_work_conflict(self):
        chi = '石川美月'
        assigned = [f'mizuki_({work})' for work in ('other', 'unindexed', 'no_work')]
        wiki = {tag: {'other_names': [chi]} for tag in assigned}
        subjects = {10: {'name': 'Origin', 'name_cn': ''}, 11: {'name': 'Other', 'name_cn': ''}}
        matcher = Matcher({7: character(chi, chi)}, subjects, {7: [(10, 1)]}, wiki, {},
                          {'origin', 'other', 'unindexed'}, {}, {}, {})
        review = set()
        self.assertIsNone(matcher.match([assigned[0]], review))
        self.assertEqual(matcher.match([assigned[1]], review)['how'], 'global')
        self.assertEqual({tag for _, tag, cid in review if cid == '7'}, {assigned[0]})
        self.assertEqual(matcher.match([assigned[2]])['how'], 'global')
        self.assertEqual(matcher.match(['mizuki_(origin)', assigned[2]])['how'], 'work')

    def test_global_keeps_a_work_with_unmapped_subject_evidence(self):
        tag = 'mizuki_(unindexed)'
        wiki = {tag: {'other_names': ['石川美月']}}
        matcher = Matcher({7: character('石川美月', '石川美月')},
                          {10: {'name': 'Unmapped Title', 'name_cn': ''}}, {7: [(10, 1)]},
                          wiki, {}, {'unindexed'}, {}, {}, {})
        self.assertEqual(matcher.match([tag])['how'], 'global')

    def test_global_keeps_existing_corroboration_for_numbered_sequel(self):
        tag = 'mizuki_(example_series)'
        chi = '石川美月'
        subjects = {
            10: {'name': 'Example Series', 'name_cn': '样例系列'},
            11: {'name': 'Example Series 2', 'name_cn': '样例系列2'},
        }
        wiki = {tag: {'other_names': [chi]}, 'sequel': {'other_names': ['Example Series 2']}}
        matcher = Matcher({7: character(chi, chi)}, subjects, {7: [(11, 1)]}, wiki, {},
                          {'example_series', 'sequel'}, {}, {}, {})
        self.assertEqual(matcher.match([tag])['how'], 'global')

    def test_global_keeps_corroborated_character_in_named_subwork(self):
        tag = 'mizuki_(example_series)'
        chi = '石川美月'
        subjects = {10: {'name': 'Example Series', 'name_cn': ''}, 11: {'name': 'Aquamarine', 'name_cn': ''}}
        wiki = {tag: {'other_names': [chi], 'body': 'A character in [[Aquamarine]].'}}
        matcher = Matcher({7: character(chi, chi)}, subjects, {7: [(11, 1)]}, wiki, {},
                          {'example_series', 'aquamarine'}, {}, {}, {})
        self.assertEqual(matcher.match([tag])['how'], 'global')

    def test_duplicate_id_requires_related_tags_or_unique_work_evidence(self):
        names = {7: character('石川美月', '石川美月'), 8: character('王月影', '王月影')}
        subjects = {10: {'name': 'Origin', 'name_cn': ''}, 11: {'name': 'Other', 'name_cn': ''}}
        matcher = Matcher(names, subjects, {7: [(10, 1)], 8: [(10, 1), (11, 1)]}, {}, {},
                          {'origin', 'other'}, {}, {}, {})

        def row(tag, bid):
            values = {'tag': tag, 'bangumi': bid, 'pick': 'name'}
            return [values.get(key) for key in build.COLUMNS]

        original, collision = 'mizuki_(origin)', 'mizuki_(other)'
        left, right = 'getsuei_(origin)', 'getsuei_(other)'
        rows = [row(tag, bid) for tag, bid in ((original, 7), (collision, 7), (left, 8), (right, 8))]
        review = set()
        self.assertEqual(build.clear_work_conflicts(rows, matcher, {}, review), 3)
        self.assertEqual({tag for _, tag, _ in review}, {collision, left, right})
        self.assertEqual([row[build.COLUMNS.index('bangumi')] for row in rows], [7, None, None, None])
        self.assertEqual([row[build.COLUMNS.index('pick')] for row in rows], ['name', None, None, None])

        related = [row(left, 8), row(right, 8)]
        self.assertEqual(build.clear_work_conflicts(related, matcher, {right: (left, 'unknown')}, set()), 0)
