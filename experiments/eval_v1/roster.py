"""冻结评测集要覆盖的新路线角色和「旧角色」（已被基线覆盖的角色）。

新路线角色（评测正例的目标）：
- #551 的 2,100 头（原 228 + 新训 1,872），其中 257 头是一级候选；
- #534 已训好、因精选索引已覆盖而没发布的头（#602 2026-11-17 的补充项）；
- #557 现有 54 个长尾锚点。长尾扩容的探查名单另由 pool.py 按元数据冻结。

旧角色：WD 词表的角色标签、完整精选索引的 7,367 个角色，按 #380 冻结的角色别名归并。
"""

from collections import Counter
import csv
import json

import numpy as np

from common import (A557, A557_SHA, CCIP_INDEX, COVERAGE, D534, D551, D557, HEADS_534, PACK_2100,
                    PACK_2100_SHA, PACK_257, PACK_257_SHA, TAGS, load, post_band, sha_file, write)
from coverage import canonical
from shortlist import read_pack


def main():
    for path, expected in ((PACK_2100, PACK_2100_SHA), (PACK_257, PACK_257_SHA), (A557, A557_SHA)):
        if sha_file(path) != expected:
            raise ValueError(f'冻结输入摘要不符：{path}')
    aliases = load(COVERAGE / 'alias-map.json')
    with TAGS.open() as stream:
        wd = {canonical(r['name'], aliases) for r in csv.DictReader(stream) if r['category'] == '4'}
    index = {canonical(r['tag'], aliases) for r in load(CCIP_INDEX / 'loaded-characters.json')}
    inventory = {r['name']: r for r in load(COVERAGE / 'inventory.json')}
    labels_2100, *_ = read_pack(PACK_2100)
    labels_257, *_ = read_pack(PACK_257)
    with np.load(HEADS_534, allow_pickle=False) as data:
        labels_534 = data['labels'].tolist()
    roster_551 = {r['label']: r for r in load(D551 / 'roster.json')}
    roster_534 = {r['name']: r for r in load(D534 / 'sample/roster.json')}
    training = load(D551 / 'fit/training.json')
    anchors = load(D557 / 'A-metadata.json')
    for name in [*labels_2100, *labels_534]:
        if canonical(name, aliases) != name:
            raise ValueError(f'头标签不是规范名：{name}')
    entries = {}
    for name in labels_2100:
        meta = roster_551.get(name) or roster_534.get(name)
        posts = meta['post_count']
        entries[name] = {
            'label': name, 'route': 'wd_head', 'pack': '2100', 'tier1_257': name in labels_257,
            'original_228': name in labels_2100[:228], 'post_count': posts, 'band': post_band(posts),
            'train_artists': training.get(name, {}).get('artists'),
            'in_wd': name in wd, 'in_index': name in index,
        }
    extra_534 = [n for n in labels_534 if n not in entries]
    for name in extra_534:
        meta = roster_534[name]
        entries[name] = {
            'label': name, 'route': 'wd_head', 'pack': '534_index_covered', 'tier1_257': False,
            'original_228': False, 'post_count': meta['post_count'], 'band': post_band(meta['post_count']),
            'in_wd': name in wd, 'in_index': name in index, 'index_tag': meta.get('index_tag'),
        }
    for anchor in anchors:
        name = canonical(anchor['tag'], aliases)
        if name in entries:
            raise ValueError(f'长尾锚点与扩展头重叠：{name}')
        entries[name] = {'label': name, 'route': 'ccip_longtail', 'pack': 'A557_v1', 'tier1_257': False,
                         'original_228': False, 'post_count': anchor['post_count'], 'band': 'lt50',
                         'in_wd': name in wd, 'in_index': name in index}
    covered = sorted(wd | index)
    overlap = Counter((e['pack'], e['in_wd'], e['in_index']) for e in entries.values())
    write('roster/targets.json', sorted(entries.values(), key=lambda e: e['label']))
    write('roster/covered.json', covered)
    receipt = {
        'targets': len(entries), 'by_pack': Counter(e['pack'] for e in entries.values()),
        'tier1_257': sum(e['tier1_257'] for e in entries.values()),
        'by_band': Counter(f"{e['pack']}/{e['band']}" for e in entries.values()),
        'pack_overlap_with_covered': {f'{k[0]} wd={k[1]} index={k[2]}': v for k, v in overlap.items()},
        'covered_labels': len(covered), 'wd_character_tags': len(wd), 'index_characters': len(index),
        'inputs': {'pack_2100': PACK_2100_SHA, 'pack_257': PACK_257_SHA, 'heads_534': sha_file(HEADS_534),
                   'a557': A557_SHA, 'alias_map': sha_file(COVERAGE / 'alias-map.json'),
                   'index_characters': sha_file(CCIP_INDEX / 'loaded-characters.json'), 'tags': sha_file(TAGS)},
        'targets_sha256': sha_file(write('roster/targets.json', sorted(entries.values(), key=lambda e: e['label']))),
    }
    write('roster/receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False, indent=1), flush=True)


if __name__ == '__main__':
    main()
