"""性能测量用的合成大图库（#683）：几万张生成的图、几十个加入日期、几百个角色和几十部作品，外加一批同步时才放进
来源文件夹的新图。全部是脚本生成的图和数据，不用真实图库、不用识别模型、不联网。

和走查夹具（tools/ui_walk/fixture.py）一样只经产品自己的入口写进图库：`library-create`、`library-scan` 建库和同步；
起宿主后经 HTTP 接口建作品和角色、确认归属，和页面上点按钮一样。没有识别结果：图都没有评级（R18 设置为「隐藏」时
照常显示），也没有 WD 标签和向量；首页、图库、资料库要的是图、加入日期和角色归属，这些都有。

**加入日期**：宿主取文件的创建时间（macOS 上 utime 把修改时间改到创建时间之前时，创建时间跟着改）。生成时把每张图的
时间设到它那一天里，所以图库里是几十个不同的日子；时区固定为东八区（`TZ`），每次建出来分天一样。

**同步导入**：`incoming/` 里另备一批图，不在来源文件夹里。测量「同步导入中」时由 `Batches` 每隔几秒复制一批到
来源文件夹的 `zz-incoming/` 下（复制出来的文件创建时间是当时，所以落在「今天」），再由页面自己的同步入口扫描导入；
测完删掉这个子文件夹、把图库恢复成刚建好的样子。

**缓存**（放在仓库根的 `.perf.noindex/`，目录名以 .noindex 结尾，Spotlight 不扫；已被 git 忽略）：
- `images-<键>/`：生成的图，键只取决于生成参数和本文件里画图的那部分，换宿主程序不用重画（3 万张约 1 GB，几分钟）；
- `library-<键>/`：图库和隔离的 HOME、应用数据（`pristine/`），键再加上宿主程序，换了宿主程序就重建（几分钟）；
  测量时用 `live/`，每次开始前从 `pristine/` 恢复，只留下 `thumbnails/`（缩略图缓存的键是图片 ID、文件时间和大小，
  恢复后仍然有效，不用每次重新生成）。
"""
from __future__ import annotations

import concurrent.futures
import datetime
import hashlib
import json
import math
import os
import random
import shutil
import sqlite3
import struct
import subprocess
import threading
import time
import zlib
from dataclasses import asdict, dataclass
from pathlib import Path

from tools import dev_preview
from tools.ui_walk.fixture import Client
from tools.ui_walk.host import Host

ROOT = dev_preview.ROOT
CACHE = ROOT / '.perf.noindex'
TZ = 'Asia/Shanghai'
# 分天用固定的东八区：Asia/Shanghai 1991 年以后没有夏令时，结果和查时区库一样；Windows 的 Python 没有时区库
# （要另装 tzdata），用 ZoneInfo 会报找不到时区。
ZONE = datetime.timezone(datetime.timedelta(hours=8), TZ)
LIBRARY = '合成大图库.pilib'
INCOMING = 'zz-incoming'
# 最新的一天。同步导入的新图落在运行当天，排在它前面。
END_DAY = datetime.date(2026, 11, 1)
# 常见插画的尺寸（宽, 高）：竖图居多，也有横图、方图和很长、很宽的图。长边约 1000–1600，缩略图（长边 400）和中图
# （长边 800）都要真的缩小。
SHAPES = [(900, 1270)] * 5 + [(1000, 1400)] * 3 + [(850, 1200)] * 2 + [(1270, 900)] * 2 + [(1400, 1000), (1100, 1100),
                                                                                          (700, 1600), (1600, 760)]
SYLLABLES = ['ka', 'ri', 'mo', 'sen', 'ta', 'vel', 'no', 'ira', 'lu', 'quen', 'sa', 'dor', 'mi', 'zan', 'el', 'tho',
             'ra', 'vin', 'ko', 'shi', 'ma', 'ren', 'yu', 'fal']


@dataclass(frozen=True)
class Spec:
    images: int = 30000         # 图库里的图
    days: int = 45              # 这些图分在多少个加入日期里
    characters: int = 300       # 角色（人工确认的归属）
    works: int = 30             # 作品（每个角色归一部）
    incoming: int = 3000        # 同步导入时备用的新图

    def key(self) -> str:
        return ','.join(f'{k}={v}' for k, v in asdict(self).items())


# ---------- 图 ----------

POOL = 800                      # 画出来的不同画面；每个文件是其中一张加上自己的编号


def png(width: int, height: int, seed: int) -> bytes:
    """一张合成图：竖向两色渐变底加几个矩形和椭圆，左上角一行 24 格黑白块写着 seed（画面彼此不同）。"""
    rnd = random.Random(seed)
    a = [rnd.randrange(30, 230) for _ in range(3)]
    b = [rnd.randrange(30, 230) for _ in range(3)]
    rows = []
    for y in range(height):
        t = y / max(1, height - 1)
        rows.append(bytearray(bytes(int(a[c] + (b[c] - a[c]) * t) for c in range(3)) * width))
    for _ in range(rnd.randrange(3, 7)):
        color = bytes(rnd.randrange(0, 256) for _ in range(3))
        x0, y0 = rnd.random() * .8, rnd.random() * .8
        x1, y1 = x0 + .1 + rnd.random() * .35, y0 + .1 + rnd.random() * .35
        px0, py0, px1, py1 = int(x0 * width), int(y0 * height), min(width, int(x1 * width)), min(height, int(y1 * height))
        ellipse = rnd.random() < .5
        cx, cy, rx, ry = (px0 + px1) / 2, (py0 + py1) / 2, max(1, (px1 - px0) / 2), max(1, (py1 - py0) / 2)
        for y in range(py0, py1):
            lo, hi = px0, px1
            if ellipse:
                d = 1 - ((y + .5 - cy) / ry) ** 2
                if d <= 0:
                    continue
                half = rx * math.sqrt(d)
                lo, hi = max(0, int(cx - half)), min(width, int(cx + half))
            rows[y][lo * 3:hi * 3] = color * (hi - lo)
    cell = 12
    for bit in range(24):
        c = b'\xf0\xf0\xf0' if (seed >> bit) & 1 else b'\x10\x10\x10'
        for y in range(cell):
            rows[y][bit * cell * 3:(bit + 1) * cell * 3] = c * cell
    raw = b''.join(b'\x00' + bytes(r) for r in rows)
    header = struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0)
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', header) + chunk(b'IDAT', zlib.compress(raw, 1)) + chunk(b'IEND', b'')


def chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))


def stamped(base: bytes, ident: int) -> bytes:
    """在 IHDR 后面插一段文字块写上 ident：像素和画面池里那张一样，文件字节各不相同（图库按文件内容的哈希认图，
    所以每个文件都是一张单独的图；没有研究 worker，不会按画面找重复和相似）。"""
    return base[:33] + chunk(b'tEXt', b'Comment\x00illustash-perf %d' % ident) + base[33:]


@dataclass(frozen=True)
class Planned:
    rel: str          # 相对来源文件夹（或 incoming/）的路径
    pool: int         # 画面池里的哪一张
    ident: int
    stamp: float      # 文件时间（Unix 秒）；同步导入的新图为 0，复制时才定


def shapes() -> list[tuple[int, int]]:
    """画面池每一张的尺寸。"""
    rnd = random.Random('ui-perf-shapes')
    return [rnd.choice(SHAPES) for _ in range(POOL)]


def day_counts(spec: Spec) -> list[tuple[datetime.date, int]]:
    """几十个加入日期和每天的张数：日期在最近约三倍天数的范围里挑（中间有空着的日子），张数是长尾的（有几天一次加了一两千
    张，多数日子几十到几百张），最新的一天总在里面。"""
    rnd = random.Random('ui-perf-days')
    offsets = sorted({0, *rnd.sample(range(1, spec.days * 3), spec.days - 1)})
    weights = [rnd.lognormvariate(0, 1.3) for _ in offsets]
    total = sum(weights)
    counts = [max(3, int(spec.images * w / total)) for w in weights]
    counts[counts.index(max(counts))] += spec.images - sum(counts)
    return [(END_DAY - datetime.timedelta(days=o), n) for o, n in zip(offsets, counts)]


def plan(spec: Spec) -> tuple[list[Planned], list[Planned]]:
    """图库里的图（按天的子文件夹）和同步导入备用的新图。"""
    rnd = random.Random('ui-perf-images')
    zone = ZONE
    out: list[Planned] = []
    for day, n in day_counts(spec):
        start = datetime.datetime.combine(day, datetime.time(8), zone).timestamp()
        for k in range(n):
            # 一天里的时间从 8 点到 23 点均匀摆开，同一天里的先后每次一样。
            stamp = start + (k + .5) * 15 * 3600 / n
            out.append(Planned(f'{day:%Y%m%d}/img-{len(out):06d}.png', rnd.randrange(POOL), len(out), stamp))
    incoming = [Planned(f'new-{k:05d}.png', rnd.randrange(POOL), len(out) + k, 0) for k in range(spec.incoming)]
    return out, incoming


def _draw(folder: str, indices: list[int]) -> int:
    sizes = shapes()
    for i in indices:
        w, h = sizes[i]
        (Path(folder) / f'{i:04d}.png').write_bytes(png(w, h, i))
    return len(indices)


def draw(pool: Path, jobs: int) -> None:
    """多进程画画面池。调用方用 `collab_run.py bg` 运行时子进程继承后台优先级。"""
    pool.mkdir(parents=True, exist_ok=True)
    chunks = [list(range(i, min(POOL, i + 20))) for i in range(0, POOL, 20)]
    with concurrent.futures.ProcessPoolExecutor(jobs) as ex:
        done = 0
        for n in ex.map(_draw, [str(pool)] * len(chunks), chunks):
            done += n
            if done % 200 == 0:
                print(f'  画面池 {done}/{POOL}', flush=True)


def write(pool: Path, folder: Path, items: list[Planned]) -> None:
    bases = {}
    for it in items:
        base = bases.get(it.pool) or bases.setdefault(it.pool, (pool / f'{it.pool:04d}.png').read_bytes())
        path = folder / it.rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(stamped(base, it.ident))
        if it.stamp:
            os.utime(path, (it.stamp, it.stamp))


def image_key(spec: Spec) -> str:
    """生成的图只取决于生成参数和画图的代码。"""
    source = Path(__file__).read_text(encoding='utf-8')
    drawing = source[source.index('\n# ---------- 图 ----------\n'):source.index('\ndef image_key(')]
    return hashlib.sha256(repr((spec.key(), drawing, SHAPES, END_DAY, TZ)).encode()).hexdigest()[:12]


def images(spec: Spec, jobs: int) -> Path:
    """取缓存里生成好的图，没有就生成一份：<缓存>/images-<键>/library 是来源文件夹，incoming 是同步导入备用的新图，
    pool 是画面池。"""
    root = CACHE / f'images-{image_key(spec)}'
    if (root / 'done.json').is_file():
        return root
    shutil.rmtree(root, ignore_errors=True)
    library, incoming = plan(spec)
    print(f'生成合成图 {len(library)} + {len(incoming)} 张（画面池 {POOL} 张；第一次或参数变了）…', flush=True)
    draw(root / 'pool', jobs)
    write(root / 'pool', root / 'library', library)
    write(root / 'pool', root / 'incoming', incoming)
    (root / 'done.json').write_text(json.dumps({'spec': asdict(spec), 'days': [[str(d), n] for d, n in day_counts(spec)]}),
                                    encoding='utf-8')
    return root


# ---------- 图库 ----------

def coined(rnd: random.Random, parts: int) -> str:
    return ''.join(rnd.choice(SYLLABLES) for _ in range(parts)).capitalize()


def names(spec: Spec) -> tuple[list[str], list[str]]:
    """造词的作品名和角色名，不会和真实作品、角色重名；重复的加序号。"""
    rnd = random.Random('ui-perf-names')
    works, chars, seen = [], [], set()
    while len(works) < spec.works:
        name = f'{coined(rnd, 2)} {coined(rnd, 2)}'
        if name not in seen:
            seen.add(name)
            works.append(name)
    while len(chars) < spec.characters:
        name = f'{coined(rnd, 2)} {coined(rnd, rnd.choice([1, 2]))}'
        if name not in seen:
            seen.add(name)
            chars.append(name)
    return works, chars


def memberships(spec: Spec, count: int) -> list[list[int]]:
    """每个角色拥有的图（图的序号）：约四成半的图没有角色，四成半一个，一成两个；角色热度是长尾的。"""
    rnd = random.Random('ui-perf-members')
    weights = [1 / (k + 1) ** .9 for k in range(spec.characters)]
    owned: list[list[int]] = [[] for _ in range(spec.characters)]
    for i in range(count):
        r = rnd.random()
        n = 0 if r < .45 else 1 if r < .9 else 2
        for c in set(rnd.choices(range(spec.characters), weights, k=n)):
            owned[c].append(i)
    return owned


@dataclass
class Fixture:
    root: Path              # library-<键>/：pristine/、live/、fixture.json
    images: Path            # images-<键>/
    summary: dict

    @property
    def live(self) -> Path:
        return self.root / 'live'

    @property
    def library(self) -> Path:
        return self.live / LIBRARY

    @property
    def source(self) -> Path:
        return self.images / 'library'

    def reset(self) -> Path:
        """把 live/ 恢复成刚建好的样子，保留缩略图缓存；删掉上次同步导入留下的新图。返回图库路径。"""
        shutil.rmtree(self.source / INCOMING, ignore_errors=True)
        thumbs = self.library / 'thumbnails'
        keep = self.root / 'thumbnails.keep'
        if thumbs.is_dir():
            shutil.rmtree(keep, ignore_errors=True)
            thumbs.rename(keep)
        shutil.rmtree(self.live, ignore_errors=True)
        shutil.copytree(self.root / 'pristine', self.live, symlinks=True)
        if keep.is_dir():
            shutil.rmtree(self.library / 'thumbnails', ignore_errors=True)
            keep.rename(self.library / 'thumbnails')
        return self.library


def library_key(spec: Spec, core: Path) -> str:
    h = hashlib.sha256((spec.key() + image_key(spec)).encode())
    h.update(Path(__file__).read_bytes())
    st = core.stat()
    h.update(f'{core.resolve()}:{st.st_size}:{st.st_mtime_ns}'.encode())
    return h.hexdigest()[:12]


def env_for(work: Path) -> dict[str, str]:
    env = dev_preview.isolated_env(work)
    env['TZ'] = TZ
    return env


QUIET = ['--no-auto-sync', '--no-provenance', '--no-index-check']


def build(spec: Spec, core: Path, root: Path, pictures: Path) -> Fixture:
    root.mkdir(parents=True)
    work = root / 'live'
    work.mkdir()
    env = env_for(work)
    library = work / LIBRARY
    source = pictures / 'library'
    shutil.rmtree(source / INCOMING, ignore_errors=True)
    run = lambda *a: subprocess.run([str(core), *a], env=env, check=True, capture_output=True, text=True)
    started = time.monotonic()
    run('library-create', '--dest', str(library), '--folder', str(source), '--name', '合成大图库')
    run('library-scan', '--library', str(library))
    scanned = time.monotonic() - started
    with sqlite3.connect(library / 'source.sqlite') as c:
        rows = c.execute('SELECT uuid, src_path FROM assets').fetchall()
    by_rel = {str(Path(p).relative_to(source)): u for u, p in rows}
    ordered = [by_rel[it.rel] for it in plan(spec)[0]]
    work_names, char_names = names(spec)
    owned = memberships(spec, len(ordered))
    with Host(core, ROOT, library, work, extra=QUIET, env={'TZ': TZ}) as host:
        api = Client(host.url)
        works = [api.post('/api/works', {'action': 'create', 'name': n})['id'] for n in work_names]
        for k, (name, members) in enumerate(zip(char_names, owned)):
            entity = api.post('/api/entities', {'name': name})['id']
            api.post('/api/character/profile', {'entity': entity, 'name': name, 'work': works[k % len(works)]})
            ids = [ordered[i] for i in members]
            for i in range(0, len(ids), 2000):
                api.post('/api/change', {'ids': ids[i:i + 2000], 'action': 'confirmed', 'entity': entity, 'confirm': True})
        # 引导：标成跳过并收起清单；只问一次的提示卡记成问过；不自动检查更新（和走查夹具一样）。
        api.post('/api/onboarding', {'action': 'skip'})
        api.post('/api/onboarding', {'action': 'hide', 'hidden': True})
        for key in ('names', 'works'):
            api.post('/api/onboarding', {'action': 'seen', 'key': key})
        api.post('/api/updates', {'action': 'auto', 'allow': False})
    shutil.rmtree(library / 'thumbnails', ignore_errors=True)
    shutil.copytree(work, root / 'pristine', symlinks=True)
    days = day_counts(spec)
    summary = {'spec': asdict(spec), 'assets': len(ordered), 'days': len(days), 'largest_day': max(n for _, n in days),
               'newest_day': str(max(d for d, _ in days)), 'oldest_day': str(min(d for d, _ in days)),
               'with_character': sum(1 for i in set().union(*map(set, owned))), 'core': str(core),
               'build_seconds': round(time.monotonic() - started), 'scan_seconds': round(scanned)}
    (root / 'fixture.json').write_text(json.dumps(summary, ensure_ascii=False, indent=1), encoding='utf-8')
    return Fixture(root, pictures, summary)


def cached(spec: Spec, core: Path, jobs: int = 3) -> Fixture:
    """取缓存里的夹具，没有就建一份；同一个规格的旧图库（换了宿主程序以前建的）删掉。"""
    pictures = images(spec, jobs)
    key = library_key(spec, core)
    root = CACHE / f'library-{key}'
    if (root / 'fixture.json').is_file() and (root / 'pristine').is_dir():
        return Fixture(root, pictures, json.loads((root / 'fixture.json').read_text(encoding='utf-8')))
    for old in CACHE.glob('library-*'):
        meta = old / 'fixture.json'
        if old != root and (not meta.is_file() or json.loads(meta.read_text(encoding='utf-8')).get('spec') == asdict(spec)):
            shutil.rmtree(old, ignore_errors=True)
    print('建合成大图库（第一次或宿主程序变了，几分钟）…', flush=True)
    shutil.rmtree(root, ignore_errors=True)
    # 图库里存的是绝对路径（来源文件夹），所以直接在最终位置建，建完才写 fixture.json 表示可用。
    return build(spec, core, root, pictures)


class Batches:
    """同步导入：每隔 every 秒把 incoming/ 里的下一批 size 张复制到来源文件夹的 zz-incoming/ 下。扫描和导入由页面
    自己的同步入口做（见 probe.js 的 sync）；这里只负责「来源文件夹里出现了新图」。"""

    def __init__(self, fixture: Fixture, size: int = 150, every: float = 4.0):
        self.fixture, self.size, self.every = fixture, size, every
        self.files = sorted((fixture.images / 'incoming').glob('*.png'))
        self.copied = 0
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.loop, daemon=True)

    def loop(self) -> None:
        target = self.fixture.source / INCOMING
        target.mkdir(exist_ok=True)
        while not self.stop.is_set() and self.copied < len(self.files):
            # 先复制到点开头的文件夹（扫描跳过），整批复制完再改名，扫描不会读到写了一半的文件。
            name = f'batch-{self.copied // self.size:03d}'
            staging = target / ('.' + name)
            staging.mkdir()
            for f in self.files[self.copied:self.copied + self.size]:
                # 复制出来的文件创建时间是现在，图库按它记加入日期：落在「今天」。
                shutil.copyfile(f, staging / f.name)
            staging.rename(target / name)
            self.copied += self.size
            self.stop.wait(self.every)

    def __enter__(self) -> 'Batches':
        self.thread.start()
        return self

    def __exit__(self, *_: object) -> None:
        self.stop.set()
        self.thread.join()
