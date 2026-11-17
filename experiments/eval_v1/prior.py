"""汇总所有历史研究取过的图（#380、#534、#551、#557、#456），作为评测集的排除边界。

元数据边界：画师标签（按画师别名归并）、帖子 ID、父子家族、MD5、像素摘要。
内容边界：已提特征图片的缩图 SHA-256 和 pHash/dHash，用于近重复排除。

只收「取样清单」类文件（请求、分片、清单、候选），不收抽样池和 API 缓存：池子里的帖子
没被取用，排除它们只会白白缩小供给。
"""

import argparse
from collections import Counter
from io import BytesIO
import json
from pathlib import Path
from zipfile import ZipFile

import numpy as np

from common import D380, D456, D534, D551, D557, DATA, load, sha_file, write
from coverage import canonical

ROW_FILES = ('part-*.json', 'requests*.json', '*-requests.json', 'manifest.json', 'candidates.json',
             'live-candidates.json', 'test-targets.json', '*-accepted.json', 'sample.json')
SKIP_PARTS = {'live', 'metadata', 'artist-alias-cache', 'coverage', 'duckdb-temp', 'native-target',
              'native', 'native-library', 'index', 'index-smoke', 'received', 'runs'}


def row_files():
    seen = set()
    for root in (D380, D534, D551, D557, D456):
        for pattern in ROW_FILES:
            for path in sorted(root.rglob(pattern)):
                if SKIP_PARTS.intersection(path.relative_to(root).parts[:-1]) or path in seen:
                    continue
                seen.add(path)
                yield path


def rows_of(path):
    try:
        data = load(path)
    except (ValueError, UnicodeDecodeError):
        return None
    if isinstance(data, dict):
        data = data.get('rows') or data.get('selected')
    if not isinstance(data, list) or not data or not isinstance(data[0], dict):
        return None
    if 'id' not in data[0] or 'artist' not in data[0]:
        return None
    return data


def feature_files():
    for root in (D534, D551, D557, D456):
        for path in sorted(root.rglob('*.npz')):
            yield path, None
        for path in sorted(root.rglob('received/*.zip')):
            yield path, 'features.npz'


def fingerprints_of(path, member):
    try:
        if member:
            with ZipFile(path) as package:
                if member not in package.namelist():
                    return None
                data = np.load(BytesIO(package.read(member)), allow_pickle=False)
        else:
            data = np.load(path, allow_pickle=False)
        with data:
            if not {'ids', 'phash', 'dhash', 'image_shas'} <= set(data.files):
                return None
            return data['ids'].astype(np.int64), data['image_shas'], data['phash'], data['dhash']
    except (OSError, ValueError, KeyError):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artist-aliases', type=Path, default=DATA / 'artist-aliases.json')
    args = parser.parse_args()
    aliases = load(args.artist_aliases)
    artists, ids, md5, pixels, parents = set(), set(), set(), set(), {}
    sources = []
    for path in row_files():
        rows = rows_of(path)
        if rows is None:
            continue
        sources.append({'path': str(path.relative_to(path.parents[len(path.parents) - 1])), 'rows': len(rows),
                        'sha256': sha_file(path)})
        for row in rows:
            ids.add(int(row['id']))
            if row.get('artist'):
                artists.add(canonical(row['artist'], aliases))
            for key, bucket in (('md5', md5), ('pixel_hash', pixels)):
                if row.get(key):
                    bucket.add(row[key])
            if row.get('parent_id'):
                parents[int(row['id'])] = int(row['parent_id'])
    shas, near, feature_sources = {}, {}, []
    for path, member in feature_files():
        found = fingerprints_of(path, member)
        if found is None:
            continue
        feature_sources.append({'path': str(path), 'member': member, 'images': int(len(found[0]))})
        for post, sha, phash, dhash in zip(*found):
            shas[str(sha)] = int(post)
            near[int(post)] = (str(phash), str(dhash))
    unknown = ids - set(near)
    write('prior/boundary.json', {
        'artists': sorted(artists), 'ids': sorted(ids), 'md5': sorted(md5), 'pixel_hashes': sorted(pixels),
        'parents': {str(k): v for k, v in sorted(parents.items())},
    })
    posts = sorted(near)
    np.savez(DATA / 'prior/fingerprints.npz', ids=np.array(posts, dtype=np.int64),
             image_shas=np.array(sorted(shas)),
             phash=np.array([near[p][0] for p in posts]), dhash=np.array([near[p][1] for p in posts]))
    receipt = {
        'row_sources': sources, 'feature_sources': feature_sources,
        'artist_tags': len(artists), 'posts': len(ids), 'md5': len(md5), 'pixel_hashes': len(pixels),
        'feature_fingerprints': len(near), 'image_shas': len(shas),
        'listed_posts_without_features': len(unknown),
        'artist_aliases_sha256': sha_file(args.artist_aliases),
        'boundary_sha256': sha_file(DATA / 'prior/boundary.json'),
        'fingerprints_sha256': sha_file(DATA / 'prior/fingerprints.npz'),
        'files_by_tree': Counter(s['path'].split('/_scratch/')[0].rsplit('/', 1)[-1] for s in sources),
    }
    write('prior/receipt.json', receipt)
    print(json.dumps({k: v for k, v in receipt.items() if k not in ('row_sources', 'feature_sources')},
                     ensure_ascii=False), flush=True)


if __name__ == '__main__':
    main()
