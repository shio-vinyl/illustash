"""学习夹具：切换到核心的固定旧格式图库输入，加上合成的 8 维特征。"""
import tempfile
from pathlib import Path

import numpy as np

from tests.research_fixture import Features
from tests.research_fixture import seed
from tests.fixed_expectations import expected_folds


def assert_memberships_current(test, store):
    """存储的 memberships 必须与按归属规则重新求出的结果相同。"""
    cols = 'uuid,entity,status,origin,sources,score,source_count,ccip_only'
    with store.connect() as c:
        stored = c.execute(f'SELECT {cols} FROM (SELECT * FROM memberships UNION ALL SELECT * FROM tag_memberships) ORDER BY uuid,entity').fetchall()
        fresh = c.execute(f'SELECT {cols} FROM membership_rules ORDER BY uuid,entity').fetchall()
    test.assertEqual([tuple(r) for r in stored], [tuple(r) for r in fresh])
    with store.connect() as c:
        expected = expected_folds(c.execute('SELECT uuid,width,height FROM assets'),
                                  c.execute('SELECT uuid,entity,status,origin FROM assertions'),
                                  c.execute('SELECT uuid,grp,pos FROM duplicates'))
        actual = c.execute('SELECT uuid,rep,hidden FROM duplicates')
        test.assertEqual(sorted(map(tuple, actual)), sorted(expected))
    with store.connect() as c:
        stored = c.execute('SELECT uuid FROM excluded ORDER BY 1').fetchall()
        fresh = c.execute("""SELECT a.uuid FROM assets a LEFT JOIN curation k ON k.uuid=a.uuid
            LEFT JOIN auto_rules r ON r.uuid=a.uuid AND r.kind='exclude'
            WHERE k.status='excluded' OR (r.uuid IS NOT NULL AND k.uuid IS NULL) ORDER BY 1""").fetchall()
    test.assertEqual([tuple(r) for r in stored], [tuple(r) for r in fresh])


def unit(v):
    v = np.asarray(v, np.float32)
    return v / np.linalg.norm(v)


class Learning:
    """学习夹具的 setUp/tearDown/owners/learned_snapshot（原 tests/test_learn.py）。"""
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        root = Path(self.temp.name)
        self.store, attrs = seed(self, root)
        self.alice, self.bob = attrs['alice'], attrs['bob']
        rng = np.random.default_rng(0)
        A, B = unit([1, 0, 0, 0, 0, 0, 0, 0]), unit([0, 0, 0, 0, 1, 0, 0, 0])
        near = lambda centre, spread: unit(centre + rng.normal(0, spread, 8))
        vectors = {f's{i}': near(A, 0.08) for i in range(6)}                  # alice 已确认的图片
        vectors |= {'close': near(A, 0.05), 'edge': unit(A + 0.35 * unit([0, 1, 0, 0, 0, 0, 0, 0])),
                    'far': unit([0, 0, 1, 0, 0, 0, 0, 0])}
        vectors |= {f'u{i}': near(B, 0.05) for i in range(4)}                 # 一个相似组，还不属于任何人
        vectors |= {'ring': unit([0, 0, 0, 0, 0, 0, 0, 1]), 'hintb': unit([0, 0, 0, 0, 0, 0, 1, 1])}      # 由各个测试摆放
        vectors |= {'a': unit([0, 0, 0, 1, 0, 0, 0, 0]), 'b': unit([0, 0, 0, 0, 0, 1, 0, 0]), 'c': unit([0, 0, 0, 0, 0, 0, 1, 0])}
        self.features = Features(self.store.source, np.stack(list(vectors.values())), list(vectors))

    def tearDown(self):
        assert_memberships_current(self, self.store)
        self.temp.cleanup()

    def owners(self, uuid):
        with self.store.connect() as c:
            return [r[0] for r in c.execute('SELECT entity FROM memberships WHERE uuid=? ORDER BY entity', (uuid,))]

    def learned_snapshot(self):
        with self.store.connect() as c:
            return {table: [tuple(r) for r in c.execute(f'SELECT * FROM {table} ORDER BY 1,2')]
                    for table in ('prototypes', 'proto_claims')}
