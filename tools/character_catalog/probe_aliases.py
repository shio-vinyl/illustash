"""研究用：范围内的旧 tag（D 站已改名）和它们的去向。"""
import collections

from tools.character_catalog import work as work_mod


def main(argv=None):
    args = work_mod.parser(__doc__).parse_args(argv)
    work = work_mod.open_work(args.work)
    scope = work.scope
    alias = {r['antecedent_name']: r['consequent_name'] for r in work.danbooru('aliases_from')}
    old = {t: alias[t] for t in scope if t in alias}
    print('范围内已改名的旧 tag', len(old))
    by = collections.Counter(s for t in old for s in scope[t].sources)
    print('  按来源', dict(by))
    inside = {t: c for t, c in old.items() if c in scope}
    print('  新名也在范围内（同一角色在两份来源里各一条）', len(inside))
    pairs = collections.Counter()
    for t, c in inside.items():
        pairs[(min(scope[t].sources), min(scope[c].sources))] += 1
    print('  旧名来源 → 新名来源', dict(pairs))
    for t, c in list(inside.items())[:15]:
        print('    ', t, sorted(scope[t].sources), '→', c, sorted(scope[c].sources))
    outside = {t: c for t, c in old.items() if c not in scope}
    print('  新名不在范围内', len(outside))
    for t, c in list(outside.items())[:15]:
        print('    ', t, sorted(scope[t].sources), '→', c)
    into = collections.Counter(r['consequent_name'] for r in work.danbooru('aliases_to') if r['consequent_name'] in scope)
    print('指向范围内 tag 的别名（旧名不一定在范围内）', sum(into.values()), '涉及 tag', len(into))


if __name__ == '__main__':
    main()
