"""角色资料包 0.5 的发布前核对：用发布代码导入、启用、重扫，并量重扫耗时（用户 2026-11-18 定）。

不碰真实图库：HOME、App Support、共享目录都在 --work 下（`tools/dev_preview.isolated_env`），不联网
（ILLUSTASH_UPDATE_BASE 指向本机一个关着的端口）。目录草稿还没签名，这里用测试密钥（`tests/fixtures/pack_catalog`
的 key A，只在 ILLUSTASH_UPDATE_BASE 指向本机时才认）给同一份目录字节签名，打成离线包导入；正式签名另由
`pack_catalog.py verify` 核对。图片是逐张不同颜色的 8×8 PNG，识别向量取二期验收集（公开 Danbooru 帖子）的
WD 向量，图多于验收集时循环使用。不写 CCIP 向量：CCIP 长尾要先装基础角色索引，隔离环境里没有；0.5 的 CCIP 部分
和 0.4 是同一个文件，从 0.4 换到 0.5 时不重扫它，所以这里 CCIP 部分只记下启用了哪个文件，耗时只含 WD 部分。

    python3 pack05_check.py prepare --checkout <5ef3e710 检出> --work <目录> --images 2000 [--pick]
    python3 pack05_check.py run --checkout <检出> --work <目录> --steps 0.5
    python3 pack05_check.py run --checkout <检出> --work <目录> --steps 0.4,0.5,withdraw,0.5

`--pick` 让小图库里同时有 #611 头、0.4 一级头和都不中的图，便于核对级别；`run` 依次执行各步并记下每步任务的耗时，
`--check` 时按 Python 打分核对：一级头的主归属、二级头只出候选、候选集合与离线打分一致。
"""

import argparse
import hashlib
import json
from pathlib import Path
import sqlite3
import struct
import subprocess
import sys
import time
import zipfile
import zlib

import numpy as np

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
OUT = ROOT / '_scratch/issue-611'
PACKS = Path.home() / 'Projects/github-public/illustash-packs/_scratch'
DRAFT = PACKS / 'character-pack-0.5'
FILES = {'0.4': PACKS / 'character-pack-0.4/wd-extension-0.4.bin', '0.5': DRAFT / 'wd-extension-0.5.bin'}
CCIP = DRAFT / 'ccip-longtail-0.3.zip'
FEATURES = OUT / 'accept/features.npz'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def png(path, index):
    """8×8 的纯色 PNG，颜色由序号推出，每张字节都不同。"""
    r, g, b = (index >> 16) & 255, (index >> 8) & 255, index & 255
    raw = b''.join(b'\0' + bytes((r, g, b)) * 8 for _ in range(8))

    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data) & 0xffffffff)
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 8, 8, 8, 2, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))


def read_pack(path):
    with path.open('rb') as stream:
        header = json.loads(stream.readline())
        values = np.frombuffer(stream.read(), dtype='<f4')
    n = len(header['labels'])
    return header, values[:-n].reshape(n, 1024), values[-n:]


def scores(x, path):
    header, w, b = read_pack(path)
    logits = x.astype(np.float64) @ w.astype(np.float64).T + b
    t = np.asarray(header['thresholds'], dtype=np.float64)
    return header, logits >= np.log(t / (1 - t))


def setup(args):
    sys.path.insert(0, str(args.checkout))
    from tools.dev_preview import isolated_env
    work = args.work.resolve()
    env = {**isolated_env(work), 'USERPROFILE': str(work / 'home'), 'ILLUSTASH_UPDATE_BASE': 'http://127.0.0.1:9/',
           'HTTPS_PROXY': 'http://127.0.0.1:9', 'HTTP_PROXY': 'http://127.0.0.1:9',
           'ILLUSTASH_TEST_PACKS_PUBKEY': (args.checkout / 'tests/fixtures/pack_catalog/key-a.pub').read_text().strip()}
    return work, env, args.checkout / f'native/terms/target/{args.profile}/illustash-terms'


def prepare(args):
    work, env, binary = setup(args)
    from tests.fixtures.pack_catalog.make import Key
    catalog = (DRAFT / 'packs.json').read_bytes()
    offline = work / 'offline'
    offline.mkdir(parents=True, exist_ok=True)
    signature = Key('A').sign(catalog)
    for version, path in FILES.items():
        with zipfile.ZipFile(offline / f'character-pack-{version}-test-offline.zip', 'w', zipfile.ZIP_DEFLATED) as z:
            z.writestr('packs.json', catalog)
            z.writestr('packs.json.sig', signature)
            z.write(path, path.name)
            z.write(CCIP, CCIP.name)
            for extra in sorted((DRAFT / 'extra').iterdir()):
                z.write(extra, f'licenses/{extra.name}')
    resources = work / 'resources/tags'
    resources.mkdir(parents=True, exist_ok=True)
    (resources / 'tags_v3.csv').write_bytes((args.checkout / 'tags/tags_v3.csv').read_bytes())

    with np.load(FEATURES) as z:
        wd = z['wd_vectors']
    order = np.arange(len(wd))
    if args.pick:
        _, h05 = scores(wd, FILES['0.5'])
        header = read_pack(FILES['0.5'])[0]
        tiers = np.asarray(header['tiers'])
        new = h05[:, 1873:].any(1)
        tier1 = h05[:, tiers == 1].any(1)
        rng = np.random.default_rng(611)
        pick = [rng.permutation(np.flatnonzero(m))[:k] for m, k in
                ((new & ~tier1, args.images // 2), (tier1, args.images // 4), (~new & ~tier1, args.images))]
        order = np.concatenate(pick)[:args.images]
    rows = [int(order[i % len(order)]) for i in range(args.images)]
    folder = work / 'images.noindex'
    folder.mkdir(exist_ok=True)
    for i in range(args.images):
        png(folder / f'{i:06d}.png', i + 1)
    library = work / 'synthetic.pilib'
    started = time.perf_counter()
    for command in (['library-create', '--dest', str(library), '--folder', str(folder)],
                    ['library-scan', '--library', str(library)]):
        result = subprocess.run([str(binary), *command], env=env, capture_output=True, cwd=work)
        if result.returncode:
            raise SystemExit(result.stderr.decode(errors='replace'))
    scanned = time.perf_counter() - started
    source = next(library.glob('source.sqlite'))
    with sqlite3.connect(source) as db:
        db.execute('CREATE TABLE IF NOT EXISTS embeddings(uuid TEXT,model TEXT,vec BLOB,PRIMARY KEY(uuid,model))')
        assets = dict(db.execute('SELECT filename, uuid FROM assets WHERE in_scope=1'))
        mapping = []
        for i in range(args.images):
            uuid = assets[f'{i:06d}.png']
            row = rows[i]
            db.execute("INSERT OR REPLACE INTO embeddings VALUES (?,'wd',?)", (uuid, wd[row].astype('<f4').tobytes()))
            mapping.append([uuid, row])
    (work / 'mapping.json').write_text(json.dumps(mapping))
    print(json.dumps({'images': args.images, 'assets': len(assets), 'create_scan_seconds': round(scanned, 1),
                      'offline': sorted(p.name for p in offline.iterdir())}, ensure_ascii=False))


def run(args):
    work, env, binary = setup(args)
    from tools.verify_read_http import enter, fetch
    from tools.dev_preview import free_port
    library = work / 'synthetic.pilib'
    port = free_port()
    origin = f'http://127.0.0.1:{port}'
    log = (work / 'host.log').open('w+b')
    process = subprocess.Popen([str(binary), 'http', '--no-auto-sync', '--library', str(library), '--port', str(port),
                                '--root', str(work / 'resources'), '--no-names', '--no-index-check'],
                               env=env, stdout=log, stderr=log, cwd=work)
    report = {'binary_sha256': sha(binary.read_bytes()), 'steps': []}
    try:
        enter(origin, log, process)
        token = json.loads(fetch(origin, {'url': '/api/bootstrap'}, origin)['body'])['token']

        def call(body=None):
            item = {'url': '/api/packs'}
            if body is not None:
                item.update(method='POST', body=json.dumps(body).encode(),
                            headers={'Content-Type': 'application/json', 'X-CSRF-Token': token})
            response = fetch(origin, item, origin, timeout=600)
            if response['status'] != 200:
                raise RuntimeError(f'{body} → {response["status"]} {response["body"][:500]!r}')
            return json.loads(response['body'])

        def settle(what):
            while True:
                observed = call()
                job = observed[what]
                if job and job['status'] != 'running':
                    return observed
                time.sleep(0.2)

        call({'action': 'import', 'paths': sorted(str(p) for p in (work / 'offline').glob('*.zip'))})
        while True:
            imported = call()
            jobs = [imported[kind]['download'] for kind in ('wd', 'ccip')]
            if all(job and job['status'] != 'running' for job in jobs):
                break
            time.sleep(0.2)
        # 两个组成部分各自一个导入任务；认不出的文件在另一边是 unknown，合并时以认出的那边为准。
        merged = {}
        for job in jobs:
            for r in job['results']:
                if merged.get(r['name']) in (None, 'unknown'):
                    merged[r['name']] = r['result']
        report['import'] = {'results': merged, 'status': [job['status'] for job in jobs],
                            'serial': imported['catalog']['serial']}
        print(json.dumps({'import': report['import']}, ensure_ascii=False), flush=True)
        for step in args.steps.split(','):
            body = {'action': 'withdraw'} if step == 'withdraw' else {'action': 'enable', 'version': step}
            started = time.perf_counter()
            call(body)
            done = settle('task')
            elapsed = time.perf_counter() - started
            entry = {'step': step, 'seconds': round(elapsed, 2), 'task': done['task'], 'library': done['library']}
            report['steps'].append(entry)
            print(json.dumps({'step': step, 'seconds': entry['seconds'], 'status': done['task']['status']}), flush=True)
            if done['task']['status'] != 'done':
                (work / 'failed.json').write_text(json.dumps(done, ensure_ascii=False, indent=1))
                raise RuntimeError(json.dumps(done['task'], ensure_ascii=False)[:3000])
        report['final'] = {k: call()[k] for k in ('current', 'library', 'wd', 'ccip')}
    finally:
        process.terminate()
        process.wait(60)
        log.close()
    if args.check:
        report['check'] = check(work, library)
    out = work / f'report-{args.steps.replace(",", "_")}.json'
    out.write_text(json.dumps(report, ensure_ascii=False, indent=1))
    print(json.dumps({'report': str(out), 'steps': [(s['step'], s['seconds']) for s in report['steps']],
                      'check': report.get('check')}, ensure_ascii=False))


def check(work, library):
    """库里 WD_EXT 的归属和候选与离线打分对照（最后一步启用的是 0.5 时）。"""
    mapping = json.loads((work / 'mapping.json').read_text())
    with np.load(FEATURES) as z:
        wd = z['wd_vectors']
    x = wd[[row for _, row in mapping]]
    header, hit = scores(x, FILES['0.5'])
    _, w, b = read_pack(FILES['0.5'])
    labels, tiers = header['labels'], header['tiers']
    tier_of = dict(zip(labels, tiers))
    uuids = [u.upper() for u, _ in mapping]
    expected = {(uuids[i], labels[j]) for i, j in zip(*np.nonzero(hit))}
    with sqlite3.connect(library / 'library.sqlite') as db:
        evidence = db.execute(
            "SELECT upper(a.key), json_extract(e.data,'$.tag'), coalesce(json_extract(e.data,'$.tier'),1), r.key "
            "FROM term_evidence e JOIN term_assets a ON a.id=e.asset_id JOIN term_records r ON r.id=e.term_id "
            "WHERE e.source='wd_extension'").fetchall()
        tag_of = {entity: tag for _, tag, _, entity in evidence}
        candidates = [(u.upper(), tag_of.get(e), s) for u, e, s in
                      db.execute("SELECT uuid, entity, score FROM candidates WHERE source='WD_EXT'")]
        members = [(u.upper(), tag_of.get(e)) for u, e in
                   db.execute("SELECT uuid, entity FROM memberships WHERE sources LIKE '%WD_EXT%'")]
    index = {u: i for i, u in enumerate(uuids)}
    position = {name: j for j, name in enumerate(labels)}
    gap = max(abs(s - 1 / (1 + np.exp(-(float(x[index[u]].astype(np.float64) @ w[position[t]].astype(np.float64))
                                         + float(b[position[t]]))))) for u, t, s in candidates)
    got = {(u, t) for u, t, _, _ in evidence}
    member_tiers = sorted({tier_of.get(t) for _, t in members}, key=str)
    tier1_images = {uuids[i] for i in np.flatnonzero(hit[:, np.asarray(tiers) == 1].any(1))}
    return {
        'images': len(uuids),
        'expected_hits': len(expected), 'evidence_rows': len(evidence), 'evidence_equals_offline': got == expected,
        'evidence_tier_matches_pack': all(int(tier) == tier_of[t] for _, t, tier, _ in evidence),
        'candidate_rows': len(candidates), 'candidates_equal_offline': {(u, t) for u, t, _ in candidates} == expected,
        'candidate_score_max_abs_diff': gap,
        'hits_by_tier': {'tier1': int(hit[:, np.asarray(tiers) == 1].sum()), 'tier2_04': int(hit[:, 296:1873].sum()),
                         'tier2_611': int(hit[:, 1873:].sum())},
        'membership_rows': len(members), 'membership_tiers': member_tiers,
        'membership_images_equal_tier1_hit_images': {u for u, _ in members} == tier1_images,
        'memberships_from_611_heads': sum(position[t] >= 1873 for _, t in members),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('mode', choices=('prepare', 'run', 'check'))
    parser.add_argument('--checkout', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--images', type=int, default=2000)
    parser.add_argument('--pick', action='store_true')
    parser.add_argument('--steps', default='0.5')
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--profile', default='release', choices=('release', 'debug'))
    args = parser.parse_args()
    args.checkout = args.checkout.resolve()
    if args.mode == 'check':
        print(json.dumps(check(args.work.resolve(), args.work.resolve() / 'synthetic.pilib'), ensure_ascii=False, indent=1))
        return
    (prepare if args.mode == 'prepare' else run)(args)


if __name__ == '__main__':
    main()
