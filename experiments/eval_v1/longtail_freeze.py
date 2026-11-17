"""冻结 #557 第二轮的长尾锚点包（v2 增量，叠在 v1 的 54 个锚点之上）。

做法和 #557 第一轮相同：参考图去重（先参考、后校准），每角色至少 5 张参考图、清洗后形成一致的
核心（#380 core），锚点 = 等权单位向量中心，阈值取 p75（生产默认）；离完整精选索引或已有锚点
（含 v1）的距离 < 0.03 的不收。只用开发数据，不读评测集。
"""

from collections import Counter
import json
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

import numpy as np

from common import COVERAGE, D557, DATA, INDEX_SHA, load, priority, sha_file, write
from colab_run import MODELS, TAG_SHA
from evaluate import Images, add_patches, matrix_file
from prepare import rows_and_features
from sample import family_roots
from score import index_entries, longtail


def main():
    work = DATA / 'lt2'
    if (work / 'freeze.json').exists():
        raise ValueError('长尾 v2 已冻结')
    requests = load(work / 'requests.json')
    if sha_file(work / 'requests.json') != load(work / 'sample-receipt.json')['requests_sha256']:
        raise ValueError('开发请求清单发生变化')
    rows, arrays, failures, receipts, ancestry = rows_and_features((work / 'cloud',), ('train', 'calibration'))
    indexed = Images(arrays)
    roots = family_roots(ancestry, {})
    excluded = Counter()
    for split in ('train', 'calibration'):
        for row in sorted((r for r in requests if r['split'] == split), key=lambda r: priority(r['id'])):
            reason = indexed.filter(dict(row, parent_id=roots.get(row['id'], row['id'])))
            if reason:
                excluded[split, reason] += 1
    kept = indexed.selected
    write('lt2/development-accepted.json', kept)
    vectors = arrays['ccip_vectors'][[indexed.position[r['id']] for r in kept]]
    calibrated = {n for r in kept if r['split'] == 'calibration' for n in r['focus_roles']}
    tail_posts = load(DATA / 'pool/live-tail-posts.json')
    targets = {n: {'post_count': len([p for p in tail_posts[n] if not p['is_deleted'] and not p['is_banned']])}
               for n in sorted(calibrated)}
    base, anchors = index_entries()
    v1_meta, v1_anchors = longtail(D557)
    aliases = load(COVERAGE / 'alias-map.json')
    entries, extended, statuses = add_patches(kept, vectors, targets, base + v1_meta,
                                              np.concatenate([anchors, v1_anchors]), aliases, work)
    sampled = set(load(work / 'sample-receipt.json')['roles_sampled'])
    statuses.update({n: {'state': 'no_calibration_after_dedup'} for n in sampled - calibrated})
    write('lt2/A-coverage.json', statuses)
    matrix_file(work / 'A-anchors.f32', extended)
    provenance = {
        'format': 'illustash-557-ccip-offline/1', 'round': 'eval-v1 / #557 第二轮', 'base_v1_sha256': sha_file(D557 / 'A-longtail-p75.zip'),
        'ccip_sha256': MODELS['ccip/model_feat.onnx'], 'wd_tags_sha256': TAG_SHA, 'index_sha256': INDEX_SHA,
        'character_aliases_sha256': sha_file(COVERAGE / 'alias-map.json'),
        'artist_aliases_sha256': sha_file(DATA / 'artist-aliases.json'),
        'fit': 'issue_534 add_patches / issue_380 core; separation >=0.03 from curated index and v1 anchors',
        'scoring': 'registry: curated p90, added p75; nearest nine with production aliases',
        'reference_set_sha256': sha_file(work / 'development-accepted.json'),
        'files': {name: sha_file(work / name) for name in ('A-metadata.json', 'A-references.npz', 'A-anchors.f32')},
    }
    write('lt2/A-provenance.json', provenance)
    package = work / 'A-longtail-v2.zip'
    with ZipFile(package, 'w', ZIP_DEFLATED, compresslevel=6) as archive:
        for name in ('A-provenance.json', *provenance['files']):
            archive.writestr(ZipInfo(name, (2026, 11, 18, 0, 0, 0)), (work / name).read_bytes(),
                             compress_type=ZIP_DEFLATED, compresslevel=6)
    freeze = {
        'package_sha256': sha_file(package), 'bytes': package.stat().st_size, 'anchors': len(entries),
        'references': sum(len(e['posts']) for e in entries), 'states': Counter(s['state'] for s in statuses.values()),
        'development_images': Counter(r['split'] for r in kept), 'excluded': {f'{s}:{r}': n for (s, r), n in excluded.items()},
        'failures': dict(failures), 'archives_sha256': [r['archive_sha256'] for r in receipts],
    }
    write('lt2/freeze.json', freeze)
    print(json.dumps(freeze, ensure_ascii=False), flush=True)


if __name__ == '__main__':
    main()
