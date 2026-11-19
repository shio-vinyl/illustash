#!/usr/bin/env python3
"""M0 生产迁移边界清点（仅静态，不导入应用或打开图库）。

check 仅阻塞新增 HTTP 分支、数据库写/动态执行点、启动/CLI 入口或缺失归属。
普通源码改动仅报告。研究/测试/生成代码按类别保留，无逐文件指纹门槛。
scan 输出当前边界；expand 展开归属、验收及删除条件；均不更新已审核规格。
fmt 把规格写成规范格式；sync-sources 只把报告用的 sources 指纹改成当前值，不动边界与归属。
"""
import argparse
import ast
from collections import Counter
import fnmatch
import hashlib
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
SPEC = ROOT / 'tests/migration_inventory.json'
ROOTS = ('web', 'tools', 'desktop', 'native', 'crates')
EXTENSIONS = {'.py', '.rs', '.sql', '.swift', '.sh', '.command', '.ps1', '.bat'}
EXCLUDE_DIRS = {'target', '__pycache__', '.venv', 'node_modules', '.git', 'tests'}
PRODUCTION_TOOLS = {'char_patch.py', 'build_characters.py', 'char_scan.py', 'ccip.py', 'index_worker.py'}
# 只在开发时用、不进发布包的工具目录（样式清单页 #318、界面走查 #453），里面的脚本不算生产代码。
DEV_TOOL_DIRS = {('tools', 'styleguide'), ('tools', 'ui_walk')}
GENERATED = {'casefold.rs', 'http_unicode.rs', 'name_codecs.rs'}
WRITE_SQL = re.compile(r'\b(INSERT(?:\s+OR\s+\w+)?\s+INTO|REPLACE\s+INTO|UPDATE|DELETE\s+FROM|CREATE\s+(?:TABLE|INDEX|TRIGGER|VIEW)(?:\s+IF\s+NOT\s+EXISTS)?|ALTER\s+TABLE|DROP\s+(?:TABLE|INDEX|TRIGGER|VIEW)(?:\s+IF\s+EXISTS)?)\s+([\w.]+)', re.I)
TRANSACTIONS = re.compile(r'\b(BEGIN|COMMIT|ROLLBACK|VACUUM|ATTACH|DETACH|REINDEX)\b', re.I)
DB_METHODS = {'execute', 'executemany', 'executescript', 'execute_batch', 'transaction', 'transaction_with_behavior', 'commit', 'rollback', 'backup', 'pragma_update'}
HTTP_SCOPES = {'do_GET', 'do_POST', 'library_action', 'account_action', 'provenance_action'}


def source_files(root):
    paths = []
    for folder in ROOTS:
        if not (root / folder).exists():
            continue
        for path in (root / folder).rglob('*'):
            rel = path.relative_to(root)
            if not path.is_file() or set(rel.parts) & EXCLUDE_DIRS or path.name in GENERATED or path.name in {'tests.rs', 'old_schema.sql'}:
                continue
            if rel.as_posix() == 'native/terms/examples/read-http.rs':
                continue  # 只在明确开启 test-read-host feature 时才有；从不是发布的入口。
            if path.parent.name == 'tools' and path.name not in PRODUCTION_TOOLS:
                continue
            if rel.parts[:2] in DEV_TOOL_DIRS:
                continue
            if path.suffix in EXTENSIONS:
                paths.append(path)
    paths.extend(p for p in root.iterdir() if p.is_file() and p.suffix in EXTENSIONS)
    paths.extend(root / p for p in ('.claude/launch.json', 'desktop/tauri.conf.json') if (root / p).is_file())
    return sorted(set(paths))


def sql_anchor(sql):
    writes = WRITE_SQL.findall(sql)
    if writes:
        return '|'.join(' '.join(op.upper().split()) + ':' + table.lower() for op, table in writes)
    transaction = TRANSACTIONS.search(sql)
    if transaction:
        return transaction.group(1).upper()
    if re.search(r'\bPRAGMA\b.*=', sql, re.I | re.S):
        return 'PRAGMA:' + sql.split('=')[0].strip().lower()
    return None


def python_sites(source):
    tree = ast.parse(source)
    parents = {child: parent for parent in ast.walk(tree) for child in ast.iter_child_nodes(parent)}
    sites = []

    def scope(node):
        names = []
        while node in parents:
            node = parents[node]
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                names.append(node.name)
        return '.'.join(reversed(names)) or '<module>'

    def add(node, kind, key, detail):
        sites.append({'line': node.lineno, 'scope': scope(node), 'kind': kind, 'key': key, 'detail': detail})

    for node in ast.walk(tree):
        if isinstance(node, (ast.If, ast.IfExp)):
            test = ast.unparse(node.test)
            constants = [n.value for n in ast.walk(node.test) if isinstance(n, ast.Constant) and isinstance(n.value, str)]
            if '__name__' in test and '__main__' in constants:
                add(node, 'entrypoint', 'python-main', test)
            elif any(n in HTTP_SCOPES for n in scope(node).split('.')) or any(s.startswith('/api/') for s in constants):
                # 名称、限制和格式可以变；边界由路径/动作和分支个数界定。
                add(node, 'http-branch', json.dumps(sorted(set(constants)), ensure_ascii=False), test)
                if isinstance(node, ast.If) and node.orelse and not isinstance(node.orelse[0], ast.If):
                    add(node.orelse[0], 'http-branch', '<default>', 'else: ' + test)
        elif isinstance(node, ast.Call):
            name = node.func.attr if isinstance(node.func, ast.Attribute) else node.func.id if isinstance(node.func, ast.Name) else ''
            if name in DB_METHODS:
                if node.args and isinstance(node.args[0], ast.Constant) and isinstance(node.args[0].value, str):
                    anchor = sql_anchor(node.args[0].value)
                    if anchor:
                        add(node, 'database-write', anchor, ast.unparse(node))
                elif node.args and isinstance(node.args[0], ast.JoinedStr):
                    parts = node.args[0].values
                    literal = ''.join(p.value for p in parts if isinstance(p, ast.Constant) and isinstance(p.value, str))
                    anchor = sql_anchor(literal)
                    if anchor:
                        add(node, 'database-write', anchor, ast.unparse(node))
                    elif not (name in {'execute', 'executemany'} and parts and isinstance(parts[0], ast.Constant) and re.match(r'\s*SELECT\b', parts[0].value, re.I)):
                        add(node, 'database-write', name + ':dynamic-or-wrapper', ast.unparse(node))
                else:
                    add(node, 'database-write', name + ':dynamic-or-wrapper', ast.unparse(node))
            if name in {'Popen', 'Process', 'execv', 'execve'}:
                add(node, 'entrypoint', 'process:' + name, ast.unparse(node))
            if name == 'add_parser':
                command = ast.unparse(node.args[0]) if node.args else '<dynamic>'
                add(node, 'cli-entry', command, ast.unparse(node))
    return sites


def text_sites(source, suffix):
    source = re.split(r'#\[cfg\(test\)\]\s*mod\s+tests\b', source, maxsplit=1)[0]
    sites, scope, commands = [], '<module>', False
    for number, line in enumerate(source.splitlines(), 1):
        text = line.strip()
        if not text or text.startswith(('//', '#', '--')):
            continue
        function = re.search(r'\b(?:fn|func)\s+(\w+)\s*\(', text)
        if function:
            scope = function.group(1)
            if scope == 'main':
                sites.append({'line': number, 'scope': scope, 'kind': 'entrypoint', 'key': 'native-main', 'detail': text})
        if 'pub const COMMANDS:' in text:
            commands = True
        elif commands and text == '];':
            commands = False
        elif commands and re.fullmatch(r'"[\w-]+",', text):
            sites.append({'line': number, 'scope': '<cli>', 'kind': 'cli-entry', 'key': text.strip(',"'), 'detail': text})
        anchor = sql_anchor(text)
        if not anchor and re.search(r'\.\s*(?:execute\w*|transaction\w*|commit|rollback|backup|pragma_update)\s*\(', text):
            # 字面量 SELECT 算读取；多行或由变量拼成的 SQL 仍算明确的审计边界。
            if not re.search(r'"\s*(?:SELECT|PRAGMA\s+[^=]+")', text, re.I):
                anchor = 'dynamic-or-wrapper'
        if anchor:
            sites.append({'line': number, 'scope': scope, 'kind': 'database-write', 'key': anchor, 'detail': text})
        # 基线之后新增的静态页面路由（/i18n.js，#173；模块文件夹 /core/ 和 /ui/，#199；样式文件夹 /css/，#705）也列为边界。
        routes = re.findall(r'"(/(?:api/[^" ]+|image|app\.[^" ]+|i18n\.js|favicon[^" ]*|core/|ui/|css/))"', text)
        if routes or 'Backend::Forward' in text or re.search(r'\bforward\(', text):
            key = '|'.join(routes) if routes else '<forward>'
            sites.append({'line': number, 'scope': scope, 'kind': 'http-branch', 'key': key, 'detail': text})
        if 'Command::new' in text or 'Process(' in text:
            sites.append({'line': number, 'scope': scope, 'kind': 'entrypoint', 'key': 'process-start', 'detail': text})
    if suffix in {'.sh', '.command', '.ps1', '.bat'}:
        sites.append({'line': 1, 'scope': '<script>', 'kind': 'entrypoint', 'key': 'shell-script', 'detail': '脚本启动入口'})
    return sites


def scan(root):
    sources, sites = {}, []
    for path in source_files(root):
        rel = path.relative_to(root).as_posix()
        text = path.read_text(encoding='utf-8')
        candidates = python_sites(text) if path.suffix == '.py' else text_sites(text, path.suffix)
        # ccip.py：只有函数里的数据库写入才算生产边界。
        if rel == 'tools/ccip.py':
            candidates = [s for s in candidates if s['kind'] == 'database-write' and s['scope'] != '<module>']
        if path.suffix == '.json':
            config = json.loads(text)
            commands = [c.get('runtimeExecutable', '<dynamic>') for c in config.get('configurations', [])] if rel.startswith('.claude/') else ['desktop-app']
            candidates = [{'line': 1, 'scope': '<config>', 'kind': 'entrypoint', 'key': command, 'detail': command} for command in commands]
        if rel != 'tools/ccip.py':
            sources[rel] = hashlib.sha256(text.encode()).hexdigest()  # 只用于报告，从不作为门槛
        for site in sorted(candidates, key=lambda s: (s['line'], s['kind'])):
            sites.append({'path': rel, **site})
    return {'sources': sources, 'sites': sites}


def assigned(site, spec):
    rule = spec['inventory'].get(site['path'])
    if rule is None:
        rule = next((r for r in spec['groups'] if any(fnmatch.fnmatch(site['path'], p) for p in r['patterns'])), {})
    result = {k: rule.get(k) for k in ('policy', 'current_runtime', 'storage_domain')}
    overrides = rule.get('scope_overrides', {})
    scope_rule = overrides.get(site['scope'])
    if scope_rule is None:
        scope_rule = next((value for scopes, value in overrides.items() if site['scope'] in scopes.split('|')), {})
    result.update(scope_rule)
    result.update(rule.get('kind_overrides', {}).get(site['kind'], {}))
    return result


def boundary_counts(sites):
    boundaries = {}
    for (path, kind, key), count in Counter((s['path'], s['kind'], s['key']) for s in sites).items():
        boundaries.setdefault(path, {}).setdefault(kind, {})[key] = count
    return boundaries


def recorded_boundaries(spec):
    """对重复的边界键用无损的字典引用；计数保持精确。"""
    keys = spec.get('boundary_keys', [])
    if not isinstance(keys, list) or not all(isinstance(k, str) for k in keys):
        raise ValueError('boundary_keys 必须是字符串列表')
    def expand(key):
        if isinstance(key, int) and not isinstance(key, bool):
            index = key
        elif isinstance(key, str) and re.fullmatch(r'@\d+', key):
            index = int(key[1:])
        elif isinstance(key, str):
            return key
        else:
            raise ValueError('边界键类型错误')
        if not 0 <= index < len(keys):
            raise ValueError('边界键引用越界')
        return keys[index]
    def list_pairs(values):
        for entry in values:
            if isinstance(entry, list):
                if len(entry) != 2 or not isinstance(entry[0], int) or isinstance(entry[0], bool):
                    raise ValueError('计数边界引用必须为 [index,count]')
                key, count = entry
            else:
                key, count = entry, 1
            yield expand(key), count
    out = {}
    for path, kinds in spec['boundaries'].items():
        out[path] = {}
        for kind, values in kinds.items():
            pairs = list_pairs(values) if isinstance(values, list) else ((expand(k), n) for k, n in values.items())
            counts = {}
            for key, count in pairs:
                if key in counts or not isinstance(count, int) or isinstance(count, bool) or count < 1:
                    raise ValueError('重复边界键或无效计数')
                counts[key] = count
            out[path][kind] = counts
    return out


def check(root, spec):
    current = scan(root)
    errors, warnings = [], []
    policies = spec['policies']
    try:
        recorded = recorded_boundaries(spec)
    except (ValueError, TypeError, KeyError, AttributeError) as error:
        current['warnings'] = []
        return [f'迁移边界清单损坏：{error}'], current
    for name, policy in policies.items():
        if policy.get('final_owner') not in {'rust', 'native-platform', 'optional-worker', 'retained-test-research'} or not all(policy.get(k) for k in ('responsibility', 'acceptance', 'deletion_condition')):
            errors.append(f'缺少归属规则：{name}')
    for path, kinds in boundary_counts(current['sites']).items():
        for kind, counts in kinds.items():
            for key, count in counts.items():
                previous = recorded.get(path, {}).get(kind, {}).get(key, 0)
                if count > previous:
                    errors.append(f'新增迁移边界：{path} {kind}:{key}（{previous} → {count}）')
    for site in current['sites']:
        rule = assigned(site, spec)
        policy = policies.get(rule['policy'], {})
        if not policy or not rule['current_runtime'] or not rule['storage_domain']:
            errors.append(f'归属缺失：{site["path"]}:{site["line"]} {site["kind"]}')
        elif site['kind'] == 'database-write' and policy['final_owner'] == 'optional-worker' and rule['storage_domain'] != 'research':
            errors.append(f'worker 不得持有正式写入边界：{site["path"]}:{site["line"]}')
    current['sources'] = {p: h for p, h in current['sources'].items() if assigned({'path': p, 'scope': '', 'kind': ''}, spec)['policy'] not in {'research', 'retained'}}
    for path in sorted(set(spec.get('sources', {})) | set(current['sources'])):
        if spec.get('sources', {}).get(path) != current['sources'].get(path):
            warnings.append(f'源码变化（仅报告）：{path}')
    current['warnings'] = warnings
    return errors, current


def dump_spec(spec):
    """规范的字节形式：每个顶层键，以及顶层对象或列表的每个条目，各占紧凑的一行。

    这样两个分支刷新不同的 sources 指纹时改的是不同的行，git 能合并；sources 的条目之间还隔一个空行，
    因为 git 把相邻行的改动当作冲突。键的顺序保持原样，只有 sources 和 scan() 一样排序。
    """
    if isinstance(spec.get('sources'), dict):
        spec = {**spec, 'sources': dict(sorted(spec['sources'].items()))}
    compact = lambda value: json.dumps(value, ensure_ascii=False, separators=(',', ':'))
    items = []
    for key, value in spec.items():
        if isinstance(value, dict) and value:
            entries, brackets = [compact(k) + ':' + compact(v) for k, v in value.items()], '{}'
        elif isinstance(value, list) and value:
            entries, brackets = [compact(v) for v in value], '[]'
        else:
            items.append(compact(key) + ':' + compact(value))
            continue
        separator = ',\n\n' if key == 'sources' else ',\n'
        items.append(compact(key) + ':' + brackets[0] + '\n' + separator.join(' ' + e for e in entries) + '\n' + brackets[1])
    return '{\n' + ',\n'.join(items) + '\n}\n'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('check', 'scan', 'report', 'expand', 'fmt', 'sync-sources'), nargs='?', default='check')
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--spec', type=Path, default=SPEC)
    args = parser.parse_args(argv)
    if sys.version_info < (3, 12):
        parser.error('当前仓库源码使用 Python 3.12 语法；请用 python3.12 运行此测试工具')
    if args.command == 'scan':
        print(json.dumps(scan(args.root), ensure_ascii=False, indent=2))
        return 0
    text = args.spec.read_text(encoding='utf-8')
    spec = json.loads(text)
    if args.command == 'fmt':
        args.spec.write_text(dump_spec(spec), encoding='utf-8')
        return 0
    errors, current = check(args.root, spec)
    if args.command == 'sync-sources':
        if any(e.startswith('迁移边界清单损坏') for e in errors):
            print(errors[0], file=sys.stderr)
            return 1
        changed = sorted(set(spec.get('sources', {}).items()) ^ set(current['sources'].items()))
        spec['sources'] = current['sources']
        text = dump_spec(spec)
        args.spec.write_text(text, encoding='utf-8')
        current['warnings'] = []
        print(f'已同步 {len({path for path, _ in changed})} 个文件的指纹（只改 sources，不批准新增边界）')
    if text != dump_spec(spec):
        errors.append('清单格式不规范：运行 python -m tools.migration_inventory fmt')
    if args.command == 'expand':
        for site in current['sites']:
            rule = assigned(site, spec)
            print(json.dumps({**site, **rule, **spec['policies'].get(rule['policy'], {})}, ensure_ascii=False))
    elif args.command == 'report':
        print(json.dumps({'production_files': len(current['sources']), 'boundary_counts': dict(Counter(s['kind'] for s in current['sites'])), 'warnings': current['warnings'], 'errors': errors, 'limitations': spec['limitations']}, ensure_ascii=False, indent=2))
    else:
        for message in current['warnings'] + errors:
            print(message, file=sys.stderr)
        if not errors:
            print('生产边界归属有效；普通源码变化不阻塞（不表示生产迁移完成）')
    return int(bool(errors))


if __name__ == '__main__':
    raise SystemExit(main())
