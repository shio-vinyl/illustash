"""首版词表的审核规则。每条自动关系均须有生效的蕴含、同一本名和指向本体的文字证据。"""
import re

from tools.character_catalog import tagparse

# 按规范 tag 对照缓存中同名 wiki 的定义。只收明确以多人或组织为指代对象的条目。
GROUPS = frozenset({'adepta_sororitas', 'adeptus_astartes', 'cerberus_(helltaker)', "t'au", 'tamamo_(fate)'})
# 这只具名宝可梦是物种的一个具体个体。蕴含用于搜图，没有需要用户裁决的身份关系。
INSTANCES = frozenset({('peakychu', 'pikachu')})

# 逐条核对的角色条目。作品条目与本人 ID 由公开资料确认。
BANGUMI = {
    # https://endfield.hypergryph.com/news/6094 及 https://bgm.tv/character/208373
    'si_(arknights)': (208373, 374638, 'name'),
    # 17449 以《死或生》本体为主；另有《碧蓝航线》联动。参见 71967 和原作角色页。
    # https://teamninja-studio.com/doa6/lastround/tch/characters/kasumi.html
    'kasumi_(doa)': (17449, 71967, ''),
}

LINK = re.compile(r'\[\[([^\]|]+)(?:\|([^\]]*))?\]\]')
COSTUME = re.compile(r'\b(costumes?|outfits?|attire|dress|swimsuit|bikini|kimono|yukata|uniform|skin|clothes|clothing|mystic code)\b', re.I)
AMBIGUOUS = re.compile(r'\b(form|transformation|battlesuit|remodel|counterpart|version of|reincarnation|impersonat\w*|disguised as)\b', re.I)
ASCENSION = re.compile(r'^(?:first|second|third|fourth|final|[1-4](?:st|nd|rd|th))_ascension$')


def appearance(child, parent, paragraph):
    """只有 wiki 首句既指向本体又说明造型，才批准同名、附加限定词的蕴含。"""
    extra = tagparse.diff(child, parent)
    if not extra or any(token in {'archer', 'assassin', 'avenger', 'berserker', 'caster', 'foreigner', 'lancer',
                                  'mooncancer', 'pretender', 'rider', 'ruler', 'saber', 'shielder'}
                        for qualifier in extra for token in qualifier.split('_')):
        return False
    lead = re.split(r'(?:[.!?](?:\s|$)|[\r\n])', paragraph, maxsplit=1)[0]
    links = {m[1].lower().replace(' ', '_') for m in LINK.finditer(lead)}
    plain = LINK.sub(lambda m: m[2] or m[1], lead)
    if parent not in links or AMBIGUOUS.search(plain):
        return False
    # Fate 的灵基阶段是已定的造型类别；限定词须直接修饰同一角色的同一职阶。
    if len(extra) == 1 and ASCENSION.fullmatch(extra[0]) and 'fate' in tagparse.split(parent)[1]:
        return bool(re.search(r'\bascension\b', plain, re.I))
    return bool(COSTUME.search(plain))

# 人工核对的形态。共享 Bangumi ID 仍须满足相容的作品范围。
FORMS = {
    # https://www.starwars.com/databank/darth-vader
    'darth_vader': 'anakin_skywalker',
    # https://sailormoon-official.com/information/crystal3_129.php
    'sailor_moon': 'tsukino_usagi',
    'sailor_mercury': 'mizuno_ami',
    'sailor_mars': 'hino_rei',
    'sailor_jupiter': 'kino_makoto',
    'sailor_venus': 'aino_minako',
    'sailor_saturn': 'tomoe_hotaru',
    # D 站形态页记载 HDD，紫心页还描述变身前后差别。
    'purple_heart_(neptunia)': 'neptune_(neptunia)',
    'black_heart_(neptunia)': 'noire_(neptunia)',
    'white_heart_(neptunia)': 'blanc_(neptunia)',
    'green_heart_(neptunia)': 'vert_(neptunia)',
    'purple_sister': 'nepgear',
    # https://www.hoyolab.com/article/29770753
    'sam_(honkai:_star_rail)': 'firefly_(honkai:_star_rail)',
    # https://wutheringwaves.kurogames.com/en/main/news/detail/4918
    'fleurdelys_(wuthering_waves)': 'cartethyia_(wuthering_waves)',
    # 第四章第五幕「罪人舞步旋」：人性与神格，保留两个可区分实体。
    'focalors_(genshin_impact)': 'furina_(genshin_impact)',
    # D 站角色页及原作剧情分别核对。
    'yagi_toshinori': 'all_might',
    'kamukura_izuru': 'hinata_hajime',
    'kokushibou': 'tsugikuni_michikatsu',
    'yami_yugi': 'atem',
    'exs-tia_flora': 'amakubo_yuri',
    'leoparde_(mahou_shoujo_ni_akogarete)': 'araga_kiwi',
    'magia_baiser': 'hiiragi_utena',
    'venera-sama': 'hoshimiya_kate',
    'kazamaki_matsuri_(female)': 'kazamaki_matsuri',
    'leafa_(sao)': 'kirigaya_suguha',
    'shy_(character)': 'momijiyama_teru',
    'phosphophyllite_(ll)': 'phosphophyllite',
    'ro-500_(kancolle)': 'u-511_(kancolle)',
    'boudica_(first_ascension)_(fate)': 'boudica_(fate)',
    # Bangumi 分列为两个条目，D 站角色页明确记载装甲、化名或另一形态。
    'angelina_the_mellow_wish_(arknights)': 'angelina_(arknights)',
    "ch'en_the_dawnstreak_(arknights)": "ch'en_(arknights)",
    "ch'en_the_holungday_(arknights)": "ch'en_(arknights)",
    'carmilla_(fate)': 'elizabeth_bathory_(fate)',
    'dan_heng_(imbibitor_lunae)_(honkai:_star_rail)': 'dan_heng_(honkai:_star_rail)',
    'dan_heng_(permansor_terrae)_(honkai:_star_rail)': 'dan_heng_(honkai:_star_rail)',
    'edogawa_conan': "kudou_shin'ichi",
    'exs-tia': 'katsuragi_marina',
    'eyjafjalla_the_hvit_aska_(arknights)': 'eyjafjalla_(arknights)',
    'fugue_(honkai:_star_rail)': 'tingyun_(honkai:_star_rail)',
    'gavial_the_invincible_(arknights)': 'gavial_(arknights)',
    "jeanne_d'arc_alter_santa_lily_(fate)": "jeanne_d'arc_alter_(fate)",
    'jessica_the_liberated_(arknights)': 'jessica_(arknights)',
    'kroos_the_keen_glint_(arknights)': 'kroos_(arknights)',
    'lappland_the_decadenza_(arknights)': 'lappland_(arknights)',
    'nearl_the_radiant_knight_(arknights)': 'nearl_(arknights)',
    'reed_the_flame_shadow_(arknights)': 'reed_(arknights)',
    'skadi_the_corrupting_heart_(arknights)': 'skadi_(arknights)',
    'specter_the_unchained_(arknights)': 'specter_(arknights)',
    'texas_the_omertosa_(arknights)': 'texas_(arknights)',
    # Danbooru 角色页中明确记载变身者与常用身份的对应。
    'cure_answer': 'akechi_anna',
    'cure_aqua': 'minazuki_karen',
    'cure_beauty': 'aoki_reika',
    'cure_black': 'misumi_nagisa',
    'cure_blossom': 'hanasaki_tsubomi',
    'cure_butterfly': 'hijiri_ageha',
    'cure_chocolat': 'kenjou_akira',
    'cure_diamond': 'hishikawa_rikka',
    'cure_earth': 'fuurin_asumi',
    'cure_echo': 'sakagami_ayumi',
    'cure_flora': 'haruno_haruka',
    'cure_fortune': 'hikawa_iona',
    'cure_gelato': 'tategami_aoi',
    'cure_grace': 'hanadera_nodoka',
    'cure_heart': 'aida_mana',
    'cure_honey': 'oomori_yuuko',
    'cure_idol': 'sakura_uta_(precure)',
    'cure_kyunkyun': 'shigure_kokoro',
    'cure_macaron': 'kotozume_yukari',
    'cure_marine': 'kurumi_erika',
    'cure_melody': 'houjou_hibiki',
    'cure_milky': 'hagoromo_lala',
    'cure_mint': 'akimoto_komachi',  # https://www.toei-anim.co.jp/tv/yes_precure5/character/mint.html
    'cure_miracle': 'asahina_mirai',
    'cure_moonlight': 'tsukikage_yuri',
    'cure_muse': 'shirabe_ako',
    'cure_mystique': 'kobayashi_mikuru',
    'cure_passion': 'higashi_setsuna',
    'cure_peace': 'kise_yayoi',
    'cure_peach': 'momozono_love',
    'cure_precious': 'nagomi_yui',
    'cure_rhythm': 'minamino_kanade',
    'cure_rosetta': 'yotsuba_alice',
    'cure_sunshine': 'myoudouin_itsuki',
    'cure_sword': 'kenzaki_makoto',
    'cure_twinkle': 'amanogawa_kirara',
    'cure_whip': 'usami_ichika',
    'cure_white': 'yukishiro_honoka',
    'cure_wing': 'yuunagi_tsubasa',
}
# 曾用于同一角色的旧 tag。公开 ID 各自保留，按整对询问处理。
RENAMED_TAGS = {
    # 旧角色页 https://danbooru.donmai.us/wiki_page_versions/636229.json
    # 原帖换标 https://danbooru.donmai.us/post_versions/60201909.json
    'the_shorekeeper_(wuthering_waves)': 'shorekeeper_(wuthering_waves)',
    # 旧角色页 https://danbooru.donmai.us/wiki_page_versions/593423.json
    # 原帖换标 https://danbooru.donmai.us/post_versions/59132896.json
    'hoshino_ai_(oshi_no_ko)': 'hoshino_ai',
}

# 漫画里的替身、共享代号和复制品不能靠同 ID 归为形态。
BANGUMI_EXCLUSIONS = {
    'star_platinum': {878},  # 这是空条承太郎本人的条目。
    'pollux_(fate)': {76639},  # Bangumi 记录的是双子合称，单人的原名另有对应。
    "mdr_(girls'_frontline)": {131879},  # 此条目以《云图计划》的另一身份为名。
}
DISTINCT = {
    frozenset(('kujo_jotaro', 'star_platinum')),
    frozenset(('kaitou_kid', 'kuroba_kaito')),
    frozenset(('sparkle_(doll)_(honkai:_star_rail)', 'sparkle_(honkai:_star_rail)')),
    frozenset(('kusuriuri_(kon_no_tsurugi)_(mononoke)', 'kusuriuri_(ri_no_tsurugi)_(mononoke)')),
}
