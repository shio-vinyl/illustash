"""tools/check_frontend.py 的合成负例：在 web/static 的副本上故意违反每条规则，确认会报错。"""
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
import io
import json
import shutil
import subprocess
import tempfile
import unittest
from tools.check_frontend import BASELINE, ROOT, STATIC, baseline_growth, check, css_declarations, main, strip_comments


class Tree(tempfile.TemporaryDirectory):
    """检查所读文件的临时副本，每个用例在副本上只违反一条规则。"""

    def __enter__(self):
        self.root = Path(super().__enter__())
        base = self.root / STATIC
        base.mkdir(parents=True)
        shutil.copyfile(ROOT / STATIC / 'app.js', base / 'app.js')
        shutil.copyfile(ROOT / STATIC / 'app.css', base / 'app.css')
        for folder in ('core', 'ui', 'css'):
            shutil.copytree(ROOT / STATIC / folder, base / folder)
        (self.root / BASELINE).parent.mkdir(parents=True)
        shutil.copyfile(ROOT / BASELINE, self.root / BASELINE)
        return self

    def append(self, relative, text):
        path = self.root / STATIC / relative
        path.write_text(path.read_text(encoding='utf-8') + text, encoding='utf-8')

    def errors(self):
        return check(self.root)[0]

    def replace(self, relative, old, new):
        path = self.root / STATIC / relative
        text = path.read_text(encoding='utf-8')
        assert text.count(old) == 1, old
        path.write_text(text.replace(old, new), encoding='utf-8')

    def write_baseline(self, rules):
        (self.root / BASELINE).write_text(json.dumps({'rules': rules}), encoding='utf-8')

    def git(self, *args):
        subprocess.run(['git', '-c', 'user.name=t', '-c', 'user.email=t@example.invalid', *args],
                       cwd=self.root, check=True, capture_output=True)

    def baseline(self):
        return json.loads((self.root / BASELINE).read_text(encoding='utf-8'))['rules']

    def run_main(self, *args):
        out = io.StringIO()
        with redirect_stdout(out):
            code = main(['--root', str(self.root), *args])
        return code, out.getvalue()


class FrontendArchitectureTest(unittest.TestCase):
    def assertViolation(self, tree, rule, needle=''):
        errors = tree.errors()
        matching = [e for e in errors if f'· {rule}]' in e and needle in e]
        self.assertTrue(matching, f'expected a {rule} error containing {needle!r}, got {errors}')
        # 每条报错都写明违反的约定和改法
        self.assertIn('约定：', matching[0])
        self.assertIn('改法：', matching[0])
        return matching[0]

    def test_repository_matches_baseline(self):
        errors, _ = check()
        self.assertEqual(errors, [])

    def test_clean_copy_has_no_errors(self):
        with Tree() as tree:
            self.assertEqual(tree.errors(), [])

    def test_core_must_not_import_ui(self):
        with Tree() as tree:
            tree.append('core/format.js', "\nimport {toast} from '../ui/dom.js';\n")
            self.assertViolation(tree, 'import-core-ui', 'core/format.js -> ui/dom.js')

    def test_core_must_not_import_the_entry(self):
        with Tree() as tree:
            tree.append('core/format.js', "\nimport '/app.js';\n")
            self.assertViolation(tree, 'import-core-ui', 'core/format.js -> app.js')

    def test_new_cycle_through_a_leaf_module(self):
        with Tree() as tree:
            # ui/dom.js 被环里的模块导入，自己不导入其中任何一个；反过来导入就闭合出新的环。
            tree.append('ui/dom.js', "\nimport {renderGrid} from './grid.js';\n")
            self.assertViolation(tree, 'import-cycle', 'ui/dom.js -> ui/grid.js')

    def test_new_edge_inside_an_existing_cycle(self):
        with Tree() as tree:
            baseline = {e for e in tree.baseline()['import-cycle']}
            self.assertNotIn('ui/artists.js -> ui/tasks.js', baseline)
            tree.append('ui/artists.js', "\nimport {taskTick} from './tasks.js';\n")
            self.assertViolation(tree, 'import-cycle', 'ui/artists.js -> ui/tasks.js')

    def test_dynamic_and_reexport_imports_count(self):
        with Tree() as tree:
            tree.append('core/format.js', "\nexport {toast} from '../ui/dom.js';\n")
            self.assertViolation(tree, 'import-core-ui')
        with Tree() as tree:
            tree.append('core/format.js', "\nconst later = () => import('../ui/dom.js');\n")
            self.assertViolation(tree, 'import-core-ui')

    def test_undeclared_state_field(self):
        with Tree() as tree:
            tree.append('ui/tasks.js', '\nstate.brandNewField = 1;\n')
            message = self.assertViolation(tree, 'undeclared-state', 'brandNewField')
            self.assertIn('core/state.js', message)

    def test_state_field_in_template_expression(self):
        with Tree() as tree:
            tree.append('ui/tasks.js', '\nconst label = `${state.alsoNew} items`;\n')
            self.assertViolation(tree, 'undeclared-state', 'alsoNew')

    def test_declared_state_field_is_fine(self):
        with Tree() as tree:
            tree.append('ui/tasks.js', '\nif (state.total) go();\n')
            tree.append('core/state.js', '')
            self.assertEqual(tree.errors(), [])

    def test_declaring_a_field_in_state_js_fixes_it(self):
        with Tree() as tree:
            tree.append('ui/tasks.js', '\nstate.brandNewField = 1;\n')
            self.assertTrue(tree.errors())
            path = tree.root / STATIC / 'core/state.js'
            # 声明在一个写明 ui/tasks.js 是写入者的组里，规则 7 也不报
            path.write_text(path.read_text(encoding='utf-8').replace('  // ---- 路由', '  // ---- 任务面板：ui/tasks.js ----\n  brandNewField: 0,\n  // ---- 路由', 1),
                            encoding='utf-8')
            self.assertEqual(tree.errors(), [])

    def test_state_in_comments_and_strings_is_ignored(self):
        with Tree() as tree:
            tree.append('ui/tasks.js', "\n// state.inComment = 1\n/* state.inBlock */\nconst s = 'state.inString';\n"
                                       "import {state as unused} from './state.js';\n")
            self.assertEqual([e for e in tree.errors() if 'undeclared-state' in e], [])

    def test_new_document_listener_in_a_new_file_position(self):
        for event in ('keydown', 'click'):
            with self.subTest(event=event), Tree() as tree:
                tree.append('ui/artists.js', f"\ndocument.addEventListener('{event}', e => {{}});\n")
                self.assertViolation(tree, 'document-listener', f'ui/artists.js::{event}')

    def test_the_overlay_manager_is_the_one_allowed_entry(self):
        # 用户 2026-11-12 在 #343 批准：ui/overlay.js 是全局 keydown / click 监听的唯一入口，别的文件照旧报错。
        with Tree() as tree:
            overlay = (tree.root / STATIC / 'ui/overlay.js').read_text(encoding='utf-8')
            self.assertIn("document.addEventListener('keydown'", overlay)
            self.assertIn("document.addEventListener('click'", overlay)
            tree.append('ui/overlay.js', "\ndocument.addEventListener('click', e => {});\nwindow.addEventListener('keydown', e => {});\n")
            self.assertEqual([e for e in tree.errors() if 'document-listener' in e], [])
        with Tree() as tree:
            tree.append('ui/dialog.js', "\ndocument.addEventListener('keydown', e => {});\n")
            self.assertViolation(tree, 'document-listener', 'ui/dialog.js::keydown')

    def test_extra_listener_in_a_file_that_already_has_some(self):
        with Tree() as tree:
            tree.append('ui/settings.js', "\ndocument.addEventListener('click', async e => {});\n")
            self.assertViolation(tree, 'document-listener', 'ui/settings.js::click')

    def test_window_and_property_listeners_count_too(self):
        with Tree() as tree:
            tree.append('ui/artists.js', '\nwindow.addEventListener("keydown", e => {});\n')
            self.assertViolation(tree, 'document-listener', 'ui/artists.js::keydown')
        with Tree() as tree:
            tree.append('ui/artists.js', '\ndocument.onclick = () => {};\n')
            self.assertViolation(tree, 'document-listener', 'ui/artists.js::click')

    def test_body_and_more_events_count_too(self):
        cases = ["document.body.addEventListener('click', e => {});",
                 "document.documentElement.addEventListener('keydown', e => {});",
                 "document.addEventListener('pointerdown', e => {});",
                 "document.addEventListener('mousedown', e => {});",
                 "document.addEventListener('keyup', e => {});",
                 "document.addEventListener('change', e => {});",   # #431 ③-12
                 "document.onchange = () => {};"]
        for text in cases:
            with self.subTest(text=text), Tree() as tree:
                tree.append('ui/artists.js', '\n' + text + '\n')
                self.assertViolation(tree, 'document-listener', 'ui/artists.js::')

    def test_dialog_markup_in_a_string_counts(self):
        for text in ("el.innerHTML = '<dialog class=\"x\"></dialog>';", "el.insertAdjacentHTML('beforeend', `<dialog>${x}</dialog>`);"):
            with self.subTest(text=text), Tree() as tree:
                tree.append('ui/home.js', '\n' + text + '\n')
                self.assertViolation(tree, 'dialog-element', 'ui/home.js')

    def test_other_events_and_element_listeners_are_fine(self):
        with Tree() as tree:
            # change 从 #431 起也算（见 test_body_and_more_events_count_too），这里换成 input。
            tree.append('ui/artists.js', "\ndocument.addEventListener('input', e => {});\n"
                                         "document.addEventListener('pointermove', e => {});\n"
                                         "button.addEventListener('keydown', e => { if (composing(e)) return; });\n"
                                         "// document.addEventListener('keydown', e => {});\n")
            self.assertEqual(tree.errors(), [])

    def test_key_handler_without_composing_guard(self):
        # 规则 9（#388）：每个 keydown / keyup / keypress 处理函数都要调用 composing()，各种写法都算。
        cases = ["input.addEventListener('keydown', e => { if (e.key === 'Enter') go(); });",
                 "input.addEventListener(\"keyup\", function (e) { go(e); });",
                 "input.addEventListener('keypress', e => go(e), true);",
                 "input.onkeydown = e => { if (e.key === 'Enter') go(); };",
                 "input.addEventListener('keydown', onKey);\nfunction onKey(e) { go(e); }",
                 "input.addEventListener('keydown', onKey);\nconst onKey = e => { go(e); };",
                 "input.addEventListener('keydown', missing);"]
        for text in cases:
            with self.subTest(text=text), Tree() as tree:
                tree.append('ui/artists.js', '\n' + text + '\n')
                message = self.assertViolation(tree, 'ime-composing', 'ui/artists.js: 键盘处理函数没有调用 composing()')
                self.assertIn('core/keys.js', message)

    def test_key_handler_with_composing_guard_is_fine(self):
        cases = ["input.addEventListener('keydown', e => { if (composing(e)) return; go(e); });",
                 "input.onkeydown = e => composing(e) || go(e);",
                 "input.addEventListener('keydown', onKey, true);\nfunction onKey(e) { if (composing(e)) return; go(e); }",
                 "input.addEventListener('keydown', onKey);\nconst onKey = e => { if (composing(e)) return; go(e); };",
                 "const same = a === b; if (a.onkeydown == null) go();",
                 "// input.onkeydown = e => go(e);"]
        for text in cases:
            with self.subTest(text=text), Tree() as tree:
                tree.append('ui/artists.js', '\n' + text + '\n')
                self.assertEqual([e for e in tree.errors() if 'ime-composing' in e], [])

    def test_only_core_keys_reads_the_composition_flags(self):
        for text in ("if (e.isComposing) return;", "if (e.keyCode === 229) return;", "const c = ev .isComposing;"):
            with self.subTest(text=text), Tree() as tree:
                tree.append('ui/artists.js', '\n' + text + '\n')
                self.assertViolation(tree, 'ime-composing', 'ui/artists.js: 直接读')
        with Tree() as tree:
            keys = (tree.root / STATIC / 'core/keys.js').read_text(encoding='utf-8')
            self.assertIn('isComposing', keys)
            self.assertIn('229', keys)
            self.assertEqual([e for e in tree.errors() if 'ime-composing' in e], [])
        with Tree() as tree:  # 注释和字符串里提到不算
            tree.append('ui/artists.js', "\n// e.isComposing 或 keyCode 229\nconst note = 'e.keyCode';\n")
            self.assertEqual([e for e in tree.errors() if 'ime-composing' in e], [])

    def test_removing_a_guard_from_the_repository_is_caught(self):
        # 真实文件上的回归：把已有的防护拿掉，规则要报出来（覆盖具名函数、onkeydown 和 addEventListener 三种写法）。
        for relative, old, new in (
                ('ui/overlay.js', "  if (composing(e)) return;  // 组字时的 Esc 是取消组字，不关浮层；方向键是选字\n", ""),
                ('ui/palette.js', "q('#pfAlias').onkeydown = e => { if (composing(e)) return; ", "q('#pfAlias').onkeydown = e => { "),
                ('ui/combo.js', "if (composing(e) || !shown()) return;", "if (!shown()) return;")):  # 命令面板等四处结果菜单（#432）
            with self.subTest(relative=relative, old=old), Tree() as tree:
                tree.replace(relative, old, new)
                self.assertViolation(tree, 'ime-composing', f'{relative}: 键盘处理函数没有调用 composing()')

    def test_new_runtime_dialog(self):
        with Tree() as tree:
            tree.append('ui/home.js', "\nconst d = document.createElement('dialog');\n")
            message = self.assertViolation(tree, 'dialog-element', 'ui/home.js')
            self.assertIn('ui/dialog.js', message)
        with Tree() as tree:  # 用户 2026-11-15 批准（#470）：ui/dialog.js 是唯一放行的文件，别的文件照旧报错
            dialog = (tree.root / STATIC / 'ui/dialog.js').read_text(encoding='utf-8')
            self.assertIn("document.createElement('dialog')", dialog)
            tree.append('ui/dialog.js', '\nconst d = document.createElement("dialog");\n')
            self.assertEqual(tree.errors(), [])
            tree.append('ui/catalog.js', "\nel.innerHTML = '<dialog></dialog>';\n")
            self.assertViolation(tree, 'dialog-element', 'ui/catalog.js')

    def test_new_page_name_comparison(self):
        # 页面名判断只能写在 core/pages.js 的注册表里；新增比较要报错，改写成读注册表字段
        with Tree() as tree:
            tree.append('ui/palette.js', "\nif (state.route.page === 'library') go();\n")
            message = self.assertViolation(tree, 'page-name', 'ui/palette.js')
            self.assertIn('core/pages.js', message)
        with Tree() as tree:
            tree.append('ui/palette.js', "\nif (['a', 'b'].includes(state.route.page)) go();\n")
            self.assertViolation(tree, 'page-name', 'ui/palette.js')
        with Tree() as tree:  # 注册表自己不受限，读注册表字段不算
            tree.append('core/pages.js', "\nconst x = r.page === 'home';\n")
            tree.append('ui/palette.js', "\nif (viewOf(state.route).click === 'cleanup' && onPage(state.route, 'home')) go();\n")
            self.assertEqual(tree.errors(), [])

    def test_long_line_boundary(self):
        with Tree() as tree:
            tree.append('ui/home.js', '\nconst ok = "' + 'x' * (200 - len('const ok = "";')) + '";\n')
            self.assertEqual(tree.errors(), [])
            tree.append('ui/home.js', 'const tooLong = "' + 'x' * 190 + '";\n')
            message = self.assertViolation(tree, 'long-line', 'ui/home.js')
            self.assertIn('200', message)

    def test_long_comment_line_counts(self):
        with Tree() as tree:
            tree.append('core/format.js', '\n// ' + 'word ' * 50 + '\n')
            self.assertViolation(tree, 'long-line', 'core/format.js')

    def test_baseline_is_not_keyed_by_line_number(self):
        with Tree() as tree:
            # 在文件开头插入无关行、让所有行号后移，检查结果不变
            path = tree.root / STATIC / 'ui/settings.js'
            path.write_text('// one\n// two\n// three\n' + path.read_text(encoding='utf-8'), encoding='utf-8')
            self.assertEqual(tree.errors(), [])

    def test_fixed_violation_is_reported_as_stale_not_failure(self):
        with Tree() as tree:
            # 改掉基线里仍登记着的违规（ui/models.js 的那一处全局 click 监听），应报「过期」而不是失败
            tree.replace('ui/models.js', "document.addEventListener('click'", "$('models').addEventListener('click'")
            errors, stale = check(tree.root)
            self.assertEqual(errors, [])
            self.assertTrue(any('document-listener' in line and 'ui/models.js::click' in line for line in stale))
            code, output = tree.run_main('--strict-stale')
            self.assertEqual(code, 1)
            self.assertIn('--shrink', output)

    def test_shrink_lowers_but_never_adds(self):
        with Tree() as tree:
            tree.replace('ui/models.js', "document.addEventListener('click'", "$('models').addEventListener('click'")
            tree.append('ui/home.js', "\ndocument.addEventListener('click', e => {});\n")  # 一处新违规
            before = tree.baseline()
            code, _ = tree.run_main('--shrink')
            after = tree.baseline()
            self.assertEqual(code, 1, 'the new violation still fails after shrinking')
            self.assertNotIn('ui/models.js::click', after['document-listener'])
            self.assertNotIn('ui/home.js::click', after['document-listener'])
            self.assertEqual(baseline_growth(after, before), [])

    def test_baseline_growth_is_detected(self):
        reference = {'dialog-element': {'ui/a.js': 2}, 'long-line': {'ui/a.js': 5}}
        self.assertEqual(baseline_growth(reference, reference), [])
        self.assertEqual(baseline_growth({'dialog-element': {'ui/a.js': 1}}, reference), [])
        self.assertTrue(baseline_growth({'dialog-element': {'ui/a.js': 3}}, reference))
        self.assertTrue(baseline_growth({'dialog-element': {'ui/b.js': 1}}, reference))
        self.assertTrue(baseline_growth({'undeclared-state': {'x': 1}}, reference))

    def test_base_ref_rejects_a_grown_baseline(self):
        with Tree() as tree:
            def git(*args):
                subprocess.run(['git', '-c', 'user.name=t', '-c', 'user.email=t@example.invalid', *args],
                               cwd=tree.root, check=True, capture_output=True)
            git('init', '-q')
            git('add', str(BASELINE))
            git('commit', '-q', '-m', 'baseline')
            code, _ = tree.run_main('--base-ref', 'HEAD')
            self.assertEqual(code, 0)
            # 把新违规写进基线来“让它通过”
            tree.append('ui/home.js', "\ndocument.addEventListener('click', e => {});\n")
            grown = tree.baseline()
            grown['document-listener']['ui/home.js::click'] = 1
            (tree.root / BASELINE).write_text(json.dumps({'rules': grown}), encoding='utf-8')
            self.assertEqual(tree.errors(), [])
            code, output = tree.run_main('--base-ref', 'HEAD')
            self.assertEqual(code, 1)
            self.assertIn('基线变大', output)

    def test_local_and_ci_runs_are_strict(self):
        # 本地全量与 ci.yml 保持一致：都带 --strict-stale；本地另带 --base-ref。
        script = (ROOT / 'tools/check_all.sh').read_text(encoding='utf-8')
        workflow = (ROOT / '.github/workflows/ci.yml').read_text(encoding='utf-8')
        line = next(l for l in script.splitlines() if 'tools/check_frontend.py' in l and l.startswith('run '))
        self.assertIn('--strict-stale', line)
        self.assertIn('--base-ref', line)
        self.assertIn('check_frontend.py --strict-stale', workflow)

    def test_missing_baseline_is_an_error(self):
        with Tree() as tree:
            (tree.root / BASELINE).unlink()
            self.assertIn('frontend_baseline.json', tree.errors()[0])

    def test_scanner_handles_strings_regexes_and_templates(self):
        src = ("const a = '//x'; // tail\n"
               "const b = /\\/\\/[/]/g.test(x) ? 1 : 2 / 3; /* block\nspans */ state.real = `${state.inTemplate}//kept`;\n")
        code = strip_comments(src)
        self.assertEqual(code.count('\n'), src.count('\n'))
        self.assertIn("'//x'", code)
        self.assertNotIn('tail', code)
        self.assertNotIn('block', code)
        self.assertIn('state.inTemplate', code)
        self.assertIn('//kept', code)
        bare = strip_comments(src, strings=False)
        self.assertNotIn('//x', bare)
        self.assertIn('state.inTemplate', bare)
        with self.assertRaises(ValueError):
            strip_comments('const t = `unterminated')


    # ---------- 规则 7：state 字段的写入者（#353） ----------
    def test_state_written_outside_its_writer(self):
        # cursor 的写入者是 ui/grid.js，focused 的是 ui/detail.js，route 的是 core/route.js
        cases = [('ui/home.js', 'state.cursor = 3;', 'state.cursor', 'ui/grid.js'),
                 ('ui/home.js', 'state.focused ??= null;', 'state.focused', 'ui/detail.js'),
                 ('ui/palette.js', 'state.dataVersion++;', 'state.dataVersion', 'ui/grid.js'),
                 ('ui/palette.js', '--state.generation;', 'state.generation', 'ui/grid.js'),
                 ('ui/shell.js', 'state.route.sort = "x";', 'state.route', 'core/route.js'),
                 ('ui/shell.js', 'state.sel.ids = new Set();', 'state.sel', 'ui/grid.js'),
                 ('ui/catalog.js', 'state.items[0] = null;', 'state.items', 'ui/grid.js'),
                 ('ui/catalog.js', 'Object.assign(state.route, {q: ""});', 'state.route', 'core/route.js'),
                 ('app.js', 'delete state.char;', 'state.char', 'core/meta.js')]
        for path, text, field, writer in cases:
            with self.subTest(text=text), Tree() as tree:
                tree.append(path, '\n' + text + '\n')
                message = self.assertViolation(tree, 'state-writer', f'{path}: {field}（声明的写入者是 {writer}）')
                self.assertIn(text, message)
                self.assertIn('core/state.js', message)

    def test_state_reads_and_the_writer_itself_are_fine(self):
        with Tree() as tree:
            tree.append('ui/home.js', '\nif (state.cursor === 3 || state.cursor >= 2 || state.cursor <= 1 || state.cursor !== 0) go(state.cursor == 1);\n'
                                      'const f = state.items.filter(x => x).length; state.pending.add(1); const s = "state.cursor = 3";\n'
                                      '// state.cursor = 3;\n')
            tree.append('ui/grid.js', '\nstate.cursor = 3; state.items[0] = null; state.sel.ids = new Set();\n')
            tree.append('core/route.js', '\nObject.assign(state.route, {q: ""});\n')
            self.assertEqual(tree.errors(), [])

    def test_state_groups_need_exactly_one_writer(self):
        state = 'core/state.js'
        with Tree() as tree:
            path = tree.root / STATIC / state
            text = path.read_text(encoding='utf-8')
            path.write_text(text.replace('// ---- 详情栏与大图：ui/detail.js ----', '// ---- 详情栏与大图：ui/detail.js、ui/home.js ----'), encoding='utf-8')
            self.assertViolation(tree, 'state-writer', '写了 2 个')
        with Tree() as tree:
            path = tree.root / STATIC / state
            text = path.read_text(encoding='utf-8')
            path.write_text(text.replace('// ---- 详情栏与大图：ui/detail.js ----', '// ---- 详情栏与大图 ----'), encoding='utf-8')
            self.assertViolation(tree, 'state-writer', '写了 0 个')
        with Tree() as tree:
            # 换了写入者：原来的写入者再赋值就报错，新写入者不报
            path = tree.root / STATIC / state
            text = path.read_text(encoding='utf-8')
            path.write_text(text.replace('// ---- 详情栏与大图：ui/detail.js ----', '// ---- 详情栏与大图：ui/home.js ----'), encoding='utf-8')
            message = self.assertViolation(tree, 'state-writer', 'ui/detail.js: state.focused（声明的写入者是 ui/home.js）')
            self.assertIn('setCursor', message)


    # ---------- 规则 8：app.css 的字面量（#349） ----------
    def assertCss(self, css, *needles):
        with Tree() as tree:
            tree.append('app.css', '\n' + css + '\n')
            message = self.assertViolation(tree, 'css-literal', needles[0])
            for needle in needles[1:]:
                self.assertIn(needle, message)
            return message

    def assertCssClean(self, css):
        with Tree() as tree:
            tree.append('app.css', '\n' + css + '\n')
            self.assertEqual(tree.errors(), [])

    def test_hardcoded_font_size(self):
        self.assertCss('.new-thing{font-size:12px}', 'font-size: 12px', 'var(--fs-12)', 'app.css:', '.new-thing')
        self.assertCss('.new-thing{font:600 13px/1.4 inherit}', 'font: 13px', 'var(--fs-13)')
        # 展示字号按 #339 的登记给出 --fs-display-N，并说明只用于一次性的大字
        self.assertCss('.new-title{font-size:28px}', 'font-size: 28px', 'var(--fs-display-28)', '展示字号')
        # 没有这一档：写明最近的一档，并要求先在 Issue 里问
        self.assertCss('.new-thing{font-size:12.5px}', 'font-size: 12.5px', '没有 12.5px 这一档', 'var(--fs-12)', '先在 Issue 里问')

    def test_hardcoded_color(self):
        self.assertCss('.new-thing{color:#123456}', 'color: #123456', '没有这个颜色的令牌', '先在 Issue 里问')
        # 和某个令牌的亮色值相同：指出那个令牌和它的暗色值，提醒核对暗色
        self.assertCss('.new-thing{background:#F4F4F6}', 'background: #f4f4f6', 'var(--canvas)', '暗色下是 #1d1d21')
        self.assertCss('.new-thing{border:1px solid white}', 'border: white')
        self.assertCss('.new-thing{box-shadow:0 1px 2px rgba(0, 0, 0, 0.3)}', 'box-shadow: rgba(0,0,0,.3)')
        self.assertCss('.new-thing{--glow:#abcdef}', '--glow: #abcdef')

    def test_hardcoded_z_index(self):
        self.assertCss('.new-layer{z-index:20}', 'z-index: 20', 'var(--z-popover)')
        self.assertCss('.new-layer{z-index:15}', 'z-index: 15', '没有 z-index 15 这一层', '--z-bar < --z-popover', '先在 Issue 里问')

    def test_hardcoded_radius_and_spacing(self):
        self.assertCss('.new-thing{border-radius:6px}', 'border-radius: 6px', 'var(--radius-6)')
        self.assertCss('.new-thing{border-top-left-radius:50%}', 'border-top-left-radius: 50%', 'var(--radius-round)')
        self.assertCss('.new-thing{padding:0 8px}', 'padding: 8px', 'var(--space-8)')
        self.assertCss('.new-thing{padding:20px 26px}', 'padding: 20px', 'var(--space-20)')
        self.assertCss('.new-thing{gap:5px}', 'gap: 5px', '没有 5px 这一档', '先在 Issue 里问')
        self.assertCss('@media(max-width:1px){.new-thing{column-gap:6px}}', 'column-gap: 6px')

    def test_tokens_zero_and_registered_exceptions_are_fine(self):
        self.assertCssClean(
            # 引用令牌、0、auto、var() 通道的颜色函数、透明和 currentColor
            '.new-thing{font-size:var(--fs-12);padding:0 var(--space-8) 0 0;gap:0;z-index:0;border-radius:0;color:currentColor;'
            'background:transparent;box-shadow:0 0 0 1px rgba(var(--sh),.2);border:1px solid var(--line)}\n'
            # 按 #339 的登记不查：margin，以及不在档位上的 18px 以上 padding、gap
            '.new-thing{margin:7px 13px;padding:26px 18px;gap:18px 40px}\n'
            # 注释、字符串和 url() 里的内容
            '/* .new-thing{color:#fff;font-size:9px} */\n.new-thing::after{content:"#fff; z-index:3";background:url("data:x;#fff")}')

    def test_token_definitions_live_in_tokens_css(self):
        # 令牌定义处只有 css/tokens.css（#705）：那里的变量声明不查，app.css 和别的文件里的照常查颜色
        with Tree() as tree:
            tree.replace('css/tokens.css', ':root{--fw-400:400;', ':root{--new-tone:#abcdef;--fs-9:9px;--fw-400:400;')
            tree.replace('css/tokens.css', ':root[data-theme=dark]{--shadow-hover:', ':root[data-theme=dark]{--new-tone:#123;--shadow-hover:')
            self.assertEqual(tree.errors(), [])
        self.assertCss(':root{--new-tone:#abcdef}', '--new-tone: #abcdef')

    def test_css_folder_literals_have_no_baseline(self):
        # css/ 下的文件没有基线，键带文件名：从 app.css 挪过去的字面量算新增
        with Tree() as tree:
            tree.replace('css/base.css', '.text-meta{color:var(--muted);', '.text-meta{color:#e0e0e6;')
            message = self.assertViolation(tree, 'css-literal', 'css/base.css: color: #e0e0e6')
            self.assertIn('web/static/css/base.css:', message)
            self.assertIn('基线允许 0 处', message)
        with Tree() as tree:
            tree.replace('css/rail.css', '@layer pages{\n', '@layer pages{\n.new-rail{gap:6px;font-size:13px}\n')
            self.assertViolation(tree, 'css-literal', 'css/rail.css: gap: 6px')
            self.assertViolation(tree, 'css-literal', 'css/rail.css: font-size: 13px')

    def test_existing_css_literals_are_frozen(self):
        with Tree() as tree:
            frozen = tree.baseline()['css-literal']
            self.assertIn('font: 9px', frozen)
            self.assertEqual(tree.errors(), [])
            # 基线里已有的字面量，再多用一处也算新增
            tree.append('app.css', '\n.new-thing{font:800 9px/1 inherit}\n')
            message = self.assertViolation(tree, 'css-literal', 'font: 9px')
            self.assertIn(f'基线允许 {frozen["font: 9px"]} 处', message)

    def test_fixed_css_literal_must_be_shrunk(self):
        with Tree() as tree:
            tree.replace('app.css', 'font:800 9px/1', 'font:800 var(--fs-11)/1')
            errors, stale = check(tree.root)
            self.assertEqual(errors, [])
            self.assertTrue(any('css-literal' in line and 'font: 9px' in line for line in stale))
            code, output = tree.run_main('--strict-stale')
            self.assertEqual(code, 1)
            self.assertIn('--shrink', output)
            tree.run_main('--shrink')
            self.assertNotIn('font: 9px', tree.baseline()['css-literal'])
            self.assertEqual(tree.run_main('--strict-stale')[0], 0)

    def test_css_declarations_track_lines_and_nesting(self):
        decls = css_declarations('/* a{b:c} */\n.x{color:red;background:url("a;b{c}")}\n@media(x){.y{gap:1px}}\n'
                                 '@keyframes k{to{opacity:0}}')
        self.assertEqual([(d.selector, d.prop, d.line) for d in decls],
                         [('.x', 'color', 2), ('.x', 'background', 2), ('.y', 'gap', 3), ('to', 'opacity', 4)])
        self.assertEqual(decls[2].context, ('@media(x)', '.y'))
        self.assertEqual(decls[1].value, 'url("a;b{c}")')

    # ---------- 规则 13–17：样式文件的结构和三档令牌（#705） ----------
    def assertPage(self, rule, css, *needles, file='css/library.css'):
        """往 css/ 下一个文件的层里加一段，确认报 rule 的错，报错里有 needles。"""
        layer = {'css/base.css': '@layer base{\n', 'css/overlays.css': '@layer overlays{\n'}.get(file, '@layer pages{\n')
        with Tree() as tree:
            tree.replace(file, layer, layer + css + '\n')
            message = self.assertViolation(tree, rule, needles[0])
            for needle in needles[1:]:
                self.assertIn(needle, message)
            return message

    def assertPageClean(self, css):
        with Tree() as tree:
            tree.replace('css/library.css', '@layer pages{\n', '@layer pages{\n' + css + '\n')
            self.assertEqual(tree.errors(), [])

    def test_weights_shadows_motion_and_tiers(self):
        self.assertPage('css-scale', '.x{font-weight:700}', 'css/library.css: font-weight: 700', 'var(--fw-750)')
        self.assertPage('css-scale', '.x{font-weight:bold}', 'font-weight: bold', 'var(--fw-750)')
        self.assertPage('css-scale', '.x{font:500 var(--fs-12)/1.4 inherit}', 'font-weight: 500', 'var(--fw-400)')
        self.assertPage('css-scale', '.x{box-shadow:0 4px 12px rgba(var(--sh),.2)}', 'box-shadow: 0 4px 12px', 'var(--shadow-hover)', '描边环')
        self.assertPage('css-scale', '.x{box-shadow:var(--sheet-shadow)}', 'box-shadow: var(--sheet-shadow)')
        self.assertPage('css-scale', '.x{transition:opacity .2s ease}', 'transition-duration: .2s', 'var(--dur-1)')
        self.assertPage('css-scale', '.x{transition:opacity .2s ease}', 'transition-timing-function: ease', 'var(--ease)')
        self.assertPage('css-scale', '.x{transition:transform var(--dur-2) cubic-bezier(.3, 1, .3, 1)}',
                        'transition-timing-function: cubic-bezier(.3,1,.3,1)')
        self.assertPage('css-scale', '.x{animation:pop 300ms var(--ease)}', 'animation-duration: 300ms')
        self.assertPage('css-scale', '.x{font-size:var(--fs-14)}', 'font-size: var(--fs-14)', 'var(--fs-13)')
        self.assertPage('css-scale', '.x{font-size:var(--fs-display-32)}', 'font-size: var(--fs-display-32)', 'var(--fs-display-28)')
        self.assertPage('css-scale', '.x{border-radius:var(--radius-10)}', 'border-radius: var(--radius-10)', 'var(--radius-8)', '只留给旧样式')

    def test_scale_tokens_and_exempt_forms_are_fine(self):
        self.assertPageClean(
            '.x{font-weight:var(--fw-600);font:var(--fw-750) var(--fs-display-40)/1.1 inherit;font-size:var(--fs-11);border-radius:var(--radius-18)}\n'
            # 三档阴影、不带模糊的描边环、none
            '.x{box-shadow:var(--shadow-pop)}\n.y{box-shadow:0 0 0 2px var(--accent),inset 0 1px 0 var(--line)}\n.z{box-shadow:none}\n'
            # 三档时长和两条缓动、0 时长；延迟不查；循环动画不查
            '.x{transition:opacity var(--dur-1) var(--ease),transform var(--dur-3) var(--ease-spring) 40ms}\n.y{transition:none;transition-duration:0s}\n'
            '.z{animation:spin 1s linear infinite}\n'
            # 嵌套的内圆角：外层 − 间距
            '.x{border-radius:calc(var(--radius-18) - var(--space-6))}')

    def test_existing_scale_issues_are_frozen_per_file(self):
        with Tree() as tree:
            frozen = tree.baseline()['css-scale']
            self.assertIn('app.css: font-weight: 700', frozen)
            self.assertEqual(tree.errors(), [])
            tree.append('app.css', '\n.new-thing{font-weight:700}\n')
            message = self.assertViolation(tree, 'css-scale', 'app.css: font-weight: 700')
            self.assertIn(f'基线允许 {frozen["app.css: font-weight: 700"]} 处', message)

    def test_css_files_live_in_their_layer(self):
        with Tree() as tree:
            tree.append('css/library.css', '.loose{color:var(--ink)}\n')
            self.assertViolation(tree, 'css-layer', 'css/library.css: .loose', '@layer pages')
        with Tree() as tree:
            tree.replace('css/detail.css', '@layer pages{', '@layer base{')
            self.assertViolation(tree, 'css-layer', 'css/detail.css: @layer base', '@layer pages')
        with Tree() as tree:
            tree.replace('css/overlays.css', '@layer overlays{', '@layer pages{')
            self.assertViolation(tree, 'css-layer', 'css/overlays.css', '@layer overlays')
        with Tree() as tree:
            tree.replace('css/rail.css', '@layer pages{', '@layer legacy, pages;\n@layer pages{')
            self.assertViolation(tree, 'css-layer', 'css/rail.css: @layer legacy, pages', '第一行')
        with Tree() as tree:
            tree.replace('css/tokens.css', '@layer legacy, tokens, base, pages, overlays;', '')
            self.assertViolation(tree, 'css-layer', 'css/tokens.css: 没有层的声明')

    def test_class_substring_and_descendant_has(self):
        self.assertPage('css-selector', '.grid [class*=card-]{opacity:.5}', 'css/library.css: 类名子串选择器', '2.4 倍')
        self.assertPage('css-selector', 'div[class^="x"]{opacity:.5}', '类名子串选择器')
        self.assertPage('css-selector', '.page:has(.selected){opacity:.5}', '后代形式的 :has()', ':has(> …)')
        self.assertPageClean('.row:has(> .avatar){gap:0}\n.item:has(+ .item:hover){opacity:.9}\n.x[class~=y]{opacity:.9}')

    def test_variables_must_be_defined(self):
        self.assertPage('css-var', '.x{color:var(--inkk)}', 'css/library.css: --inkk', 'JS_VARIABLES')
        # 别的文件里的局部变量不能借用
        self.assertPage('css-var', '.x{padding:0 var(--gutter)}', 'css/library.css: --gutter')
        # 回退值里的变量同样要有定义
        self.assertPage('css-var', '.x{color:var(--tint-wash,var(--nope))}', 'css/library.css: --nope')
        # 同一个文件里定义的局部变量、tokens.css 的令牌、登记的 JS 变量都可以
        self.assertPageClean('.x{--cols:4;grid-template-columns:repeat(var(--cols),1fr);background:var(--tint-wash);width:calc(var(--p) * 1%)}')

    def test_backdrop_filter_only_on_glass(self):
        self.assertPage('backdrop-filter', '.card{backdrop-filter:blur(8px)}', 'css/library.css: backdrop-filter', '.glass', '显存')
        self.assertPage('backdrop-filter', '.glass-card{-webkit-backdrop-filter:blur(8px)}', 'css/base.css: backdrop-filter', file='css/base.css')
        with Tree() as tree:
            tree.append('app.css', '\n.new-thing{backdrop-filter:blur(4px)}\n')
            self.assertViolation(tree, 'backdrop-filter', 'app.css: backdrop-filter')

    def test_newer_css_only_in_registered_enhancements(self):
        self.assertPage('css-compat', '.x{content-visibility:auto}', 'css/library.css: content-visibility', '@supports', '退路')
        self.assertPage('css-compat', '.x{transition-timing-function:linear(0,1)}', 'linear() 缓动')
        self.assertPage('css-compat', '@starting-style{.x{opacity:0}}', '@starting-style')
        self.assertPage('css-compat', '::view-transition-old(root){opacity:0}', 'View Transitions 的伪元素')
        self.assertPage('css-compat', '.x{color:light-dark(var(--ink),var(--sheet))}', 'light-dark()')
        # 在 @supports 里但前面没写退路，或者 @supports 测的是别的东西，都不算登记
        self.assertPage('css-compat', '@supports (content-visibility:auto){.x{content-visibility:auto}}', 'content-visibility')
        self.assertPage('css-compat', '/* 增强。退路：照常画。 */\n@supports (display:grid){.x{content-visibility:auto}}', 'content-visibility')
        # 嵌套和相对颜色是硬规则，没有增强的写法
        self.assertPage('css-compat', '.x{color:var(--ink);.y{color:var(--muted)}}', 'CSS 嵌套', '展开')
        self.assertPage('css-compat', '.x{color:rgb(from var(--accent) r g b / .5)}', '相对颜色语法', 'color-mix()')
        self.assertPageClean('.x{opacity:1}\n/* 增强：content-visibility（Safari 18.0）。退路：上面照常画全部。 */\n'
                             '@supports (content-visibility:auto){.x{content-visibility:auto}}\n'
                             '/* 增强：入场的起始样式（Safari 17.5）。退路：没有起始样式时直接出现。 */\n@starting-style{.x{opacity:0}}\n'
                             '@keyframes k{from{opacity:0}to{opacity:1}}')

    # ---------- 新规则首次登记存量：--init-rule（#349） ----------
    def test_init_rule_registers_only_a_new_rule(self):
        with Tree() as tree:
            before = tree.baseline()
            tree.write_baseline({k: v for k, v in before.items() if k != 'css-literal'})
            errors = tree.errors()
            self.assertTrue(errors)
            self.assertIn('--init-rule css-literal', errors[0])
            code, output = tree.run_main('--init-rule', 'css-literal')
            self.assertEqual(code, 0, output)
            self.assertEqual(tree.baseline(), before)
            self.assertEqual(tree.errors(), [])
            # 已登记的、硬规则、不存在的规则都拒绝，基线不动
            for name in ('css-literal', 'long-line', 'page-name', 'no-such-rule'):
                with self.subTest(name=name), redirect_stderr(io.StringIO()):
                    self.assertEqual(tree.run_main('--init-rule', name)[0], 1)
            self.assertEqual(tree.baseline(), before)

    def test_base_ref_accepts_a_first_registration(self):
        with Tree() as tree:
            full = tree.baseline()
            tree.write_baseline({k: v for k, v in full.items() if k != 'css-literal'})
            tree.git('init', '-q')
            tree.git('add', str(BASELINE))
            tree.git('commit', '-q', '-m', 'before rule 8')
            tree.run_main('--init-rule', 'css-literal')
            code, output = tree.run_main('--strict-stale', '--base-ref', 'HEAD')
            self.assertEqual(code, 0, output)
            self.assertIn('首次登记', output)
            # 首次登记也不能多于现状
            inflated = json.loads(json.dumps(full))
            inflated['css-literal']['font: 9px'] += 1
            tree.write_baseline(inflated)
            code, output = tree.run_main('--base-ref', 'HEAD')
            self.assertEqual(code, 1)
            self.assertIn('首次登记多于现状', output)

    def test_base_ref_rejects_a_rule_dropped_from_the_baseline(self):
        # 基准上的脚本已经有这条规则，基线里却没有这一项：说明有人删了它，照常算基线变大
        with Tree() as tree:
            full = tree.baseline()
            tree.write_baseline({k: v for k, v in full.items() if k != 'css-literal'})
            script = tree.root / 'tools/check_frontend.py'
            shutil.copyfile(ROOT / 'tools/check_frontend.py', script)
            tree.git('init', '-q')
            tree.git('add', str(BASELINE), 'tools/check_frontend.py')
            tree.git('commit', '-q', '-m', 'rule 8 exists')
            tree.run_main('--init-rule', 'css-literal')
            code, output = tree.run_main('--base-ref', 'HEAD')
            self.assertEqual(code, 1)
            self.assertIn('基线变大', output)
            self.assertNotIn('首次登记', output)


if __name__ == '__main__':
    unittest.main()
