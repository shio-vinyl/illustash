"""按 tag 批量抓 Danbooru 的公开数据并缓存（#834 的离线制作端，应用不调用）。

    python3 -m tools.character_catalog.danbooru 缓存目录 --index … --extension … --longtail …

抓五类数据，都只按名字精确查：

- `tags`：类别、张数、是否弃用；
- `aliases`：生效的别名（antecedent → consequent），两个方向都查；
- `implications`：生效的蕴含，两个方向都查；
- `wiki`：wiki 的 `other_names` 和正文（正文只在制作端当证据读，不进包）。

每批 100 个名字一次请求，结果满 1000 条时对半拆开重查。每类一个 JSONL，一行一批，记下这批查了哪些名字；
中断后重跑只补没查过的名字。请求间隔默认 0.6 秒，单线程，不带账号。
"""
import argparse
import json
from pathlib import Path
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

BASE = 'https://danbooru.donmai.us/'
USER_AGENT = 'catalog-research/0.1'
BATCH = 100
LIMIT = 1000

ENDPOINTS = {
    'tags': ('tags.json', 'search[name_comma]', 'id,name,category,post_count,is_deprecated,created_at', {}),
    'aliases_from': ('tag_aliases.json', 'search[antecedent_name_comma]',
                     'id,antecedent_name,consequent_name,status,created_at', {'search[status]': 'active'}),
    'aliases_to': ('tag_aliases.json', 'search[consequent_name_comma]',
                   'id,antecedent_name,consequent_name,status,created_at', {'search[status]': 'active'}),
    'implications_from': ('tag_implications.json', 'search[antecedent_name_comma]',
                          'id,antecedent_name,consequent_name,status,created_at', {'search[status]': 'active'}),
    'implications_to': ('tag_implications.json', 'search[consequent_name_comma]',
                        'id,antecedent_name,consequent_name,status,created_at', {'search[status]': 'active'}),
    'wiki': ('wiki_pages.json', 'search[title_comma]', 'id,title,other_names,body,is_deleted,updated_at', {}),
}


class Client:
    def __init__(self, interval=0.6):
        self.interval = interval
        self.last = 0.0
        self.requests = 0

    def get(self, path, params):
        query = urllib.parse.urlencode(params, safe='[],')
        url = f'{BASE}{path}?{query}'
        for attempt in range(6):
            wait = self.last + self.interval - time.monotonic()
            if wait > 0:
                time.sleep(wait)
            self.last = time.monotonic()
            self.requests += 1
            request = urllib.request.Request(url, headers={'User-Agent': USER_AGENT, 'Accept': 'application/json'})
            try:
                with urllib.request.urlopen(request, timeout=60) as response:
                    return json.loads(response.read())
            except urllib.error.HTTPError as error:
                if error.code in (429, 500, 502, 503, 504):
                    time.sleep(5 * (attempt + 1))
                    continue
                raise
            except (urllib.error.URLError, TimeoutError):
                time.sleep(5 * (attempt + 1))
        raise RuntimeError(f'多次重试仍失败：{url[:200]}')


def done_names(path):
    names = set()
    if path.exists():
        with open(path, encoding='utf-8') as fh:
            for line in fh:
                names.update(json.loads(line)['q'])
    return names


def query(client, kind, names):
    """查一批名字；结果满额时对半拆开，保证不漏。"""
    path, param, only, extra = ENDPOINTS[kind]
    params = {param: ','.join(names), 'only': only, 'limit': LIMIT, **extra}
    rows = client.get(path, params)
    if len(rows) >= LIMIT and len(names) > 1:
        half = len(names) // 2
        return query(client, kind, names[:half]) + query(client, kind, names[half:])
    if len(rows) >= LIMIT:
        raise RuntimeError(f'{kind} 单个名字的结果超过 {LIMIT} 条：{names[0]}')
    return rows


def fetch(client, cache, kind, names, log=sys.stderr):
    target = cache / f'{kind}.jsonl'
    done = done_names(target)
    pending = [n for n in names if n not in done]
    total = len(pending)
    with open(target, 'a', encoding='utf-8') as out:
        for start in range(0, total, BATCH):
            batch = pending[start:start + BATCH]
            rows = query(client, kind, batch)
            out.write(json.dumps({'q': batch, 'rows': rows, 'at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())},
                                 ensure_ascii=False) + '\n')
            out.flush()
            print(f'{kind} {min(start + BATCH, total)}/{total} 请求 {client.requests}', file=log, flush=True)


def rows(cache, kind):
    """读回缓存里某一类的全部行（同一行可能因为拆批重复出现，按 id 去重）。"""
    seen = {}
    path = cache / f'{kind}.jsonl'
    if path.exists():
        with open(path, encoding='utf-8') as fh:
            for line in fh:
                for row in json.loads(line)['rows']:
                    seen[row.get('id', id(row))] = row
    return list(seen.values())


def main(argv=None):
    from tools.character_catalog import work as work_mod

    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('work', type=Path, help='工作目录（含 inputs.json；缓存写进 cache/danbooru/）')
    parser.add_argument('--kinds', default=','.join(ENDPOINTS))
    parser.add_argument('--extra-names', type=Path, help='另外要查的名字（一行一个），例如关系另一端的范围外 tag')
    parser.add_argument('--interval', type=float, default=0.6)
    args = parser.parse_args(argv)

    work = work_mod.open_work(args.work)
    names = sorted(work.scope)
    if args.extra_names:
        names = sorted(set(names) | {line.strip() for line in args.extra_names.read_text().splitlines() if line.strip()})
    cache = work.cache / 'danbooru'
    cache.mkdir(parents=True, exist_ok=True)
    client = Client(args.interval)
    for kind in args.kinds.split(','):
        fetch(client, cache, kind, names)
    print(f'完成，共 {client.requests} 次请求', file=sys.stderr)


if __name__ == '__main__':
    main()
