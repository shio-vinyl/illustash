"""M4 同步验收：只用测试自己的合成照片数据库、文件夹和图库包。"""
from tests.native_worker_fixture import command as worker_command, source as worker_source, exists as worker_exists

from contextlib import closing
import json
import sqlite3
import subprocess
import os
import shutil
import tempfile
import unittest
from pathlib import Path
from tests.test_native_folder import NativeFolderHTTPTest
from tests import photos_fixture
from tests.test_native_human_labels import snapshot
from tests.test_read_http import wait_until
from tools.wait_limit import WAIT

# 给那些 worker 不能被当成卡住的测试用的空闲窗口：远长于繁忙机器上 worker 的启动时间（#219）。
# 产品默认值是 10 分钟，保持不变。
WIDE_IDLE_MS=8000
# 宿主显示上一条报告之后，限速的 worker 等多久再发下一条。
PACE_S=.8

class NativeSyncTest(unittest.TestCase):
    setUpClass = classmethod(NativeFolderHTTPTest.setUpClass.__func__)
    package = NativeFolderHTTPTest.package
    start = NativeFolderHTTPTest.start
    stop = NativeFolderHTTPTest.stop
    request = NativeFolderHTTPTest.request
    sync = NativeFolderHTTPTest.sync
    finish = NativeFolderHTTPTest.finish
    _folder_setup = NativeFolderHTTPTest.setUp
    _fixture_setup = NativeFolderHTTPTest._fixture_setup
    photo = photos_fixture.photo

    def setUp(self):
        self._folder_setup()
        self.stop(self.process)
        self.library = self.root / 'synthetic.photoslibrary'
        (self.library / 'database').mkdir(parents=True)
        with sqlite3.connect(self.library / 'database/Photos.sqlite') as c:
            c.execute('CREATE TABLE ZASSET(Z_PK INTEGER PRIMARY KEY,ZUUID TEXT,ZKIND INTEGER,ZTRASHEDSTATE INTEGER,ZWIDTH INTEGER,ZHEIGHT INTEGER,ZADDEDDATE REAL,ZHIDDEN INTEGER,ZDIRECTORY TEXT,ZFILENAME TEXT,ZADDITIONALATTRIBUTES INTEGER)')
            c.execute('CREATE TABLE ZADDITIONALASSETATTRIBUTES(Z_PK INTEGER PRIMARY KEY,ZIMPORTEDBYBUNDLEIDENTIFIER TEXT,ZORIGINALSTABLEHASH TEXT,ZORIGINALFILESIZE INTEGER)')
        from tests.library_fixture import ensure_source_schema
        with sqlite3.connect(self.lib.source) as c:
            ensure_source_schema(c)
            if 'z_pk' not in {row[1] for row in c.execute('PRAGMA table_info(assets)')}:
                c.execute('ALTER TABLE assets ADD COLUMN z_pk INTEGER')
        self.source={'id':'photos','kind':'photos','path':str(self.library),'apps':{'com.atebits.Tweetie2':'x','net.pxv.iphone':'pixiv'}}
        self.lib.config['sources'].append(self.source)
        (self.lib.path/'library.json').write_text(json.dumps(self.lib.config))
        self.process,self.log=self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']

    def test_native_library_new_mixed_source_deletion_restoration_paths_and_notifications(self):
        self.photo('D','com.atebits.Tweetie2')
        self.photo('E','net.pxv.iphone')
        self.photo('F','com.apple.springboard')
        with sqlite3.connect(self.lib.source) as c:c.execute("INSERT INTO labels(uuid,dim,value,conf) VALUES ('D','rating','general',1)")
        original=self.library/'originals/D/D.png'
        original.parent.mkdir(parents=True)
        original.write_bytes(b'synthetic')
        before=snapshot(self.library/'database/Photos.sqlite')
        state=self.sync()
        self.assertEqual(state['state']['status'],'needs_recognition',state)
        self.assertEqual(state['state']['total'],2) # 文件夹 + E；D 已经识别过
        self.assertEqual(list(state['history'][0]['sources']),['s1','com.atebits.Tweetie2','net.pxv.iphone','com.apple.springboard'])
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute("SELECT uuid,preset FROM assets WHERE source_id='photos' ORDER BY uuid").fetchall(), [('D','x'),('E','pixiv')])
            self.assertEqual(c.execute("SELECT src_kind,src_path FROM assets WHERE uuid='D'").fetchone(), ('orig',str(original)))
            folder=c.execute("SELECT uuid FROM assets WHERE source_id!='photos'").fetchone()[0]
        self.request('/api/tag',{'ids':['D'],'tag':'合成保留'})
        with sqlite3.connect(self.library/'database/Photos.sqlite') as c:
            c.execute("UPDATE ZASSET SET ZTRASHEDSTATE=1 WHERE ZUUID='D'")
        self.assertEqual(self.sync()['state']['removed'],1)
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT trashed FROM assets WHERE uuid=?',(folder,)).fetchone()[0],0)
        with sqlite3.connect(self.library/'database/Photos.sqlite') as c:
            c.execute("UPDATE ZASSET SET ZTRASHEDSTATE=0 WHERE ZUUID='D'")
        self.assertEqual(self.sync()['state']['found'],1)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute("SELECT tag FROM asset_tags WHERE uuid='D'").fetchall(), [('合成保留',)])
            self.assertGreater(c.execute("SELECT count(*) FROM backend_effects WHERE kind='imported'").fetchone()[0],0)
        self.assertEqual(snapshot(self.library/'database/Photos.sqlite'),before)
        self.assertFalse(self.sentinel.exists())

    def test_native_library_new_source_transaction_failure_and_retry(self):
        self.photo('D','com.atebits.Tweetie2')
        with sqlite3.connect(self.lib.source) as c:
            c.execute("CREATE TRIGGER fail_photo BEFORE INSERT ON asset_links WHEN NEW.system='photos' BEGIN SELECT RAISE(ABORT,'synthetic'); END")
        before=snapshot(self.lib.source)
        result=subprocess.run([str(self.binary),'photos-scan','--db',str(self.lib.source)],input=json.dumps(self.source).encode(),capture_output=True)
        self.assertNotEqual(result.returncode,0)
        self.assertEqual(snapshot(self.lib.source),before)
        with sqlite3.connect(self.lib.source) as c: c.execute('DROP TRIGGER fail_photo')
        self.assertEqual(self.sync()['state']['status'],'needs_recognition')

    def test_native_library_new_original_download_refresh_and_existing_results_retry(self):
        old_message = "[Errno 13] Permission denied: 'synthetic/old-input'"
        with sqlite3.connect(self.lib.db) as c:
            old_id = c.execute("INSERT INTO sync_runs(status,message) VALUES ('error',?)", (old_message,)).lastrowid
        self.photo('D','com.atebits.Tweetie2')
        self.sync()
        original=self.library/'originals/D/D.png';original.parent.mkdir(parents=True);original.write_bytes(b'synthetic')
        with sqlite3.connect(self.lib.source) as c: c.execute("INSERT INTO labels(uuid,dim,value,conf) VALUES ('D','rating','general',1)")
        with sqlite3.connect(self.lib.db) as c:
            c.execute("CREATE TRIGGER fail_app BEFORE INSERT ON assets WHEN NEW.uuid='D' BEGIN SELECT RAISE(ABORT,'synthetic'); END")
        self.assertEqual(self.sync()['state']['status'],'error')
        with sqlite3.connect(self.lib.db) as c: c.execute('DROP TRIGGER fail_app')
        self.assertEqual(self.sync()['state']['status'],'needs_recognition')
        history = json.loads(self.request('/api/sync')['body'])['history']
        self.assertEqual(next(row['message'] for row in history if row['id'] == old_id), old_message)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT message FROM sync_runs WHERE id=?', (old_id,)).fetchone(), (old_message,))
            self.assertEqual(c.execute("SELECT rating FROM asset_rating WHERE uuid='D'").fetchall(), [('general',)])
            self.assertEqual(c.execute("SELECT src_path FROM assets WHERE uuid='D'").fetchone()[0],str(original))

    def test_native_library_new_photos_without_content_hash_keep_rating_on_repeat_scan(self):
        for asset in ('D', 'E'):
            self.photo(asset, 'net.pxv.iphone')
        with sqlite3.connect(self.library / 'database' / 'Photos.sqlite') as c:
            c.execute("""UPDATE ZADDITIONALASSETATTRIBUTES SET ZORIGINALSTABLEHASH=NULL
                         WHERE Z_PK=(SELECT ZADDITIONALATTRIBUTES FROM ZASSET WHERE ZUUID='D')""")
        with sqlite3.connect(self.lib.source) as c:
            c.executemany("INSERT INTO labels(uuid,dim,value,conf) VALUES (?,'rating','general',1)", [('D',), ('E',)])

        def scan():
            result = subprocess.run([str(self.binary), 'library-scan', '--library', str(self.lib.path)],
                                    env=self.env, capture_output=True, timeout=WAIT)
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)

        self.assertGreaterEqual(scan()['added'], 2)
        for _ in range(2):
            result = scan()
            self.assertEqual((result['added'], result['removed']), (0, 0))
            with sqlite3.connect(self.lib.db) as c:
                ratings = c.execute("SELECT uuid,rating FROM asset_rating WHERE uuid IN ('D','E') ORDER BY uuid")
                self.assertEqual(ratings.fetchall(), [('D', 'general'), ('E', 'general')])

    def test_native_library_new_label_only_failure_retries_without_rewriting_unchanged_assets(self):
        self.photo('D', 'com.atebits.Tweetie2')
        with sqlite3.connect(self.lib.source) as c:
            c.execute("INSERT INTO labels(uuid,dim,value,conf) VALUES ('D','rating','general',1)")
        self.assertEqual(self.sync()['state']['status'], 'needs_recognition')

        def fingerprint(c):
            return c.execute("SELECT value FROM meta WHERE key='folder_labels:D'").fetchone()[0]

        with sqlite3.connect(self.lib.db) as c:
            previous = fingerprint(c)
            c.execute("""CREATE TRIGGER fail_import BEFORE INSERT ON assets WHEN NEW.uuid='D'
                         BEGIN SELECT RAISE(ABORT,'synthetic label projection failure'); END""")
        with sqlite3.connect(self.lib.source) as c:
            c.execute("INSERT INTO labels(uuid,dim,value,conf) VALUES ('D','general','1girl',0.9)")
        self.assertEqual(self.sync()['state']['status'], 'error')
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(fingerprint(c), previous)
            c.execute('DROP TRIGGER fail_import')
        retried = self.sync()['state']
        self.assertEqual((retried['found'], retried['imported']), (0, 1))
        with sqlite3.connect(self.lib.db) as c:
            self.assertNotEqual(fingerprint(c), previous)
            self.assertEqual(c.execute("SELECT value FROM general_hits WHERE uuid='D'").fetchall(), [('1girl',)])
            c.execute("""CREATE TRIGGER reject_unchanged_path BEFORE UPDATE OF filename,src_path ON assets WHEN OLD.uuid='D'
                         BEGIN SELECT RAISE(ABORT,'unchanged asset path'); END""")
            c.execute("""CREATE TRIGGER reject_unchanged_hash BEFORE INSERT ON meta WHEN NEW.key='folder_labels:D'
                         BEGIN SELECT RAISE(ABORT,'unchanged asset fingerprint'); END""")
        unchanged = self.sync()['state']
        self.assertEqual((unchanged['status'], unchanged['found'], unchanged['imported']), ('needs_recognition', 0, 0))

    def worker(self, mode='ok', per_run=0):
        import hashlib, sys
        self.stop(self.process)
        (self.resources/'models/ccip').mkdir(parents=True,exist_ok=True)
        (self.resources/'models/wd_swinv2_v3_emb.onnx').write_bytes(b'synthetic WD')
        (self.resources/'models/ccip/model_feat.onnx').write_bytes(b'synthetic CCIP')
        (self.resources/'tags/tags_v3.csv').write_text('name,category\ngeneral,9\nblue_hair,0\nalice,4\n')
        specs={
            'wd': dict(version='swinv2_448',dimension=1024,sha256=hashlib.sha256(b'synthetic WD').hexdigest(),tags_sha256=hashlib.sha256((self.resources/'tags/tags_v3.csv').read_bytes()).hexdigest()),
            'ccip':dict(version='ccip-model_feat',dimension=768,sha256=hashlib.sha256(b'synthetic CCIP').hexdigest())}
        with sqlite3.connect(self.lib.source) as c:
            columns = {row[1] for row in c.execute('PRAGMA table_info(labels)')}
            for col in ('source TEXT DEFAULT "legacy"','model_ver TEXT','run_id TEXT'):
                if col.split()[0] not in columns:
                    c.execute('ALTER TABLE labels ADD COLUMN '+col)
        script=self.root/'synthetic-worker'
        script.write_text('#!'+sys.executable+'\n'+r"""
import hashlib,json,sys,time,os
from pathlib import Path
r=json.loads(Path(sys.argv[1]).read_text())
mode=MODE
def paced_report(stage,processed):
    prog=Path(os.environ['ILLUSTASH_RECOGNITION_PROGRESS']);tmp=prog.with_suffix('.tmp')
    tmp.write_text(json.dumps(dict(format='illustash-recognition-progress/1',task_id=r['identity']['task_id'],stage=stage,total=len(r['assets']),processed=processed,failed=0)))
    tmp.replace(prog)
def wait_for(name):
    # 等待测试放行；无人放行时自行退出，避免一直占着任务。
    go=Path(sys.argv[0]).parent/name;until=time.monotonic()+120
    while not go.exists():
        if time.monotonic()>until: sys.exit(8)
        time.sleep(.01)
# 先报告进度再加载 NumPy：机器繁忙时，仅导入模块就可能耗费数秒。
if mode=='paced': paced_report('preparing',0)
import numpy as np
if mode=='hang':
    # 完整写入后再重命名，保证测试看到文件时能读到完整 pid（#132）。
    (Path(r['output'])/'pid.tmp').write_text(str(os.getpid()));os.replace(Path(r['output'])/'pid.tmp',Path(r['output'])/'pid')
    time.sleep(60)
if mode=='fail': sys.exit(7)
if mode=='progress':
    # 先报告过期任务的值（应忽略），再报告本任务的值；测试看到上一条后才继续。
    prog=Path(os.environ['ILLUSTASH_RECOGNITION_PROGRESS'])
    def report(stage,processed,failed=0,task=r['identity']['task_id']):
        tmp=prog.with_suffix('.tmp')
        tmp.write_text(json.dumps(dict(format='illustash-recognition-progress/1',task_id=task,stage=stage,total=len(r['assets']),processed=processed,failed=failed)))
        tmp.replace(prog)
    report('recognising',len(r['assets']),task='stale-task');time.sleep(.5)
    report('preparing',0);wait_for('go-1')
    report('recognising',1);wait_for('go-2')
    report('recognising',2);wait_for('release')
(Path(sys.argv[0]).parent/'last-request').write_text(json.dumps([a['id'] for a in r['assets']]))
if mode=='wait-for-import': wait_for('release-import')
failures=[]
if mode=='input-changed':
    # 请求之后改变一张图，模拟生产者发现实际字节与请求版本不符。
    changed=r['assets'].pop(0)
    Path(changed['path']).write_bytes(b'synthetic changed input')
    (Path(sys.argv[0]).parent/'changed-asset').write_text(changed['id'])
    failures.append(dict(uuid=changed['id'],reason='input_changed'))
if mode in ('classify','classify-legacy'):
    # 模拟原生生产者（#112）：读不到的文件算 unreadable；完整读入后缺少结束块的 PNG
    # 或其他内容算 undecodable。旧 worker 不提供原因。
    kept=[]
    for a in r['assets']:
        try: b=Path(a['path']).read_bytes()
        except OSError: failures.append(dict(uuid=a['id'],reason='unreadable')); continue
        if b.startswith(b'\x89PNG') and b.endswith(b'IEND\xaeB`\x82'): kept.append(a)
        else: failures.append(dict(uuid=a['id'],reason='undecodable'))
    if mode=='classify-legacy': failures=[dict(uuid=f['uuid']) for f in failures]
    r['assets']=kept
    if not kept:
        Path(sys.argv[2]).write_text(json.dumps(dict(identity=r['identity'],packages=[],failed=failures)))
        sys.exit(0)
if mode in ('stream','stream-hang','stream-gated'):
    # 每张图单独成批，全部文件落盘后才标记完成。
    specs=SPECS
    for i,a in enumerate(r['assets']):
        if mode=='stream-gated':
            wait_for(f'release-batch-{i}')
        if mode=='stream-hang' and i==2:
            time.sleep(60)
        dest=Path(r['output'])/f'batch-{i}';dest.mkdir()
        (dest/'labels.jsonl').write_text(json.dumps(dict(asset=a['id'],dim='rating',value='general',confidence=.9)))
        for model in ('wd','ccip'): np.save(dest/(model+'.npy'),np.ones((1,specs[model]['dimension']),dtype='<f4'))
        m=dict(format='illustash-recognition/2',preprocessing='illustash-wd-ccip/1',library_id=r['identity']['library_id'],assets=[a['id']],inputs={a['id']:a['input_sha256']},models=specs,files={f:dict(bytes=(dest/f).stat().st_size,sha256=hashlib.sha256((dest/f).read_bytes()).hexdigest()) for f in ('labels.jsonl','wd.npy','ccip.npy')})
        (dest/'manifest.json').write_text(json.dumps(m))
        (dest/'complete.json').write_text(json.dumps(dict(format='illustash-recognition-batch/1',task_id=r['identity']['task_id'],assets=[a['id']],manifest_sha256=hashlib.sha256((dest/'manifest.json').read_bytes()).hexdigest(),sidecar_sha256=None)))
        time.sleep(.6)
    Path(sys.argv[2]).write_text(json.dumps(dict(identity=r['identity'],packages=[str(Path(r['output'])/f'batch-{i}') for i in range(len(r['assets']))],failed=[])))
    sys.exit(0)
if mode=='paced':
    # 每条进度和最终结果都等待测试放行，由测试控制节奏（#219）。
    for step in range(1,len(r['assets'])+2):
        wait_for(f'step-{step}')
        if step<=len(r['assets']): paced_report('recognising',step)
p=Path(r['output'])/'result';p.mkdir()
ids=[a['id'] for a in r['assets']]
(p/'labels.jsonl').write_text('\n'.join(json.dumps(dict(asset=u,dim='rating',value='general',confidence=.9)) for u in ids))
specs=SPECS
for model in ('wd','ccip'): np.save(p/(model+'.npy'),np.ones((len(ids),specs[model]['dimension']),dtype='<f4'))
if mode=='content':
    # 像真实模型一样按文件字节生成向量：同路径换图后得到不同向量。
    fills=[[sum(Path(a['path']).read_bytes())%97+1] for a in r['assets']]
    for model in ('wd','ccip'): np.save(p/(model+'.npy'),np.repeat(np.asarray(fills,dtype='<f4'),specs[model]['dimension'],axis=1))
m=dict(format='illustash-recognition/2',preprocessing='illustash-wd-ccip/1',library_id=r['identity']['library_id'],assets=ids,inputs={a['id']:a['input_sha256'] for a in r['assets']},models=specs,files={f:dict(bytes=(p/f).stat().st_size,sha256=hashlib.sha256((p/f).read_bytes()).hexdigest()) for f in ('labels.jsonl','wd.npy','ccip.npy')})
(p/'manifest.json').write_text(json.dumps(m))
if mode=='stale-result':
    # 模拟推理后文件被原地改写：大小、mtime 和资产 ID 不变，仍回传旧字节的识别结果。
    path=Path(r['assets'][0]['path']);stat=path.stat();data=path.read_bytes()
    path.write_bytes(bytes([data[0]^1])+data[1:])
    os.utime(path,ns=(stat.st_atime_ns,stat.st_mtime_ns))
if mode=='wrong':r['identity']['generation']+=1
packages=[str(p)]
if mode=='splitbad' and len(ids)>1:
    packages=[]
    for i,u in enumerate(ids):
        dest=Path(r['output'])/('batch'+str(i));dest.mkdir()
        (dest/'labels.jsonl').write_text(json.dumps(dict(asset=u,dim='rating',value='general',confidence=.9)))
        for model in ('wd','ccip'):np.save(dest/(model+'.npy'),np.full((1,specs[model]['dimension']),0 if i==1 else 1,dtype='<f4'))
        manifest=dict(m,assets=[u],inputs={u:m['inputs'][u]},files={f:dict(bytes=(dest/f).stat().st_size,sha256=hashlib.sha256((dest/f).read_bytes()).hexdigest()) for f in ('labels.jsonl','wd.npy','ccip.npy')})
        (dest/'manifest.json').write_text(json.dumps(manifest));packages.append(str(dest))
Path(sys.argv[2]).write_text(json.dumps(dict(identity=r['identity'],packages=packages,failed=[])))
if failures: Path(sys.argv[2]).write_text(json.dumps(dict(identity=r['identity'],packages=packages,failed=failures)))
""".replace('MODE',repr(mode)).replace('SPECS',repr(specs)))
        script.chmod(0o700)
        self.env['ILLUSTASH_RECOGNITION_WORKER']=worker_command(script)
        self.process,self.log=self.start(self.lib.path,extra=('--sync-limit',str(per_run)))
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']

    def finish(self):
        import time
        # 比这里最宽的空闲窗口还长：卡住的 worker 由宿主停下，而不是靠这个等待。
        until=time.monotonic()+WIDE_IDLE_MS/1000+WAIT
        while time.monotonic()<until:
            response=self.request('/api/sync')
            self.assertEqual(response['status'],200)
            value=json.loads(response['body'])
            if value['state']['status'] not in ('scanning','importing','recognising'): return value
            time.sleep(.02)
        self.fail(f'sync did not finish: {value}')

    def test_native_library_new_worker_per_run_failure_retry_and_identity(self):
        self.worker(per_run=1)
        state=self.sync()
        self.assertEqual(state['state']['status'],'done',state)
        self.assertEqual((state['state']['total'],state['state']['done']),(1,1))
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM recognition_assets').fetchone()[0],1)
        self.assertEqual(self.sync()['state']['status'],'done')
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM recognition_assets').fetchone()[0],1)
        # 先给 source 加新的活，再让进程失败。不能留下部分完成的正式识别结果。
        self.photo('D','com.atebits.Tweetie2')
        original=self.library/'originals/D/D.png';original.parent.mkdir(parents=True);original.write_bytes(b'synthetic')
        script=self.root/'synthetic-worker'
        contents=script.read_text();script.write_text(contents.replace("mode='ok'","mode='fail'"))
        self.assertEqual(self.sync()['state']['status'],'error')
        script.write_text(contents.replace("mode='ok'","mode='wrong'"))
        self.assertEqual(self.sync()['state']['status'],'error')
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute("SELECT count(*) FROM recognition_assets WHERE asset='D'").fetchone()[0],0)
        script.write_text(contents)
        self.assertEqual(self.sync()['state']['status'],'done')
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute("SELECT count(*) FROM recognition_assets WHERE asset='D'").fetchone()[0],1)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute("SELECT rating FROM asset_rating WHERE uuid='D'").fetchall(),[('general',)])
        self.assertFalse(self.sentinel.exists())

    def test_native_library_new_recognition_takes_the_newest_saved_pictures_first(self):
        # #360：待识别的图按保存时间从新到旧送去识别，同一时间按资产 ID，没有保存时间的排在最后；
        # 每轮上限留下的是最新的那几张，其余的等下一轮。保存时间各差一天以上，换算成本地时间后先后不变。
        day=86400
        saved={'E':780000000+3*day,'F':780000000+day,'G':780000000+2*day,'I':780000000+2*day,'H':None}
        for uuid in saved:
            self.photo(uuid,'com.atebits.Tweetie2')
            original=self.library/f'originals/{uuid}/{uuid}.png';original.parent.mkdir(parents=True);original.write_bytes(b'synthetic')
        with sqlite3.connect(self.library/'database/Photos.sqlite') as c:
            c.executemany('UPDATE ZASSET SET ZADDEDDATE=? WHERE ZUUID=?',[(when,uuid) for uuid,when in saved.items()])
        self.worker(per_run=2)
        sent=[]
        for _ in range(3):
            state=self.sync()
            self.assertEqual(state['state']['status'],'done',state)
            self.assertEqual((state['state']['total'],state['state']['done']),(2,2))
            sent.append(json.loads((self.root/'last-request').read_text()))
        with sqlite3.connect(self.lib.source) as c:
            added=dict(c.execute('SELECT uuid,added_at FROM assets'))
        self.assertEqual(added['H'],'')
        # 文件夹里那张图的创建时间随平台而定（folder_fixture.settle），它排在哪里按它实际的保存时间算。
        expected=sorted(sorted(added),key=added.get,reverse=True)
        self.assertEqual(len(expected),6)
        self.assertEqual([u for u in expected if u in saved],['E','G','I','F','H'])
        self.assertEqual(sent,[expected[:2],expected[2:4],expected[4:]])
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM recognition_assets').fetchone()[0],6)
        self.assertFalse(self.sentinel.exists())

    def test_native_library_new_worker_progress_before_exit(self):
        for uuid in ('E','F'):
            self.photo(uuid,'com.atebits.Tweetie2')
            original=self.library/f'originals/{uuid}/{uuid}.png';original.parent.mkdir(parents=True);original.write_bytes(b'synthetic')
        self.worker('progress')
        self.assertEqual(self.request('/api/sync',{})['body'],b'{"started": true}')
        seen=[]
        def shown(stage,done):
            s=json.loads(self.request('/api/sync')['body'])['state']
            seen.append({k:s.get(k) for k in ('status','stage','done','total','failed')})
            if s['status'] not in ('scanning','importing','recognising'): self.fail(seen[-5:])
            return s['status']=='recognising' and s.get('stage')==stage and s.get('done')==done
        # 测试看到每个值之后 worker 才往下走，不论哪一边跑得多慢（#219）。
        wait_until(lambda: shown('preparing',0));(self.root/'go-1').touch()
        wait_until(lambda: shown('recognising',1));(self.root/'go-2').touch()
        wait_until(lambda: shown('recognising',2))
        total=seen[-1]['total']
        self.assertGreaterEqual(total,3)
        self.assertEqual(seen[-1]['failed'],0)
        recognising=[v for v in seen if v['status']=='recognising']
        # 面板上只出现这个任务不断变化的计数；过期的总数从不出现。
        self.assertIn('preparing',[v['stage'] for v in recognising])
        self.assertTrue(all(v['done'] in (0,1,2) for v in recognising),recognising)
        self.assertFalse((self.root/'release').exists())
        (self.root/'release').touch()
        value=self.finish()['state']
        self.assertEqual(value['status'],'done',value)
        self.assertEqual((value['done'],value['total'],value['imported'],value['import_done'],value['import_total']),
                         (total,total,total,total,total))

    def test_native_library_new_changed_input_keeps_other_results_and_retries(self):
        for uuid in ('E', 'F'):
            self.photo(uuid, 'com.atebits.Tweetie2')
            original = self.library / f'originals/{uuid}/{uuid}.png'
            original.parent.mkdir(parents=True)
            original.write_bytes(b'synthetic')
        self.worker('input-changed')
        state = self.sync()['state']
        self.assertEqual(state['status'], 'done', state)
        self.assertEqual(state['failed'], 1)
        self.assertEqual(state['input_done'], state['input_total'])
        self.assertEqual(state.get('damaged', 0), 0)
        changed = (self.root / 'changed-asset').read_text()
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM recognition_assets WHERE asset=?', (changed,)).fetchone()[0], 0)
            self.assertEqual(c.execute('SELECT count(*) FROM recognition_assets').fetchone()[0], state['total'] - 1)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM asset_rating WHERE uuid=?', (changed,)).fetchone()[0], 0)
            self.assertIsNone(c.execute("SELECT value FROM prefs WHERE key='sync_damaged'").fetchone())
        self.worker('ok')
        retried = self.sync()['state']
        self.assertEqual((retried['status'], retried['failed']), ('done', 0), retried)
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM recognition_assets WHERE asset=?', (changed,)).fetchone()[0], 1)

    def test_native_library_new_stale_worker_result_rolls_back_the_entire_batch(self):
        self.photo('E', 'com.atebits.Tweetie2')
        original = self.library / 'originals/E/E.png'
        original.parent.mkdir(parents=True)
        original.write_bytes(b'synthetic')
        self.sync()
        # 未识别的照片还没进入应用库；给已扫描导入的文件夹资产建立人工标签。
        with sqlite3.connect(self.lib.db) as c:
            tagged = c.execute('SELECT uuid FROM assets ORDER BY uuid LIMIT 1').fetchone()[0]
        self.assertEqual(self.request('/api/tag', {'ids': [tagged], 'tag': '合成保留'})['status'], 200)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT tag FROM asset_tags WHERE uuid=?', (tagged,)).fetchall(), [('合成保留',)])
        self.worker('stale-result')
        state = self.sync()['state']
        self.assertEqual(state['status'], 'error', state)
        self.assertIn('已过期', state['message'])
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute("SELECT count(*) FROM labels WHERE source='wd_v3'").fetchone()[0], 0)
            self.assertEqual(c.execute('SELECT count(*) FROM embeddings').fetchone()[0], 0)
            self.assertEqual(c.execute("SELECT count(*) FROM sqlite_master WHERE name='recognition_imports'").fetchone()[0], 0)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM asset_rating').fetchone()[0], 0)
            self.assertEqual(c.execute('SELECT tag FROM asset_tags WHERE uuid=?', (tagged,)).fetchall(), [('合成保留',)])
        self.worker('ok')
        self.assertEqual(self.sync()['state']['status'], 'done')

    def test_native_library_new_worker_deadline_follows_progress_and_reports_why(self):
        import time
        photos='EFGHIJKLMN'
        for uuid in photos:
            self.photo(uuid,'com.atebits.Tweetie2')
            original=self.library/f'originals/{uuid}/{uuid}.png';original.parent.mkdir(parents=True);original.write_bytes(b'synthetic')
        # worker 每张图报告一次，宿主显示上一条之后，测试稍等片刻再放行下一条，所以这次运行只靠进度就能撑过
        # 空闲窗口。窗口很宽，所以繁忙机器上 worker 启动慢也能及时报告（#219）；时限规则本身在手动拨动的时钟上
        # 做单元测试（app_tasks）。
        self.env['ILLUSTASH_RECOGNITION_TIMEOUT_MS']=str(WIDE_IDLE_MS)
        self.worker('paced')
        self.assertEqual(self.request('/api/sync',{})['body'],b'{"started": true}')
        def shown(stage,done):
            state=json.loads(self.request('/api/sync')['body'])['state']
            if state['status'] not in ('scanning','importing','recognising'): self.fail(state)
            return state['status']=='recognising' and state.get('stage')==stage and state.get('done')==done and state
        total=wait_until(lambda: shown('preparing',0))['total']
        self.assertEqual(total,len(photos)+1)
        first=time.monotonic()
        for step in range(1,total+2):
            time.sleep(PACE_S)
            (self.root/f'step-{step}').touch()
            if step<=total: wait_until(lambda: shown('recognising',step))
        value=self.finish()
        self.assertEqual(value['state']['status'],'done',value)
        self.assertEqual(value['state']['done'],value['state']['total'])
        self.assertGreater(time.monotonic()-first,WIDE_IDLE_MS/1000)
        # 同一个 worker，不放它越过第一条报告，由总时限停下，并保留这个原因。
        for step in self.root.glob('step-*'): step.unlink()
        with sqlite3.connect(self.lib.source) as c: c.execute(f"DELETE FROM labels WHERE uuid IN ({','.join('?'*len(photos))})",tuple(photos))
        self.env.update(ILLUSTASH_RECOGNITION_TIMEOUT_MS='60000',ILLUSTASH_RECOGNITION_MAX_MS='1000')
        self.worker('paced')
        value=self.sync()
        self.assertEqual(value['state']['status'],'interrupted',value)
        self.assertEqual(value['state']['message'],'任务超时：超过 1 秒上限')
        self.assertEqual((value['history'][0]['status'],value['history'][0]['message']),('interrupted','任务超时：超过 1 秒上限'))
        self.env.pop('ILLUSTASH_RECOGNITION_MAX_MS')

    def test_native_library_new_finished_batches_survive_interruption(self):
        for uuid in ('E','F'):
            self.photo(uuid,'com.atebits.Tweetie2')
            original=self.library/f'originals/{uuid}/{uuid}.png';original.parent.mkdir(parents=True);original.write_bytes(b'synthetic')
        # 两批完成后，worker 卡住超过空闲窗口。窗口很宽，
        # 所以繁忙机器上第一批之前加载 NumPy 的时间不会被当成卡住（#219）。
        self.env['ILLUSTASH_RECOGNITION_TIMEOUT_MS']=str(WIDE_IDLE_MS)
        self.worker('stream-hang')
        value=self.sync()
        self.assertEqual(value['state']['status'],'interrupted',value)
        self.assertEqual(value['state']['message'],f'任务超时：{WIDE_IDLE_MS//1000} 秒没有进展')
        first=json.loads((self.root/'last-request').read_text())
        self.assertEqual(len(first),3)
        # 文件夹里的图片在扫描导入时计数；流式送来的一批只计它带来的照片图片。前两批包含哪些图片按识别的顺序
        # （保存时间从新到旧，#360）决定。
        self.assertEqual((value['history'][0]['status'],value['history'][0]['imported']),
                         ('interrupted',1+sum(u in ('E','F') for u in first[:2])))
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(sorted(u for u, in c.execute('SELECT asset FROM recognition_assets')),sorted(first[:2]))
            self.assertEqual(c.execute("SELECT count(*) FROM recognition_imports WHERE status='done'").fetchone()[0],2)
        # 下一次同步只识别剩下的那张图片，运行完成。
        self.env.pop('ILLUSTASH_RECOGNITION_TIMEOUT_MS')
        self.worker('stream')
        value=self.sync()
        self.assertEqual(value['state']['status'],'done',value)
        self.assertEqual(json.loads((self.root/'last-request').read_text()),first[2:])
        self.assertEqual((value['state']['imported'],value['state']['import_done'],value['state']['import_total']),(1,1,1))
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM recognition_assets').fetchone()[0],3)

    def test_native_library_new_worker_timeout_and_switch_cancel(self):
        self.env['ILLUSTASH_RECOGNITION_TIMEOUT_MS']='200'
        self.worker('hang')
        value=self.sync()
        self.assertEqual(value['state']['status'],'interrupted')
        self.assertEqual(value['state']['message'],'任务超时：200 毫秒没有进展')
        self.env.pop('ILLUSTASH_RECOGNITION_TIMEOUT_MS')
        self.stop(self.process)
        self.process,self.log=self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(self.request('/api/sync',{})['status'],200)
        import time
        for _ in range(100):
            if json.loads(self.request('/api/sync')['body'])['state']['status']=='recognising':break
            time.sleep(.01)
        self.assertEqual(self.request('/api/sync',{})['body'],b'{"started": false}')
        other = self.package('other')
        other.config['id']=self.lib.config['id']
        (other.path/'library.json').write_text(json.dumps(other.config))
        self.assertEqual(self.request('/api/libraries/open',{'path':str(other.path)})['status'],200)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(json.loads(self.request('/api/sync')['body'])['state']['status'],'off')
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT status FROM sync_runs ORDER BY id DESC LIMIT 1').fetchone()[0],'interrupted')
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM sqlite_master WHERE name="recognition_assets"').fetchone()[0],0)

    def test_native_library_new_startup_preferences_off_freeze_and_limits(self):
        self.stop(self.process)
        self.process,self.log=self.start(self.lib.path,extra=('--auto-sync',))
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(self.finish()['state']['status'],'needs_recognition')
        with sqlite3.connect(self.lib.db) as c:
            c.execute("INSERT OR REPLACE INTO prefs VALUES ('startup',?)",(json.dumps({"sync":False}),))
        self.stop(self.process)
        self.process,self.log=self.start(self.lib.path,extra=('--auto-sync',))
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(json.loads(self.request('/api/sync')['body'])['state']['status'],'off')
        self.assertEqual(self.request('/api/sync',{})['status'],400)
        self.stop(self.process)
        self.process,self.log=self.start(self.lib.path,extra=('--no-sync',))
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(self.request('/api/sync',{})['status'],400)
        self.assertEqual(json.loads(self.request('/api/sync')['body'])['state']['status'],'off')
        self.assertIn('sync',json.loads(self.request('/api/settings')['body'])['forced_off'])
        self.stop(self.process)
        self.lib.config['test']={'synthetic':True}
        (self.lib.path/'library.json').write_text(json.dumps(self.lib.config))
        self.process,self.log=self.start(self.lib.path,extra=('--auto-sync',))
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        self.assertEqual(self.request('/api/sync',{})['status'],400)
        self.stop(self.process)
        self.lib.config.pop('test')
        with sqlite3.connect(self.lib.db) as c:c.execute("INSERT OR REPLACE INTO prefs VALUES ('startup',?)",(json.dumps({'sync':True}),))
        (self.lib.path/'library.json').write_text(json.dumps(self.lib.config))
        for i in range(101):self.photo('S'+str(i),'com.atebits.Tweetie2')
        self.process,self.log=self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        # 本地识别数量上限已经去掉（#100）：不论多少新图片，都等本机的 worker 识别。
        self.assertEqual(self.request('/api/startup',{'local_limit':100})['status'],400)
        state=self.sync()
        self.assertEqual(state['state']['status'],'needs_recognition')
        self.assertEqual(state['state']['total'],102)
        self.assertEqual(state['history'][0]['status'],'needs_recognition')

    @unittest.skipIf(os.name == 'nt', 'Apple 照片图库只在 macOS 上提供')
    def test_native_library_new_photos_exact_source_reference(self):
        # 原 Python 照片扫描器在同样输入上的结果已录制（tests/fixed_expectations.py）。
        for u,app in (('D','com.atebits.Tweetie2'),('E','net.pxv.iphone'),('F','com.apple.springboard')): self.photo(u,app)
        for i,app in enumerate(('com.apple.camera','com.apple.mobileslideshow','com.apple.mobilesafari','tv.danmaku.bilianime','ai.x.GrokApp','com.tencent.xin','com.baidu.netdisk','app.nicegram','')):self.photo('N'+str(i),app)
        def pair():
            old=self.root/'reference-source.sqlite'
            with sqlite3.connect(self.lib.source) as src,sqlite3.connect(old) as dest:src.backup(dest)
            result=subprocess.run([str(self.binary),'photos-scan','--db',str(self.lib.source)],input=json.dumps(self.source).encode(),capture_output=True,env=self.env)
            self.assertEqual(result.returncode,0,result.stderr)
            expected=self.rec.value('old scan')
            self.assertEqual(json.loads(result.stdout),expected['result'])
            self.assertEqual(snapshot(self.lib.source),expected['source'])
        pair()
        with sqlite3.connect(self.library/'database/Photos.sqlite') as c:c.execute("UPDATE ZASSET SET ZTRASHEDSTATE=1 WHERE ZUUID='D'")
        pair()
        with sqlite3.connect(self.library/'database/Photos.sqlite') as c:c.execute("UPDATE ZASSET SET ZTRASHEDSTATE=0 WHERE ZUUID='D'")
        original=self.library/'originals/E/E.png';original.parent.mkdir(parents=True);original.write_bytes(b'synthetic')
        pair()

    @unittest.skipUnless(shutil.which('sandbox-exec'), 'macOS/Linux only: the Photos permission is a macOS privacy setting, refused here through sandbox-exec')
    def test_native_library_new_photos_permission_refused_is_named(self):
        # macOS 隐私设置以 EPERM 拒绝访问照片数据库，与沙箱拒绝时一样（#202）。
        from tests.test_read_http import SPAWN
        from tools.verify_read_http import enter
        self.photo('D','com.atebits.Tweetie2')
        self.stop(self.process)
        database=os.path.realpath(self.library/'database/Photos.sqlite')
        profile='(version 1)(allow default)(deny file-read* (literal %s))'%json.dumps(database)
        args=[shutil.which('sandbox-exec'),'-p',profile,str(self.binary),'http','--no-auto-sync','--library',str(self.lib.path),'--port',str(self.port),
              '--root',str(self.resources),'--no-names','--no-index-check']
        log=tempfile.TemporaryFile();self.addCleanup(log.close)
        self.process=subprocess.Popen(args,env=self.env,stdout=log,stderr=subprocess.DEVNULL,**SPAWN)
        self.addCleanup(self.stop,self.process)
        enter(self.origin,log,self.process)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        state=self.sync()['state']
        self.assertEqual((state['status'],state['problem']),('error','photos_permission'),state)
        self.assertIn('Photos.sqlite',state['message'])
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute('SELECT status FROM sync_runs ORDER BY id DESC').fetchone(),('error',))
        # 其他任何失败都不带问题代码：页面按原样显示错误。
        self.stop(self.process)
        self.process,self.log=self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        with sqlite3.connect(self.library/'database/Photos.sqlite') as c:c.execute('DROP TABLE ZASSET')
        state=self.sync()['state']
        self.assertEqual(state['status'],'error',state)
        self.assertNotIn('problem',state)

    def test_native_library_new_unreadable_counts_and_retry(self):
        self.photo('E','net.pxv.iphone')
        self.worker()
        state=self.sync()
        self.assertEqual(state['state']['status'],'done',state)
        self.assertEqual((state['state']['total'],state['state']['done'],state['state']['failed'],state['state']['imported']),(2,2,1,1))
        self.assertIn('暂时无法识别',state['state']['message'])
        original=self.library/'originals/E/E.png';original.parent.mkdir(parents=True);original.write_bytes(b'synthetic')
        state=self.sync()
        self.assertEqual((state['state']['status'],state['state']['failed'],state['state']['imported']),('done',0,1))
        with sqlite3.connect(self.lib.source) as c:self.assertEqual(c.execute("SELECT count(*) FROM recognition_assets WHERE asset='E'").fetchone()[0],1)

    def hold_unreadable(self, path):
        """让一个已有文件读不了，同时元数据保持原样：用 POSIX 权限位，在 Windows 上则用一个不共享任何访问的
        打开句柄。返回解除用的函数。"""
        if os.name != 'nt':
            path.chmod(0)
            release = lambda: path.chmod(0o644)
        else:
            import ctypes
            from ctypes import wintypes
            kernel = ctypes.WinDLL('kernel32', use_last_error=True)
            kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
                                           wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
            kernel.CreateFileW.restype = wintypes.HANDLE
            kernel.CloseHandle.argtypes = [wintypes.HANDLE]
            handle = kernel.CreateFileW(str(path), 0x80000000, 0, None, 3, 0, None)  # GENERIC_READ，不允许共享，OPEN_EXISTING
            if handle in (None, wintypes.HANDLE(-1).value):
                raise ctypes.WinError(ctypes.get_last_error())
            release = lambda: kernel.CloseHandle(handle)
        held = [True]
        def done():
            if held.pop() if held else False:
                release()
        self.addCleanup(done)
        try:
            path.read_bytes()
            done()
            self.skipTest('this account reads files whatever their permissions')
        except OSError:
            pass
        return done

    def test_native_library_new_changed_picture_is_recognised_again_with_its_new_result(self):
        # 同名替换的图片会得到别的向量；同一张图片换一个执行后端也是如此（#270）。旧的识别记录随它的标签一起去掉，
        # 所以导入取新的结果。
        from tests.folder_fixture import image, settle
        folder = self.new_folder
        image(folder / 'changing.png', 'red')
        self.assertEqual(self.sync()['state']['status'], 'needs_recognition')
        self.worker('content')
        def run():
            self.assertEqual(self.request('/api/sync', {})['status'], 200)
            value = self.finish()
            self.assertEqual(value['state']['status'], 'done', value)
            with sqlite3.connect(self.lib.source) as c:
                return c.execute("SELECT e.vec FROM embeddings e JOIN assets a ON a.uuid=e.uuid "
                                 "WHERE a.filename='changing.png' AND e.model='ccip'").fetchone()[0]
        settle(folder)
        before = run()
        image(folder / 'changing.png', 'blue')
        settle(folder)
        after = run()
        self.assertNotEqual(after, before)
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute("SELECT count(*) FROM recognition_assets r JOIN assets a ON a.uuid=r.asset "
                                       "WHERE a.filename='changing.png'").fetchone()[0], 1)
        self.assertEqual(run(), after, 'unchanged: nothing is recognised again')

    def test_native_library_new_damaged_pictures_wait_for_a_change_unreadable_ones_retry(self):
        # #112：读不了的图片每次同步都重试；完整读出来却不是有效图片的，在文件改动之前一直跳过；两者分开计数。
        from tests.folder_fixture import image, settle
        folder = self.new_folder
        image(folder / 'good.png', 'red'); image(folder / 'locked.png', 'blue'); image(folder / 'bad.png', 'green', (41, 30))
        whole = (folder / 'bad.png').read_bytes(); (folder / 'bad.png').write_bytes(whole[:len(whole) // 2])
        self.assertEqual(self.sync()['state']['status'], 'needs_recognition')
        with sqlite3.connect(self.lib.db) as c:
            ids = dict(c.execute("SELECT filename,uuid FROM assets WHERE filename LIKE '%.png'").fetchall())
        self.assertEqual(set(ids), {'not-scanned.png', 'good.png', 'locked.png', 'bad.png'})
        for name in ('good.png', 'bad.png'):
            self.assertEqual(self.request('/api/tag', {'ids': [ids[name]], 'tag': '合成人工'})['status'], 200)
        with sqlite3.connect(self.lib.db) as c:
            tags = c.execute('SELECT uuid,tag FROM asset_tags ORDER BY uuid,tag').fetchall()
        self.worker('classify')
        request = self.root / 'last-request'
        def run():
            request.unlink(missing_ok=True)
            response = self.request('/api/sync', {})
            self.assertEqual(response['status'], 200, response['body'])
            value = self.finish()
            self.assertEqual(value['state']['status'], 'done', value)
            sent = set(json.loads(request.read_text())) if request.exists() else set()
            return value, {name for name, u in ids.items() if u in sent}
        settle(folder)
        release = self.hold_unreadable(folder / 'locked.png')
        value, sent = run()
        state = value['state']
        self.assertEqual(sent, {'not-scanned.png', 'good.png', 'bad.png'})
        self.assertEqual((state['failed'], state['damaged'], state['imported']), (1, 2, 1), state)
        self.assertEqual(state['message'], '1 张暂时无法识别，下次同步重试；'
                                           '2 张图片已损坏或格式不支持，留在图库里但不识别；文件改动后会重新识别')
        self.assertEqual(value['damaged']['count'], 2)
        self.assertEqual(sorted(Path(p).name for p in value['damaged']['paths']), ['bad.png', 'not-scanned.png'])
        self.assertEqual(value['history'][0]['failed'], 1)
        # 再来一次：损坏的图片不送出，读不了的那张再试一次。
        value, sent = run()
        self.assertEqual(sent, set())
        self.assertEqual((value['state']['failed'], value['state']['damaged']), (1, 2), value['state'])
        release()
        value, sent = run()
        self.assertEqual(sent, {'locked.png'})
        self.assertEqual((value['state']['failed'], value['state']['damaged'], value['state']['imported']), (0, 2, 1), value['state'])
        self.assertEqual(value['state']['message'], '2 张图片已损坏或格式不支持，留在图库里但不识别；文件改动后会重新识别')
        # 损坏的文件换成一张有效图片：下一次同步识别它，UUID 不变。
        image(folder / 'bad.png', 'yellow', (51, 31))
        settle(folder)
        value, sent = run()
        self.assertEqual(sent, {'bad.png'})
        self.assertEqual((value['state']['failed'], value['state']['damaged'], value['state']['imported']), (0, 1, 1), value['state'])
        self.assertEqual([Path(p).name for p in value['damaged']['paths']], ['not-scanned.png'])
        (folder / 'not-scanned.png').unlink()
        value, sent = run()
        self.assertEqual(sent, set())
        self.assertNotIn('damaged', value['state']); self.assertNotIn('damaged', value)
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(dict(c.execute("SELECT filename,uuid FROM assets WHERE filename LIKE '%.png'").fetchall()),
                             {k: v for k, v in ids.items() if k != 'not-scanned.png'})
            self.assertEqual(c.execute('SELECT uuid,tag FROM asset_tags ORDER BY uuid,tag').fetchall(), tags)
            self.assertIsNone(c.execute("SELECT value FROM prefs WHERE key='sync_damaged'").fetchone())
        with sqlite3.connect(self.lib.source) as c:
            done = {u for u, in c.execute('SELECT asset FROM recognition_assets')}
        self.assertEqual(done & set(ids.values()), {ids[n] for n in ('good.png', 'locked.png', 'bad.png')})
        self.assertFalse(self.sentinel.exists())

    def test_native_library_new_worker_without_failure_reasons_keeps_retrying(self):
        # #112 之前的 worker 不给原因：所有失败都照旧可以重试。
        from tests.folder_fixture import image
        image(self.new_folder / 'good.png', 'red')
        self.worker('classify-legacy')
        for _ in range(2):
            (self.root / 'last-request').unlink(missing_ok=True)
            value = self.sync()
            self.assertEqual((value['state']['status'], value['state']['failed']), ('done', 1), value)
            self.assertNotIn('damaged', value['state']); self.assertNotIn('damaged', value)
            with sqlite3.connect(self.lib.db) as c:
                text = c.execute("SELECT uuid FROM assets WHERE filename='not-scanned.png'").fetchone()[0]
            self.assertIn(text, json.loads((self.root / 'last-request').read_text()))
            self.assertEqual(value['state']['message'], '1 张暂时无法识别，下次同步重试')

    def test_native_library_new_recognition_import_recovers_from_app_database_busy_in_same_sync(self):
        self.worker('wait-for-import')
        self.assertEqual(self.request('/api/sync', {})['body'], b'{"started": true}')
        wait_until(lambda: (self.root / 'last-request').exists())

        with closing(sqlite3.connect(self.lib.db)) as app, app:
            app.execute('BEGIN IMMEDIATE')
            (self.root / 'release-import').touch()

            def busy_receipt():
                with closing(sqlite3.connect(self.lib.source)) as source:
                    table = source.execute("SELECT count(*) FROM sqlite_master WHERE name='recognition_imports'")
                    if not table.fetchone()[0]:
                        return False
                    return source.execute(
                        "SELECT 1 FROM recognition_imports WHERE status='pending' AND error LIKE '%database is locked%'"
                    ).fetchone()

            # 看到回执以后才释放，确保已经超过一次 SQLite 写入等待上限。
            wait_until(busy_receipt)

        value = self.finish()
        self.assertEqual(value['state']['status'], 'done', value)
        self.assertNotIn('diagnostic', value['state'])
        self.assertEqual(value['state']['import_done'], 1)
        with closing(sqlite3.connect(self.lib.source)) as source:
            self.assertEqual(source.execute("SELECT status,error FROM recognition_imports").fetchall(), [('done', '')])
            self.assertEqual(source.execute("SELECT count(*) FROM recognition_assets").fetchone()[0], 1)
        with closing(sqlite3.connect(self.lib.db)) as app, app:
            self.assertEqual(app.execute("SELECT count(*) FROM meta WHERE key LIKE 'recognition_projection:%'").fetchone()[0], 1)

    def test_native_library_new_batch_commit_then_failure_recovers_once(self):
        self.photo('D','com.atebits.Tweetie2')
        original=self.library/'originals/D/D.png';original.parent.mkdir(parents=True);original.write_bytes(b'synthetic')
        self.worker('splitbad')
        state=self.sync()
        self.assertEqual(state['state']['status'],'error',state)
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM recognition_assets').fetchone()[0],1)
            first=c.execute('SELECT asset FROM recognition_assets').fetchone()[0]
        with sqlite3.connect(self.lib.db) as c:
            before=c.execute('SELECT count(*) FROM term_evidence').fetchone()[0]
            self.assertEqual(c.execute('SELECT rating FROM asset_rating WHERE uuid=?',(first,)).fetchall(),[('general',)])
        script=self.root/'synthetic-worker';script.write_text(script.read_text().replace("mode='splitbad'","mode='ok'"))
        self.assertEqual(self.sync()['state']['status'],'done')
        with sqlite3.connect(self.lib.source) as c:
            self.assertEqual(c.execute('SELECT count(*) FROM recognition_assets').fetchone()[0],2)
            self.assertEqual(c.execute('SELECT count(*) FROM recognition_imports').fetchone()[0],2)
        self.assertEqual(self.sync()['state']['status'],'done')
        with sqlite3.connect(self.lib.source) as c:self.assertEqual(c.execute('SELECT count(*) FROM recognition_imports').fetchone()[0],2)

    def test_native_library_new_multiple_photos_statistics_accumulate(self):
        self.photo('D','com.atebits.Tweetie2')
        self.stop(self.process)
        second=self.root/'second.photoslibrary';(second/'database').mkdir(parents=True)
        with sqlite3.connect(self.library/'database/Photos.sqlite') as src,sqlite3.connect(second/'database/Photos.sqlite') as dest:src.backup(dest)
        with sqlite3.connect(second/'database/Photos.sqlite') as c:c.execute("UPDATE ZASSET SET ZUUID='G'")
        self.lib.config['sources'].append(dict(self.source,id='second-photos',path=str(second)))
        (self.lib.path/'library.json').write_text(json.dumps(self.lib.config))
        self.process,self.log=self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        state=self.sync()
        self.assertEqual(state['history'][0]['sources']['com.atebits.Tweetie2'],{'name':'X','photos':2,'collected':2})
        with sqlite3.connect(self.lib.source) as c:self.assertEqual(c.execute("SELECT uuid,source_id FROM assets WHERE source_id LIKE '%photos' ORDER BY uuid").fetchall(),[('D','photos'),('G','second-photos')])

del NativeFolderHTTPTest
