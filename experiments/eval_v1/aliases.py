"""冻结本轮的画师别名快照（Danbooru 活跃的 artist 类别名）。角色别名沿用 #380 的冻结表。"""

import json

from common import DATA, sha_file
from coverage import page_all


def main():
    cache = DATA / 'artist-alias-cache'
    cache.mkdir(parents=True, exist_ok=True)
    rows = page_all(cache, 'artist-aliases', '/tag_aliases.json',
                    **{'search[category]': 1, 'search[status]': 'active'})
    aliases = {r['antecedent_name']: r['consequent_name'] for r in rows}
    encoded = json.dumps(aliases, sort_keys=True) + '\n'
    destination = DATA / 'artist-aliases.json'
    if destination.exists() and destination.read_text() != encoded:
        raise ValueError('已冻结的画师别名快照与缓存不符')
    destination.write_text(encoded)
    print(f'画师别名 {len(aliases)}，SHA-256 {sha_file(destination)}', flush=True)


if __name__ == '__main__':
    main()
