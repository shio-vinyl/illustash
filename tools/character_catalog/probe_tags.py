"""研究用：范围内 tag 在 D 站的现状（查不到、改了类别、弃用、零张）。"""
import collections

from tools.character_catalog import work as work_mod


def main(argv=None):
    args = work_mod.parser(__doc__).parse_args(argv)
    work = work_mod.open_work(args.work)
    scope = work.scope
    tags = {r['name']: r for r in work.danbooru('tags')}
    print('查到的 tag', len(tags), '范围', len(scope))
    missing = [t for t in scope if t not in tags]
    by = collections.Counter(s for t in missing for s in scope[t].sources)
    print('D站查不到', len(missing), dict(by), missing[:12])
    cat = collections.Counter(tags[t]['category'] for t in scope if t in tags)
    print('当前类别', dict(cat))
    other = [(t, tags[t]['category'], tags[t]['post_count'], sorted(scope[t].sources))
             for t in scope if t in tags and tags[t]['category'] != 4]
    print('已不是角色类', len(other))
    for row in other[:30]:
        print('  ', row)
    deprecated = [t for t in scope if t in tags and tags[t].get('is_deprecated')]
    print('弃用', len(deprecated), deprecated[:12])
    zero = [t for t in scope if t in tags and tags[t]['post_count'] == 0]
    print('张数为 0', len(zero), zero[:12])


if __name__ == '__main__':
    main()
