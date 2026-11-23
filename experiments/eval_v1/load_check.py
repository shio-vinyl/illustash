"""角色资料包正式文件的本机加载核对（照 #602 的做法）：用测试密钥签的目录、合成图库，离线导入、启用、撤回。

在 main 的检出里运行（要用它的 tests、tools 和构建好的 illustash-terms）：

    cd <main 检出>
    ILLUSTASH_TERMS_BIN=native/terms/target/debug/illustash-terms \\
      python3 -m unittest <本文件> -v      # 用 PYTHONPATH=. 让 tests、tools 可导入

环境变量 PACK_WD、PACK_CCIP 指向要核对的两个文件，PACK_VERSION 是条目的版本名（如 0.4）。
目录只用测试密钥（tests/fixtures/pack_catalog/make.py 的 key A）签名，应用只在 ILLUSTASH_UPDATE_BASE
指向本机替身服务时才认它；整个过程不联网。

合成图库的向量就是挑出来的几个头自己的权重方向（再放大），所以会同时触发很多头，命中数不作参考。核对的是：
1,873 头全部被接收、二级头数对得上；补上的主归属全部来自一级头；二级头只进候选；撤回以后干净。
"""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import struct
import unittest
import zipfile

from tools import pack_catalog

if __name__ != '__main__':
    import numpy as np
    from tests import test_native_pack_catalog as base

WD = Path(os.environ['PACK_WD']).resolve()
CCIP = Path(os.environ['PACK_CCIP']).resolve()
VERSION = os.environ.get('PACK_VERSION', '0.4')
MIN_APP = os.environ.get('PACK_MIN_APP', '0.2.0')


def read_wd(path):
    data = path.read_bytes()
    end = data.index(b'\n')
    header = json.loads(data[:end])
    n = len(header['labels'])
    values = np.frombuffer(data[end + 1:], dtype='<f4')
    return header, values[:-n].reshape(n, 1024), values[-n:]


def probabilities(vector, weights, biases):
    return 1 / (1 + np.exp(-(weights.astype(np.float64) @ vector.astype(np.float64) + biases.astype(np.float64))))


class PackLoadCheck(base.NativePackCatalogTest if __name__ != '__main__' else object):
    def setUp(self):
        super().setUp()
        self.header, self.weights, self.biases = read_wd(WD)
        tiers = self.header['tiers']
        first = tiers.index(1)
        second = tiers.index(2)
        last_second = len(tiers) - 1 - tiers[::-1].index(2)
        # 三张图：一张朝一个一级头的权重方向，两张朝二级头的权重方向；放大到这个头一定过阈值。
        self.vectors = []
        for j in (first, second, last_second):
            w = self.weights[j].astype(np.float64)
            vector = w / np.linalg.norm(w) * 40.0
            self.vectors.append(vector.astype(np.float32))
        with sqlite3.connect(self.lib.source) as source:
            for uuid, vector in zip(self.ids, self.vectors):
                source.execute("UPDATE embeddings SET vec=? WHERE uuid=? AND model='wd'", (vector.tobytes(), uuid))
        thresholds = np.asarray(self.header['thresholds'])
        self.expected = []
        for vector in self.vectors:
            p = probabilities(vector, self.weights, self.biases).astype(np.float32)
            hit = np.flatnonzero(p >= thresholds.astype(np.float32))
            self.expected.append({'tier1': {self.header['labels'][j] for j in hit if tiers[j] == 1},
                                  'tier2': {self.header['labels'][j] for j in hit if tiers[j] == 2}})

    def bundle(self):
        """离线包由 make_bundle()（要 cryptography）先做好，放在 PACK_BUNDLE。"""
        path = Path(os.environ['PACK_BUNDLE'])
        with zipfile.ZipFile(path) as archive:
            catalog = json.loads(archive.read('packs.json'))
        return path, catalog['packs'][0]

    def evidence(self):
        with sqlite3.connect(self.lib.db) as app:
            rows = app.execute("""SELECT a.key, json_extract(e.data,'$.tag'), json_extract(e.data,'$.tier')
                                  FROM term_current_evidence e JOIN term_assets a ON a.id=e.asset_id
                                  WHERE e.source='wd_extension'""").fetchall()
        return rows

    def test_offline_import_enable_and_withdraw(self):
        files = {}
        with base.github(files) as (origin, seen):
            files.update(self.serve(origin))
            self.restart()
            path, entry = self.bundle()
            self.packs({'action': 'import', 'paths': [str(path)]})
            packs = self.settle('download')
            results = {r['name'].split('/')[-1]: r['result'] for r in packs['download']['results']}
            print('import', results)
            self.assertEqual(results['packs.json'], 'catalog')
            self.assertEqual(results[entry['components']['wd']['file']['name']], 'accepted')
            self.assertEqual(results[entry['components']['ccip']['file']['name']], 'accepted')
            current = packs['current']
            print('current', {k: current.get(k) for k in ('version', 'stage', 'heads', 'candidate_only', 'status')})
            self.assertEqual((current['version'], current['status']), (VERSION, 'ready'))
            self.assertEqual(entry['components']['wd']['heads'], 1873)
            self.assertEqual(entry['components']['wd']['candidate_only'], 1577)
            self.packs({'action': 'enable'})
            enabled = self.settle('task')
            self.assertEqual(enabled['task']['status'], 'done', enabled)
            print('enabled', {k: enabled['library'].get(k) for k in ('version', 'images', 'assigned')})
            assigned, candidates = self.members()
            should_assign = {self.ids[i] for i, e in enumerate(self.expected) if e['tier1']}
            should_candidate = {self.ids[i] for i, e in enumerate(self.expected) if e['tier1'] | e['tier2']}
            self.assertEqual(set(assigned), should_assign)
            self.assertEqual(set(candidates), should_candidate)
            # 证据：每个命中一行；二级命中带 tier 2。补上的归属的角色都是一级头。
            rows = self.evidence()
            tier_of = {(uuid, label): tier or 1 for uuid, label, tier in rows}
            expected_rows = {(self.ids[i], n): 1 for i, e in enumerate(self.expected) for n in e['tier1']}
            expected_rows.update({(self.ids[i], n): 2 for i, e in enumerate(self.expected) for n in e['tier2']})
            self.assertEqual(tier_of, expected_rows)
            with sqlite3.connect(self.lib.db) as app:
                # 每个补上的归属都要有同一角色的一级证据（tier 为空），没有一条只靠二级证据。
                owners = app.execute("""SELECT m.uuid, m.entity,
                    sum(json_extract(e.data,'$.tier') IS NULL), count(e.id)
                    FROM memberships m JOIN term_assets a ON a.key=m.uuid
                    JOIN terms t ON t.key=m.entity
                    LEFT JOIN term_current_evidence e ON e.asset_id=a.id AND e.term_id=t.id AND e.source='wd_extension'
                    WHERE m.sources='WD_EXT' GROUP BY m.uuid, m.entity""").fetchall()
            self.assertTrue(owners and all(tier1_rows > 0 for _, _, tier1_rows, _ in owners), owners)
            print('evidence rows', len(rows), 'tier2 rows', sum(t == 2 for t in tier_of.values()),
                  'assigned', len(owners), 'all from tier1', True)
            self.packs({'action': 'withdraw'})
            withdrawn = self.settle('task')
            self.assertEqual(withdrawn['task']['status'], 'done', withdrawn)
            self.assertEqual(self.members(), ([], []))
            self.assertEqual(self.evidence(), [])
            with sqlite3.connect(self.lib.db) as app:
                self.assertEqual(app.execute("SELECT count(*) FROM candidates WHERE source IN ('WD_EXT','CCIP_LONGTAIL')")
                                 .fetchone(), (0,))
            print('withdrawn clean')
            self.assertEqual(seen, [])

def make_bundle(output):
    """用测试密钥签目录，打成离线包。这一步只需要 tools.pack_catalog 和 cryptography。"""
    entry = pack_catalog.entry(version=VERSION, revision=99, wd=WD, ccip=CCIP, min_app=MIN_APP, stage='preview',
                               note_zh='本机加载核对', note_en='Local load check')
    catalog = pack_catalog.build([entry])
    raw = pack_catalog.dumps(catalog)
    spec = importlib.util.spec_from_file_location('make', Path('tests/fixtures/pack_catalog/make.py').resolve())
    make = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(make)
    with zipfile.ZipFile(output, 'w') as archive:
        archive.writestr('packs.json', raw)
        archive.writestr('packs.json.sig', make.Key('A').sign(raw))
        archive.writestr(entry['components']['wd']['file']['name'], WD.read_bytes())
        archive.writestr(entry['components']['ccip']['file']['name'], CCIP.read_bytes())
    print(output, hashlib.sha256(raw).hexdigest())


if __name__ == '__main__':
    import sys
    make_bundle(Path(sys.argv[1]))
