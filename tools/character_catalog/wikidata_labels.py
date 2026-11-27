"""分发 Wikidata (CC0) 原标签。独立中文源只用作旁证。"""
import collections
import json
import re
import unicodedata

from tools.character_catalog import names, zhtext

ITEM = re.compile(r'https?://www\.wikidata\.org/entity/(Q[1-9][0-9]{0,18})\Z')
LANGUAGES = ('zh_hans', 'zh_cn', 'zh')


def load(path):
    bindings = json.loads(path.read_bytes())['results']['bindings']
    records = collections.defaultdict(list)
    for binding in bindings:
        row = {key: field['value'] for key, field in binding.items()}
        records[row['tag']].append(row)
    return records


def qid_for(tags, records):
    ids = set()
    for tag in tags:
        for row in records.get(tag, ()):
            matched = ITEM.fullmatch(row['item'])
            if not matched:
                return None
            ids.add(matched[1])
    return next(iter(ids)) if len(ids) == 1 else None


def name_evidence(records):
    """同一个 tag 若指向多个条目，任何一项都不能用来辅助取名。"""
    evidence = {}
    for tag, rows in records.items():
        if qid_for([tag], records):
            evidence[tag] = {}
            for lang in LANGUAGES:
                labels = {row[lang] for row in rows if row.get(lang)}
                if len(labels) == 1:
                    evidence[tag][lang] = next(iter(labels))
    return evidence


def reviewed_labels(rows, columns, records, translations):
    qid_col = columns.index('wikidata')
    qid_users = collections.Counter(row[qid_col] for row in rows)
    accepted = {}
    for values in rows:
        row = dict(zip(columns, values, strict=True))
        qid = row['wikidata']
        if not qid or qid_users[qid] != 1 or row['bangumi'] or row['group'] or row['action'] == 'auto':
            continue
        tags = [row['tag'], *row['tags']]
        evidence = {zhtext.key(translations[tag]['zh']) for tag in tags if tag in translations}
        for lang in LANGUAGES:
            labels = {record[lang] for tag in tags for record in records.get(tag, ()) if record.get(lang)}
            if len(labels) > 1:
                break
            if not labels:
                continue
            label = next(iter(labels))
            normalized, qualifiers = zhtext.strip_qualifiers(label)
            if (not qualifiers and zhtext.han(normalized) and not zhtext.kana(normalized) and not zhtext.hangul(normalized)
                    and not names.groupish(label) and 0 < len(label) <= 80 and label == label.strip()
                    and not any(unicodedata.category(char).startswith('C') for char in label)
                    and zhtext.key(label) in evidence):
                accepted[qid] = label
                break
    return dict(sorted(accepted.items()))
