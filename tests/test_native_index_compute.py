"""#150 与旧算法的合成对照；显式给出候选，不自动查找模型/图库。"""
import contextlib
import csv
import io
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import numpy as np
from tests.index_expectations import build, char_patch, char_scan
from tests import index_expectations, numeric_reference_mode, numeric_reference_assertions as checks, vector_math as ccip
char_scan.cross = ccip.cross



class NativeIndexComputeTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._missing_numeric_candidate = None
        path = os.environ.get('ILLUSTASH_INDEX_COMPUTE_BIN')
        if not path:
            error = unittest.SkipTest('explicit candidate ILLUSTASH_INDEX_COMPUTE_BIN required')
            if numeric_reference_mode.defer_missing(cls, error):
                return
            raise error
        cls.binary = Path(path).resolve(strict=True)

    def setUp(self):
        numeric_reference_mode.check_candidate(self)

    def numeric(self, rows, distances):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root/'request.json').write_text(json.dumps(dict(mode='numeric', rows=rows.tolist(), distances=distances.tolist())))
            proc = subprocess.run([str(self.binary), str(root/'request.json'), str(root/'response.json')], capture_output=True, env=os.environ | {'PATH': ''})
            self.assertEqual(proc.returncode, 0, proc.stderr.decode())
            return json.loads((root/'response.json').read_text())

    def test_merge_percentile_core_and_partition_ties(self):
        references = numeric_reference_mode.references(self)
        from tests import numpy_index_reference as reference
        # 用户 2026-10-04 批准：所有操作系统上都用标量 NumPy 的选择/排序顺序。
        # 独立的判定基准：macOS arm64 上的 NumPy 2.5.3（标量 argintroselect/aquicksort）。
        # Windows 上 NumPy SIMD 对并列值的排序有意不再作为契约。
        tied_nearest = {
            3: [0, 1, 2], 5: [0, 3, 1, 4, 2], 6: [0, 3, 1, 4, 2, 5],
            9: [0, 3, 6, 1, 4, 7, 2, 5, 8], 10: [6, 9, 0, 3, 7, 4, 1, 8, 5],
            17: [9, 0, 15, 3, 12, 6, 13, 10, 16],
            32: [9, 15, 30, 27, 24, 21, 18, 12, 0],
            257: [177, 174, 75, 180, 78, 72, 81, 87, 171],
        }
        for count in (3, 5, 6, 9, 10, 17, 32, 257):
            with self.subTest(count=count):
                rng = np.random.default_rng(count)
                rows = rng.normal(size=(count,768)).astype('f4') * .015
                rows[:,0] += 1
                distances = np.resize(np.array([0., .01, .1],np.float32), count)
                value = self.numeric(rows, distances)
                for kind in references:
                    with numeric_reference_mode.comparing(self,kind):
                        anchor = reference.merge(rows) if kind=='numpy' else build.merge(rows)
                        np.testing.assert_array_equal(np.array(value['anchor'],np.float32), anchor)
                        dd = (1-(rows/np.linalg.norm(rows,axis=1,keepdims=True)) @ (anchor/np.linalg.norm(anchor)))/2
                        np.testing.assert_array_equal(value['percentiles'],[float(np.percentile(dd,q)) for q in (50,75,90)])
                        keep = reference.core_of(rows) if kind=='numpy' else char_patch.core_of(rows)
                        self.assertEqual(value['core'], None if keep is None else keep.tolist())
                        self.assertEqual(value['nearest'], tied_nearest[count])
                        if kind=='numpy':
                            self.assertEqual(reference.scalar_order(distances, partition=True).tolist(), tied_nearest[count])
                k = min(9,count)
                # 没有并列时的选择仍与本地的独立算法一致。
                unique = rng.permutation(count).astype('f4')
                selected = np.argpartition(unique,k-1)[:k]
                self.assertEqual(self.numeric(rows, unique)['nearest'], selected[np.argsort(unique[selected])].tolist())

    def prepare(self, root):
        for folder in ('char-index','char-patch','game-chars'):
            (root/folder).mkdir(parents=True)
        tags = root/'tags.csv'
        tags.write_text('name,category\nalice_(blue_archive),4\n')
        (root/'album-char-map.json').write_text(json.dumps({'map':{'合成相册':{'char_id':'alice','also':[]}}}))
        (root/'game-chars/bluearchive_index.json').write_text(json.dumps({'data':[{'ennames':['Alice Example','Alice'],'jpnames':['合成名'],'cnnames':['示例名']}]}))
        rng = np.random.default_rng(731)
        rows = []
        for cid,tag,axis,post_count,source in [('alice','alice_(blue_archive)',0,900,None),('new','new_(example)',1,500,'danbooru-patch'),('twin','twin_(example)',1,200,'danbooru-patch')]:
            matrix = rng.normal(size=(8,768)).astype('f4')*.0005
            matrix[:,axis] += 1
            if source:
                matrix[-2:] = 0
                matrix[-2:,2] = 1
            np.save(root/'char-index'/f'{cid}.npy',matrix)
            meta = dict(short_tag=cid,tag=tag,copyright='example',gender={'girl':1},post_count=post_count,core_tags=['blue_hair'])
            if source:
                meta.update(source=source,posts=list(range(8)),gender={'girl':.5,'boy':.5})
                rows.append({k:meta[k] for k in ('short_tag','tag','copyright','post_count')})
            (root/'char-index'/f'{cid}.json').write_text(json.dumps(meta))
        (root/'char-index/malformed.json').write_text('invalid json')
        (root/'char-patch/plan.json').write_text(json.dumps(rows))
        features = np.zeros((4,768),np.float32);features[0,0]=1;features[1:,1]=1;features[3,2]=.6
        np.save(root/'full-features.npy',features)
        (root/'full-manifest.csv').write_text('uuid\na\nb\nc\nd\n')
        with contextlib.closing(sqlite3.connect(root/'pi.db')) as db, db:
            db.executescript("CREATE TABLE assets(uuid TEXT);CREATE TABLE embeddings(uuid TEXT,model TEXT,vec BLOB);CREATE TABLE labels(uuid,dim,value,conf,source,model_ver,run_id,PRIMARY KEY(uuid,dim,value));")
            db.executemany('INSERT INTO assets VALUES (?)',[(u,) for u in ('a','b','c','d','extra')])
            db.executemany('INSERT INTO embeddings VALUES (?,\'ccip\',?)',[('A',features[3].tobytes()),('extra',features[1].tobytes())])
            db.execute('INSERT INTO labels VALUES (\'a\',\'general\',\'solo\',1,\'human\',NULL,NULL)')
        return tags,rows

    def legacy(self, root, tags, request):
        # 固定的算法参考结果：不检查 model_metrics、不加载模型；也不用数值替代品。
        return index_expectations.legacy(self, root)

    def test_cached_apply_clean_build_scan_matches_algorithm_reference(self):
        references = numeric_reference_mode.references(self)
        from tests import numpy_index_reference as reference
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);old=root/'old';new=root/'new';old.mkdir()
            tags,rows=self.prepare(old);shutil.copytree(old,new)
            expected=[]
            # 每种参考都从未计算的同一份输入开始，候选仅运行一次。
            for kind in references:
                folder=root/kind;shutil.copytree(old,folder)
                request=dict(format='illustash-index/1',identity={'task_id':'synthetic'},before='fixed-input',
                             action='apply',rows=rows,review=str(folder),reason=None)
                (folder/'request.json').write_text(json.dumps(request))
                result=(reference.cached_apply(folder,folder/'tags.csv',folder/'request.json') if kind=='numpy'
                        else self.legacy(folder,folder/'tags.csv',folder/'request.json'))
                expected.append((kind,folder,result))
            request.update(review=str(new))
            (new/'request.json').write_text(json.dumps(request))
            proc=subprocess.run([str(self.binary),str(new/'request.json'),str(new/'response.json')],
                                env=os.environ|{'PATH':'','ILLUSTASH_INDEX_TAGS':str(new/'tags.csv')},capture_output=True)
            self.assertEqual(proc.returncode,0,proc.stderr.decode())
            actual=json.loads((new/'response.json').read_text())
            for kind,folder,result in expected:
                with numeric_reference_mode.comparing(self,kind):
                    self.assertEqual(actual['characters'],result['characters'])
                    self.assertEqual(sorted(actual['aliases']),sorted(result['aliases']))
                    self.assertEqual(sorted(actual['labels']),sorted(result['labels']))
                    self.assertEqual(len(actual['characters']),2)
                    checks.assert_index_files(self,new,folder,cached=True)
                    with contextlib.closing(sqlite3.connect(new/'pi.db')) as db:
                        self.assertEqual(db.execute("SELECT value,source FROM labels WHERE dim='general'").fetchall(),[('solo','human')])

    def prepare_large(self, old):
        return index_expectations.large(self,old)

    def test_large_alias_matrix_at_boundary_matches_old_scan(self):
        references = numeric_reference_mode.references(self)
        from tests import numpy_index_reference as reference
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            old = root/'old'
            _, _, edges = self.prepare_large(old)
            with (old/'full-manifest.csv').open(newline='') as manifest:
                manifest_ids = [row['uuid'].strip() for row in csv.DictReader(manifest)]
            self.assertEqual((len(manifest_ids), len(set(manifest_ids))), (129, 129))
            with contextlib.closing(sqlite3.connect(old/'pi.db')) as db:
                source_ids = {uuid for (uuid,) in db.execute('SELECT uuid FROM assets')}
                old_claims = {uuid for (uuid,) in db.execute("SELECT uuid FROM labels WHERE dim='char_ext'")}
            self.assertTrue(source_ids.isdisjoint(manifest_ids))
            self.assertTrue(old_claims.isdisjoint(manifest_ids))

            def request_for(folder):
                path = folder/'request.json'
                path.write_text(json.dumps(dict(format='illustash-index/1', identity={'task_id': 'large'}, before='fixed',
                                                action='apply', rows=[], review=str(folder), reason=None)))
                return path

            expected = []
            for kind in references:
                folder = root/kind
                shutil.copytree(old, folder)
                request = request_for(folder)
                result = (reference.cached_apply(folder, folder/'tags.csv', request) if kind == 'numpy'
                          else self.legacy(folder, folder/'tags.csv', request))
                expected.append((kind, folder, result))

            candidates = []
            # 原源表下只检查扫描诊断；登记相同清单后精确比较可发布的全部认领。
            for name in ('orphan', 'registered'):
                folder = root/name
                shutil.copytree(old, folder)
                if name == 'registered':
                    with contextlib.closing(sqlite3.connect(folder/'pi.db')) as db, db:
                        db.executemany('INSERT INTO assets(uuid) VALUES (?)', ((uuid,) for uuid in manifest_ids))
                request = request_for(folder)
                response = folder/'response.json'
                proc = subprocess.run([str(self.binary), str(request), str(response)], capture_output=True,
                                      env=os.environ | {'PATH': '', 'ILLUSTASH_INDEX_TAGS': str(folder/'tags.csv')})
                self.assertEqual(proc.returncode, 0, proc.stderr.decode())
                candidates.append((name, folder, json.loads(response.read_text())))

            for kind, folder, result in expected:
                with numeric_reference_mode.comparing(self, kind):
                    boundary = reference.boundary_edges(folder, edges) if kind == 'numpy' else edges
                    labels = {(u, c) for u, c, _ in result['labels']}
                    self.assertEqual({u for u, _ in labels}, set(manifest_ids))
                    for i, parent, within in boundary:
                        self.assertIn((f'asset{i}', f'large{parent if within else i:03}'), labels)
                    for name, candidate, actual in candidates:
                        self.assertEqual(actual['characters'], result['characters'], name)
                        self.assertEqual(sorted(actual['aliases']), sorted(result['aliases']), name)
                        claims = result['labels'] if name == 'registered' else []
                        self.assertEqual(sorted(actual['labels']), sorted(claims), name)
                        checks.assert_index_files(self, candidate, folder)

    def test_recognition_snapshot_alias_and_claims_on_large_boundary(self):
        references = numeric_reference_mode.references(self)
        from tests import numpy_index_reference as reference
        path=os.environ.get('ILLUSTASH_RECOGNITION_COMPUTE_BIN')
        if not path:
            if numeric_reference_mode.mode() == 'numpy':
                raise RuntimeError('explicit recognition computation candidate required')
            raise unittest.SkipTest('explicit recognition computation candidate required')
        binary=Path(path).resolve(strict=True)
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);old=root/'old';tags,centres,edges=self.prepare_large(old)
            request=dict(format='illustash-index/1',identity={'task_id':'large'},before='fixed',action='apply',rows=[],review=str(old),reason=None)
            (old/'request.json').write_text(json.dumps(request));self.legacy(old,tags,old/'request.json')
            anchors=np.load(old/'char-anchors.npy');ids=(old/'char-anchors.txt').read_text().splitlines()
            assets=[f'asset{i}' for i,_,_ in edges]
            features=[centres[i].tolist() for i,_,_ in edges]
            expected=[]
            for kind in references:
                if kind=='numpy':
                    _,th,_,th75,entities=reference.anchor_index(anchors,ids,old/'pi.db')
                    limits=reference.claim_thresholds(old,ids,th,th75)
                    boundary=reference.boundary_edges(old,edges)
                else:
                    with patch.object(char_scan,'cross',side_effect=lambda a,b,check=True:ccip.cross(a,b,check=False)),contextlib.redirect_stdout(io.StringIO()):
                        _,limits,_,_,entities=char_scan.anchor_index(anchors,ids,str(old/'pi.db'))
                    boundary=edges
                # 原 Recogniser.labels 的独立判定；模型推理不在本用例范围。
                claims=[]
                for i,_,_ in edges:
                    asset=f'asset{i}';feature=centres[i]
                    distances=ccip.cross(anchors,feature[None],check=False)[:,0];seen=set()
                    # 两种参考都遵守获批的标量并列顺序，不让 SIMD 排序改变业务契约。
                    for a in reference.scalar_order(distances)[:9]:
                        entity=entities[a]
                        if entity in seen:continue
                        seen.add(entity)
                        if limits[a]>0 and distances[a]<=limits[a]:
                            claims.append(dict(asset=asset,character=entity,distance=float(distances[a]),
                                               threshold=float(limits[a]),confidence=1-float(distances[a])))
                expected.append((kind,entities,claims,boundary))
            tamper_asset,tamper_character='asset128','large003'
            # 某内核把 asset128 合法归到 large003，固定替换就不再修改 sidecar。
            # 显式同机窗口只按独立期望选一个现存但不获认领的 ID，仍须严格拒绝篡改。
            if 'numpy' in references:
                accepted={claim['character'] for kind,_,claims,_ in expected if kind=='numpy'
                          for claim in claims if claim['asset']==tamper_asset}
                if tamper_character in accepted:
                    tamper_character=next(cid for cid in ids if cid not in accepted)
            for _,_,claims,_ in expected:
                self.assertNotIn(tamper_character,{claim['character'] for claim in claims if claim['asset']==tamper_asset})
            request=dict(mode='claims',shared=str(old),source=str(old/'pi.db'),assets=assets,features=features,
                         identity=dict(package=str(root),library_id='synthetic',generation=1,session='0'*64,task_id='1'*32),
                         tamper_asset=tamper_asset,tamper_character=tamper_character)
            (root/'claims-request.json').write_text(json.dumps(request))
            proc=subprocess.run([str(binary),str(root/'claims-request.json'),str(root/'claims-response.json')],env=os.environ|{'PATH':''},capture_output=True)
            self.assertEqual(proc.returncode,0,proc.stderr.decode());got=json.loads((root/'claims-response.json').read_text())
            for claim in got['claims']:
                for key in ('distance','threshold'):claim[key]=float(np.float32(claim[key]))
            for kind,entities,claims,boundary in expected:
                with numeric_reference_mode.comparing(self,kind):
                    self.assertEqual(got['entities'],entities)
                    self.assertEqual(got['claims'],claims)
                    self.assertTrue(got['tampered_rejected'])
                    for i,parent,within in boundary:
                        self.assertIn((f'asset{i}',f'large{parent if within else i:03}'),
                                      [(c['asset'],c['character'])for c in got['claims']])

    def test_missing_runtime_with_good_image_fails_without_response_or_rejection(self):
        from PIL import Image
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);self.prepare(root)
            image=root/'char-patch/img/first/1.png';image.parent.mkdir(parents=True);Image.new('RGB',(8,8),'white').save(image)
            (root/'char-patch/first-refs.json').write_text(json.dumps([dict(path=str(image),id=1,general='solo 1girl')]))
            (root/'request.json').write_text(json.dumps(dict(format='illustash-index/1',identity={'task_id':'synthetic'},before='fixed',action='apply',rows=[dict(short_tag='first')],review=str(root))))
            p=subprocess.run([str(self.binary),str(root/'request.json'),str(root/'response.json')],capture_output=True,env=os.environ|{'ILLUSTASH_INDEX_TAGS':str(root/'tags.csv'),'ILLUSTASH_INDEX_RUNTIME':str(root/'missing-runtime'),'ILLUSTASH_INDEX_MODEL':str(root/'missing-model')})
            self.assertNotEqual(p.returncode,0)
            self.assertFalse((root/'response.json').exists())
            self.assertFalse((root/'char-patch/rejected/first.json').exists())

    def test_reference_dtype_layout_endian(self):
        references = numeric_reference_mode.references(self)
        from tests import numpy_index_reference as reference
        for dtype in ('<f4','>f4','<f8','>f8'):
            for order in ('C','F'):
                with self.subTest(dtype=dtype,order=order),tempfile.TemporaryDirectory() as tmp:
                    root=Path(tmp);tags,rows=self.prepare(root)
                    rng=np.random.default_rng(163)
                    a=rng.normal(size=(19,768)).astype(dtype)*.003;a[:,0]+=1
                    a=np.array(a,dtype=dtype,order=order)
                    np.save(root/'char-index/alice.npy',a)
                    # 缓存的上游行不用任何模型重建；patch 行不动。
                    request=dict(format='illustash-index/1',identity={'task_id':'dtype'},before='fixed',action='apply',rows=[],review=str(root),reason=None)
                    (root/'request.json').write_text(json.dumps(request))
                    proc=subprocess.run([str(self.binary),str(root/'request.json'),str(root/'response.json')],env=os.environ|{'PATH':'','ILLUSTASH_INDEX_TAGS':str(tags)},capture_output=True)
                    self.assertEqual(proc.returncode,0,proc.stderr.decode())
                    got=json.loads((root/'response.json').read_text())['characters'][0]
                    saved_header,saved_data=checks.npy_content(root/'char-anchors.npy')
                    for kind in references:
                        with numeric_reference_mode.comparing(self,kind):
                            anchor=reference.merge(a) if kind=='numpy' else build.merge(a)
                            distances=(1-(a/np.linalg.norm(a,axis=1,keepdims=True))@(anchor/np.linalg.norm(anchor)))/2
                            for key,q in [('anchor_p50',50),('anchor_p75',75),('anchor_p90',90)]:
                                expected=float(np.percentile(distances,q));self.assertEqual(got[key],expected)
                            self.assertEqual(saved_header,((3,768),False,np.dtype('<f4')))
                            self.assertEqual(len(saved_data),3*768*4)
                            self.assertEqual(saved_data[:768*4],anchor.astype('<f4').tobytes())

    def test_scan_exact_threshold_and_adjacent_float_inputs_match_old_csv(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);old=root/'old';new=root/'new';old.mkdir();tags,_=self.prepare(old)
            for path in (old/'char-index').glob('*'):
                path.unlink()
            a=np.zeros((6,768),np.float32);a[:,0]=1;a[:3,1]=.125;a[3:,1]=-.125
            np.save(old/'char-index/edge.npy',a)
            (old/'char-index/edge.json').write_text(json.dumps(dict(short_tag='edge',tag='edge',gender={'girl':1},post_count=1)))
            anchor=build.merge(a);dd=(1-(a/np.linalg.norm(a,axis=1,keepdims=True))@(anchor/np.linalg.norm(anchor)))/2
            threshold=np.float32(np.percentile(dd,90))
            rows=[a[0].copy()]
            for direction,expected_within in ((np.float32(-np.inf),True),(np.float32(np.inf),False)):
                v=a[0].copy()
                for _ in range(2048):
                    v[1]=np.nextafter(v[1],direction)
                    distance=ccip.cross(anchor[None],v[None],check=False)[0,0]
                    if (distance<=threshold)==expected_within and distance!=threshold:
                        rows.append(v.copy());break
                else:self.fail('independent reference failed to bracket threshold')
            self.assertEqual(ccip.cross(anchor[None],rows[0][None],check=False)[0,0],threshold)
            np.save(old/'full-features.npy',np.stack(rows));(old/'full-manifest.csv').write_text('uuid\na\nb\nc\n')
            with contextlib.closing(sqlite3.connect(old/'pi.db')) as c, c:c.execute('DELETE FROM embeddings')
            shutil.copytree(old,new)
            for folder in (old,new):
                request=dict(format='illustash-index/1',identity={'task_id':'edge'},before='fixed',action='apply',rows=[],review=str(folder),reason=None)
                (folder/'request.json').write_text(json.dumps(request))
                if folder==old:self.legacy(old,tags,old/'request.json')
                else:
                    p=subprocess.run([str(self.binary),str(new/'request.json'),str(new/'response.json')],env=os.environ|{'PATH':'','ILLUSTASH_INDEX_TAGS':str(new/'tags.csv')},capture_output=True)
                    self.assertEqual(p.returncode,0,p.stderr.decode())
            self.assertEqual((new/'char-scan.csv').read_text(),(old/'char-scan.csv').read_text())
            with (new/'char-scan.csv').open() as f:
                self.assertEqual([r['within'] for r in csv.DictReader(f)],['1','1','0'])

class NativeIndexActualCcipTest(unittest.TestCase):
    """只在显式授权的模型窗口里运行：8 张生成图片，真实的 App 索引发布。"""
    @classmethod
    def setUpClass(cls):
        from tests.test_native_index import NativeIndexTest
        path=os.environ.get('ILLUSTASH_INDEX_COMPUTE_BIN')
        if not path:raise RuntimeError('explicit index computation candidate required')
        cls.binary=Path(path).resolve(strict=True)
        cls.model=os.environ.get('ILLUSTASH_INDEX_MODEL')
        cls.runtime=os.environ.get('ILLUSTASH_INDEX_RUNTIME')
        if not cls.model or not cls.runtime:
            raise RuntimeError('explicit authorised CCIP model/runtime required')
        NativeIndexTest.setUpClass()

    def test_download_compute_publish_drop_preserves_human_history(self):
        from tests.test_native_index import NativeIndexTest
        from tests.test_native_human_labels import snapshot
        from PIL import Image
        fixture=NativeIndexTest('test_check_watch_apply_drop_formal_projection_history_names')
        self.addCleanup(fixture.doCleanups)
        fixture.setUp()
        self.addCleanup(fixture.tearDown)
        fixture.stop(fixture.process)
        for cid,axis in [('alice',0),('bob',2)]:
            a=np.zeros((6,768),np.float32);a[:,axis]=1;a[:,3]=np.arange(6)*.002
            np.save(fixture.shared/'char-index'/f'{cid}.npy',a)
            (fixture.shared/'char-index'/f'{cid}.json').write_text(json.dumps(dict(short_tag=cid,tag=cid+'_tag',copyright='example',gender={'girl':1},post_count=300)))
        with contextlib.closing(sqlite3.connect(fixture.lib.source)) as c, c:
            for col in ('anchor_p90 REAL','base_tag TEXT','skin TEXT','post_count INTEGER','gender_conf REAL','anchor_p50 REAL','anchor_p75 REAL'):
                c.execute('ALTER TABLE characters ADD COLUMN '+col)
            c.execute('CREATE TABLE embeddings(uuid TEXT,model TEXT,vec BLOB)')
            for col in ('source','model_ver','run_id'):
                c.execute('ALTER TABLE labels ADD COLUMN '+col+' TEXT')
        images=[]
        for i in range(8):
            y,x=np.indices((64,64));rgb=np.stack(((x*3+y*2+i)%256,(x+y*3+i)%256,(x*2+y+i)%256),axis=2).astype('u1')
            images.append(Image.fromarray(rgb))
        old_dir=ccip.MODEL_DIR
        ccip.MODEL_DIR=str(Path(self.model).parent)
        self.addCleanup(setattr,ccip,'MODEL_DIR',old_dir)
        cc=ccip.features(images) # 独立的真实模型参考结果，输入是同样的 8 张生成图片
        feature_root=fixture.lib.path/'features';feature_root.mkdir(exist_ok=True)
        np.save(feature_root/'full-features.npy',cc[:3]);(feature_root/'full-manifest.csv').write_text('uuid\na\nb\nc\n')
        # 用同样准备好的合成输入构建独立的旧 worker 期望。
        reference=fixture.root/'old-index-calculation';reference.mkdir()
        shutil.copytree(fixture.shared/'char-index',reference/'char-index')
        (reference/'game-chars').mkdir();(reference/'char-patch').mkdir()
        (reference/'album-char-map.json').write_text('{"map":{}}')
        shutil.copy2(feature_root/'full-features.npy',reference/'full-features.npy')
        shutil.copy2(feature_root/'full-manifest.csv',reference/'full-manifest.csv')
        with contextlib.closing(sqlite3.connect(fixture.lib.source)) as src,contextlib.closing(sqlite3.connect(reference/'pi.db')) as dest, dest:src.backup(dest)
        row=fixture.remote_rows[0].copy()
        np.save(reference/'char-index/carol_example.npy',cc)
        (reference/'char-index/carol_example.json').write_text(json.dumps(dict(row,source='danbooru-patch',posts=list(range(8)),gender={'girl':1,'boy':0},core_tags=['synthetic_hair'])))
        (reference/'char-patch/plan.json').write_text(json.dumps([row]))
        reference_request=dict(format='illustash-index/1',identity={'task_id':'synthetic'},before='fixed',review=str(reference),action='apply',rows=[row],reason=None)
        (reference/'request.json').write_text(json.dumps(reference_request))
        expected=NativeIndexComputeTest().legacy(reference,Path(__file__).resolve().parents[1]/'tags/tags_v3.csv',reference/'request.json')
        expected_labels=[tuple(r)for r in expected['labels']if r[1]=='carol_example']
        original=fixture.remote.RequestHandlerClass.do_GET
        def serve(handler):
            if handler.path.startswith('/image/'):
                output=io.BytesIO();images[int(handler.path.rsplit('/',1)[-1])].save(output,format='PNG');data=output.getvalue()
                fixture.calls.append(handler.path);handler.send_response(200);handler.send_header('Content-Type','image/png');handler.end_headers();handler.wfile.write(data)
            else:original(handler)
        fixture.remote.RequestHandlerClass.do_GET=serve
        fixture.env.update(ILLUSTASH_INDEX_WORKER=str(self.binary),ILLUSTASH_INDEX_TAGS=str(Path(__file__).resolve().parents[1]/'tags/tags_v3.csv'),ILLUSTASH_INDEX_MODEL=self.model,ILLUSTASH_INDEX_RUNTIME=self.runtime)
        fixture.remote_rows=fixture.remote_rows[:1]
        fixture.posts=[dict(id=i,tag_string_general='solo 1girl synthetic_hair',media_asset={'variants':[dict(type='720x720',url=f'http://127.0.0.1:{fixture.remote.server_port}/image/{i}')]})for i in range(8)]
        before=snapshot(fixture.lib.db)['actions']
        fixture.restart();fixture.check();fixture.index({'action':'apply','tags':['carol_example']});result=fixture.finish()
        self.assertEqual(result['state']['status'],'done',result)
        self.assertEqual(result['history'][0]['characters'][0]['status'],'added',result)
        self.assertEqual(result['history'][0]['characters'][0]['claims'],len(expected_labels),result)
        self.assertTrue(expected_labels)
        np.testing.assert_array_equal(np.load(fixture.shared/'char-patch/raw/carol_example.npy'),cc)
        with contextlib.closing(sqlite3.connect(fixture.lib.source)) as c, c:self.assertEqual(sorted(c.execute("SELECT uuid,value,conf FROM labels WHERE dim='char_ext' AND value='carol_example'").fetchall()),sorted(expected_labels))
        self.assertEqual(sum(p.startswith('/image/')for p in fixture.calls),8)
        self.assertEqual(snapshot(fixture.lib.db)['actions'],before)
        self.assertFalse((fixture.lib.path/'index-job.json').exists())
        fixture.index({'action':'drop','char':'carol_example','reason':'合成放弃'});result=fixture.finish()
        self.assertEqual(result['state']['status'],'done',result)
        self.assertEqual(result['history'][0]['characters'][0]['status'],'dropped',result)
        self.assertEqual(snapshot(fixture.lib.db)['actions'],before)
        self.assertFalse((fixture.lib.path/'index-job.json').exists())
        self.assertTrue((fixture.shared/'char-patch/raw/carol_example.npy').exists())


def load_tests(loader, tests, pattern):
    # 真实模型的用例只在显式点名时运行；这时缺少输入就失败。
    return unittest.TestSuite(group for group in tests if not any(isinstance(t, NativeIndexActualCcipTest) for t in group))
