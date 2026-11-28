"""给范围内每个角色 tag 找候选关系，并按证据分级（#834 制作端）。

关系种类（和 #834 2026-11-22 评论第 3 节的表一致）：

- `redirect`：D 站已改名，旧 tag 指向新 tag（别名）。确定，自动用；
- `appearance`：造型（换装、灵基、节日服）。审核过的自动挂到本体下；
- `form`：另一种形态（变身、平行版本、年龄、性转、换了身体）。不自动合并，整对问用户（#829）；
- `instance`：物种、职业形象下的具体个体（几只不同的皮卡丘都蕴含 pikachu）。各自是独立角色；
- `unknown`：有蕴含、判断不出是哪一种。只出题，不自动改；
- `prefix`：只有名字前缀相同、D 站没有蕴含（`kei_(x)_(w)` 和 `kei_(w)`）。不出题、不挂，留给审核看。

判断只用证据：D 站的别名和蕴含定「有没有关系」，限定词和 wiki 第一段定「是哪一种」。
限定词表只收常见、几乎不会误判的词；判断不了的一律退到 `unknown`，不猜。
"""
import collections
import re

from tools.character_catalog import tagparse

# 明确是换装的限定词（整词或词头）。灵基、精英化按用户 2026-11-22 的说法算造型（#833）。
APPEARANCE_WORDS = {
    'swimsuit', 'swimwear', 'bikini', 'beach', 'summer', 'winter', 'spring', 'autumn', 'fall',
    'new_year', 'new_year\'s', 'newyear', 'halloween', 'christmas', 'xmas', 'valentine', 'valentines', 'white_day',
    'tanabata', 'birthday', 'anniversary', 'wedding', 'bride', 'bridal', 'wedding_dress',
    'kimono', 'yukata', 'furisode', 'hakama', 'qipao', 'china_dress', 'cheongsam',
    'maid', 'bunny', 'bunny_girl', 'playboy_bunny', 'nurse', 'police', 'cheerleader', 'cheer_squad', 'idol',
    'casual', 'casual_wear', 'casual_clothes', 'plain_clothes', 'civilian', 'private_clothes', 'off-duty',
    'school_uniform', 'uniform', 'gym_uniform', 'track', 'tracksuit', 'sportswear', 'racing', 'race_queen',
    'pajamas', 'sleepwear', 'nightwear', 'pyjamas', 'lingerie', 'dress', 'party_dress', 'formal', 'formal_dress',
    'evening_gown', 'gown', 'band', 'stage', 'concert', 'live', 'outfit', 'costume', 'skin', 'hot_spring',
    'onsen', 'camp', 'camping', 'apron', 'kitchen', 'winter_clothes', 'winter_uniform', 'summer_uniform',
    'first_ascension', 'second_ascension', 'third_ascension', 'final_ascension', 'fourth_ascension',
    '1st_ascension', '2nd_ascension', '3rd_ascension', '4th_ascension',
    'elite_i', 'elite_ii', 'elite_1', 'elite_2', 'promotion',
    '1st_costume', '2nd_costume', '3rd_costume', '4th_costume', '5th_costume', 'new_costume', 'default_costume',
    'original_costume', 'alternate_costume', 'alternate_outfit', 'new_outfit', 'new_year_costume',
}
APPEARANCE_PREFIXES = ('swimsuit_', 'casual_', 'school_uniform', 'new_year', 'summer_', 'winter_', 'christmas',
                       'halloween', 'birthday', 'wedding', 'bunny_', 'maid_', 'idol_', 'dress_', 'kimono',
                       'yukata', 'pajama')
APPEARANCE_SUFFIXES = ('_costume', '_outfit', '_uniform', '_dress', '_swimsuit', '_ascension', '_skin',
                       '_clothes', '_attire', '_wear', '_kimono', '_yukata', '_bikini')

# 另一种形态（换了身体、年龄、性别、平行版本、变身）。这些出题，不自动挂。
FORM_WORDS = {
    'alter', 'young', 'younger', 'child', 'kid', 'loli', 'shota', 'adult', 'older', 'old', 'elder', 'grown',
    'future', 'past', 'female', 'male', 'genderswap', 'gender_swap', 'genderbend', 'rule_63', 'human',
    'humanized', 'human_form', 'beast', 'beast_form', 'animal', 'true_form', 'final_form', 'transformed',
    'transformation', 'awakened', 'corrupted', 'evil', 'dark', 'shadow', 'berserk', 'possessed', 'demon',
    'devil', 'mecha', 'robot', 'machine', 'ghost', 'spirit', 'doll', 'mini', 'chibi', 'student', 'original',
    'lily', 'santa', 'alternate', 'alternative', 'another', 'clone', 'copy', 'reincarnation', 'avatar',
    'saber', 'archer', 'lancer', 'rider', 'caster', 'assassin', 'berserker', 'ruler', 'avenger', 'alter_ego',
    'mooncancer', 'moon_cancer', 'foreigner', 'pretender', 'shielder', 'beast_class', 'unbeast',
}
FORM_SUFFIXES = ('_form', '_mode', '_version', '_ver.')

WIKI_APPEARANCE = re.compile(
    r'\b(outfit|costume|attire|swimsuit|bikini|uniform|kimono|yukata|skin|spiritron dress|mystic code|'
    r'wardrobe|getup|clothes|clothing)\b', re.I)
WIKI_FORM = re.compile(
    r'\b(alternate (?:form|version|self|universe)|alter(?:nate)? ego|younger (?:self|version)|older (?:self|version)|'
    r'(?:child|adult|future|past) (?:self|version|form)|transformation|transformed|true form|final form|'
    r'incarnation|counterpart|parallel (?:world|universe)|gender-?(?:bent|swapped|flipped)|possessed|'
    r'reincarnat\w*|clone|another version|different version|other self)\b', re.I)
WIKI_INSTANCE = re.compile(r'\b(species|pok[eé]mon (?:owned|belonging)|individual|specific)\b', re.I)
WIKI_GROUP = re.compile(r'\b(duo|trio|group|unit|team|pair|twins|sisters|brothers|siblings|squad|band) (?:of|consisting|composed|made up|formed)\b', re.I)


# 限定词里出现这些词就不自动挂（宁可多问一次）：律者、Alter、觉醒、真身、年龄、性别、换了身体……
# 「beast_style」「little_devil」这类其实是衣服的也会被拦下，代价只是多出一道题。
FORM_TOKENS = {
    'herrscher', 'alter', 'awakened', 'awakening', 'possessed', 'corrupted', 'transformed', 'transformation',
    'true', 'form', 'forme', 'young', 'younger', 'child', 'kid', 'adult', 'older', 'elder', 'future', 'past',
    'female', 'male', 'human', 'humanized', 'robot', 'mecha', 'android', 'ghost', 'spirit', 'beast', 'demon',
    'devil', 'evil', 'dark', 'shadow', 'corruption', 'reborn', 'reincarnation', 'clone', 'another', 'other',
    'genderswap', 'rule_63', 'lily', 'santa', 'loli', 'shota', 'baby', 'old',
}


def form_token(qualifier):
    return any(t in FORM_TOKENS for t in re.split(r'[_\s:\-]+', qualifier.lower()))


def word_kind(qualifier):
    q = qualifier.lower()
    if q in FORM_WORDS or q.endswith(FORM_SUFFIXES) or form_token(q):
        return 'form'
    if q in APPEARANCE_WORDS or q.startswith(APPEARANCE_PREFIXES) or q.endswith(APPEARANCE_SUFFIXES):
        return 'appearance'
    return None


def first_paragraph(body):
    if not body:
        return ''
    text = body.replace('\r', '')
    return text.split('\n\n', 1)[0][:600]


def wiki_kind(body):
    p = first_paragraph(body)
    form = WIKI_FORM.search(p)
    look = WIKI_APPEARANCE.search(p)
    if form and not look:
        return 'form', form.group(0)
    if look and not form:
        return 'appearance', look.group(0)
    if look and form:
        return 'mixed', f'{look.group(0)} / {form.group(0)}'
    return None, ''


def classify_pair(child, parent, wiki):
    """child 蕴含 parent 时判断关系种类，返回 (种类, 证据说明)。"""
    extra = tagparse.diff(child, parent)
    words = [word_kind(q) for q in extra] if extra else []
    wkind, wtext = wiki_kind((wiki.get(child) or {}).get('body', ''))
    if extra and words and all(w == 'appearance' for w in words):
        if wkind in (None, 'appearance', 'mixed'):
            return 'appearance', f'限定词 {extra}' + (f'；wiki「{wtext}」' if wtext else '')
        return 'unknown', f'限定词 {extra} 像造型，wiki「{wtext}」像形态'
    if extra and any(w == 'form' for w in words):
        return 'form', f'限定词 {extra}' + (f'；wiki「{wtext}」' if wtext else '')
    if wkind == 'appearance' and (extra is None or not any(form_token(q) for q in extra)):
        if extra is None and form_token(child.replace(parent, '')):
            return 'unknown', f'名字不同，wiki「{wtext}」'
        return 'appearance', f'wiki「{wtext}」' + (f'；限定词 {extra}' if extra else '')
    if wkind == 'form':
        return 'form', f'wiki「{wtext}」' + (f'；限定词 {extra}' if extra else '')
    return 'unknown', (f'限定词 {extra}' if extra else '名字不同') + (f'；wiki「{wtext}」' if wtext else '')


def analyze(scope, graph, wiki):
    """返回每个范围内 tag 的记录：规范名、上级、关系种类和证据。"""
    out = {}
    canon_count = collections.Counter(graph.canonical(t) for t in scope)
    for tag in sorted(scope):
        canon = graph.canonical(tag)
        rec = {'tag': tag, 'canonical': canon, 'sources': sorted(scope[tag].sources)}
        if canon != tag:
            rec['redirect'] = canon
            rec['duplicate'] = canon in scope or canon_count[canon] > 1
        parents = graph.parents(canon)
        if parents:
            rec['parents'] = sorted(parents)
            kinds = {p: classify_pair(canon, p, wiki) for p in parents}
            rec['relations'] = {p: {'kind': k, 'why': why} for p, (k, why) in kinds.items()}
        else:
            base, quals = tagparse.split(canon)
            if len(quals) >= 2:
                shorter = f'{base}_({quals[-1]})'
                if shorter not in (tag, canon) and (graph.canonical(shorter) in scope or shorter in graph.tags):
                    rec['prefix'] = shorter
        out[tag] = rec
    return out
