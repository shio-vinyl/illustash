"""核对 Colab 回传，按内容指纹去重后冻结共用评测集 v1（清单 + 特征 + 校验值）。

去重顺序：先登记全部历史研究已提特征图片的缩图 SHA-256 和 pHash/dHash，再按「正例 → 混淆库 →
对照」、组内按帖子 ID 散列的固定顺序逐张过滤（帖子、家族、MD5、像素摘要、缩图 SHA 完全重复，或
pHash ≤6 且 dHash ≤8 的近重复都舍弃）。此步只看指纹，不看识别分数。
"""

from collections import Counter
import json

import numpy as np

from common import DATA, load, priority, sha_file, write
from evaluate import Images
from infer import atomic_npz
from prepare import ARRAYS, rows_and_features, seed_holdout
from eval_sample import family_roots

ORDER = ('head_solo', 'head_other', 'tail_solo', 'tail_other', 'same_work_visual_old', 'same_work_other_old',
         'same_work_multi_old', 'unrelated_old', 'multi_old', 'multi_other', 'no_character', 'other_character')


def main():
    if (DATA / 'eval/receipt.json').exists():
        raise ValueError('评测集已冻结')
    requests = load(DATA / 'requests.json')
    if sha_file(DATA / 'requests.json') != load(DATA / 'sample-receipt.json')['requests_sha256']:
        raise ValueError('冻结的请求清单发生变化')
    rows, arrays, failures, receipts, ancestry = rows_and_features((DATA / 'cloud',), ('test',))
    if {r['id'] for r in rows} != {r['id'] for r in requests}:
        raise ValueError('回传清单与请求不一致')
    prior = load(DATA / 'prior/boundary.json')
    parents = {int(k): v for k, v in prior['parents'].items()}
    roots = family_roots(ancestry, parents)
    with np.load(DATA / 'prior/fingerprints.npz', allow_pickle=False) as data:
        seen = [{'id': int(p), 'artist': f'prior:{int(p)}', 'sha256': '', 'pixel_hash': None, 'md5': None,
                 'phash': f'{int(ph):016x}', 'dhash': f'{int(dh):016x}'}
                for p, ph, dh in zip(data['ids'], data['phash'], data['dhash'])]
        prior_shas = set(data['image_shas'].tolist())
    indexed = Images(arrays)
    seed_holdout(indexed, seen, roots)
    indexed.exact.update(('sha', s) for s in prior_shas)
    excluded = Counter()
    for row in sorted(requests, key=lambda r: (ORDER.index(r['group']), priority(r['id']))):
        reason = indexed.filter(dict(row, parent_id=roots.get(row['id'], row['id'])))
        if reason:
            excluded[row['group'], reason] += 1
    kept = sorted(indexed.selected, key=lambda r: r['id'])
    if len({r['artist'] for r in kept}) != len(kept):
        raise ValueError('评测集出现重复画师')
    ix = [indexed.position[r['id']] for r in kept]
    (DATA / 'eval').mkdir(exist_ok=True)
    write('eval/manifest.json', kept)
    atomic_npz(DATA / 'eval/features.npz', {k: arrays[k][ix] if k in ARRAYS else arrays[k] for k in arrays})
    receipt = {
        'requests_sha256': sha_file(DATA / 'requests.json'), 'manifest_sha256': sha_file(DATA / 'eval/manifest.json'),
        'features_sha256': sha_file(DATA / 'eval/features.npz'), 'images': len(kept), 'requested': len(requests),
        'failures': dict(failures), 'archives_sha256': [r['archive_sha256'] for r in receipts],
        'excluded': [{'group': g, 'reason': r, 'count': n} for (g, r), n in sorted(excluded.items())],
        'groups': Counter(r['group'] for r in kept), 'protected_fingerprints': len(seen),
        'artists': len({r['artist'] for r in kept}),
    }
    write('eval/receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False, indent=1), flush=True)


if __name__ == '__main__':
    main()
