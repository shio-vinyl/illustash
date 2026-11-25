"""M0 的窄门槛；夹具只是源码文本，从不使用活的图库或服务器。"""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
module_spec = importlib.util.spec_from_file_location('migration_inventory', ROOT / 'tools/migration_inventory.py')
inventory = importlib.util.module_from_spec(module_spec)
module_spec.loader.exec_module(inventory)


class MigrationInventoryTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / 'web').mkdir()
        self.path = self.root / 'web/server.py'
        self.source = 'def write(c, value):\n    c.execute("INSERT INTO facts VALUES (?)", (value,))\n\ndef do_GET(path):\n    if path == "/api/one":\n        pass\n'
        self.path.write_text(self.source)
        current = inventory.scan(self.root)
        self.rule = {'policy': 'app', 'current_runtime': 'python-host', 'storage_domain': 'application'}
        self.spec = {
            'sources': current['sources'], 'boundaries': inventory.boundary_counts(current['sites']),
            'policies': {'app': {'final_owner': 'rust', 'acceptance': '事务及撤销对照', 'deletion_condition': '无 Python 验收', 'responsibility': '应用数据'}},
            'groups': [], 'inventory': {'web/server.py': self.rule},
        }

    def errors(self):
        return inventory.check(self.root, self.spec)[0]

    def test_current_repository_has_owned_production_boundaries(self):
        text = (ROOT / 'tests/migration_inventory.json').read_text(encoding='utf-8')
        spec = json.loads(text)
        errors, current = inventory.check(ROOT, spec)
        self.assertEqual([], errors)
        self.assertEqual([], current['warnings'])
        self.assertEqual(inventory.dump_spec(spec), text, '运行 python -m tools.migration_inventory fmt')
        # 预算按内容计，不计每条一行的空白。
        # 人工结论的存储、后台任务和 HTTP 归属扩充后，无损整理为约 83 KB（#441）。
        # 0.3 的 v10 批（CCIP 长尾、收藏、画廊、相似检索、WD 扩展）和桌面功能新增源文件指纹、HTTP 与迁移边界，
        # 合起来约 90 KB（2026-11-17）。超限时先看增长是不是新的源文件和边界，再调这里。
        # 0.4.1 的旧图库折叠投影、人工结论归属与孤立认领写入边界合计约 94.2 KB（#483、#836）。
        self.assertLess(len(json.dumps(spec, ensure_ascii=False, separators=(',', ':')).encode()), 95_000)
        self.assertNotIn('coverage', spec)
        self.assertNotIn('sites', spec)
        self.assertNotIn('tools/verify_read_http.py', spec['sources'])
        self.assertNotIn('native/terms/src/casefold.rs', spec['sources'])

    def test_canonical_format_is_lossless_and_puts_each_fingerprint_on_its_own_line(self):
        spec = {'sources': {'web/b.py': '2', 'web/a.py': '1'}, 'format': 2, 'retained_references': [],
                'boundaries': {'web/a.py': {'database-write': [[0, 2], 'INSERT INTO:t']}}, 'boundary_keys': ['UPDATE:t']}
        text = inventory.dump_spec(spec)
        self.assertEqual(json.loads(text), spec)
        self.assertEqual(inventory.dump_spec(json.loads(text)), text)
        self.assertEqual(text.splitlines()[1:6], ['"sources":{', ' "web/a.py":"1",', '', ' "web/b.py":"2"', '},'])
        self.assertIn('\n"retained_references":[],\n', text)
        self.assertIn('\n "web/a.py":{"database-write":[[0,2],"INSERT INTO:t"]}\n', text)
        self.assertTrue(text.endswith('\n]\n}\n'))

    def test_fmt_and_sync_sources_rewrite_only_format_and_fingerprints(self):
        spec_path = self.root / 'spec.json'
        stale = {**self.spec, 'sources': {'web/server.py': 'old', 'web/deleted.py': 'gone'}}
        spec_path.write_text(json.dumps(stale), encoding='utf-8')
        args = ['--root', str(self.root), '--spec', str(spec_path)]
        with contextlib.redirect_stderr(io.StringIO()) as stderr, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(inventory.main(['check', *args]), 1)
        self.assertIn('清单格式不规范', stderr.getvalue())
        self.assertEqual(inventory.main(['fmt', *args]), 0)
        self.assertEqual(json.loads(spec_path.read_text(encoding='utf-8')), stale)
        self.path.write_text(self.source + '\nsubprocess.Popen(["new-worker"])\n')
        with contextlib.redirect_stderr(io.StringIO()) as stderr, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(inventory.main(['sync-sources', *args]), 1)
        self.assertIn('新增迁移边界', stderr.getvalue())
        synced = json.loads(spec_path.read_text(encoding='utf-8'))
        self.assertEqual(synced, {**stale, 'sources': inventory.scan(self.root)['sources']})
        self.assertEqual(spec_path.read_text(encoding='utf-8'), inventory.dump_spec(synced))

    def test_boundary_dictionary_preserves_counts_and_rejects_corrupt_references(self):
        baseline=inventory.recorded_boundaries(self.spec)
        keys=list(dict.fromkeys(key for kinds in baseline.values() for values in kinds.values() for key in values))
        self.spec['boundary_keys']=keys
        self.spec['boundaries']={path:{kind:{'@'+str(keys.index(key)):count for key,count in values.items()} for kind,values in kinds.items()} for path,kinds in baseline.items()}
        self.assertEqual(inventory.recorded_boundaries(self.spec),baseline)
        self.assertEqual([],self.errors())
        self.spec['boundaries']={path:{kind:([keys.index(key) for key in values] if all(n==1 for n in values.values()) else {'@'+str(keys.index(key)):n for key,n in values.items()}) for kind,values in kinds.items()} for path,kinds in baseline.items()}
        self.assertEqual(inventory.recorded_boundaries(self.spec),baseline)
        self.assertEqual([],self.errors())
        self.path.write_text(self.source+'\nsubprocess.Popen(["new-worker"])\n')
        self.assertTrue(any('新增迁移边界' in error for error in self.errors()))
        self.path.write_text(self.source)
        self.spec['boundary_keys']=[]
        self.assertTrue(any('清单损坏' in error for error in self.errors()))

    def test_counted_lists_preserve_counts_and_n_plus_one_is_blocked(self):
        statement = '    c.execute("INSERT INTO facts VALUES (?)", (value,))\n'
        self.path.write_text(self.source.replace(statement, statement * 2))
        expected = {'web/server.py': {'database-write': {'INSERT INTO:facts': 2},
                                      'http-branch': {'["/api/one"]': 1}}}
        self.spec['boundary_keys'] = ['INSERT INTO:facts', '["/api/one"]']
        for writes, reads in (({'INSERT INTO:facts': 2}, {'["/api/one"]': 1}),
                              ({'@0': 2}, [1]), ([[0, 2]], ['@1']),
                              ([[0, 2]], [[1, 1]])):
            with self.subTest(writes=writes, reads=reads):
                self.spec['boundaries'] = {'web/server.py': {'database-write': writes,
                                                            'http-branch': reads}}
                self.assertEqual(inventory.recorded_boundaries(self.spec), expected)
                self.assertEqual([], self.errors())
        self.spec['boundary_keys'].append('UPDATE:facts')
        self.spec['boundaries']['web/server.py']['database-write'] = [2, [0, 2], 'literal']
        expected['web/server.py']['database-write'].update({'UPDATE:facts': 1, 'literal': 1})
        self.assertEqual(inventory.recorded_boundaries(self.spec), expected)
        self.assertEqual([], self.errors())
        self.path.write_text(self.source.replace(statement, statement * 3))
        self.assertTrue(any('新增迁移边界' in error and '（2 → 3）' in error
                            for error in self.errors()))

    def test_invalid_counted_lists_and_alias_duplicates_are_diagnosed_by_check(self):
        self.spec['boundary_keys'] = ['INSERT INTO:facts']
        invalid = ([True], [False], [-1], [1], [[True, 2]], [[False, 2]],
                   [[-1, 2]], [[1, 2]], [[0, True]], [[0, False]], [[0, 0]],
                   [[0, -1]], [[0, 1.5]], [[0, '2']], [[0, None]], [[0, []]],
                   [[]], [[0]], [[0, 2, 3]], [['@0', 2]], [[0.0, 2]], [[[0], 2]],
                   [0, [0, 2]], ['INSERT INTO:facts', [0, 2]], ['@0', [0, 2]],
                   [[0, 2], [0, 3]], {'@0': True}, {'@0': 0}, {'@0': -1},
                   {'@0': 1.5}, {'@1': 1})
        for values in invalid:
            with self.subTest(values=values):
                self.spec['boundaries']['web/server.py']['database-write'] = values
                with self.assertRaises(ValueError):
                    inventory.recorded_boundaries(self.spec)
                errors, current = inventory.check(self.root, self.spec)
                self.assertEqual(len(errors), 1)
                self.assertTrue(errors[0].startswith('迁移边界清单损坏：'), errors)
                self.assertEqual([], current['warnings'])

    def test_new_http_route_or_action_branch_fails(self):
        self.path.write_text(self.source + '    elif path == "/api/two":\n        pass\n')
        self.assertTrue(any('http-branch' in e for e in self.errors()))
        self.path.write_text(self.source + '\ndef library_action(action):\n    if action == "create":\n        pass\n')
        self.assertTrue(any('http-branch' in e for e in self.errors()))

    def test_new_write_point_same_table_and_scope_fails(self):
        self.path.write_text(self.source.replace('\n\ndef do_GET', '\n    c.execute("INSERT INTO facts VALUES (?)", (2,))\n\ndef do_GET'))
        self.assertTrue(any('database-write' in e for e in self.errors()))

    def test_removed_writer_in_one_file_cannot_cancel_new_writer_in_another(self):
        self.path.write_text('def do_GET(path):\n    if path == "/api/one":\n        pass\n')
        (self.root / 'web/new.py').write_text('def write(c):\n    c.execute("INSERT INTO facts VALUES (?)", (1,))\n')
        self.spec['inventory']['web/new.py'] = copy.deepcopy(self.rule)
        self.assertTrue(any('web/new.py' in e and 'database-write' in e for e in self.errors()))

    def test_new_startup_and_cli_entries_fail(self):
        self.path.write_text(self.source + '\nif __name__ == "__main__":\n    pass\n')
        self.assertTrue(any('entrypoint' in e for e in self.errors()))
        self.path.write_text(self.source + '\nparser.add_parser("new-command")\n')
        self.assertTrue(any('cli-entry' in e for e in self.errors()))
        self.path.write_text(self.source + '\nsubprocess.Popen(["worker"] )\n')
        self.assertTrue(any('entrypoint' in e for e in self.errors()))

    def test_ordinary_implementation_read_comment_and_in_file_refactor_are_report_only(self):
        changed = self.source.replace('def write(c, value)', 'def renamed(db, item)').replace('c.execute', 'db.execute').replace('(value,)', '(item + 1,)')
        self.path.write_text('# ordinary comment\n' + changed + '\ndef read(c):\n    return c.execute("SELECT uuid FROM assets").fetchall()\n')
        errors, current = inventory.check(self.root, self.spec)
        self.assertEqual([], errors)
        self.assertTrue(current['warnings'])

    def test_added_fstring_select_read_does_not_block(self):
        self.path.write_text(self.source + '\ndef read(c, table):\n    return c.execute(f"SELECT uuid FROM {table}").fetchall()\n')
        self.assertEqual([], self.errors())

    def test_existing_sql_parameter_changes_do_not_need_boundary_reapproval(self):
        self.path.write_text(self.source.replace('VALUES (?)', 'VALUES (? + 1)'))
        self.assertEqual([], self.errors())

    def test_deleted_write_point_is_report_only(self):
        self.path.write_text(self.source.replace('    c.execute("INSERT INTO facts VALUES (?)", (value,))', '    pass'))
        errors, current = inventory.check(self.root, self.spec)
        self.assertEqual([], errors)
        self.assertTrue(current['warnings'])

    def test_missing_or_invalid_owner_fails(self):
        self.spec['inventory']['web/server.py'].pop('policy')
        self.assertTrue(any('归属缺失' in e for e in self.errors()))
        self.rule['policy'] = 'app'
        self.spec['policies']['app']['final_owner'] = 'unknown-but-truthy'
        self.assertTrue(any('缺少归属规则' in e for e in self.errors()))

    def test_worker_cannot_own_formal_writes_but_can_write_research_storage(self):
        self.spec['policies']['app']['final_owner'] = 'optional-worker'
        self.assertTrue(any('worker 不得持有' in e for e in self.errors()))
        self.rule['storage_domain'] = 'research'
        self.assertEqual([], self.errors())

    def test_research_test_and_generated_edits_need_no_file_baseline(self):
        (self.root / 'tools').mkdir()
        (self.root / 'tools/experiment.py').write_text('if __name__ == "__main__":\n    c.execute("UPDATE private_cache SET value=1")\n')
        (self.root / 'native').mkdir()
        (self.root / 'native/casefold.rs').write_text('fn main() { c.execute("DELETE FROM cache", []); }')
        (self.root / 'tests').mkdir()
        (self.root / 'tests/test_new.py').write_text('c.execute("DELETE FROM fixture")\n')
        errors, current = inventory.check(self.root, self.spec)
        self.assertEqual([], errors)
        self.assertEqual([], current['warnings'])

    def test_styleguide_dev_tool_is_not_production_code(self):
        # 样式清单页（#318）只在开发时用；它的脚本有正常的 __main__ 入口，也不算生产边界。
        (self.root / 'tools/styleguide').mkdir(parents=True)
        (self.root / 'tools/styleguide/census.py').write_text('def main():\n    pass\n\nif __name__ == "__main__":\n    main()\n')
        self.assertNotIn('tools/styleguide/census.py', [p.relative_to(self.root).as_posix() for p in inventory.source_files(self.root)])
        errors, current = inventory.check(self.root, self.spec)
        self.assertEqual([], errors)
        self.assertEqual([], current['warnings'])

    def test_production_tools_sql_and_dynamic_writer_boundaries_remain_visible(self):
        (self.root / 'tools').mkdir()
        (self.root / 'tools/char_scan.py').write_text('def load(c, sql):\n    c.execute(sql)\n')
        (self.root / 'tools/ccip.py').write_text('def compute(c):\n    c.execute("DELETE FROM clusters")\n')
        (self.root / 'native').mkdir()
        (self.root / 'native/commands.sql').write_text('INSERT INTO facts VALUES (1);')
        current = inventory.scan(self.root)
        self.assertTrue(any(s['path'] == 'tools/char_scan.py' and s['kind'] == 'database-write' for s in current['sites']))
        self.assertTrue(any(s['path'] == 'tools/ccip.py' and s['kind'] == 'database-write' for s in current['sites']))
        self.assertTrue(any(s['path'].endswith('.sql') and s['kind'] == 'database-write' for s in current['sites']))
        self.assertTrue(any('tools/char_scan.py' in e and 'database-write' in e for e in self.errors()))


if __name__ == '__main__':
    unittest.main()
