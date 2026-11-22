"""读 Bangumi 官方整库导出（github.com/bangumi/Archive，#832 制作端）。

导出是 zip 里的几份 JSON Lines：`character`（角色名和原始 wiki 信息框）、`subject`（作品名、中文名、类型）、
`subject-characters`（作品和角色的对应）、`person-relations`（角色之间的关系，`person_type == 'crt'`）。

导出属于外部数据，按不可信输入处理：只用标准库读，不执行里面的任何东西。
用 `python3 -I tools/character_catalog/bangumi.py <zip>` 单独运行时打印概况。
"""
import collections
import json
from pathlib import Path
import re
import sys
import unicodedata
import zipfile

# 用 -I 运行时脚本目录不在 sys.path 里，按文件位置找仓库根。
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from tools.character_catalog import zhtext  # noqa: E402

FIELD = re.compile(r'^\|\s*([^=|]+?)\s*=\s*(.*)$')
ITEM = re.compile(r'^\[(.*)\]$')
SEPARATORS = '・·•･=＝ 　_-‐'


def norm(text):
    """比较用：NFKC、去空白和间隔号、小写。"""
    s = unicodedata.normalize('NFKC', text or '')
    return ''.join(c for c in s if c not in SEPARATORS and not c.isspace()).casefold()


def infobox(text):
    """Bangumi wiki 信息框：`|键= 值`，多值写成 `|键={` 换行若干 `[标签|值]` 再 `}`。返回 {键: 值或[(标签, 值)]}。"""
    out, key, items = {}, None, None
    for line in (text or '').splitlines():
        s = line.strip()
        if items is not None:
            if s == '}':
                out[key] = items
                items = None
                continue
            m = ITEM.match(s)
            if m:
                label, _, value = m.group(1).partition('|') if '|' in m.group(1) else ('', '', m.group(1))
                if value.strip():
                    items.append((label.strip(), value.strip()))
            continue
        m = FIELD.match(s)
        if m:
            key, value = m.group(1).strip(), m.group(2).strip()
            if value == '{':
                items = []
            else:
                out[key] = value
    return out


def lines(archive, name):
    member = next((n for n in archive.namelist() if n.endswith(name)), None)
    if member is None:
        raise FileNotFoundError(name)
    with archive.open(member) as fh:
        for raw in fh:
            raw = raw.strip()
            if raw:
                yield json.loads(raw)


def load(zip_path):
    """返回 (characters, subjects, links, relations)。

    characters: {id: {'name', 'zh', 'names'(原名和各种别名), 'roles'}}；subjects: {id: {'name', 'name_cn', 'type'}}；
    links: {角色 id: [(作品 id, 主配角类型)]}；relations: [(角色 id, 关系类型, 关联角色 id)]。
    """
    characters, subjects, links, relations = {}, {}, collections.defaultdict(list), []
    with zipfile.ZipFile(zip_path) as archive:
        for row in lines(archive, 'character.jsonlines'):
            box = infobox(row.get('infobox'))
            zh = box.get('简体中文名') or box.get('中文名') or ''
            names = {row.get('name') or ''}
            aliases = box.get('别名')
            labelled = {}
            if isinstance(aliases, list):
                for label, value in aliases:
                    names.add(value)
                    labelled.setdefault(label, value)
            for k in ('日文名', '英文名', '罗马字', '纯假名'):
                if isinstance(box.get(k), str) and box[k]:
                    names.add(box[k])
                    labelled.setdefault(k, box[k])
            zh = zh.strip() if isinstance(zh, str) else ''
            original = row.get('name') or ''
            # 国产作品的角色，原名本身就是中文，「简体中文名」常空着或者写的是别名（如雷电将军写成巴尔泽布）。
            if (original and not zh and zhtext.han(original) and not zhtext.kana(original)
                    and not zhtext.hangul(original) and zhtext.gb2312(zhtext.unify_dots(original).replace('·', ''))):
                zh = original
            characters[row['id']] = {
                'name': original, 'zh': zh,
                'names': {n for n in names if n}, 'labelled': labelled, 'role': row.get('role'),
                'collects': row.get('collects') or 0,
            }
        for row in lines(archive, 'subject.jsonlines'):
            subjects[row['id']] = {'name': row.get('name') or '', 'name_cn': row.get('name_cn') or '',
                                   'type': row.get('type')}
        for row in lines(archive, 'subject-characters.jsonlines'):
            links[row['character_id']].append((row['subject_id'], row.get('type')))
        try:
            for row in lines(archive, 'person-relations.jsonlines'):
                if row.get('person_type') == 'crt':
                    relations.append((row['person_id'], row.get('relation_type'), row['related_person_id']))
        except FileNotFoundError:
            pass
    return characters, subjects, links, relations


def main(argv=None):
    args = argv or sys.argv[1:]
    characters, subjects, links, relations = load(args[0])
    with_zh = sum(1 for c in characters.values() if c['zh'])
    print('角色', len(characters), '有简体中文名', with_zh)
    print('按角色类型', collections.Counter(c['role'] for c in characters.values()))
    print('作品', len(subjects), '按类型', collections.Counter(s['type'] for s in subjects.values()))
    print('角色-作品对应', sum(len(v) for v in links.values()), '有对应的角色', len(links))
    print('角色之间的关系', len(relations), collections.Counter(r[1] for r in relations).most_common(12))


if __name__ == '__main__':
    main()
