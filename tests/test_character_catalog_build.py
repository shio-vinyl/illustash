"""角色词表的公共 ID 和自动挂载审核边界。"""
import unittest

from tools.character_catalog import build, reviewed


class CharacterCatalogBuildTest(unittest.TestCase):
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
