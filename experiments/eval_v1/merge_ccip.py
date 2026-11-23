"""把长尾 v1（54 锚点）和第二轮 v2（38 锚点）合成角色资料包的一个 CCIP 组成部分文件。

格式照 A-longtail-p75.zip（#557 第一轮）。按 #602（PR #614）加载时的核对逐条自查：
provenance 格式和 CCIP 模型 SHA、成员摘要；metadata 的 id/tag 非空不重复、post_count 1–49、
gender_conf 0–1、0≤p50≤p75≤p90≤1 且 p75>0、大小上限；anchors 为 角色数×768 个有限且非全零的 f32。
"""

from io import BytesIO
import json
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

import numpy as np

from common import A557, A557_SHA, D557, DATA, sha_file, write
from colab_run import MODELS


def main():
    if sha_file(A557) != A557_SHA:
        raise ValueError('v1 包摘要不符')
    parts = []
    for folder in (D557, DATA / 'lt2'):
        meta = json.loads((folder / 'A-metadata.json').read_bytes())
        anchors = np.fromfile(folder / 'A-anchors.f32', dtype='<f4').reshape(-1, 768)
        if len(meta) != len(anchors):
            raise ValueError(f'{folder} 锚点数与 metadata 不符')
        with np.load(folder / 'A-references.npz', allow_pickle=False) as refs:
            references = {k: refs[k] for k in refs.files}
        parts.append((meta, anchors, references))
    meta = [m for part in parts for m in part[0]]
    anchors = np.concatenate([part[1] for part in parts]).astype('<f4')
    references = {k: v for part in parts for k, v in part[2].items()}
    problems = []
    ids, tags = [m['id'] for m in meta], [m['tag'] for m in meta]
    if not all(ids) or not all(tags) or len(set(ids)) != len(ids) or len(set(tags)) != len(tags):
        problems.append('id/tag 为空或重复')
    for m in meta:
        if not 1 <= m['post_count'] <= 49:
            problems.append(f"post_count 越界：{m['id']} {m['post_count']}")
        if not 0 <= m['gender_conf'] <= 1:
            problems.append(f"gender_conf 越界：{m['id']}")
        if not (0 <= m['p50'] <= m['p75'] <= m['p90'] <= 1 and m['p75'] > 0):
            problems.append(f"分位数不合规：{m['id']}")
    if not np.isfinite(anchors).all() or (np.abs(anchors).sum(axis=1) == 0).any():
        problems.append('锚点有非有限值或全零')
    out = DATA / 'packs/ccip'
    out.mkdir(parents=True, exist_ok=True)
    (out / 'A-metadata.json').write_text(json.dumps(meta, ensure_ascii=False, separators=(',', ':')) + '\n')
    anchors.tofile(out / 'A-anchors.f32')
    buffer = BytesIO()
    np.savez(buffer, **references)
    (out / 'A-references.npz').write_bytes(buffer.getvalue())
    limit = max(128 * 1024, len(meta) * 2048)
    if (out / 'A-metadata.json').stat().st_size > limit:
        problems.append('metadata 超过大小上限')
    provenance = {
        'format': 'illustash-557-ccip-offline/1', 'ccip_sha256': MODELS['ccip/model_feat.onnx'],
        'round': 'v1（#557 第一轮 54 锚点）+ v2（共用评测集 v1 轮 38 锚点）',
        'components_sha256': {'v1': A557_SHA, 'v2': sha_file(DATA / 'lt2/A-longtail-v2.zip')},
        'scoring': 'registry: curated p90, added p75; nearest nine with production aliases',
        'files': {name: sha_file(out / name) for name in ('A-metadata.json', 'A-anchors.f32')},
    }
    (out / 'A-provenance.json').write_text(json.dumps(provenance, ensure_ascii=False, indent=1) + '\n')
    if (out / 'A-provenance.json').stat().st_size > 16 * 1024:
        problems.append('provenance 超过 16 KiB')
    if problems:
        raise ValueError('；'.join(problems))
    package = DATA / 'packs/ccip-longtail-v2.zip'
    with ZipFile(package, 'w', ZIP_DEFLATED, compresslevel=6) as archive:
        # 参考向量 A-references.npz 加载时不读，只留在本地备查，不放进发给用户的包。
        for name in ('A-provenance.json', 'A-metadata.json', 'A-anchors.f32'):
            archive.writestr(ZipInfo(name, (2026, 11, 18, 0, 0, 0)), (out / name).read_bytes(),
                             compress_type=ZIP_DEFLATED, compresslevel=6)
    info = {'anchors': len(meta), 'v1': len(parts[0][0]), 'v2': len(parts[1][0]), 'bytes': package.stat().st_size,
            'sha256': sha_file(package), 'post_count_range': [min(m['post_count'] for m in meta),
                                                              max(m['post_count'] for m in meta)]}
    write('packs/ccip-longtail-v2.json', info)
    print(json.dumps(info, ensure_ascii=False))


if __name__ == '__main__':
    main()
