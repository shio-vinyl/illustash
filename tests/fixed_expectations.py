"""只读的历史固定期望，按顺序恢复值、摘要和合成包。

夹具无法录制或保存。临时路径、令牌、服务地址按登记占位符还原，时钟按 <clock> 比较。
其他类型、列、rowid、响应字节与固定录制保持精确一致。
"""
import base64
import gzip
import hashlib
import json
import lzma
import os
import re
import sqlite3
import struct
import subprocess
import sys
import tarfile
import unittest
from contextlib import closing
from functools import lru_cache
from pathlib import Path

WINDOWS = os.name == 'nt'
# 录制的路径在占位符之后接着用 / 分隔的各段。
_TAIL = r'((?:/[^/"\'\s,;:}\]\)\\]+)*)'
FIXTURES = Path(__file__).resolve().parent / 'fixtures' / 'reference_recordings'
_current = None  # 正在运行的测试的记录器：它这一轮特有的字符串在摘要里写作占位符
CLOCK = re.compile(r'\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:\d{2})?')
CLOCK_BYTES = re.compile(CLOCK.pattern.encode())
# 这些期望都录在导入按 tag 后缀建作品（#296）之前：加载本模块的测试及其启动的原生进程照旧规则导入，录下的期望
# 不改（用户 2026-11-12 定，与 native/terms/tests/baseline.rs 相同）。只供测试，产品从不设置；新规则由 #296 新加的
# Rust 测试覆盖。
os.environ['ILLUSTASH_TEST_NO_IMPORT_WORKS'] = '1'
# 录制那天（#301）。按今天算的窗口（角色「近 30 天」、「近 12 个月」）录的是这天的结果，原生侧经仅测试用的
# ILLUSTASH_TEST_TODAY 把今天固定在这里，期望不随运行日期过期。
RECORDED_TODAY = {'ILLUSTASH_TEST_TODAY': '2026-10-07'}


def fixed_values(name):
    """tests/fixtures 下的一个冻结值文件。旧参考实现在 Windows 上算得不同的值（浮点求和顺序、图片编解码器）
    也在那里录制过，作为按所列路径替换的值保存在 tests/fixtures/windows/ 里；只在 Windows 上生效。"""
    opener = lzma.open if name.endswith('.xz') else gzip.open
    with opener(FIXTURES.parent / name, 'rt', encoding='utf-8') as f:
        values = json.load(f)
    overlay = FIXTURES.parent / 'windows' / name
    if WINDOWS and overlay.exists():
        with opener(overlay, 'rt', encoding='utf-8') as f:
            for path, value in json.load(f)['replace']:
                node = values
                for key in path[:-1]:
                    node = node.setdefault(key, {})
                node[path[-1]] = value
    return values


def with_ambiguous_pool(url, response):
    """#786：这些合成图库没有可能认错的图，在旧响应末尾精确追加这一个池。"""
    if url.split('?', 1)[0] != '/api/review-pools' or response['status'] != 200:
        return response
    pool = {'kind': 'ambiguous', 'name': '可能认错', 'count': 0, 'cover': None}
    body = response['body']
    assert body.endswith(b']')
    body = body[:-1] + b', ' + json.dumps(pool, ensure_ascii=False).encode() + b']'
    return {**response, 'body': body, 'headers': {**response['headers'], 'Content-Length': str(len(body))}}


def unclocked(value):
    """把 str 或 bytes 里形似时钟的文本遮盖掉。"""
    if isinstance(value, str):
        if _current is not None:
            value = _current._to_placeholders(value)
        return CLOCK.sub('<clock>', value)
    if isinstance(value, bytes):
        return CLOCK_BYTES.sub(b'<clock>', value)
    return value


def quote(name):
    return '"' + name.replace('"', '""') + '"'


def table_digests(path):
    """schema、user_version、每张表（带类型的单元格和 rowid）和每个视图（行），各取一个摘要。"""
    with closing(sqlite3.connect(Path(path).resolve().as_uri() + '?mode=ro', uri=True)) as c:
        c.execute('BEGIN')
        schema = [tuple(r) for r in c.execute('SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name')]
        out = {'schema': _hash(schema), 'user_version': _hash(c.execute('PRAGMA user_version').fetchone()[0])}
        for kind, name, _, sql in schema:
            if kind not in ('table', 'view'):
                continue
            columns = '*' if kind == 'view' or 'WITHOUT ROWID' in (sql or '').upper() else 'rowid,*'
            cursor = c.execute('SELECT ' + columns + ' FROM ' + quote(name))
            # 文本单元格按比较时的形式：这一轮特有的字符串（存储的临时路径）写作占位符。
            out[name] = _hash(([d[0] for d in cursor.description],
                               sorted((tuple((type(v).__name__, unclocked(v)) for v in row) for row in cursor), key=repr)))
        return out


def _hash(value):
    return hashlib.sha256(repr(value).encode()).hexdigest()[:20]


def library_digests(lib):
    return Live({name: table_digests(lib.path / filename) for name, filename in
                 (('app', 'library.sqlite'), ('source', 'source.sqlite'), ('provenance', 'provenance.sqlite'))},
                baseline_state(lib.path))


class Library:
    """测试用到的图库包的各个部分：它的目录和数据库路径。"""
    def __init__(self, path):
        self.path = Path(path)
        self.db, self.source = self.path / 'library.sqlite', self.path / 'source.sqlite'
        self.thumbnails, self.backups, self.features = (self.path / sub for sub in ('thumbnails', 'backups', 'features'))
        for sub in ('features', 'thumbnails', 'backups'):
            (self.path / sub).mkdir(exist_ok=True)

    @property
    def config(self):
        """第一次读到的 library.json（之后保留并原地修改，与应用自己的对象一样）。"""
        if not hasattr(self, '_config'):
            self._config = json.loads((self.path / 'library.json').read_text(encoding='utf-8'))
        return self._config

    name = property(lambda self: self.config['name'])
    frozen = property(lambda self: bool(self.config.get('test') or self.config.get('frozen')))

    def save(self, config=None):
        if config is not None:
            self._config = config
        (self.path / 'library.json').write_text(json.dumps(self.config, ensure_ascii=False, indent=2) + '\n',
                                                encoding='utf-8', newline='\n')


class Expectations:
    def __init__(self, test):
        """一个测试的值；传入的是类时，取该类 ``setUpClass`` 的值。"""
        global _current
        _current = self
        kind = test if isinstance(test, type) else type(test)
        self.module = kind.__module__.split('.')[-1]
        # 不论测试是怎样被发现的，都以 tests 包下的 id 为键。
        test_id = f'{kind.__module__}.{kind.__qualname__}.setUpClass' if isinstance(test, type) else test.id()
        self.test = 'tests.' + test_id.removeprefix('tests.')
        self.aliases = []      # (actual, placeholder)
        self.position = 0
        self.states = 0        # 到目前为止的 state() 调用次数：#201 对照基准按这个序号给状态编键
        try:
            data = _load(self.module)
        except AssertionError:
            if isinstance(test, type):
                raise
            data = {'tests': {}, 'blobs': {}}   # 只含平台专用子类的模块
        self.entries = data['tests'].get(self.test)
        if self.entries is None and not isinstance(test, type):
            # 平台专用的子类（如 Windows 上的 OpenBLAS 运行）重放父类的录制。
            definer = None   # 定义这个测试方法的父类所在模块：父类的测试也没录制时，共用准备值从那里取
            for parent in kind.__mro__[1:]:
                if not (isinstance(parent, type) and issubclass(parent, unittest.TestCase)) \
                        or parent is unittest.TestCase:
                    continue
                name = parent.__module__.split('.')[-1]
                key = f"tests.{parent.__module__.removeprefix('tests.')}.{parent.__qualname__}.{test._testMethodName}"
                try:
                    found = _load(name)
                except AssertionError:
                    continue
                if key in found['tests']:
                    self.module, self.test, data = name, key, found
                    self.entries = found['tests'][key]
                    break
                if definer is None and test._testMethodName in parent.__dict__:
                    definer = name, key, found
            else:
                if definer is not None and test._testMethodName not in kind.__dict__:
                    # 父类里新加、没录制的测试（如 #792 的拆组）：照它在父类模块里运行时那样取共用准备值，
                    # 不取子类模块的（只含平台专用子类的模块往往一条录制也没有）。
                    self.module, self.test, data = definer
        if self.entries is None:
            # 从未录制过：只在录制平台以外的平台上运行的测试，或者不用参考值的测试。
            # 它共用的准备阶段的值取自本模块第一个录制过的测试（同样的合成包和种子）。
            donors = [entries for _, entries in sorted(data['tests'].items())]
            self.entries, self.donor = [], donors[0] if donors else []
        self.blobs = data['blobs']

    def alias(self, actual, placeholder):
        """登记一个这一轮特有的字符串；同一个占位符以最后一次登记为准。macOS 的临时路径
        也按未解析的 /var 写法识别。"""
        actual = str(actual)
        pairs = [(actual, placeholder)]
        if actual.startswith('/private/'):
            pairs.append((actual[len('/private'):], placeholder[:-1] + '_SHORT@'))
        names = {p for _, p in pairs}
        self.aliases = [(a, p) for a, p in self.aliases if p not in names] + pairs

    def _ordered(self):
        return sorted(self.aliases, key=lambda x: -len(x[0]))

    def _to_placeholders(self, text):
        for actual, placeholder in self._ordered():
            if WINDOWS and _is_path(actual):
                # 录制里的路径用 /：Windows 路径（包括经过 JSON 转义的）要规范化。
                escaped = json.dumps(actual, ensure_ascii=False)[1:-1]
                text = re.sub(re.escape(escaped) + r'((?:\\\\[^\\"]+)*)',
                              lambda m: placeholder + m.group(1).replace('\\\\', '/'), text)
                text = re.sub(re.escape(actual) + r'((?:\\[^\\"\s,;:}\]\)]+)*)',
                              lambda m: placeholder + m.group(1).replace('\\', '/'), text)
            text = text.replace(actual, placeholder)
        return text

    def _from_placeholders(self, text):
        """把这一轮的字符串放回原处；在 Windows 上路径改用本平台的分隔符，文本是 JSON 文档时再做 JSON 转义。"""
        if not WINDOWS:
            for actual, placeholder in self.aliases:
                text = text.replace(placeholder, actual)
            return text
        as_json = _is_json(text)
        for actual, placeholder in sorted(self.aliases, key=lambda x: -len(x[1])):
            if placeholder not in text:
                continue
            if not _is_path(actual):
                text = text.replace(placeholder, actual)
                continue
            def path(m, actual=actual):
                value = actual + m.group(1).replace('/', '\\')
                return json.dumps(value, ensure_ascii=False)[1:-1] if as_json else value
            text = re.sub(re.escape(placeholder) + _TAIL, path, text)
        return text


    def _decode(self, value):
        if isinstance(value, str):
            return self._from_placeholders(value)
        if isinstance(value, list):
            return [self._decode(v) for v in value]
        if isinstance(value, dict):
            if set(value) == {'$bytes'}:
                return self._from_placeholders(base64.b64decode(value['$bytes']).decode('utf-8', 'surrogateescape')).encode('utf-8', 'surrogateescape')
            if set(value) == {'$tuple'}:
                return tuple(self._decode(v) for v in value['$tuple'])
            if set(value) == {'$items'}:
                return {self._decode(k): self._decode(v) for k, v in value['$items']}
            out = {k: self._decode(v) for k, v in value.items()}
            # 响应体换入这一轮的字符串之后，长度响应头也随之保持一致。
            if isinstance(out.get('body'), bytes) and isinstance(out.get('headers'), dict) and 'Content-Length' in out['headers']:
                out['headers']['Content-Length'] = str(len(out['body']))
            return out
        return value

    def _next(self, label):
        donor = getattr(self, 'donor', None)
        if donor is not None and self.position < len(donor) and donor[self.position]['label'] == label:
            self.position += 1
            return donor[self.position - 1]
        if self.position >= len(self.entries):
            raise AssertionError(f'{self.test}: no recorded value #{self.position} ({label})')
        entry = self.entries[self.position]
        if entry['label'] != label:
            raise AssertionError(f'{self.test}: recorded value #{self.position} is {entry["label"]!r}, test asks {label!r}')
        self.position += 1
        return entry

    def value(self, label):
        """读取一个已冻结的历史返回值。"""
        return self._decode(self._next(label)['value'])

    def state(self, label):
        """参考图库的一个状态，以摘要形式保存；与实时的 ``library_digests`` 比较相等。
        与实时状态比较时，它还带着为这个状态录制的 #201 对照基准（见 ``Live``）。"""
        value = self._next(label)['value']
        slot = (self.module, self.test, f'{self.states}:{label}')
        self.states += 1
        return Digests(value, slot)

    def baseline(self, label, package, rename=None, only=None):
        """图库包在某个命名时刻的 #201 对照基准（测试整体比较的新图库）：录制时录下，否则与录过的内容比较
        （如果录过）。``rename`` 先把随机生成的值（生成的键）映射成稳定的名字；``only``（{db: [tables]}）
        只比较录制状态里的这些表。"""
        return _check_baseline((self.module, self.test, f'@{label}'), baseline_state(Path(package), rename), only)

    def package(self, label, path, library=True):
        """恢复冻结的合成输入包，数据库文本中的临时路径按本轮根目录还原。"""
        path = Path(path)
        files = self._next(label)['value']
        # 存在数据库文本里的那一轮特有的字符串（路径里的临时根目录）换回这一轮的。
        rebase = [(p, a) for a, p in sorted(self.aliases, key=lambda x: -len(x[1]))]
        path.mkdir(parents=True, exist_ok=True)
        for name, key in files.items():
            if WINDOWS and any(c in name for c in '<>:"|?*'):
                continue   # 本平台存不了这个文件名；相关测试只在 POSIX 上运行
            target = path / name
            target.parent.mkdir(parents=True, exist_ok=True)
            for suffix in ('-wal', '-shm', '-journal'):
                Path(str(target) + suffix).unlink(missing_ok=True)
            raw = isinstance(key, list)
            data = self.blobs[key[1] if raw else key]
            if not raw and not name.endswith(('.sqlite', '.db')):
                data = self._from_placeholders(data.decode('utf-8')).encode()
            target.write_bytes(data)
            if not raw and name.endswith(('.sqlite', '.db')) and rebase:
                _rebase(target, rebase, self._from_placeholders)
            # 图库（不含其备份）升级到当前核心版本（#201）。
            if Path(name).name in ('library.sqlite', 'app.sqlite') and not {'backups', 'library-backups'} & set(Path(name).parts):
                upgrade_recorded(target)
        return Library(path) if library else path

    def skip(self, label):
        """本平台没有对应项的录制值（原因由测试写明）。"""
        self._next(label)

    def finish(self, test):
        """每个录制的值都必须用到。"""
        if getattr(self, 'donor', None) is None and self.position != len(self.entries):
            test.fail(f'{self.test}: {len(self.entries) - self.position} recorded reference values not reached')


class Digests:
    """参考图库某个状态的逐表摘要；与摘要相同的实时状态相等。"""
    def __init__(self, tree, slot=None):
        self.tree = tree
        self.slot = slot

    def __getitem__(self, key):
        value = self.tree[key]
        return Digests(value) if isinstance(value, dict) else value

    def __eq__(self, other):
        if isinstance(other, Digests):
            return self.tree == other.tree
        if isinstance(other, Live):
            if RECORDING:
                # 录制时（只在核心版本 7 上，PR #239）：按上面的录制判断相等，按 #201 规则取的实时状态
                # 就是该状态的对照基准。
                equal = self.tree == dict(other)
                return equal and (self.slot is None or _check_baseline(self.slot, other.baseline))
            if self.slot is None:
                return self.tree == dict(other)
            # core v8 改了 schema，并退役了这些摘要覆盖的表：以 #201 对照基准为参考。
            return _check_baseline(self.slot, other.baseline)
        if isinstance(other, dict):
            return self.tree == other
        return NotImplemented

    def __repr__(self):
        return f'Digests({self.tree!r})'


def _rebase(path, pairs, convert=None):
    """用 ``convert``（默认是普通替换）替换普通表每个文本单元格里录制的那一轮特有的字符串（``pairs`` 为
    旧、新对），rowid 保留。期间先把触发器移开（它们把守写入，不是数据）。"""
    if convert is None:
        def convert(text):
            for old, new in pairs:
                text = text.replace(old, new)
            return text
    with closing(sqlite3.connect(path)) as c:
        c.create_function('convert_text', 1, lambda v: convert(v) if isinstance(v, str) else v, deterministic=True)
        tables = [(name, sql) for name, sql in c.execute("SELECT name,sql FROM sqlite_schema WHERE type='table'")
                  if sql and not name.startswith('sqlite_') and not sql.upper().startswith('CREATE VIRTUAL')]
        virtual = [name for (name,) in c.execute("SELECT name FROM sqlite_schema WHERE type='table' AND upper(sql) LIKE 'CREATE VIRTUAL%'")]
        updates = []
        for name, _ in tables:
            if any(name.startswith(v + '_') for v in virtual):
                continue
            for (column,) in c.execute('SELECT name FROM pragma_table_info(?)', (name,)).fetchall():
                for old, new in pairs:
                    if c.execute(f'SELECT 1 FROM {quote(name)} WHERE typeof({quote(column)})=\'text\' '
                                 f'AND instr({quote(column)},?) LIMIT 1', (old,)).fetchone():
                        updates.append((name, column, old, new))
        if not updates:
            return
        triggers = c.execute("SELECT name,sql FROM sqlite_schema WHERE type='trigger' ORDER BY rowid").fetchall()
        c.execute('BEGIN')
        for name, _ in triggers:
            c.execute(f'DROP TRIGGER {quote(name)}')
        for name, column, old in dict.fromkeys((n, col, o) for n, col, o, _ in updates):
            c.execute(f'UPDATE {quote(name)} SET {quote(column)}=convert_text({quote(column)}) '
                      f'WHERE typeof({quote(column)})=\'text\' AND instr({quote(column)},?)', (old,))
        for _, sql in triggers:
            c.execute(sql)
        c.commit()


def upgrade_recorded(db):
    """录制的旧核心版本（7）图库，在测试使用之前像用户的图库一样用核心自己的命令升级（#201）；
    升级时生成的备份删除，目录保持原样。"""
    with closing(sqlite3.connect(db)) as c:
        has_meta = c.execute("SELECT 1 FROM sqlite_schema WHERE type='table' AND name='meta'").fetchone()
        row = c.execute("SELECT value FROM meta WHERE key='terms_schema'").fetchone() if has_meta else None
    if row is None or not 0 < int(row[0]) < TERMS_VERSION:
        return
    binary = os.environ.get('ILLUSTASH_TERMS_BIN')
    if not binary:
        raise unittest.SkipTest('set ILLUSTASH_TERMS_BIN: recorded packages are upgraded to this core version first')
    backups = Path(db).parent / 'library-backups'
    existing = set(backups.iterdir()) if backups.exists() else None
    done = subprocess.run([binary, 'migrate', '--db', str(db)], capture_output=True)
    if done.returncode:
        raise AssertionError(f'upgrading {db}: {done.stderr.decode(errors="replace")}')
    for made in set(backups.iterdir()) - (existing or set()):
        made.unlink()
    if existing is None:
        backups.rmdir()


def _is_path(text):
    return '\\' in text or text.startswith('/')


def _is_json(text):
    stripped = text.strip()
    if not stripped[:1] in ('{', '['):
        return False
    try:
        json.loads(stripped)
        return True
    except ValueError:
        return False






@lru_cache(maxsize=None)
def _load(module):
    path = FIXTURES / f'{module}.tar.xz'
    if not path.exists():
        raise AssertionError(f'no recorded reference fixture {path.name}')
    data = {'tests': {}, 'blobs': {}}
    with tarfile.open(path, 'r:xz') as tar:
        for member in tar.getmembers():
            content = tar.extractfile(member).read()
            if member.name == 'expectations.json':
                data['tests'] = json.loads(content)
            elif member.name.startswith('blobs/'):
                data['blobs'][member.name[6:]] = content
    return data


# ---- 第一个公开测试版的对照基准（#201）----
#
# core v8 删掉了旧表，上面的摘要（整个 schema、每张表、rowid）就对不上了。录制状态与实时状态比较时，
# 实时状态还按 native/terms/tests/common/baseline.rs 的规则再取一次，与在核心版本 7 的 main 上录制的对照基准比较：
# 除 RETIRED 之外的每张表（RETIRED_ROWS 里的设置行也除外）、带类型的单元格、行按多重集比较（不比较隐式 rowid），
# 这一轮特有的字符串和时钟同上，学习戳遮盖，少数 meta 值按含义比较。
# 用 ILLUSTASH_RECORD_BASELINE=1 录制到 fixtures/baseline_states/<module>.json.gz。

BASELINE_STATES = FIXTURES.parent / 'baseline_states'
# 录制里仍有身份关系表（录制之后才退役）；它们同样不参与比较。
RETIRED = frozenset({'entities', 'links', 'work_review', 'names', 'display_names', 'name_aliases', 'identity_baseline',
                     'identity_relations', 'identity_evidence', 'identity_decisions', 'identity_decision_evidence'})
# 其余的是 core v8 新增的（#201）：历史记录从 `actions` 这两列显示的内容，
# 以及读取方从 `term_names.seq` 和 `term_tree.seq` 取得的顺序，在响应里比较；
# 从 `terms.attrs` 移到 `term_tree` 的身份属性在 `terms.attrs` 里比较（ATTRIBUTES）。
SKIP_COLUMNS = frozenset({('term_packages', 'manifest'), ('term_evidence', 'fingerprint'),
                          ('actions', 'label_kind'), ('actions', 'label_count'), ('term_names', 'seq')}
                         | {('term_tree', col) for col in ('variant', 'artist', 'label_only', 'mascot',
                                                           'mascot_origin', 'original', 'seq')})
# 身份的 `terms.attrs` 按早先镜像写入的对象比较，由它的 `term_tree` 行构造出来。
ATTRIBUTES = ('variant', 'artist', 'label_only', 'mascot', 'mascot_origin', 'original')
STAMP = re.compile(r'learning-v1:[0-9a-f]{64}')
TERMS_VERSION = int(re.search(r'pub const SCHEMA_VERSION: i64 = (\d+);',
                              (Path(__file__).resolve().parents[1] / 'crates/core/src/schema.rs').read_text()).group(1))
RECORDING = bool(os.environ.get('ILLUSTASH_RECORD_BASELINE'))


class Live(dict):
    """实时的 ``library_digests``：摘要，以及按 #201 规则取的同一状态（``baseline``）。"""
    def __init__(self, digests, baseline):
        super().__init__(digests)
        self.baseline = baseline


def _meta_value(key, value, names_version):
    if key == 'terms_schema' and value == str(TERMS_VERSION):
        return '<terms-version>'
    if key == 'names_version':
        return '<names_version>'
    if key in ('names_projected', 'names_display_projected'):
        return '<names_version>' if value == names_version else '<stale>'
    if key == 'identity_graph_version':
        return '<derived>'
    return value


def _cell(value, rename):
    if isinstance(value, float):
        return {'real': struct.pack('>d', value).hex()}
    if isinstance(value, bytes):
        return {'blob': value.hex()}
    if isinstance(value, str):
        for old, new in rename.items():
            value = value.replace(old, new)
        return STAMP.sub('<stamp>', unclocked(value))
    return value


def _select(c, name, columns):
    physical = c.execute("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' "
                         "AND name='term_records')").fetchone()[0]
    moved = name == 'terms' and not physical and c.execute(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('term_tree') WHERE name='variant')").fetchone()[0]
    if not moved:
        return 'SELECT * FROM ' + quote(name)
    attrs = ('CASE WHEN r.term_id IS NULL THEN t.attrs ELSE json_object('
             + ','.join(f"'{a}',r.{a}" for a in ATTRIBUTES) + ') END')
    return ('SELECT ' + ','.join(attrs if col == 'attrs' else 't.' + quote(col) for col in columns)
            + ' FROM ' + quote(name) + ' t LEFT JOIN term_tree r ON r.term_id=t.id')


def _database_state(path, rename):
    with closing(sqlite3.connect(Path(path).resolve().as_uri() + '?mode=ro', uri=True)) as c:
        c.execute('BEGIN')
        names = [n for (n,) in c.execute("SELECT name FROM sqlite_schema WHERE type='table' "
                                         "AND name NOT LIKE 'sqlite_%' ORDER BY name")]
        for entry in _baseline_additions()['physical_views']:
            if entry['physical'] in names and c.execute(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='view' AND name=?)",
                    (entry['view'],)).fetchone()[0]:
                names.append(entry['view'])
        names_version = None
        if 'meta' in names:
            row = c.execute("SELECT value FROM meta WHERE key='names_version'").fetchone()
            names_version = row[0] if row else None
        out = {}
        for name in names:
            if name in RETIRED:
                continue
            columns = [r[1] for r in c.execute('PRAGMA table_info(' + quote(name) + ')')]
            keep = [i for i, col in enumerate(columns) if (name, col) not in SKIP_COLUMNS
                    or ('term_records' in names and name == 'term_tree' and col in ATTRIBUTES)]
            rows = []
            for raw in c.execute(_select(c, name, columns)):
                cells = [_cell(raw[i], rename) for i in keep]
                if name == 'meta' and isinstance(raw[0], str) and isinstance(raw[1], str):
                    cells[1] = _cell(_meta_value(raw[0], raw[1], names_version), rename)
                rows.append(json.dumps(cells, ensure_ascii=False, separators=(',', ':')))
            out[name] = {'columns': [columns[i] for i in keep], 'rows': sorted(rows)}
        return out


def baseline_state(package, rename=None):
    package = Path(package)
    return {name: _database_state(package / filename, rename or {}) for name, filename in
            (('app', 'library.sqlite'), ('source', 'source.sqlite'), ('provenance', 'provenance.sqlite'))
            if (package / filename).exists()}


@lru_cache(maxsize=1)
def _baseline_additions():
    """与 Rust 共用的精确新状态清单；只在历史参考比较中使用。"""
    additions = json.loads((Path(__file__).parent / 'baseline_additions.json').read_text())
    assert set(additions) == {'reference_terms_version', 'tables', 'meta_keys', 'physical_views'}
    assert additions['reference_terms_version'] == 7
    for entries in (additions['tables'], additions['meta_keys']):
        names = set()
        for entry in entries:
            assert set(entry) == {'name', 'introduced_in', 'checks'}
            assert re.fullmatch(r'[A-Za-z0-9_]+', entry['name']) and entry['name'] not in names
            assert entry['introduced_in'] > 7 and entry['checks']
            names.add(entry['name'])
    names = set()
    for entry in additions['physical_views']:
        assert set(entry) == {'physical', 'view', 'introduced_in', 'checks'}
        assert entry['introduced_in'] > 7 and entry['checks']
        for name in (entry['physical'], entry['view']):
            assert re.fullmatch(r'[A-Za-z0-9_]+', name) and name not in names
            names.add(name)
    return additions


def _historical_projection(state, frozen):
    """仅从实时 app 库的 v7 历史投影中分离具名新状态，不改变 baseline_state 的完整读数。"""
    app = state.get('app', {})
    meta = app.get('meta', {})
    version = next((cells[1] for row in meta.get('rows', [])
                    if (cells := json.loads(row))[0] == 'terms_schema'), None)
    if version is None:
        return state
    version = TERMS_VERSION if version == '<terms-version>' else int(version)
    additions = _baseline_additions()
    old = frozen.get('app', {})
    projected = dict(app)
    for entry in additions['physical_views']:
        if version >= entry['introduced_in']:
            raw, view = entry['physical'], entry['view']
            assert raw not in old, f'物理视图映射不能隐藏冻结侧已有表 {raw}'
            if raw not in app and view not in app:
                continue
            assert raw in app and view in app and app[raw] == app[view], (
                f'物理表 {raw} 与有效视图 {view} 的完整行不一致')
            del projected[raw]
            if view == 'terms':
                _historical_attributes(projected)
    for entry in additions['tables']:
        if version >= entry['introduced_in']:
            assert entry['name'] not in old, f"新增状态清单不能隐藏冻结侧已有表 {entry['name']}"
            projected.pop(entry['name'], None)
    keys = {entry['name'] for entry in additions['meta_keys'] if version >= entry['introduced_in']}
    for row in old.get('meta', {}).get('rows', []):
        assert json.loads(row)[0] not in keys, '新增状态清单不能隐藏冻结侧已有 meta 键'
    if 'meta' in projected:
        projected['meta'] = {**meta, 'rows': [row for row in meta['rows'] if json.loads(row)[0] not in keys]}
    return {**state, 'app': projected}


def _historical_attributes(projected):
    """当前原始属性保留；仅在旧参考投影里按第 8 版既定表达重建。"""
    tree = projected.get('term_tree')
    if not tree or 'variant' not in tree['columns']:
        return
    attributes = [tree['columns'].index(name) for name in ATTRIBUTES]
    term_id = tree['columns'].index('term_id')
    dump = lambda value: json.dumps(value, ensure_ascii=False, separators=(',', ':'))
    objects = {}
    for row in tree['rows']:
        cells = json.loads(row)
        objects[dump(cells[term_id])] = dump({name: cells[i] for name, i in zip(ATTRIBUTES, attributes)})
    terms = projected['terms']
    term_id, attrs = terms['columns'].index('id'), terms['columns'].index('attrs')
    rows = []
    for row in terms['rows']:
        cells = json.loads(row)
        if dump(cells[term_id]) in objects:
            cells[attrs] = objects[dump(cells[term_id])]
        rows.append(dump(cells))
    projected['terms'] = {**terms, 'rows': sorted(rows)}
    keep = [i for i in range(len(tree['columns'])) if i not in attributes]
    projected['term_tree'] = {'columns': [tree['columns'][i] for i in keep],
                              'rows': sorted(dump([json.loads(row)[i] for i in keep]) for row in tree['rows'])}


@lru_cache(maxsize=None)
def _baseline_file(module):
    path = BASELINE_STATES / f'{module}.json.gz'
    if not path.exists():
        return {}
    with gzip.open(path, 'rt', encoding='utf-8') as f:
        return json.load(f)


def _check_baseline(slot, state, only=None):
    """录制时为 ``slot``（module、test、key）录下 ``state``；否则返回 True，除非为它录过对照基准且两者不同
    （差异会打印出来）。"""
    module, test, key = slot
    if RECORDING:
        _record_baseline(module, test, key, state)
        return True
    want = _baseline_file(module).get(test, {}).get(key)
    if want is not None:
        want = {db: {t: v for t, v in tables.items() if t not in RETIRED} for db, tables in want.items()}
        state = _historical_projection(state, want)
    if only is not None:
        pick = lambda s: {db: {t: s.get(db, {}).get(t) for t in tables} for db, tables in only.items()}
        state, want = pick(state), want if want is None else pick(want)
    if want is not None:
        ignored_rows = RETIRED_ROWS
        if test == 'tests.test_native_local_reads.NativeLocalReadsTest.test_names_configuration_coverage_and_accounts':
            # #289：此合成包的旧密钥已改为随机引用。该测试单独断言原行消失、引用格式和 GET 尾号。
            ignored_rows |= {('name_settings', 'deepseek_key'), ('name_settings', 'model_credential')}
        want, state = (_without_retired_rows(_without_volatile(value), ignored_rows) for value in (want, state))
    if want == state:
        return True
    if want is None:
        print(f'#201 baseline {test} {key}: nothing recorded for this state', file=sys.stderr)
        return False
    for db in sorted(set(want) | set(state)):
        for table in sorted(set(want.get(db, {})) | set(state.get(db, {}))):
            w, g = want.get(db, {}).get(table), state.get(db, {}).get(table)
            if w != g:
                print(f'#201 baseline {test} {key} {db}.{table}: baseline {w!r:.400}, live {g!r:.400}', file=sys.stderr)
    return False


# 只有 Python 一侧有：宿主由来源标签构建的证据包带着构建时间，包的指纹对它做哈希（manifest 本身两边都不比较）。
# 核心的重放没有这样的包。
VOLATILE = frozenset({('term_packages', 'fingerprint')})


def _without_volatile(state):
    out = {}
    for db, tables in state.items():
        out[db] = {}
        for name, table in tables.items():
            drop = [i for i, column in enumerate(table['columns']) if (name, column) in VOLATILE]
            if not drop:
                out[db][name] = table
                continue
            keep = [i for i in range(len(table['columns'])) if i not in drop]
            rows = []
            for row in table['rows']:
                cells = json.loads(row)
                rows.append(json.dumps([cells[i] for i in keep], ensure_ascii=False, separators=(',', ':')))
            out[db][name] = {'columns': [table['columns'][i] for i in keep], 'rows': sorted(rows)}
    return out


# 宿主不再保留的设置行（第 5 条规则，每项写明原因），按键从两边去掉：
# - `name_settings.traffic_reserved`：名称来源的长期流量预算，按用户 2026-10-21 的决定（#287）删除；
#   之前的录制里仍有这一行。核心的重放从未走到它。
RETIRED_ROWS = frozenset({('name_settings', 'traffic_reserved')})


def _without_retired_rows(state, ignored_rows=RETIRED_ROWS):
    out = {}
    for db, tables in state.items():
        out[db] = {}
        for name, table in tables.items():
            keys = {key for table_name, key in ignored_rows if table_name == name}
            if keys:
                table = {**table, 'rows': [row for row in table['rows'] if json.loads(row)[0] not in keys]}
            out[db][name] = table
    return out


def _record_baseline(module, test, key, state):
    """加锁后合并进模块的文件：固定测试集在多个进程里运行测试类。"""
    import fcntl
    BASELINE_STATES.mkdir(exist_ok=True)
    path = BASELINE_STATES / f'{module}.json.gz'
    import tempfile
    with open(Path(tempfile.gettempdir()) / f'illustash-baseline-{module}.lock', 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        data = {}
        if path.exists():
            with gzip.open(path, 'rt', encoding='utf-8') as f:
                data = json.load(f)
        recorded = data.setdefault(test, {})
        if key in recorded and recorded[key] != state:
            raise AssertionError(f'#201 baseline {test} {key}: two different live states compared equal to it')
        recorded[key] = state
        with gzip.GzipFile(path, 'wb', mtime=0) as f:
            f.write(json.dumps(data, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode('utf-8'))
