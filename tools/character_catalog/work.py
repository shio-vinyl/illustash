"""制作端的工作目录（#834）：输入包的位置和各来源的缓存。

工作目录里放一份 `inputs.json`，写明三个包文件的位置（WD 原始词表用仓库里的 `tags/tags_v3.csv`）：

    {"index": "…/character-index-v1.bin", "extension": "…/wd-extension-0.5.bin", "longtail": "…/ccip-longtail-0.3.zip"}

缓存放在工作目录的 `cache/` 下：`danbooru/`（tools.character_catalog.danbooru 抓的 JSONL）、`zh-tags/`（固定提交的
amenorira/danbooru-tags-data-zh）、`wikidata/`（P13236 的 SPARQL 结果）。工作目录不进仓库。
"""
import argparse
from dataclasses import dataclass
import json
from pathlib import Path

from tools.character_catalog import danbooru, scope as scope_mod


@dataclass
class Work:
    root: Path
    scope: dict

    @property
    def cache(self):
        return self.root / 'cache'

    def danbooru(self, kind):
        return danbooru.rows(self.cache / 'danbooru', kind)


def open_work(root):
    root = Path(root).expanduser()
    inputs = json.loads((root / 'inputs.json').read_text(encoding='utf-8'))
    return Work(root, scope_mod.load(inputs['index'], inputs['extension'], inputs['longtail']))


def parser(description):
    p = argparse.ArgumentParser(description=description)
    p.add_argument('work', type=Path, help='工作目录（含 inputs.json 和 cache/）')
    return p
