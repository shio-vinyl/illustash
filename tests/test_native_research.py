"""#52：可选的真实计算程序和受控的失败，只用生成的图库包。

现有 Python 计算程序在同样输入上的结果（相似扫描、聚类、关系、OC 分组）和准备好的包，
已通过 tests/fixed_expectations.py 录制（#9）。
"""
from tests.native_worker_fixture import command as worker_command, source as worker_source, exists as worker_exists

import json
import os
from pathlib import Path
import signal
import sqlite3
import sys
import time
import unittest
from concurrent.futures import ThreadPoolExecutor
import numpy as np
from PIL import Image
from tests import test_native_lifecycle as lifecycle
from tests.fixed_expectations import expected_folds, unclocked
from tests.test_read_http import wait_until
from tools.wait_limit import WAIT

# 不测超时的用例给研究任务配的时限：它只是兜底，按等待上限放宽（#508）；要触发超时的用例另配短时限。
TASK_LIMIT_MS = str(int(WAIT * 1000))


class NativeResearchTest(unittest.TestCase):
    setUpClass = lifecycle.NativeLifecycleTest.__dict__['setUpClass']
    package = lifecycle.NativeLifecycleTest.package
    start = lifecycle.NativeLifecycleTest.start
    stop = lifecycle.NativeLifecycleTest.stop
    request = lifecycle.NativeLifecycleTest.request
    pair = lifecycle.NativeLifecycleTest.pair
    def setUp(self):
        lifecycle.NativeLifecycleTest.setUp(self)
        self.stop(self.process)
        self.repo = Path(__file__).resolve().parents[1]
        self.worker = self.root/'research-worker'
        self.mode = self.root/'mode'
        self.pid = self.root/'worker.pid'
        self.runs = self.root/'worker.runs'
        self.kinds = self.root/'worker.kinds'
        self.gate = self.root/'worker.release'
        self.mode.write_text('ok')
        self.worker_log=self.root/'worker.stderr.log'
        self.worker.write_text('#!'+sys.executable+'\n'+f'''import sys,os,json,time,subprocess,traceback
from pathlib import Path
sys.path.insert(0,{str(self.repo)!r})
# 失败证据（#501）：每次尝试的编号、kind、模式、未捕获的异常和计算程序的 stderr 追加进 worker.stderr.log；
# 原本的 stderr 照旧交给宿主。主体保持顶层语句：别的测试会在这里按原文插入代码。
log=open({str(self.worker_log)!r},'a',encoding='utf-8')
def note(text): log.write(text if text.endswith('\\n') else text+'\\n');log.flush()
def logged(kind,value,trace): note(''.join(traceback.format_exception(kind,value,trace)));sys.__excepthook__(kind,value,trace)
sys.excepthook=logged
r=json.loads(Path(sys.argv[1]).read_text(encoding='utf-8'))
# 父进程看到 pid 或启动记录后就可能改写模式，先固定本次运行的模式。
mode=Path({str(self.mode)!r}).read_text()
# 先关闭临时文件再发布，读到的一定是完整 pid（#132）。
Path({str(self.pid)!r}+'.tmp').write_text(str(os.getpid()));os.replace({str(self.pid)!r}+'.tmp',{str(self.pid)!r})
with open({str(self.runs)!r},'a') as f: f.write(r['version']+'\\n')
with open({str(self.kinds)!r},'a') as f: f.write(r['kind']+'\\n')
note('--- 第 '+str(len(Path({str(self.runs)!r}).read_text().split()))+' 次 worker kind='+r['kind']+' mode='+mode+' pid='+str(os.getpid()))
if mode=='hang':
    subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'])
    time.sleep(60)
if mode=='crash': note('按模式退出，退出码 7');sys.exit(7)
if mode=='held':
    # 每次运行取走一次放行文件，测试在放行前改动输入（#263）。
    while True:
        try: os.remove({str(self.gate)!r}); break
        except FileNotFoundError: time.sleep(.02)
    note('已取走放行文件')
compute=subprocess.run([os.environ['ILLUSTASH_RESEARCH_COMPUTE_BIN'],sys.argv[1],sys.argv[2]],stderr=subprocess.PIPE)
if compute.stderr:
    sys.stderr.buffer.write(compute.stderr);sys.stderr.flush()
    note('计算程序 stderr：'+compute.stderr.decode('utf-8','replace'))
compute.check_returncode()
v=json.loads(Path(sys.argv[2]).read_text(encoding='utf-8'))
if mode=='wrong': v['identity']['generation']+=1
if mode=='bad': v['tables']['relations']=[['a','foreign','identical',0,'','2026-10-01']]
if mode=='invalid': v['tables']['cluster_info']=[['invalid']]
Path(sys.argv[2]).write_text(json.dumps(v,allow_nan=False))
note('正常结束')
''')
        self.worker.chmod(0o755)
        self.env.update(ILLUSTASH_RESEARCH_WORKER=worker_command(self.worker),ILLUSTASH_RESEARCH_TIMEOUT_MS=TASK_LIMIT_MS,ILLUSTASH_RESEARCH_RETRY_MS='300')
        with sqlite3.connect(self.lib.source) as c:
            c.execute('CREATE TABLE IF NOT EXISTS embeddings(uuid TEXT,model TEXT,vec BLOB,PRIMARY KEY(uuid,model))')
            v=np.zeros(768,np.float32);v[0]=1
            for u in 'abc': c.execute('INSERT OR REPLACE INTO embeddings VALUES (?,?,?)',(u,'ccip',v.tobytes()))
            for i in range(9):
                u=('seed'+str(i)) if i<6 else 'unmatched'+str(i)
                v=np.zeros(768,np.float32);v[0 if i<6 else 1]=1
                c.execute('INSERT INTO assets VALUES (?,?,?,?,?,?,?,?,?)',(u,u+'.png',str(self.lib.path/(u+'.png')),120,150,'2026-10-01','orig',1,0))
                c.execute('INSERT OR REPLACE INTO embeddings VALUES (?,?,?)',(u,'ccip',v.tobytes()))
        self.lib=self.rec.package('prepared',self.lib.path)
        with sqlite3.connect(self.lib.db) as c:
            self.fold_assets = c.execute('SELECT uuid,width,height FROM assets').fetchall()
            self.fold_assertions = c.execute('SELECT uuid,entity,status FROM assertions').fetchall()
        self.alice=self.rec.value('alice')
        pixels=np.random.default_rng(1).integers(0,255,(150,120,3),dtype=np.uint8)
        for u in list('abc')+[('seed'+str(i)) if i<6 else 'unmatched'+str(i) for i in range(9)]: Image.fromarray(pixels).save(self.lib.path/(u+'.png'))
        self.restart()
        self.finished()

    def restart(self):
        self.process,self.log=self.start(self.lib.path)
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']

    def state(self): return json.loads(self.request('/api/similar')['body'])['state']
    def wait(self,predicate,timeout=WAIT,what='等待超时'): return wait_for(self,predicate,timeout,what)
    def why(self,*context): return why(self,*context)
    def finished(self, status='idle'):
        # 直接调用模块级函数：别的测试类借用 setUp 和 finished 时不必再借这些辅助方法。
        wait_for(self,lambda:self.state()['status'] in ('idle','error'),what='研究任务没有结束')
        self.assertEqual(self.state()['status'],status,why(self))
    def rows(self, tables):
        with sqlite3.connect(self.lib.db) as c:
            return {t:c.execute('SELECT * FROM '+t+' ORDER BY 1').fetchall() for t in tables}
    def pending(self):
        with sqlite3.connect(self.lib.db) as c: return c.execute("SELECT count(*) FROM backend_effects WHERE kind IN ('verdict','imported','index_changed') AND status='pending'").fetchone()[0]
    def versions(self): return self.runs.read_text().split() if self.runs.exists() else []
    def run_kinds(self): return self.kinds.read_text().split() if self.kinds.exists() else []
    def clustered(self):
        with sqlite3.connect(self.lib.db) as c: return {u for (u,) in c.execute('SELECT uuid FROM clusters')}
    def release(self):
        # Windows 上 worker 一看到最终文件名就会删除它，此时写句柄必须已经关闭。
        staged = self.gate.with_suffix('.tmp')
        staged.write_bytes(b'')
        staged.replace(self.gate)
    def retry_visibly(self):
        # 重试只在退避期间显示 `waiting`；退避拉长一些，机器繁忙时轮询也能看到它（#263）。
        self.stop(self.process);self.env.update(ILLUSTASH_RESEARCH_RETRY_MS='3000',ILLUSTASH_RESEARCH_TIMEOUT_MS='60000')
        self.restart();self.finished()
    def poke_event(self):
        with sqlite3.connect(self.lib.db) as c:c.execute("INSERT INTO backend_effects(kind) VALUES ('imported')")
    def regroup_with_all_copies(self):
        """保留合成样本里的每张图，让聚类回归使用和历史录制一致的输入。"""
        result = self.request('/api/fold', {'kinds': []})
        self.assertEqual((result['status'], result['body']), (200, b'{"kinds": []}'))
        self.assertEqual(self.rows(['duplicates'])['duplicates'], [])
        self.assertEqual(self.request('/api/regroup', {})['body'], b'{"started": true}')
        self.finished()

    def test_real_similar_relate_cluster_and_oc_routes(self):
        # 现有的真实算法跑同样的生成输入，除挂钟时间字段外，表内容完全一致。
        tables=['relations','image_sigs','similar_paired','similar_retry','similar_groups','duplicates']
        native=self.rows(tables)
        self.assertGreater(len(self.rows(['prototypes'])['prototypes']),0)
        # 挂钟时间列（关系是何时算出的）按 <clock> 比较。
        masked=lambda rows:{t:[tuple(unclocked(v) for v in r) for r in rs] for t,rs in rows.items()}
        expected = self.rec.value('scanned')
        duplicates = expected['duplicates']
        projection = {
            uuid: (rep, hidden)
            for uuid, rep, hidden in expected_folds(
                self.fold_assets, self.fold_assertions, ((r[0], r[1], r[3]) for r in duplicates))
        }
        expected['duplicates'] = [(*r[:5], *projection[r[0]]) for r in duplicates]
        self.assertEqual(masked(native), masked(expected))
        # 历史录制的学习包含整个副本组。用相同输入比较聚类和 OC。
        self.regroup_with_all_copies()
        clusters=self.rows(['clusters','cluster_info'])
        self.assertGreater(len(clusters['clusters']),0)
        expected=self.rec.value('clustered')
        self.assertEqual(clusters['clusters'],expected['clusters'])
        self.assertEqual([r[:4]+r[5:] for r in clusters['cluster_info']], [r[:4]+r[5:] for r in expected['cluster_info']])
        expected=self.rec.value('related')
        actual=self.request('/api/relate',{'ids':['a','b','a']})
        self.assertEqual(actual['status'],200,actual)
        self.assertEqual(actual['body'],expected.encode())
        self.assertEqual(self.request('/api/relate',{'ids':['a']})['status'],400)
        entity=self.alice
        expected=self.rec.value('oc groups')
        actual=self.request('/api/oc/groups?entity='+entity)
        self.assertEqual(actual['status'],200,actual)
        self.assertEqual(actual['body'],expected.encode())
        self.assertEqual(self.request('/api/regroup',{})['body'],b'{"started": true}')
        self.finished()
        for url in ['/api/identity/queue','/api/assets/count','/api/identity/count']:
            self.pair(url)
        self.assertFalse(self.sentinel.exists())

    def test_wrong_malformed_crash_timeout_results_do_not_publish_and_retry(self):
        tables=['relations','image_sigs','clusters','cluster_info','actions','assertions']
        before=self.rows(tables)
        for mode in ['wrong','bad','crash','hang']:
            self.mode.write_text(mode)
            if mode=='hang':self.env['ILLUSTASH_RESEARCH_TIMEOUT_MS']='200'
            self.stop(self.process);self.restart();self.finished('error')
            self.assertEqual(before,self.rows(tables))
            with sqlite3.connect(self.lib.db) as c:
                self.assertGreater(c.execute("SELECT count(*) FROM research_runs WHERE status='pending'").fetchone()[0],0)
            self.assertEqual(self.request('/api/assets/count')['status'],200)
        self.mode.write_text('ok');self.env['ILLUSTASH_RESEARCH_TIMEOUT_MS']=TASK_LIMIT_MS
        self.stop(self.process);self.restart();self.finished()
        self.assertEqual(self.pending(),0)

    def test_inflight_human_change_cancel_switch_exit_no_process_group(self):
        self.mode.write_text('held')
        self.pid.unlink(missing_ok=True)
        self.request('/api/regroup',{})
        self.wait(lambda:self.pid.exists() and self.state()['status']=='learning')
        self.request('/api/tag',{'ids':['a'],'tag':'合成'})
        self.mode.write_text('ok');self.release()
        self.finished()
        self.mode.write_text('hang')
        self.pid.unlink(missing_ok=True)
        self.request('/api/similar',{})
        self.wait(self.pid.exists)
        pid=int(self.pid.read_text())
        with sqlite3.connect(self.lib.db) as c: pending=c.execute('SELECT count(*) FROM research_runs').fetchone()[0]
        for _ in range(3):self.assertEqual(self.request('/api/similar',{})['body'],b'{"started": true}')
        with sqlite3.connect(self.lib.db) as c:self.assertEqual(c.execute('SELECT count(*) FROM research_runs').fetchone()[0],pending)
        self.assertEqual(self.request('/api/similar',{'action':'cancel'})['status'],200)
        self.wait(lambda:not group_exists(pid))
        self.assertEqual(self.state()['status'],'idle')
        self.request('/api/similar',{})
        self.wait(lambda:self.pid.exists() and int(self.pid.read_text())!=pid)
        pid=int(self.pid.read_text())
        replacement = self.package('same-id')
        config=replacement.config;config['id']=self.lib.config['id'];(replacement.path/'library.json').write_text(json.dumps(config))
        self.assertEqual(self.request('/api/libraries/open',{'path':str(replacement.path)})['status'],200)
        self.wait(lambda:not group_exists(pid))
        self.token=json.loads(self.request('/api/bootstrap')['body'])['token']
        with sqlite3.connect(replacement.db) as c:self.assertEqual(c.execute("SELECT count(*) FROM research_runs WHERE id='startup'").fetchone()[0],1)
        self.request('/api/similar',{'action':'cancel'})
        self.pid.unlink(missing_ok=True)
        self.request('/api/similar',{})
        self.wait(self.pid.exists)
        pid=int(self.pid.read_text());self.stop(self.process)
        self.wait(lambda:not group_exists(pid))

    def test_inflight_input_change_retries_on_fresh_snapshot(self):
        # #194：worker 计算期间来了一条人工裁决；过期的结果被拒绝、绝不发布，任务在更新的快照上自行完成。
        self.retry_visibly()
        self.regroup_with_all_copies()
        self.assertIn('unmatched6',self.clustered())
        before=self.rows(['clusters','cluster_info','relations','prototypes'])
        runs=len(self.versions())
        self.mode.write_text('held')
        self.request('/api/regroup',{})
        self.wait(lambda:len(self.versions())>runs and self.state()['status']=='learning')
        self.assertEqual(self.request('/api/change',{'ids':['unmatched6'],'action':'confirmed','entity':self.alice})['status'],200)
        self.mode.write_text('ok');self.release()
        seen=[]
        def settled():
            s=self.state()
            if s['status']=='waiting':
                self.assertEqual(self.rows(list(before)),before)
                self.assertIn('自动重试',s['message'])
            seen.append(s['status'])
            return s['status'] in ('idle','error')
        self.wait(settled)
        # 按结束等待时的状态判断：下一次恢复轮询（每秒一次）可能已经为这条裁决启动排队中的 events 运行，
        # 而 regroup 不确认这些事件。
        self.assertEqual(seen[-1],'idle',self.why(seen))
        self.assertIn('waiting',seen,self.why(seen))
        versions=self.versions()[runs:]
        self.assertGreaterEqual(len(versions),2)
        self.assertNotEqual(versions[0],versions[-1])
        # 发布的分组是现有计算程序在裁决后图库上的结果。
        native=self.rows(['clusters'])
        self.assertNotIn('unmatched6',self.clustered())
        self.assertEqual(native,self.rec.value('clustered'))
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(c.execute("SELECT count(*) FROM research_runs WHERE status='pending'").fetchone()[0],0)

    def inflight(self, start, write):
        """用被卡住的 worker 开始一次运行，放行前提交 `write`；返回观察到的状态。"""
        runs=len(self.versions())
        self.mode.write_text('held')
        result=start()
        self.wait(lambda:len(self.versions())>runs)
        with sqlite3.connect(self.lib.db) as c: write(c)
        self.mode.write_text('ok');self.release()
        seen=[]
        def settled():
            seen.append(self.state()['status'])
            return seen[-1] in ('idle','error')
        # 放行后可能还要退避再重跑；繁忙机器上 20 秒不够（#418）。上限高于 `retry_visibly` 的任务时限，卡住的运行仍会报 `error`。
        try:wait_until(settled,timeout=60+WAIT)
        except AssertionError:self.fail(self.why('运行没有结束',seen[-20:]))
        return result,seen,len(self.versions())-runs

    def test_discriminative_claims_publish_within_core_score_range(self):
        # #156：包里的外部索引候选会打开判别式认领，它们也会认领 Alice 已确认的种子。种子离自己最近的已确认图片
        # 就是它自己；f32 舍入可能让这个距离略小于 0，而只要有一个得分超过 1，Core 的发布检查就会拒绝整批。
        self.stop(self.process)
        rng=np.random.default_rng(156)
        with sqlite3.connect(self.lib.source) as c:
            for i in range(6):
                v=rng.normal(0,.03,768).astype(np.float32);v[0]=1;v/=np.linalg.norm(v)
                c.execute("UPDATE embeddings SET vec=? WHERE uuid=? AND model='ccip'",(v.tobytes(),'seed'+str(i)))
        self.restart()
        self.poke_event()
        self.wait(lambda:self.pending()==0)
        self.finished()
        claims=self.rows(['proto_claims'])['proto_claims']
        seeds={u:s for u,e,s in claims if e==self.alice and u.startswith('seed')}
        self.assertEqual(sorted(seeds),['seed'+str(i) for i in range(6)],claims)
        self.assertTrue(all(0<=s<=1 for _,_,s in claims),claims)
        # 只有判别式认领会按种子到它自己的距离给它打分。
        self.assertIn(1.0,seeds.values(),claims)

    def test_unread_tables_written_inflight_do_not_invalidate_learning(self):
        # #29：偏好设置、名称缓存/设置和同步历史都不是研究的输入。
        self.regroup_with_all_copies()
        before=self.rows(['clusters'])
        self.assertTrue(before['clusters'])
        def write(c):
            c.execute("INSERT OR REPLACE INTO prefs VALUES ('synthetic_view','grid')")
            c.execute("INSERT OR REPLACE INTO name_cache VALUES ('synthetic','{}')")
            c.execute("INSERT OR REPLACE INTO name_settings VALUES ('synthetic_setting','42')")
            c.execute("INSERT INTO sync_runs(status) VALUES ('done')")
        _,seen,runs=self.inflight(lambda:self.request('/api/regroup',{}),write)
        self.assertEqual(self.state()['status'],'idle',self.why(seen))
        self.assertNotIn('waiting',seen,self.why(seen))
        self.assertEqual(runs,1,self.why(seen))
        self.assertEqual(self.rows(['clusters']),before)

    def test_similar_ignores_human_verdict_and_retries_on_its_own_inputs(self):
        # #29：裁决只改变学习的输入；排除改变 similar 扫描的范围。
        def verdict(c):
            response=self.request('/api/change',{'ids':['unmatched6'],'action':'confirmed','entity':self.alice})
            self.assertEqual(response['status'],200,response)
        # 两段都用较长的时限和退避：裁决排出的学习在繁忙机器上可能超过夹具默认的 10 秒（#418），
        # 退避拉长也让不该出现的重试更容易被看到。
        self.retry_visibly()
        before=len(self.run_kinds())
        _,seen,_=self.inflight(lambda:self.request('/api/similar',{}),verdict)
        self.assertEqual(seen[-1],'idle',self.why(seen))
        self.assertNotIn('waiting',seen,self.why(seen))
        # similar 结束后，裁决会把学习排进队列（一次 `events` 运行，#132）。`inflight` 看到第一次 `idle` 就返回，
        # 学习可能紧接着开始，所以先等它跑完：状态回到 `idle`，裁决事件也已确认（#418）。
        drain=[]
        def drained():
            drain.append(self.state()['status'])
            return drain[-1]=='error' or drain[-1]=='idle' and self.pending()==0
        try:wait_until(drained,timeout=60+WAIT)
        except AssertionError:self.fail(self.why('裁决排出的学习没有跑完',seen,drain,self.pending()))
        kinds=self.run_kinds()[before:]
        self.assertEqual(self.state()['status'],'idle',self.why(seen,drain,kinds))
        self.assertEqual(self.pending(),0,self.why(seen,drain,kinds))
        # 只有第二次 similar 运行才算重试。
        self.assertEqual(kinds.count('similar'),1,self.why(seen,drain,kinds))
        # 直接写 `excluded` 不产生裁决事件，similar 的重试在同一个任务里完成，结束后没有排队的学习。
        before=len(self.run_kinds())
        _,seen,runs=self.inflight(lambda:self.request('/api/similar',{}),lambda c:c.execute("INSERT INTO excluded VALUES ('b','synthetic','test')"))
        self.assertEqual(self.state()['status'],'idle',self.why(seen))
        self.assertIn('waiting',seen,self.why(seen))
        self.assertGreaterEqual(runs,2,self.why(seen))

    def test_oc_query_answers_despite_unread_write(self):
        # #29：OC 只读归属、标签、排除和向量；它从不重试。
        expected=self.request('/api/oc/groups?entity='+self.alice)
        self.assertEqual(expected['status'],200,expected)
        with ThreadPoolExecutor(1) as pool:
            answer,_,runs=self.inflight(lambda:pool.submit(self.request,'/api/oc/groups?entity='+self.alice),
                lambda c:c.execute("INSERT OR REPLACE INTO prefs VALUES ('synthetic_view','list')"))
            actual=answer.result(timeout=WAIT)
        self.assertEqual(actual['status'],200,actual)
        self.assertEqual(actual['body'],expected['body'])
        self.assertEqual(runs,1)

    def test_continuous_input_change_retries_are_bounded_then_manual_retry(self):
        # 六次慢速运行加上退避，会超过夹具默认的任务时限。
        self.stop(self.process);self.env['ILLUSTASH_RESEARCH_TIMEOUT_MS']='60000';self.restart();self.finished()
        before=self.rows(['clusters','cluster_info','relations','prototypes'])
        runs=len(self.versions())
        self.mode.write_text('held')
        self.request('/api/regroup',{})
        # 每次尝试被卡住时都会遇到一次写入：第一次运行和全部五次重试。
        for i in range(1,7):
            self.wait(lambda:len(self.versions())-runs>=i)
            self.assertEqual(self.request('/api/tag',{'ids':['a'],'tag':f'合成持续{i}'})['status'],200)
            self.release()
        self.wait(lambda:self.state()['status']=='error')
        state=self.state()
        self.assertIn('自动重试 5 次',state['message'],self.why())
        self.assertEqual(len(self.versions())-runs,6,self.why())
        self.assertEqual(self.rows(list(before)),before)
        with sqlite3.connect(self.lib.db) as c:
            self.assertIn('持续变化',c.execute("SELECT error FROM research_runs WHERE status='pending'").fetchone()[0])
        # 重试次数用完后等待显式启动，不在恢复时循环。
        time.sleep(3)
        self.assertEqual(self.state(),state)
        self.assertEqual(len(self.versions())-runs,6)
        self.mode.write_text('ok')
        self.assertEqual(self.request('/api/regroup',{})['body'],b'{"started": true}')
        def drained():
            with sqlite3.connect(self.lib.db) as c:
                return self.state()['status']=='idle' and c.execute("SELECT count(*) FROM research_runs WHERE status='pending'").fetchone()[0]==0
        self.wait(drained)
        self.assertEqual(self.pending(),0)

    def test_ack_failure_replay_is_atomic_and_restart_recovers(self):
        tables=['relations','prototypes','proto_claims','assertions','actions']
        before=self.rows(tables)
        with sqlite3.connect(self.lib.db) as c:
            c.execute("CREATE TRIGGER fail_ack BEFORE UPDATE ON backend_effects WHEN NEW.status='done' BEGIN SELECT RAISE(ABORT,'synthetic ack'); END")
        with sqlite3.connect(self.lib.db) as c:c.execute("INSERT INTO backend_effects(kind) VALUES ('index_changed')")
        # 恢复可能在这里发出任何请求之前就启动运行（#263）：判断结果，而不是判断它有没有启动。
        self.wait(lambda:self.state()['status']=='error')
        self.assertIn('synthetic ack',self.state()['message'])
        self.assertGreater(self.pending(),0)
        # 被拒绝的确认让整个发布回滚了。
        self.assertEqual(before,self.rows(tables))
        with sqlite3.connect(self.lib.db) as c:c.execute('DROP TRIGGER fail_ack')
        self.stop(self.process);self.restart();self.finished()
        self.assertEqual(self.pending(),0)
        self.assertEqual(before,self.rows(tables))

    def test_recognition_dispatch_persists_intent_and_duplicate_does_not_repeat(self):
        import hashlib
        from tests import test_native_recognition as recognition
        self.stop(self.process)
        self.resources=self.root/'worker-resources'
        (self.resources/'models/ccip').mkdir(parents=True)
        (self.resources/'models/wd_swinv2_v3_emb.onnx').write_bytes(b'synthetic WD')
        (self.resources/'models/ccip/model_feat.onnx').write_bytes(b'synthetic CCIP')
        (self.resources/'tags').mkdir()
        (self.resources/'tags/tags_v3.csv').write_text('name,category\ngeneral,9\nblue_hair,0\nalice,4\n')
        self.specs={'wd':dict(version='swinv2_448',dimension=1024,sha256=hashlib.sha256(b'synthetic WD').hexdigest(),tags_sha256=hashlib.sha256((self.resources/'tags/tags_v3.csv').read_bytes()).hexdigest()),'ccip':dict(version='ccip-model_feat',dimension=768,sha256=hashlib.sha256(b'synthetic CCIP').hexdigest())}
        with sqlite3.connect(self.lib.source) as c:
            for column in ('source TEXT DEFAULT "legacy"','model_ver TEXT','run_id TEXT'):c.execute('ALTER TABLE labels ADD COLUMN '+column)
        self.manifest=lambda p,ids=('a','b'):recognition.NativeRecognitionTest.manifest(self,p,ids)
        package=recognition.NativeRecognitionTest.incoming(self)
        self.restart();self.finished()
        history=self.rows(['assertions','actions'])
        with sqlite3.connect(self.lib.db) as c:before=c.execute('SELECT count(*) FROM research_runs').fetchone()[0]
        self.assertEqual(lifecycle.import_task(self,'/api/recognition/import',{'path':str(package)})['status'],200)
        def completed():
            with sqlite3.connect(self.lib.db) as c:return c.execute("SELECT count(*) FROM research_runs WHERE status='done'").fetchone()[0]>before
        self.wait(completed)
        self.assertEqual(history,self.rows(['assertions','actions']))
        with sqlite3.connect(self.lib.db) as c:count=c.execute('SELECT count(*) FROM research_runs').fetchone()[0]
        # 确认失败时重新分派同一条已存的事件；意图键必须保持不变。
        with sqlite3.connect(self.lib.db) as c:
            c.execute("INSERT INTO backend_effects(kind) VALUES ('imported')")
            c.execute("CREATE TRIGGER fail_dispatch_ack BEFORE UPDATE ON backend_effects WHEN NEW.status='done' BEGIN SELECT RAISE(ABORT,'synthetic'); END")
        self.mode.write_text('crash')
        self.stop(self.process);self.restart();self.finished('error')
        with sqlite3.connect(self.lib.db) as c:first=c.execute('SELECT count(*) FROM research_runs').fetchone()[0]
        self.stop(self.process);self.restart();self.finished('error')
        with sqlite3.connect(self.lib.db) as c:
            self.assertEqual(first,c.execute('SELECT count(*) FROM research_runs').fetchone()[0])
            c.execute('DROP TRIGGER fail_dispatch_ack')
        self.mode.write_text('ok');self.stop(self.process);self.restart();self.finished()
        with sqlite3.connect(self.lib.db) as c:count=c.execute('SELECT count(*) FROM research_runs').fetchone()[0]
        self.assertEqual(lifecycle.import_task(self,'/api/recognition/import',{'path':str(package)})['status'],200)
        with sqlite3.connect(self.lib.db) as c:self.assertEqual(c.execute('SELECT count(*) FROM research_runs').fetchone()[0],count)

    def test_worker_missing_dependency_and_frozen_package_preserve_basics(self):
        self.stop(self.process);self.env['ILLUSTASH_RESEARCH_WORKER']=str(self.root/'missing')
        self.restart();self.finished('error')
        self.assertEqual(self.request('/api/assets/count')['status'],200)
        self.assertEqual(self.request('/api/tag',{'ids':['a'],'tag':'合成缺依赖'})['status'],200)
        self.stop(self.process)
        self.env['ILLUSTASH_RESEARCH_WORKER']=worker_command(self.worker)
        config=self.lib.config;config['frozen']=True;(self.lib.path/'library.json').write_text(json.dumps(config))
        before=self.rows(['assertions','actions','relations','prototypes'])
        self.restart()
        self.assertEqual(self.request('/api/regroup',{})['status'],400)
        self.assertEqual(self.request('/api/assets/count')['status'],200)
        self.assertEqual(before,self.rows(['assertions','actions','relations','prototypes']))

    def test_research_and_names_concurrent_complete_without_interference(self):
        # #752：相似图片/学习与角色名称同时运行时互不打断；合并残留链接不导致名称失败；正常取消不写 invalid_input 诊断。
        import subprocess
        import tempfile
        from tests import test_native_names as names_mod
        from tests.test_read_http import SPAWN, wait_human_backup
        from tools.verify_read_http import enter

        self.stop(self.process)
        service = names_mod.TextService()
        self.addCleanup(service.close)
        self.env.update(
            ILLUSTASH_COPYRIGHT_BASE=service.base,
            ILLUSTASH_BANGUMI_BASE=service.base,
            ILLUSTASH_RESEARCH_RETRY_MS='200',
            ILLUSTASH_RESEARCH_TIMEOUT_MS=TASK_LIMIT_MS,
        )

        def start_host():
            log = tempfile.TemporaryFile()
            self.addCleanup(log.close)
            args = [
                str(self.binary), 'http', '--no-auto-sync', '--names-network',
                '--library', str(self.lib.path), '--port', str(self.port),
                '--root', str(self.resources), '--no-index-check',
            ]
            proc = subprocess.Popen(args, env=self.env, stdout=log, stderr=log, **SPAWN)
            self.addCleanup(self.stop, proc)
            enter(self.origin, log, proc)
            wait_human_backup(self, self.origin)
            return proc, log

        def names_state():
            return json.loads(self.request('/api/names?progress=1')['body'])['state']

        def wait_names(target='done'):
            wait_until(lambda: names_state()['status'] in ('done', 'error'))
            self.assertEqual(names_state()['status'], target, names_state())

        # 先准备一个已合并实体（保留旧 danbooru 链接）和一个共享同 tag 的活跃 wd 实体，验证名称任务不会因此报 invalid_input。
        with sqlite3.connect(self.lib.db) as c:
            c.execute("INSERT OR REPLACE INTO meta(key,value) VALUES ('terms_writer','1')")
            c.execute("INSERT INTO terms(key,kind,origin) VALUES ('merged_old','character','import'),('live_wd','character','import')")
            c.execute(
                "INSERT INTO term_tree(term_id,parent_id,merged_into,work_text,variant,artist,label_only,mascot,original,seq) "
                "SELECT id,NULL,'live_wd','Example','','',0,0,0,9001 FROM terms WHERE key='merged_old'"
            )
            c.execute(
                "INSERT INTO term_tree(term_id,parent_id,merged_into,work_text,variant,artist,label_only,mascot,original,seq) "
                "SELECT id,NULL,NULL,'Example','','',0,0,0,9002 FROM terms WHERE key='live_wd'"
            )
            c.execute(
                "INSERT INTO term_links(system,key,term_id,current,source,added_at) "
                "SELECT 'danbooru','shared_tag',id,1,'import','2026-11-19' FROM terms WHERE key='merged_old'"
            )
            c.execute(
                "INSERT INTO term_links(system,key,term_id,current,source,added_at) "
                "SELECT 'wd_character','shared_tag',id,1,'import','2026-11-19' FROM terms WHERE key='live_wd'"
            )
            c.execute(
                "INSERT INTO memberships(uuid,entity,status,origin,sources,source_count,ccip_only,skey) "
                "VALUES ('a','live_wd','auto','wd','WD',1,0,'')"
            )
            c.execute("DELETE FROM meta WHERE key='terms_writer'")

        self.process, self.log = start_host()
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.finished()
        wait_until(lambda: names_state()['status'] in ('idle', 'done', 'error'))
        self.assertNotEqual(names_state()['status'], 'error', names_state())

        # 场景 1：events（含 similar）运行中并发执行全库角色名称刷新，双方一次完成且研究不判定输入变化。
        runs = len(self.versions())
        self.mode.write_text('held')
        self.poke_event()
        self.wait(lambda: len(self.versions()) > runs)
        self.assertEqual(self.request('/api/names', {'action': 'refresh'})['status'], 200)
        wait_names('done')
        self.mode.write_text('ok')
        self.release()
        seen = []

        def settled():
            s = self.state()
            seen.append(s['status'])
            return s['status'] in ('idle', 'error')

        self.wait(settled)
        self.assertEqual(seen[-1], 'idle', self.why(seen))
        self.assertNotIn('waiting', seen, self.why(seen))
        self.assertEqual(len(self.versions()) - runs, 1, self.why(seen))
        self.assertEqual(self.pending(), 0, self.why(seen))

        # 场景 2：名称在线查询暂停时发布显式相似扫描，发布不能使该名称查询的来源失效。
        runs = len(self.versions())
        service.block = True
        service.entered.clear()
        service.release.clear()
        try:
            self.assertEqual(self.request('/api/names', {'action': 'refresh'})['status'], 200)
            self.assertTrue(service.entered.wait(WAIT))
            self.mode.write_text('held')
            self.assertEqual(self.request('/api/similar', {})['status'], 200)
            self.wait(lambda: len(self.versions()) > runs)
            self.mode.write_text('ok')
            self.release()
            self.finished()
            self.assertEqual(len(self.versions()) - runs, 1, self.why())
            self.assertEqual(names_state()['status'], 'running', names_state())
        finally:
            service.block = False
            service.release.set()
        wait_names('done')

        # 场景 3：研究与名称同时运行中正常关闭宿主，重启后诊断里不应出现因取消记下的 invalid_input 或 conflict。
        from tools.verify_read_http import output
        self.env['ILLUSTASH_TEST_TRACE_NAMES_CANCEL'] = '1'
        self.stop(self.process)
        self.process, self.log = start_host()
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.finished()
        wait_until(lambda: names_state()['status'] in ('idle', 'done'))

        self.mode.write_text('hang')
        self.pid.unlink(missing_ok=True)
        self.assertEqual(self.request('/api/similar', {})['status'], 200)
        self.wait(self.pid.exists)
        service.entered.clear()
        service.release.clear()
        service.block = True
        self.assertEqual(self.request('/api/names', {'action': 'refresh'})['status'], 200)
        self.assertTrue(service.entered.wait(WAIT))
        with ThreadPoolExecutor(1) as pool:
            stopping = pool.submit(self.stop, self.process)
            try:
                wait_until(lambda: b'ILLUSTASH_TEST_NAMES_CANCELLED' in output(self.log))
            finally:
                service.release.set()
            stopping.result(timeout=WAIT)
        service.block = False
        self.mode.write_text('ok')

        self.process, self.log = start_host()
        self.token = json.loads(self.request('/api/bootstrap')['body'])['token']
        self.finished()
        wait_names('done')
        records = json.loads(self.request('/api/diagnostics')['body'])['records']
        bad = [
            r for r in records
            if r.get('operation') in ('research', 'names') and r.get('cause') in ('conflict', 'invalid_input')
        ]
        self.assertEqual(bad, [], records)


def lines(path):
    return path.read_text().split() if path.exists() else []


def evidence(test):
    """失败证据（#501）：worker 启动过几次、各次的 kind、最后状态，以及合成 worker 的 stderr 结尾。
    只读 setUp 建好的文件和宿主状态，借用 setUp 的其他测试类同样可用。"""
    try: state=test.state()
    except Exception as error: state=f'读取失败：{error!r}'
    kinds=lines(test.kinds)
    parts=[f'worker 已启动 {len(kinds)} 次，依次为 {kinds}',f'最后状态：{state}']
    # 第二个是 Windows 启动器追加写的 stderr（tests/native_worker_fixture.py），收得到 Python 启动前的失败。
    for path in (test.worker_log,test.worker.with_name(test.worker.name+'.stderr')):
        if path.exists():
            text=path.read_text(encoding='utf-8',errors='replace')
            parts+=[f'--- {path.name}（末尾 {min(len(text),6000)} 字符）---',text[-6000:].rstrip()]
    return '\n'.join(parts)


def why(test,*context):
    """断言说明：给出的上下文加上失败证据；只在断言失败、生成报告时才去读状态和文件。"""
    class Why:
        def __str__(self): return '\n'.join([*(repr(item) for item in context),evidence(test)])
    return Why()


def wait_for(test,predicate,timeout=WAIT,what='等待超时'):
    """同 wait_until，时限不变；超时报告里附上失败证据。"""
    try: return wait_until(predicate,timeout=timeout)
    except AssertionError: test.fail(f'{what}（{timeout:g} 秒）\n{evidence(test)}')


def group_exists(pid):
    return worker_exists(pid)
