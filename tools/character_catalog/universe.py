"""D 站上张数够多的全部角色 tag（#834 研究：以后扩大识别范围时的上限）。

    python3 -m tools.character_catalog.universe 工作目录 [--min-posts 20]

按张数从多到少翻页（每页 1000 个，只取名字、张数、建立时间、是否弃用），翻到张数低于下限为止，
写到工作目录的 `cache/danbooru/universe.jsonl`（一行一页）。请求间隔和 danbooru.py 相同。
"""
import json
import time

from tools.character_catalog import danbooru, work as work_mod


def main(argv=None):
    p = work_mod.parser(__doc__.splitlines()[0])
    p.add_argument('--min-posts', type=int, default=20)
    args = p.parse_args(argv)
    target = args.work / 'cache/danbooru/universe.jsonl'
    client = danbooru.Client()
    page = 1
    with open(target, 'w', encoding='utf-8') as out:
        while True:
            rows = client.get('tags.json', {
                'search[category]': 4, 'search[post_count]': f'>={args.min_posts}', 'search[order]': 'count',
                'only': 'id,name,post_count,created_at,is_deprecated', 'limit': 1000, 'page': page,
            })
            out.write(json.dumps({'page': page, 'rows': rows, 'at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())},
                                 ensure_ascii=False) + '\n')
            out.flush()
            print(page, len(rows), rows[-1]['post_count'] if rows else '-', flush=True)
            if len(rows) < 1000:
                break
            page += 1


if __name__ == '__main__':
    main()
