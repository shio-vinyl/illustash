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
