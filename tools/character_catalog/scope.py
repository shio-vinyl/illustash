"""角色目录的范围：应用能自动认出的角色 tag（#834）。

四份来源都是我们自己发的包或仓库里的文件，不读用户图库：

- WD 原始词表 `tags/tags_v3.csv` 的角色类（category 4）；
- 基础角色索引 `character-index-v1.bin` 的 `characters.json`；
- WD 扩展头（`illustash-wd-extension/2` 第一行 JSON 的 `labels`）；
- CCIP 长尾锚点包的 `A-metadata.json`。

返回 `{tag: Entry}`，`Entry.sources` 记这个 tag 出现在哪几份来源里，基础索引自带的作品和张数一并带上。
"""
import csv
from dataclasses import dataclass, field
import json
from pathlib import Path
import zipfile

ROOT = Path(__file__).resolve().parents[2]
WD_TAGS = ROOT / 'tags/tags_v3.csv'

# 来源的简称，按应用里的入口顺序。
SOURCES = ('wd', 'index', 'extension', 'longtail')


@dataclass
class Entry:
    tag: str
    sources: set = field(default_factory=set)
    copyrights: list = field(default_factory=list)
    post_count: int = 0
    # 基础索引按 (short_tag, -post_count, tag) 去重时用的 short_tag；长尾的 id。
    short: str = ''


def wd_tags(path=WD_TAGS):
    with open(path, newline='', encoding='utf-8') as fh:
        return [(row['name'], int(row['count'])) for row in csv.DictReader(fh) if row['category'] == '4']


def index_characters(path):
    with zipfile.ZipFile(path) as archive:
        return json.loads(archive.read('characters.json'))


def extension_labels(path):
    with open(path, 'rb') as fh:
        header = fh.readline()
    manifest = json.loads(header)
    if manifest.get('format') not in ('illustash-wd-extension/1', 'illustash-wd-extension/2'):
        raise ValueError(f'不认识的扩展头格式：{manifest.get("format")}')
    return manifest['labels']


def longtail(path):
    with zipfile.ZipFile(path) as archive:
        return json.loads(archive.read('A-metadata.json'))


def load(index_path, extension_path, longtail_path, wd_path=WD_TAGS):
    scope = {}

    def add(tag, source):
        entry = scope.get(tag)
        if entry is None:
            entry = scope[tag] = Entry(tag)
        entry.sources.add(source)
        return entry

    for tag, count in wd_tags(wd_path):
        entry = add(tag, 'wd')
        entry.post_count = max(entry.post_count, count)
    for row in index_characters(index_path):
        entry = add(row['tag'], 'index')
        entry.copyrights = list(row.get('copyrights') or ([row['copyright']] if row.get('copyright') else []))
        entry.post_count = max(entry.post_count, int(row.get('post_count') or 0))
        entry.short = row.get('short_tag', '')
    for label in extension_labels(extension_path):
        add(label, 'extension')
    for row in longtail(longtail_path):
        entry = add(row['tag'], 'longtail')
        entry.post_count = max(entry.post_count, int(row.get('post_count') or 0))
        entry.short = row.get('id', '')
    return scope


def summary(scope):
    """各来源的 tag 数、两两重叠和并集。"""
    counts = {s: sum(1 for e in scope.values() if s in e.sources) for s in SOURCES}
    overlaps = {}
    for i, a in enumerate(SOURCES):
        for b in SOURCES[i + 1:]:
            overlaps[f'{a}&{b}'] = sum(1 for e in scope.values() if a in e.sources and b in e.sources)
    return {'sources': counts, 'overlaps': overlaps, 'union': len(scope)}
