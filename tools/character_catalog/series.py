"""D 站系列总标签对应的 Bangumi 子作。标题规则只用于缩小角色匹配范围。"""
import re
import unicodedata

# 系列总 tag 可以跨其子作找角色。具体子作的 tag 仍只在该子作匹配。
# 角色共同出现、条目间任意关联及全库命中均不能证明作品从属。
TITLES = {
    'fate': r'Fate[/／](?:stay night|Zero|hollow ataraxia|Grand Order|EXTRA|EXTELLA|Apocrypha|'
            r'Prototype|strange Fake|kaleid liner|Samurai Remnant|Requiem|Labyrinth|Lost Einherjar|'
            r'tiger colosseum|unlimited codes|complete material|school life)',
    'fire_emblem': r'ファイアーエムブレム|Fire Emblem\b|火[焰炎]纹章|火[焰炎]紋章',
    "girls'_frontline": r'少女前[线線]|ドールズフロントライン|Girls[’\x27] Frontline\b|'
                        r'ニューラルクラウド|少女前[线線][：: ]*云[图圖]计划',
    'mega_man': r'ロックマン|Mega[ -]?Man\b|洛克人|洛克人系列',
    'pokemon': r'ポケットモンスター|Pok[eé]mon\b|宝可梦|寶可夢|精灵宝可梦|寵物小精靈|神奇寶貝',
    'project_moon': r'(?:Lobotomy Corporation|Library of Ruina|Limbus Company)\b|'
                    r'脑叶公司|腦葉公司|废墟图书馆|廢墟圖書館|边狱巴士|邊獄巴士|'
                    r'로보토미 코퍼레이션|라이브러리 오브 루이나|림버스 컴퍼니',
    'touhou': r'[東东]方(?:靈異[伝傳]|灵异传|封魔[録录錄]|夢時空|梦时空|幻想[郷乡]|怪[綺绮]談|'
              r'紅魔[郷鄉]|红魔乡|妖々夢|妖妖梦|萃夢想|萃梦想|永夜抄|花映塚|文花帖|'
              r'風神[録錄]|风神录|緋想天|绯想天|地[霊靈灵]殿|星蓮船|星莲船|非想天則|非想天则|'
              r'神[霊靈灵][廟庙]|心[綺绮][楼樓]|輝[針针]城|辉针城|深秘[録录錄]|'
              r'紺珠[伝傳]|绀珠传|憑依華|凭依华|天空璋|鬼形獣|鬼形兽|虹[龍龙]洞|'
              r'獣王園|兽王园|錦上京|锦上京|紅[龍龙]洞|红龙洞|翠夢想|三月精|儚月抄|'
              r'茨歌仙|鈴奈庵|铃奈庵|智[霊靈灵]奇[伝传傳]|醉蝶華|醉蝶华|香霖堂|求聞史紀|求闻史纪)',
    'xenoblade': r'ゼノブレイド|Xenoblade Chronicles\b|异度神[剑劍]|異度神[剣劍]',
}
# Danbooru 的系列总标签不一定与角色 tag 的限定词使用同一拼写。
SERIES_COPYRIGHTS = {
    'fate_(series)': 'fate',
    'mega_man_(series)': 'mega_man',
    'xenoblade_chronicles_(series)': 'xenoblade',
}
LEAF_QUALIFIERS = {
    "girls'_frontline_2": "girls'_frontline_2:_exilium",
    'xenoblade_x': 'xenoblade_chronicles_x',
}
PATTERNS = {work: re.compile(f'(?:{title})', re.IGNORECASE) for work, title in TITLES.items()}


def subjects_by_series(subjects):
    pools = {work: set() for work in PATTERNS}
    for sid, subject in subjects.items():
        if subject.get('type') not in {1, 2, 4, 6}:  # 书籍、动画、游戏、影视
            continue
        titles = [unicodedata.normalize('NFKC', subject[field]) for field in ('name', 'name_cn')]
        for work, title in PATTERNS.items():
            if any(title.match(name) for name in titles):
                pools[work].add(sid)
    return pools
