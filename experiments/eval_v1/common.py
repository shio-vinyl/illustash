"""共用评测集 v1（#551、#557、#456 共用）的冻结来源和输出目录。

只读主检出 .worktrees 下的历史实验工作树，不在那里写文件。产物写到本工作树的
_scratch/eval-v1（或 EVAL_V1_OUTPUT）。
"""

import hashlib
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def _main_checkout():
    if 'EVAL_V1_MAIN' in os.environ:
        return Path(os.environ['EVAL_V1_MAIN'])
    for parent in ROOT.parents:
        if (parent / '.worktrees/issue-551-wd-b-expansion').is_dir():
            return parent
    raise FileNotFoundError('找不到含 .worktrees/issue-551-wd-b-expansion 的主检出，设 EVAL_V1_MAIN')


MAIN = _main_checkout()
WT = MAIN / '.worktrees'
DATA = Path(os.environ.get('EVAL_V1_OUTPUT', ROOT / '_scratch/eval-v1')).resolve()

TREE_380 = WT / 'issue-380-character-coverage'
TREE_534 = WT / 'issue-534-character-expansion'
TREE_551 = WT / 'issue-551-wd-b-expansion'
TREE_557 = WT / 'issue-557-ccip-longtail'
TREE_456 = WT / 'issue-456-crop-pilot'

D380 = TREE_380 / '_scratch/issue-380'
D534 = TREE_534 / '_scratch/issue-534'
D551 = TREE_551 / '_scratch/issue-551'
D557 = TREE_557 / '_scratch/issue-557'
D456 = TREE_456 / '_scratch/issue-456'
COVERAGE = D380 / 'coverage'
PARQUET = D380 / 'posts.parquet'
TAGS = TREE_551 / 'tags/tags_v3.csv'
CCIP_INDEX = D534 / 'index'

PACK_2100 = D551 / 'fit/b-551-keep-228.bin'
PACK_2100_SHA = 'dfcda0c99fcc17e9a8caee7931687fef6ba28c39ad82875ef7522d6e92462a09'
PACK_257 = D551 / 'expansion/blind/b-551-expansion-passed.bin'
PACK_257_SHA = 'f9157a71244607a81f751b5cfbd0e40b6ccbdb1e901bfe2a883ceed7e4a193ff'
PACK_235_SHA = 'a81d2bc3358c3a75f1b52594a9e65c33b41162a18fe033f11b0d339e1e6478ca'
HEADS_534 = D534 / 'evaluation/B-heads.npz'
A557 = D557 / 'A-longtail-p75.zip'
A557_SHA = 'd36eeab8768e30cc36e2ba5b2788f00a5f608ecad0df264a2830a44c14407fb9'
INDEX_SHA = '987f659809a320b63c14aac94f4804133349ddf7f0639a5ed9df9f10b4280f33'

WD_LAST_ID = 7_220_105
START = '2025-09-26'          # 晚于完整精选索引冻结日，与 #551/#557 历次盲测一致
STOP = '2026-11-17'
SNAPSHOT_END = '2026-07-13'   # #380 公开帖快照的截止日；之后的帖子经实时 API 补
SEED = 'illustash-eval-v1'

# 复用冻结实验的模块：#551 的头包读写和回传校验，#534 的抽样与去重，#380 的 API 和别名。
sys.path[1:1] = [str(TREE_551 / 'experiments/issue_551'), str(TREE_551 / 'experiments/issue_534'),
                str(TREE_551 / 'experiments/issue_380')]


def sha_file(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def load(path):
    return json.loads(Path(path).read_bytes())


def write(path, data):
    path = DATA / path if not Path(path).is_absolute() else Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + '.partial')
    temporary.write_text(json.dumps(data, ensure_ascii=False, separators=(',', ':')) + '\n')
    temporary.replace(path)
    return path


def priority(key, seed=SEED):
    return int.from_bytes(hashlib.sha256(f'{seed}:{key}'.encode()).digest()[:8], 'big')


def split_557(artist):
    """#557 的画师分区（种子 illustash-557-v1）：train 60%、calibration 18%、test 22%。

    长尾角色的评测正例只取 test 分区的画师，参考图和校准只用另外两区，互不相交。
    """
    slot = int.from_bytes(hashlib.sha256(f'illustash-557-v1:{artist}'.encode()).digest()[:8], 'big') % 100
    return 'train' if slot < 60 else 'calibration' if slot < 78 else 'test'


def post_band(posts):
    return '50-99' if posts < 100 else '100-499' if posts < 500 else '500+'
