"""把 D 站的别名、蕴含读成图（#834 制作端）。

- `canonical(tag)`：沿生效的别名走到底（D 站的别名不成链，这里仍按链处理，防环）；
- `parents(tag)`：这个 tag 蕴含的、当前是角色类（category 4）的 tag，先把两端都换成规范名；
- `children(tag)`：反方向。

蕴含表达的是「标了 A 的图也该标 B」，不等于身份关系；这里只建图，关系种类由 relations.py 判断。
"""
import collections

CHARACTER = 4


class Graph:
    def __init__(self, tags, aliases, implications):
        self.tags = {r['name']: r for r in tags}
        self.alias = {}
        for row in aliases:
            if row.get('status', 'active') == 'active':
                self.alias[row['antecedent_name']] = row['consequent_name']
        self.up = collections.defaultdict(set)
        self.down = collections.defaultdict(set)
        for row in implications:
            if row.get('status', 'active') != 'active':
                continue
            a = self.canonical(row['antecedent_name'])
            b = self.canonical(row['consequent_name'])
            if a != b:
                self.up[a].add(b)
                self.down[b].add(a)

    def canonical(self, tag):
        seen = set()
        while tag in self.alias and tag not in seen:
            seen.add(tag)
            tag = self.alias[tag]
        return tag

    def category(self, tag):
        row = self.tags.get(tag)
        return row['category'] if row else None

    def posts(self, tag):
        row = self.tags.get(tag)
        return row['post_count'] if row else 0

    def parents(self, tag, character_only=True):
        tag = self.canonical(tag)
        return {p for p in self.up.get(tag, ()) if not character_only or self.category(p) == CHARACTER}

    def children(self, tag, character_only=True):
        tag = self.canonical(tag)
        return {c for c in self.down.get(tag, ()) if not character_only or self.category(c) == CHARACTER}

    def ancestors(self, tag, character_only=True):
        """沿蕴含往上的所有角色类 tag（不含自己）。"""
        out, stack = set(), [self.canonical(tag)]
        while stack:
            for p in self.parents(stack.pop(), character_only):
                if p not in out:
                    out.add(p)
                    stack.append(p)
        return out

    def roots(self, tag):
        """往上走到头的角色类 tag；自己没有角色类上级时返回 {自己}。"""
        tag = self.canonical(tag)
        anc = self.ancestors(tag)
        tops = {a for a in anc if not self.parents(a)}
        return tops or {tag}
