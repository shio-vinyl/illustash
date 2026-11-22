"""名字文字的判断和归一化（制作端；判断口径和应用 `app_name_sources.rs` 的一致）。

- 「能当简体中文用」：去掉间隔号后每个字都在 GB2312 里（应用用同一张 GB2312 范围表）；
- 含假名的是日文，含谚文的是韩文；
- 比较前去掉末尾的括号限定词（「甘雨（原神）」→「甘雨」），统一间隔号，去空白，NFKC。
"""
import re
import unicodedata

DOTS = '·・･•．'
QUALIFIER = re.compile(r'^(.+?)\s*[(（]([^()（）]+)[)）]$')


def nfkc(s):
    return unicodedata.normalize('NFKC', s)


def kana(s):
    return any('぀' <= c <= 'ヿ' for c in s)


def han(s):
    return any('㐀' <= c <= '鿿' for c in s)


def hangul(s):
    return any('가' <= c <= '힣' for c in s)


def latin(s):
    return any(c.isascii() and c.isalpha() for c in s)


def gb2312(s):
    try:
        s.encode('gb2312')
        return True
    except UnicodeEncodeError:
        return False


def cp932(s):
    try:
        s.encode('cp932')
        return True
    except UnicodeEncodeError:
        return False


def unify_dots(s):
    return ''.join('·' if c in DOTS else c for c in s)


def strip_qualifiers(s):
    """去掉末尾的一个或多个括号限定词，返回 (本体, [限定词…])。"""
    s = unify_dots(nfkc(s)).strip()
    quals = []
    while True:
        m = QUALIFIER.match(s)
        if not m:
            break
        s = m.group(1).strip()
        quals.insert(0, m.group(2).strip())
    return s, quals


def key(s):
    """比较用的键：去限定词、去间隔号和空白、小写。"""
    base, _ = strip_qualifiers(s)
    return ''.join(c for c in base if c not in '·' and not c.isspace()).casefold()


def simplified_usable(s):
    """应用口径：有汉字、没有假名和谚文、去掉间隔号后全在 GB2312 里。"""
    base, _ = strip_qualifiers(s)
    return han(base) and not kana(base) and not hangul(base) and gb2312(base.replace('·', ''))


def script(s):
    """粗分文字：ja（含假名）、ko、han（只有汉字，中日未定）、latin、other。"""
    if kana(s):
        return 'ja'
    if hangul(s):
        return 'ko'
    if han(s):
        return 'han'
    if latin(s):
        return 'latin'
    return 'other'
