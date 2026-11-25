"""合成角色、作品和别名上的 Bangumi 目录对应。"""
import unittest

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
