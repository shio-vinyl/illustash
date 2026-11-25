"""角色资料包的签名目录（#602；步骤见 docs/DEVELOPMENT.md「发布角色资料包」）。

    python3 tools/pack_catalog.py keygen ~/.tauri/illustash-packs.key
    python3 tools/pack_catalog.py entry --version 0.2 --stage preview --revision 2 --wd wd-extension-v2.bin \\
        --ccip A-longtail-p75.zip --ccip-tag ccip-longtail-v1 \\
        --min-app 0.3.0 --note-zh '…' --note-en '…' --output character-pack-0.2.entry.json
    python3 tools/pack_catalog.py build --previous packs.json --entry character-pack-0.2.entry.json --output packs.json
    python3 tools/pack_catalog.py sign --key ~/.tauri/illustash-packs.key packs.json
    python3 tools/pack_catalog.py verify packs.json
    python3 tools/pack_catalog.py offline --catalog packs.json --file wd-extension-v2.bin \\
        --file A-longtail-p75.zip --output character-pack-0.2-offline.zip

版本名（`--version`，如 0.2、1.0、1.1）只用来显示和给 Release 打 tag（`character-pack-<版本名>`）；应用比新旧只看修订号
（`--revision`，每次加一的整数）。测试版加 `--stage preview`，卡片上版本号后面写「（测试版）」，行为和正式版一样。

一个角色资料包有两个组成部分：WD 扩展头（`--wd`）和 CCIP 长尾锚点（`--ccip`），各自从包文件里读格式、头数、二级头数
和基础模型的 SHA-256。组成部分的下载地址默认在这个版本自己的 Release（tag 就是版本名）下；某个组成部分的文件和上一版
相同时，用 `--wd-tag`、`--ccip-tag` 指向原来的 Release，不用重复上传。

`keygen` 和 `sign` 调用 `cargo tauri signer`（和应用更新同一种 minisign 签名，另一把专签数据包的密钥）。私钥只在
用户自己的电脑上，带密码；这个脚本从不读私钥，也不把它传给别的东西。`keygen` 把公钥写进
native/terms/pack-key.pub，之后要重新构建应用才生效。

`entry` 读包文件本身，写出一条目录条目（格式、头数、二级头数、基础模型的 SHA-256 都从包里读，不手填）。`build` 把条目
并进上一份目录、序号加一。`verify` 用 pack-key.pub 核对签名并按应用的规则检查每个字段；装了 `cryptography` 时完整
核对签名，没装时只核对密钥编号并提示。`offline` 把目录、签名和包文件打成离线包，用户在卡片上「从文件导入…」选它。
"""
import argparse
import base64
import binascii
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
PUBKEY_FILE = ROOT / 'native/terms/pack-key.pub'
CATALOG_FORMAT = 'illustash-packs/1'
RELEASE_PREFIX = 'https://github.com/shio-vinyl/illustash/releases/download/'
CATALOG_NAME = 'packs.json'
SIGNATURE_NAME = 'packs.json.sig'
PACK_KIND = 'character-pack'
CHARACTER_KIND = 'character-catalog'
CHARACTER_FORMAT = 'illustash-character-catalog/1'
CHARACTER_LIMIT = 16 << 20
# 目录条目的 `stage`：不写或 release 是正式版，preview 是测试版。
STAGES = ('preview', 'release')
# 两个组成部分各自认的包格式。
FORMATS = {
    'wd': ('illustash-wd-extension/1', 'illustash-wd-extension/2'),
    'ccip': ('illustash-557-ccip-offline/1',),
}
WD_DIM = 1024
WD_SHARD_HEADS = 1024
WD_COEFFICIENT_BYTES = (WD_DIM + 1) * 4
SHARD_LIMIT = 128
CCIP_DIM = 768
SEGMENT = re.compile(r'^[A-Za-z0-9][A-Za-z0-9._-]*$')
VERSION = re.compile(r'^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z.-]+)?$')
ENTRY_LIMIT = 64
FILE_LIMIT = 512 << 20


class CatalogError(Exception):
    pass


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def hex64(text):
    return isinstance(text, str) and re.fullmatch(r'[0-9a-f]{64}', text) is not None


def segment(text, limit):
    return isinstance(text, str) and 0 < len(text) <= limit and SEGMENT.match(text) is not None and '..' not in text


def short(text, limit):
    return isinstance(text, str) and text.strip() != '' and len(text) <= limit and not any(c < ' ' for c in text)


# ---------- 读包文件 ----------

def inspect_wd(data):
    """WD 扩展包：第一行是清单，后面是 heads × (1024 + 1) 个 float32。"""
    end = data.find(b'\n')
    if end < 0:
        raise CatalogError('不是 WD 扩展包：缺少清单行')
    manifest = json.loads(data[:end])
    labels = manifest.get('labels') or []
    fmt = manifest.get('format')
    if fmt not in FORMATS['wd']:
        raise CatalogError(f'WD 扩展包格式 {fmt} 不是应用认识的')
    if len(data) - end - 1 != len(labels) * (WD_DIM + 1) * 4:
        raise CatalogError('WD 扩展包的系数长度和头数对不上')
    tiers = manifest.get('tiers')
    if fmt == 'illustash-wd-extension/1' and tiers is not None:
        raise CatalogError('旧格式的包不能带级别；分级的包用 illustash-wd-extension/2')
    if fmt == 'illustash-wd-extension/2' and (tiers is None or manifest.get('thresholds') is None):
        raise CatalogError('illustash-wd-extension/2 的包必须带逐头的 thresholds 和 tiers')
    for key in ('thresholds', 'tiers'):
        if manifest.get(key) is not None and len(manifest[key]) != len(labels):
            raise CatalogError(f'{key} 的个数和头数对不上')
    if tiers is not None and any(t not in (1, 2) for t in tiers):
        raise CatalogError('级别只能是 1 或 2')
    return {
        'format': fmt,
        'heads': len(labels),
        'candidate_only': sum(1 for t in tiers or [] if t == 2),
        'base': {'wd_model_sha256': manifest['wd_model_sha256'], 'wd_tags_sha256': manifest['wd_tags_sha256']},
    }


def inspect_ccip(path):
    """CCIP 长尾参考包：ZIP 里的溯源、元数据和锚点。"""
    with zipfile.ZipFile(path) as archive:
        provenance = json.loads(archive.read('A-provenance.json'))
        metadata = json.loads(archive.read('A-metadata.json'))
        anchors = archive.read('A-anchors.f32')
    if provenance.get('format') not in FORMATS['ccip']:
        raise CatalogError(f'参考包格式 {provenance.get("format")} 不是应用认识的')
    if len(anchors) != len(metadata) * CCIP_DIM * 4:
        raise CatalogError('参考包的锚点个数和角色数对不上')
    return {
        'format': provenance['format'],
        'heads': len(metadata),
        'candidate_only': 0,
        'base': {'ccip_model_sha256': provenance['ccip_sha256']},
    }


def component(kind, path, tag, name=None, license_text=None):
    """一个组成部分：从包文件读字段。文件名默认就是文件现在的名字，下载地址是 `<tag>` 这个 Release 下的它。"""
    path = Path(path)
    data = path.read_bytes()
    info = inspect_wd(data) if kind == 'wd' else inspect_ccip(path)
    name = name or path.name
    return {
        'format': info['format'],
        'heads': info['heads'],
        'candidate_only': info['candidate_only'],
        'file': {'name': name, 'size': len(data), 'sha256': sha256(data), 'url': f'{RELEASE_PREFIX}{tag}/{name}'},
        'base': info['base'],
        'license': license_text or ('Apache-2.0' if kind == 'wd' else 'MIT'),
    }


def shard_files(shards):
    return [shards.get('header'), *shards['coefficients']] if shards else []


def write_wd_shards(data, heads, output, tag, previous):
    """保留完整包字节，分片由 JSON 头和各批次的权重、偏置构成。已有片沿用签名目录里的地址。"""
    known = {}
    for pack in (previous or {}).get('packs', []):
        wd = (pack.get('components') or {}).get('wd') or {}
        known.update((f['sha256'], f) for f in shard_files(wd.get('shards')))
    output.mkdir(parents=True, exist_ok=True)

    def write(body):
        digest = sha256(body)
        if digest in known:
            return known[digest]
        name = f'wd-shard-{digest}.bin'
        spec = {'name': name, 'size': len(body), 'sha256': digest, 'url': f'{RELEASE_PREFIX}{tag}/{name}'}
        (output / name).write_bytes(body)
        known[digest] = spec
        return spec

    header_end = data.index(b'\n') + 1
    bias_offset = header_end + heads * WD_DIM * 4
    header = write(data[:header_end])
    coefficients = []
    for start in range(0, heads, WD_SHARD_HEADS):
        end = min(start + WD_SHARD_HEADS, heads)
        weights = data[header_end + start * WD_DIM * 4:header_end + end * WD_DIM * 4]
        bias = data[bias_offset + start * 4:bias_offset + end * 4]
        coefficients.append(write(weights + bias))
    return {'header': header, 'coefficients': coefficients}


def entry(*, version, revision, wd, ccip, min_app, stage=None, license_text=None, note_zh=None, note_en=None,
          wd_tag=None, ccip_tag=None, wd_name=None, ccip_name=None, wd_shards=None, previous=None):
    """从两个组成部分的包文件写出一条目录条目。两个组成部分的 Release 默认就是这个版本的（tag 是
    `character-pack-<版本名>`）。`stage` 为 `preview` 时是测试版，不写就是正式版。"""
    tag = f'character-pack-{version}'
    result = {
        'kind': PACK_KIND,
        'revision': revision,
        'version': version,
        'min_app': min_app,
        'license': license_text or 'Apache-2.0 AND MIT',
        'components': {
            'wd': component('wd', wd, wd_tag or tag, wd_name),
            'ccip': component('ccip', ccip, ccip_tag or tag, ccip_name),
        },
    }
    if stage:
        result['stage'] = stage
    if note_zh or note_en:
        if not (note_zh and note_en):
            raise CatalogError('说明要中英文都写')
        result['note'] = {'zh-CN': note_zh, 'en': note_en}
    if wd_shards is not None:
        if previous is not None:
            check_catalog(previous)
        wd_component = result['components']['wd']
        wd_component['shards'] = write_wd_shards(Path(wd).read_bytes(), wd_component['heads'], Path(wd_shards), tag, previous)
    check_entry(result)
    return result


# ---------- 目录的规则（和 native/terms/src/app_packs.rs 的 Catalog::parse 一致）----------

def valid_file(file):
    url = file.get('url', '')
    rest = url[len(RELEASE_PREFIX):] if isinstance(url, str) and url.startswith(RELEASE_PREFIX) else ''
    tag, _, tail = rest.partition('/')
    size = file.get('size')
    return (segment(file.get('name'), 128) and isinstance(size, int) and 1 <= size <= FILE_LIMIT
            and hex64(file.get('sha256')) and segment(tag, 64) and tail == file.get('name'))


def check_component(key, c, version):
    f = c.get('file') or {}
    ok = (isinstance(c.get('format'), str) and 0 < len(c['format']) <= 64
          and isinstance(c.get('heads'), int) and 1 <= c['heads'] <= 1_000_000
          and isinstance(c.get('candidate_only', 0), int) and 0 <= c.get('candidate_only', 0) <= c['heads']
          and valid_file(f) and short(c.get('license'), 200))
    if not ok:
        raise CatalogError(f'条目 {version} 的组成部分 {key} 不合法')
    shards = c.get('shards')
    if shards is not None:
        coefficients = shards.get('coefficients') if isinstance(shards, dict) else None
        valid = (key == 'wd' and isinstance(coefficients, list) and 1 <= len(coefficients) <= SHARD_LIMIT
                 and all(isinstance(part, dict) and valid_file(part)
                         and part['name'] == f"wd-shard-{part['sha256']}.bin" for part in shard_files(shards))
                 and all(part['size'] % WD_COEFFICIENT_BYTES == 0 for part in coefficients)
                 and sum(part['size'] for part in coefficients) == c['heads'] * WD_COEFFICIENT_BYTES
                 and sum(part['size'] for part in shard_files(shards)) == f['size'])
        if not valid:
            raise CatalogError(f'条目 {version} 的 WD 分片不合法')
    base = c.get('base') or {}
    if key == 'wd' and not (hex64(base.get('wd_model_sha256')) and hex64(base.get('wd_tags_sha256'))):
        raise CatalogError(f'条目 {version} 缺少 WD 模型或标签表的 SHA-256')
    if key == 'ccip' and not hex64(base.get('ccip_model_sha256')):
        raise CatalogError(f'条目 {version} 缺少 CCIP 模型的 SHA-256')


def character_entry(path, min_app, license_text):
    """为独立的角色词表生成签名清单条目，修订号取自文件。"""
    data = Path(path).read_bytes()
    catalog = json.loads(data)
    revision = catalog['revision']
    version = f'catalog-{revision}'
    name = Path(path).name
    entry = {'kind': CHARACTER_KIND, 'version': version, 'revision': revision, 'min_app': min_app,
             'license': license_text, 'format': catalog['format'],
             'file': {'name': name, 'size': len(data), 'sha256': sha256(data),
                      'url': f'{RELEASE_PREFIX}character-{version}/{name}'}}
    check_entry(entry)
    return entry


def check_entry(e, strict=True):
    """结构的规则和应用一致（不合法时应用整份目录不用）。`strict` 另外拒绝应用会跳过的条目（不认识的种类或格式、
    旧格式带二级头）：发布时不该写进去，测试夹具才用。"""
    ok = (segment(e.get('kind'), 32) and segment(e.get('version'), 64)
          and isinstance(e.get('revision'), int) and e['revision'] >= 1
          and isinstance(e.get('min_app'), str) and VERSION.match(e['min_app'])
          and short(e.get('license'), 200)
          and ('stage' not in e or segment(e['stage'], 32))
          and ('note' not in e or (short(e['note'].get('zh-CN'), 200) and short(e['note'].get('en'), 200))))
    if not ok:
        raise CatalogError(f'条目 {e.get("version")} 的字段不合法')
    parts = e.get('components')
    if e['kind'] == PACK_KIND:
        if not isinstance(parts, dict) or set(parts) < {'wd', 'ccip'}:
            raise CatalogError(f'条目 {e["version"]} 缺少组成部分')
        for key in ('wd', 'ccip'):
            check_component(key, parts[key], e['version'])
    if e['kind'] == CHARACTER_KIND:
        file = e.get('file')
        if ('components' in e or not e['version'].startswith('catalog-') or not isinstance(file, dict)
                or not valid_file(file) or file['size'] > CHARACTER_LIMIT
                or (strict and (e.get('format') != CHARACTER_FORMAT or e.get('stage', 'release') not in STAGES))):
            raise CatalogError(f'条目 {e["version"]} 的角色词表字段不合法')
        return
    if not strict:
        return
    if e['kind'] != PACK_KIND:
        raise CatalogError(f'条目 {e["version"]} 的种类 {e["kind"]} 不是应用认识的')
    if e.get('stage', 'release') not in STAGES:
        raise CatalogError(f'条目 {e["version"]} 的标记 {e["stage"]} 不是应用认识的')
    for key in ('wd', 'ccip'):
        c = parts[key]
        if c['format'] not in FORMATS[key]:
            raise CatalogError(f'条目 {e["version"]} 的 {key} 格式 {c["format"]} 不是应用认识的')
        if c.get('candidate_only', 0) and (key == 'ccip' or c['format'] == 'illustash-wd-extension/1'):
            raise CatalogError(f'条目 {e["version"]}：这种格式的 {key} 部分全部是一级')


def check_catalog(catalog, strict=True):
    if catalog.get('format') != CATALOG_FORMAT:
        raise CatalogError('目录格式不对')
    if not isinstance(catalog.get('serial'), int) or catalog['serial'] < 1:
        raise CatalogError('序号须为正整数')
    packs = catalog.get('packs')
    if not isinstance(packs, list) or len(packs) > ENTRY_LIMIT:
        raise CatalogError('条目列表不对')
    seen = set()
    for e in packs:
        check_entry(e, strict)
        keys = {('version', e['version']), ('revision', e['kind'], e['revision'])}
        if keys & seen:
            raise CatalogError(f'条目 {e["version"]} 的版本或修订号重复')
        seen |= keys


def build(entries, previous=None, strict=True):
    """把条目并进上一份目录，序号加一。已有版本只能更新说明等元数据。"""
    packs = list(previous['packs']) if previous else []
    for e in entries:
        known = next((p for p in packs if p['version'] == e['version']), None)
        if known:
            def files(pack):
                if pack['kind'] == CHARACTER_KIND:
                    return [pack['file']['sha256']]
                return [(part['file']['sha256'], tuple(f['sha256'] for f in shard_files(part.get('shards'))))
                        for _, part in sorted((pack.get('components') or {}).items())]

            if known['kind'] != e['kind'] or files(known) != files(e) or known['revision'] != e['revision']:
                raise CatalogError(f'已发布的版本 {e["version"]} 不能替换文件或修订号')
        packs = [p for p in packs if p['version'] != e['version']] + [e]
    packs.sort(key=lambda p: (p['kind'], p['revision']))
    catalog = {'format': CATALOG_FORMAT, 'serial': (previous['serial'] + 1) if previous else 1, 'packs': packs}
    check_catalog(catalog, strict)
    return catalog


def dumps(catalog):
    return (json.dumps(catalog, ensure_ascii=False, indent=2) + '\n').encode('utf-8')


# ---------- 签名 ----------

def _decoded_lines(text, what):
    """Tauri 的公钥和签名都是 minisign 文本整体再 base64 一次。"""
    try:
        return base64.b64decode(text.strip(), validate=True).decode('utf-8').splitlines()
    except (binascii.Error, UnicodeDecodeError) as error:
        raise CatalogError(f'{what}不是 Tauri 写的格式') from error


def configured_pubkey(path=PUBKEY_FILE):
    lines = [line.strip() for line in Path(path).read_text(encoding='utf-8').splitlines()]
    keys = [line for line in lines if line and not line.startswith('#')]
    return keys[0] if keys else None


def parse_pubkey(text):
    lines = _decoded_lines(text, '公钥')
    raw = base64.b64decode(lines[1]) if len(lines) >= 2 else b''
    if len(raw) != 42 or raw[:2] not in (b'Ed', b'ED'):
        raise CatalogError('公钥不是 minisign 格式')
    return raw[2:10], raw[10:]


def parse_signature(text):
    lines = _decoded_lines(text, '签名')
    if len(lines) < 4 or not lines[2].startswith('trusted comment: '):
        raise CatalogError('签名不是 minisign 格式')
    sig = base64.b64decode(lines[1])
    glob = base64.b64decode(lines[3])
    if len(sig) != 74 or len(glob) != 64 or sig[:2] != b'ED':
        raise CatalogError('签名不是预哈希的 minisign 格式（cargo tauri signer 写的）')
    return sig[2:10], sig[10:], lines[2][len('trusted comment: '):], glob


def verify(data, signature_text, pubkey_text):
    """返回 True：签名完整核对通过；None：没装 cryptography，只核对了密钥编号。对不上时抛出异常。"""
    key_id, key = parse_pubkey(pubkey_text)
    sig_id, sig, trusted, glob = parse_signature(signature_text)
    if sig_id != key_id:
        raise CatalogError('签名不是用 native/terms/pack-key.pub 那把公钥对应的私钥签的')
    try:
        from cryptography.exceptions import InvalidSignature
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    except ImportError:
        return None
    public = Ed25519PublicKey.from_public_bytes(key)
    try:
        public.verify(sig, hashlib.blake2b(data, digest_size=64).digest())
        public.verify(glob, sig + trusted.encode('utf-8'))
    except InvalidSignature as error:
        raise CatalogError('签名对不上这份目录') from error
    return True


def check_signed(catalog_path, signature_path=None, pubkey=None):
    catalog_path = Path(catalog_path)
    signature_path = Path(signature_path or str(catalog_path) + '.sig')
    pubkey = pubkey or configured_pubkey()
    if not pubkey:
        raise CatalogError('native/terms/pack-key.pub 还是占位，先运行 keygen')
    data = catalog_path.read_bytes()
    check_catalog(json.loads(data))
    return verify(data, signature_path.read_text(encoding='utf-8'), pubkey)


def offline(catalog_path, package_paths, output, signature_path=None, extra=()):
    """离线包：根上是 packs.json、packs.json.sig 和包文件（名字照目录里的），再加 `extra` 里的许可文本等。应用导入时
    把认不出的成员列为不认识，不影响包文件。"""
    catalog_path = Path(catalog_path)
    signature_path = Path(signature_path or str(catalog_path) + '.sig')
    catalog = json.loads(catalog_path.read_bytes())
    files = {}
    for p in catalog['packs']:
        for c in (p.get('components') or {}).values():
            files[c['file']['sha256']] = c['file']['name']
    packages = []
    for path in map(Path, [package_paths] if isinstance(package_paths, (str, Path)) else package_paths):
        data = path.read_bytes()
        if sha256(data) not in files:
            raise CatalogError(f'{path.name} 不在这份目录里')
        packages.append((files[sha256(data)], data))
    with zipfile.ZipFile(output, 'w', zipfile.ZIP_DEFLATED) as archive:
        members = [(CATALOG_NAME, catalog_path.read_bytes()), (SIGNATURE_NAME, signature_path.read_bytes())] + packages \
            + [(Path(path).name, Path(path).read_bytes()) for path in extra]
        if len({name for name, _ in members}) != len(members):
            raise CatalogError('离线包里的文件重名')
        for name, body in members:
            archive.writestr(zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0)), body)


def write_pubkey(pubkey, path=PUBKEY_FILE):
    parse_pubkey(pubkey)
    lines = [line for line in Path(path).read_text(encoding='utf-8').splitlines() if line.startswith('#')]
    lines = [line for line in lines if '占位' not in line and '只有说明行' not in line]
    Path(path).write_text('\n'.join(lines + [pubkey.strip()]) + '\n', encoding='utf-8')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest='command', required=True)
    keygen = sub.add_parser('keygen', help='生成专签数据包的密钥对，公钥写进 native/terms/pack-key.pub')
    keygen.add_argument('key', type=Path, help='私钥写到哪里（只留在本机，例如 ~/.tauri/illustash-packs.key）')
    make = sub.add_parser('entry', help='从两个组成部分的包文件写出一条目录条目')
    make.add_argument('--version', required=True, help='版本名，如 0.2、1.0；Release 的 tag 是 character-pack-<版本名>')
    make.add_argument('--revision', required=True, type=int, help='比新旧用的整数，每次发布加一')
    make.add_argument('--stage', choices=STAGES, help='测试版写 preview；不写就是正式版')
    make.add_argument('--wd', required=True, type=Path, help='WD 扩展头的包文件')
    make.add_argument('--ccip', required=True, type=Path, help='CCIP 长尾锚点的参考包（ZIP）')
    make.add_argument('--min-app', required=True)
    make.add_argument('--license', default=None, help='整个包的许可，默认 Apache-2.0 AND MIT')
    make.add_argument('--note-zh')
    make.add_argument('--note-en')
    make.add_argument('--wd-tag', help='WD 部分所在的 Release，默认就是这个版本的')
    make.add_argument('--ccip-tag', help='CCIP 部分所在的 Release，默认就是这个版本的')
    make.add_argument('--wd-shards', type=Path, help='输出新的 WD 分片到指定目录')
    make.add_argument('--previous', type=Path, help='上一份目录，沿用其中已有分片的下载地址')
    make.add_argument('--wd-name')
    make.add_argument('--ccip-name')
    make.add_argument('--output', type=Path, required=True)
    character = sub.add_parser('character-entry', help='从角色词表文件生成独立条目')
    character.add_argument('file', type=Path)
    character.add_argument('--min-app', default='0.4.2')
    character.add_argument('--license', required=True)
    character.add_argument('--output', type=Path, required=True)
    put = sub.add_parser('build', help='把条目并进目录，序号加一')
    put.add_argument('--previous', type=Path, help='上一份目录（第一次发布时省略）')
    put.add_argument('--entry', type=Path, action='append', required=True)
    put.add_argument('--output', type=Path, required=True)
    sign = sub.add_parser('sign', help='用 cargo tauri signer 签名并核对')
    sign.add_argument('--key', type=Path, required=True)
    sign.add_argument('catalog', type=Path)
    check = sub.add_parser('verify', help='核对目录的签名和字段')
    check.add_argument('catalog', type=Path)
    check.add_argument('--signature', type=Path)
    check.add_argument('--pubkey', help='默认读 native/terms/pack-key.pub')
    bundle = sub.add_parser('offline', help='把目录、签名和包文件打成离线包')
    bundle.add_argument('--catalog', type=Path, required=True)
    bundle.add_argument('--file', type=Path, action='append', required=True, help='包文件，可重复')
    bundle.add_argument('--output', type=Path, required=True)
    bundle.add_argument('--extra', type=Path, action='append', default=[], help='一起放进去的许可文本等，可重复')
    args = parser.parse_args(argv)
    try:
        if args.command == 'keygen':
            if args.key.exists():
                raise CatalogError(f'{args.key} 已经存在；换个位置，或确认不再需要它之后自己删掉')
            # 交互地问密码；私钥由 tauri 写到用户指定的位置，这里不读它。
            subprocess.run(['cargo', 'tauri', 'signer', 'generate', '-w', str(args.key)], check=True,
                           cwd=ROOT / 'desktop')
            pubkey = Path(str(args.key) + '.pub').read_text(encoding='utf-8').strip()
            write_pubkey(pubkey)
            print(f'公钥已写进 {PUBKEY_FILE.relative_to(ROOT)}；私钥在 {args.key}，只留在这台电脑上。')
        elif args.command == 'entry':
            result = entry(version=args.version, revision=args.revision, stage=args.stage, wd=args.wd, ccip=args.ccip,
                           min_app=args.min_app, license_text=args.license, note_zh=args.note_zh, note_en=args.note_en,
                           wd_tag=args.wd_tag, ccip_tag=args.ccip_tag, wd_name=args.wd_name, ccip_name=args.ccip_name,
                           wd_shards=args.wd_shards, previous=json.loads(args.previous.read_bytes()) if args.previous else None)
            args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
            parts = result['components']
            print(f'已写出 {args.output}：{result["version"]}，WD {parts["wd"]["heads"]} 个头、CCIP {parts["ccip"]["heads"]} 个角色')
        elif args.command == 'character-entry':
            result = character_entry(args.file, args.min_app, args.license)
            args.output.write_bytes(dumps(result))
        elif args.command == 'build':
            previous = json.loads(args.previous.read_bytes()) if args.previous else None
            entries = [json.loads(path.read_bytes()) for path in args.entry]
            catalog = build(entries, previous)
            args.output.write_bytes(dumps(catalog))
            print(f'已写出 {args.output}：序号 {catalog["serial"]}，{len(catalog["packs"])} 个条目')
        elif args.command == 'sign':
            check_catalog(json.loads(args.catalog.read_bytes()))
            subprocess.run(['cargo', 'tauri', 'signer', 'sign', '-f', str(args.key.resolve()),
                            str(args.catalog.resolve())], check=True, cwd=ROOT / 'desktop')
            result = check_signed(args.catalog)
            print(f'已写出 {args.catalog}.sig' + ('，签名核对通过' if result else '；没装 cryptography，只核对了密钥编号'))
        elif args.command == 'verify':
            result = check_signed(args.catalog, args.signature, args.pubkey)
            print('目录和签名核对通过' if result else '目录核对通过；没装 cryptography，签名只核对了密钥编号')
        else:
            if not check_signed(args.catalog):
                print('提示：没装 cryptography，签名只核对了密钥编号', file=sys.stderr)
            offline(args.catalog, args.file, args.output, extra=args.extra)
            print(f'已写出 {args.output}')
        return 0
    except (CatalogError, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f'pack_catalog: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
