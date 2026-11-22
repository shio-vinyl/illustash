"""D 站角色 tag 的字面拆分（只用来找候选，不直接定关系）。

`name_(q1)_(q2)` 拆成本体名和括号限定词列表。括号里可能是作品、造型、形态、组织、称号，也可能是别的人，
所以这里不解释限定词的含义；`diff(child, parent)` 只回答「child 比 parent 多了哪几个限定词」。
"""
import re

TAIL = re.compile(r'_\(([^()]*)\)$')


def split(tag):
    quals = []
    base = tag
    while True:
        m = TAIL.search(base)
        if not m:
            break
        quals.insert(0, m.group(1))
        base = base[:m.start()]
    return base, quals


def diff(child, parent):
    """child 与 parent 本体名相同、parent 的限定词按顺序都在 child 里时，返回 child 多出来的限定词；否则 None。"""
    cb, cq = split(child)
    pb, pq = split(parent)
    if cb != pb:
        return None
    rest = list(cq)
    for q in pq:
        if q not in rest:
            return None
        rest.remove(q)
    return rest


def readable(tag):
    """可读名：去掉限定词，下划线换空格，首字母大写（只作英文名的最后退路）。"""
    base, _ = split(tag)
    words = base.replace('_', ' ').split()
    return ' '.join(w[:1].upper() + w[1:] for w in words)
