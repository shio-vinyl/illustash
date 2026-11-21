#!/usr/bin/env python3
"""样式清单页（tools/styleguide/）与实现的同步检查（#352，属 #316）。

清单页的样例分两种：调用产品代码生成的（tools/styleguide/live.js 调 web/static/ui 的函数，摆进
index.html 里 `data-sg-live` 的舞台），和照模板手写的（index.html 里的纯 HTML，条目标“手写”）。
AGENTS.md 要求写界面前先看这页、照它复用类名，所以它一过期就会把代理带偏。本脚本只做便宜的机械检查，
接在 tools/check_all.sh 和 ci.yml 里：

  1 census     census.js 必须等于 census.py 对当前样式的输出（样式改了没重跑、手改 census.js 都会报）
  2 class      index.html 里元素用到的类，必须在 web/static 的样式（app.css 和 css/ 下的文件，#705）里有样式；
               页面外壳的类（sg- 前缀，以及 styleguide.css 里挂在 .sg-* 选择器上的修饰类）必须在 styleguide.css 里有定义；
               只给 JS 当钩子、本来就没有样式的类登记在 HOOK_CLASSES，并且必须仍出现在 web/static 的代码里
  3 listed     条目标题里 <span class="sg-cls"> 列出的 `.类名`，同样必须在 web/static 的样式里有
  4 live       live.js 从 web/static 导入的每个名字，在那个文件里确实导出（产品代码改名、删导出会报）；
               live.js 的 SHELLS 列的外壳 id 在 web/static/index.html 里存在；
               index.html 的 data-sg-live 和 live.js 的 sample('名字', …) 一一对应；
               组件和壳（c-*、s-* 两组）的每个条目都标明来源：有 data-sg-live 舞台的标“调用生成”（sg-tag live），
               其余标“手写”（sg-tag hand），不能两个都标或都不标

调用生成的样例画出来什么样，要在浏览器里看（页面会把画不出来的样例连同原因写在舞台里）；这里不启动浏览器，
也不依赖 Node。手写样例的 HTML 结构和模板是否一致，这里查不出来。用法：python3 tools/check_styleguide.py [--root 目录]
"""
from html.parser import HTMLParser
from pathlib import Path
import argparse
import functools
import importlib.util
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
STATIC = Path('web/static')
STYLES = 'web/static 的样式（app.css 和 css/ 下的文件）'
GUIDE = Path('tools/styleguide')
CENSUS_FIX = '运行 `python3 tools/styleguide/census.py`，把改写后的 tools/styleguide/census.js 一起提交'
# 样例照抄了应用模板里的这些类：它们只给 JS 当选择器或钩子，样式里本来没有。
# 每项写明出处；应用代码里不再用它时检查会报，届时从这里和样例里一起删掉。
HOOK_CLASSES = {
    'tagged-dialog': 'ui/catalog.js 打过标签的图对话框',
    'tour-text': 'ui/guide.js 引导气泡正文',
    'work-dialog': 'ui/dialog.js、ui/catalog.js 作品对话框',
}
LIVE_FIX = '照报错改 tools/styleguide/live.js（产品代码改了名字就跟着改；确实不再有生成函数的，把样例改回手写并标“手写”）'
# 规则 4 只管这两组：组件（c-*）和带行为的壳（s-*）。令牌表和附录不是组件样例。
_ITEM_SECTIONS = re.compile(r'[cs]-')
_CLASS_IN_SELECTOR = re.compile(r'\.(-?[_a-zA-Z][\w-]*)')


def _census_module():
    """按路径加载 tools/styleguide/census.py（tools/styleguide 不是包）。导入不会写文件。"""
    spec = importlib.util.spec_from_file_location('styleguide_census', ROOT / GUIDE / 'census.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CENSUS = _census_module()


# selector_classes、_expected_census 按原文缓存：只读参数和模块常量（CENSUS）；测试要改模块常量先调 cache_clear()。
@functools.lru_cache(maxsize=128)
def selector_classes(css_text):
    """CSS 里所有选择器出现过的类名（含 @media 里的规则；声明体里的值不算）。"""
    found = set()
    for selector, _, _ in CENSUS.parse_rules(CENSUS.strip_comments(css_text)):
        found.update(_CLASS_IN_SELECTOR.findall(re.sub(r'\[[^\]]*\]', '', selector)))
    return frozenset(found)


def _compounds(part):
    """把一条选择器按组合符（空白、>、+、~）切成复合选择器。"""
    return [c for c in re.split(r'\s*[>+~]\s*|\s+', re.sub(r'\[[^\]]*\]|::?[\w-]+(\([^)]*\))?', '', part).strip()) if c]


def shell_classes(css_text):
    """styleguide.css 选择器里的类，分成三类：(sg- 类, 修饰类, 引用的应用类)。

    修饰类是页面外壳自己的非 sg- 类：和 .sg-* 写在同一个复合选择器里（`.sg-stage.col`），
    或者挂在 .sg-* 下面、带元素名的复合选择器里（`.sg-table td.n`）。
    其余非 sg- 类（如 `.sg-frozen .toast` 的 toast）是在改写应用组件，必须仍在 web/static 的样式里。
    """
    sg, modifiers, app = set(), set(), {}
    for selector, _, _ in CENSUS.parse_rules(CENSUS.strip_comments(css_text)):
        for part in selector.split(','):
            scoped = False
            for compound in _compounds(part):
                names = _CLASS_IN_SELECTOR.findall(compound)
                own = {n for n in names if n.startswith('sg-')}
                sg |= own
                others = [n for n in names if not n.startswith('sg-')]
                if own or (scoped and re.match(r'[a-z]', compound)):
                    modifiers.update(others)
                else:
                    for n in others:
                        app.setdefault(n, part.strip())
                scoped = scoped or bool(own)
    return sg, modifiers, app


class _Page(HTMLParser):
    """收集 index.html 里每个元素的类名（带行号）、sg-cls 标题里 <code> 列出的类，以及各条目的来源标注。"""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.used = []      # [(类名, 行号)]
        self.listed = []    # [(类名, 行号)]
        self.live = []      # [(data-sg-live 的名字, 行号)]
        self.items = []     # [{'line', 'title', 'section', 'tags': set, 'live': [名字]}]，只收 c-*、s-* 两组
        self._depth = 0     # 位于 sg-cls 的 <span> 里时，未闭合的 <span> 层数；只数 span，不受省略结束标签影响
        self._code = None   # 正在读的 sg-cls <code> 的文字与行号
        self._section = ''
        self._item = None   # 正在读的条目，以及它里面未闭合的 <div> 层数
        self._item_divs = 0
        self._title = None

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        classes = (attrs.get('class') or '').split()
        line = self.getpos()[0]
        self.used.extend((c, line) for c in classes)
        if tag == 'section' and attrs.get('id'):
            self._section = attrs['id']
        if attrs.get('data-sg-live') is not None:
            self.live.append((attrs['data-sg-live'], line))
            if self._item is not None:
                self._item['live'].append(attrs['data-sg-live'])
        if tag == 'div' and self._item is not None:
            self._item_divs += 1
        elif tag == 'div' and 'sg-item' in classes and _ITEM_SECTIONS.match(self._section):
            self._item, self._item_divs = {'line': line, 'title': '', 'section': self._section, 'tags': set(), 'live': []}, 1
            self.items.append(self._item)
        if self._item is not None and tag == 'span' and 'sg-tag' in classes:
            self._item['tags'].update(c for c in classes if c != 'sg-tag')
        if self._item is not None and tag == 'h3' and not self._item['title']:
            self._title = []
        if tag == 'span' and (self._depth or 'sg-cls' in classes):
            self._depth += 1
        elif tag == 'code' and self._depth:
            self._code = ['', line]

    def handle_endtag(self, tag):
        if tag == 'div' and self._item is not None:
            self._item_divs -= 1
            if not self._item_divs:
                self._item = None
        if tag == 'h3' and self._title is not None:
            self._item['title'], self._title = ''.join(self._title).strip(), None
        if tag == 'span' and self._depth:
            self._depth -= 1
        elif tag == 'code' and self._code is not None:
            text, line = self._code
            # 只看类选择器；`#id`、元素名、`::backdrop` 之类不查。去掉属性选择器里的内容免得误读。
            self.listed.extend((c, line) for c in _CLASS_IN_SELECTOR.findall(re.sub(r'\[[^\]]*\]', '', text)))
            self._code = None

    def handle_data(self, data):
        if self._code is not None:
            self._code[0] += data
        if self._title is not None:
            self._title.append(data)


def _lines(lines):
    unique = sorted(set(lines))
    shown = '、'.join(str(n) for n in unique[:6])
    return shown + (f' 等 {len(unique)} 处' if len(unique) > 6 else '')


@functools.lru_cache(maxsize=16)
def _expected_census(css_items):
    # 统计包括所有样式的字节指纹，缓存键保留原文与读取顺序。
    return CENSUS.render(dict(css_items))


def check_census(root):
    path = root / GUIDE / 'census.js'
    raw = CENSUS.read_files(root)
    expected = _expected_census(tuple(raw.items()))
    if not path.is_file():
        return [f'[样式清单 · census] 缺少 {GUIDE}/census.js。改法：{CENSUS_FIX}。']
    actual = path.read_bytes().decode('utf-8')
    if actual == expected:
        return []
    now = CENSUS.build(raw)
    recorded = re.search(r'"sha256":\s*"([0-9a-f]{64})"', actual)
    lines = re.search(r'"lines":\s*(\d+)', actual)
    if not recorded or recorded.group(1) != now['sha256']:
        was = f'{lines.group(1) if lines else "?"} 行、sha256 {recorded.group(1)[:12] if recorded else "（未记录）"}'
        return [f'[样式清单 · census] census.js 过期：统计时的样式是 {was}，现在是 {now["lines"]} 行、'
                f'sha256 {now["sha256"][:12]}。{STYLES}改过之后没有重跑统计，清单页的令牌表会是旧的。改法：{CENSUS_FIX}。']
    return [f'[样式清单 · census] census.js 与 census.py 现在的输出不一致，但样式没变：census.js 被手改过，'
            f'或者改了 census.py 的口径没有重跑。改法：{CENSUS_FIX}，不要手改 census.js。']


def check_classes(root):
    app = set().union(*(selector_classes(text) for text in CENSUS.read_files(root).values()))
    shell, modifiers, overrides = shell_classes((root / GUIDE / 'styleguide.css').read_text(encoding='utf-8'))
    page = _Page()
    page.feed((root / GUIDE / 'index.html').read_text(encoding='utf-8'))
    page.close()
    missing, missing_shell, missing_listed, hooks_used = {}, {}, {}, set()
    for name, line in page.used:
        if name.startswith('sg-'):
            if name not in shell:
                missing_shell.setdefault(name, []).append(line)
        elif name in app or name in modifiers:
            continue
        elif name in HOOK_CLASSES:
            hooks_used.add(name)
        else:
            missing.setdefault(name, []).append(line)
    for name, line in page.listed:
        if name not in app and name not in HOOK_CLASSES:
            missing_listed.setdefault(name, []).append(line)
    errors = []
    for name, lines in sorted(missing.items()):
        errors.append(f'[样式清单 · class] {GUIDE}/index.html 第 {_lines(lines)} 行：样例用了类 .{name}，{STYLES}里没有它的样式。'
                      '约定：样例是给人和代理照抄的，只能用样式里现有的类；类改名或删掉时要同步改这页。'
                      '改法：照样式现在的类名改样例；类已删就删掉这个样例或换成替代写法；'
                      '如果它只是 JS 钩子、本来就没有样式，把它加进 tools/check_styleguide.py 的 HOOK_CLASSES 并写明出处。')
    for name, lines in sorted(missing_shell.items()):
        errors.append(f'[样式清单 · class] {GUIDE}/index.html 第 {_lines(lines)} 行：页面外壳类 .{name} 在 {GUIDE}/styleguide.css 里没有定义。'
                      '改法：在 styleguide.css 里补上，或改用已有的 sg- 类；sg- 前缀只给清单页自己用，不要拿来画应用组件。')
    for name, lines in sorted(missing_listed.items()):
        errors.append(f'[样式清单 · listed] {GUIDE}/index.html 第 {_lines(lines)} 行：条目标题列出了 .{name}，{STYLES}里没有它的样式。'
                      '改法：照样式现在的类名改标题里的 <code>，类已删就从标题里去掉。')
    for name, selector in sorted(overrides.items()):
        if name not in app and name not in modifiers:
            errors.append(f'[样式清单 · class] {GUIDE}/styleguide.css 的选择器 `{selector}` 引用了 .{name}，{STYLES}里已经没有它。'
                          '改法：按样式现在的类名改这条覆盖；组件已删就删掉这条。')
    code = '\n'.join(p.read_text(encoding='utf-8') for pattern in ('*.html', '*.js', 'core/*.js', 'ui/*.js')
                     for p in sorted((root / STATIC).glob(pattern)))
    for name, where in sorted(HOOK_CLASSES.items()):
        if not re.search(rf'(?<![\w-]){re.escape(name)}(?![\w-])', code):
            errors.append(f'[样式清单 · class] HOOK_CLASSES 里的 .{name}（{where}）在 web/static 的代码里已经找不到。'
                          '改法：从 tools/check_styleguide.py 的 HOOK_CLASSES 删掉这一项，并删改用到它的样例。')
        elif name not in hooks_used and not any(n == name for n, _ in page.listed):
            errors.append(f'[样式清单 · class] HOOK_CLASSES 里的 .{name} 已不在 {GUIDE}/index.html 里用到。'
                          '改法：从 tools/check_styleguide.py 的 HOOK_CLASSES 删掉这一项。')
    return errors


_EXPORT_DECL = re.compile(r'^export\s+(?:default\s+)?(?:async\s+)?(?:function\s*\*?|const|let|var|class)\s+([A-Za-z_$][\w$]*)', re.M)
_EXPORT_LIST = re.compile(r'^export\s*\{([^}]*)\}', re.M)
_IMPORT = re.compile(r'^import\s*\{([^}]*)\}\s*from\s*[\'"]([^\'"]+)[\'"]', re.M)
_SAMPLE = re.compile(r'^sample\(\s*[\'"]([\w-]+)[\'"]', re.M)


def js_exports(src):
    """一个 ES 模块导出的名字：`export function/const/let/class 名字` 和 `export {a, b as c}`（取对外的名字）。"""
    names = set(_EXPORT_DECL.findall(src))
    for group in _EXPORT_LIST.findall(src):
        for part in group.split(','):
            part = part.strip()
            if part:
                names.add(re.split(r'\s+as\s+', part)[-1].strip())
    return names


def _line_of(src, index):
    return src.count('\n', 0, index) + 1


def check_live(root):
    live_path = root / GUIDE / 'live.js'
    page = _Page()
    page.feed((root / GUIDE / 'index.html').read_text(encoding='utf-8'))
    page.close()
    errors = []
    if not live_path.is_file():
        if page.live:
            errors.append(f'[样式清单 · live] {GUIDE}/index.html 有 data-sg-live 舞台，但缺少 {GUIDE}/live.js。改法：{LIVE_FIX}。')
        src = ''
    else:
        src = live_path.read_text(encoding='utf-8')
    static = (root / STATIC).resolve()
    for m in _IMPORT.finditer(src):
        names, spec, line = m.group(1), m.group(2), _line_of(src, m.start())
        target = (root / GUIDE / spec).resolve()
        try:
            shown = target.relative_to(root.resolve()).as_posix()
        except ValueError:
            shown = spec
        if not target.is_relative_to(static):
            errors.append(f'[样式清单 · live] {GUIDE}/live.js 第 {line} 行从 {spec} 导入，它不在 web/static 下。'
                          '约定：live.js 只调用产品代码，不另写一套生成函数。改法：改成从 web/static 里的模块导入。')
            continue
        if not target.is_file():
            errors.append(f'[样式清单 · live] {GUIDE}/live.js 第 {line} 行导入的 {shown} 不存在：模块改名或删掉了。改法：{LIVE_FIX}。')
            continue
        exported = js_exports(target.read_text(encoding='utf-8'))
        for part in names.split(','):
            name = re.split(r'\s+as\s+', part.strip())[0].strip()
            if name and name not in exported:
                errors.append(f'[样式清单 · live] {GUIDE}/live.js 第 {line} 行从 {shown} 导入 {name}，但那个文件没有导出它：'
                              f'函数改名、删掉，或去掉了 export。改法：{LIVE_FIX}；只为清单页导出的函数，导出处有注释说明，不要去掉。')
    shells = re.search(r'^const SHELLS = \[([^\]]*)\]', src, re.M)
    if shells:
        app_page = (root / STATIC / 'index.html').read_text(encoding='utf-8')
        for shell in re.findall(r"'([\w-]+)'", shells.group(1)):
            if not re.search(rf'\bid="{re.escape(shell)}"', app_page):
                errors.append(f'[样式清单 · live] {GUIDE}/live.js 的 SHELLS 要从 web/static/index.html 取 #{shell}，那里已经没有这个 id。'
                              f'改法：{LIVE_FIX}。')
    defined = {}
    for m in _SAMPLE.finditer(src):
        defined.setdefault(m.group(1), _line_of(src, m.start()))
    used = {}
    for name, line in page.live:
        used.setdefault(name, []).append(line)
    for name, lines in sorted(used.items()):
        if name not in defined:
            errors.append(f'[样式清单 · live] {GUIDE}/index.html 第 {_lines(lines)} 行的舞台要画 data-sg-live="{name}"，'
                          f'{GUIDE}/live.js 里没有 sample(\'{name}\', …)。改法：在 live.js 里补上，或改正舞台上的名字。')
        elif len(lines) > 1:
            errors.append(f'[样式清单 · live] data-sg-live="{name}" 在 {GUIDE}/index.html 第 {_lines(lines)} 行出现了不止一次。'
                          '改法：一个样例只放一个舞台，要多画几种就在 sample 里返回几个节点。')
    for name, line in sorted(defined.items()):
        if name not in used:
            errors.append(f'[样式清单 · live] {GUIDE}/live.js 第 {line} 行定义了 sample(\'{name}\', …)，index.html 里没有对应的舞台。'
                          f'改法：在条目里加 <div class="sg-stage" data-sg-live="{name}"></div>，或删掉这个 sample。')
    for item in page.items:
        where = f'{GUIDE}/index.html 第 {item["line"]} 行的条目「{item["title"]}」'
        live, hand = 'live' in item['tags'], 'hand' in item['tags']
        if item['live'] and not live:
            errors.append(f'[样式清单 · live] {where} 有调用生成的舞台，标题里没有标“调用生成”。'
                          '改法：在 <h3> 后面加 <span class="sg-tag live">调用生成</span>。')
        if item['live'] and hand:
            errors.append(f'[样式清单 · live] {where} 有调用生成的舞台，却标了“手写”。'
                          '改法：一个条目只用一种来源：去掉“手写”，手写的部分挪到单独的条目里。')
        if not item['live'] and live:
            errors.append(f'[样式清单 · live] {where} 标了“调用生成”，但没有 data-sg-live 舞台。'
                          '改法：改成手写就标 <span class="sg-tag hand">手写</span>，并在 sg-src 里写明来源文件。')
        if not item['live'] and not hand and not live:
            errors.append(f'[样式清单 · live] {where} 没有标明样例的来源。约定：组件和壳的条目要么调用产品代码生成'
                          '（舞台写 data-sg-live，标“调用生成”），要么是手写（标 <span class="sg-tag hand">手写</span>，sg-src 写“来源：文件 函数”）。'
                          '改法：能调用生成函数的改成调用；没有生成函数的标“手写”。')
    return errors


def check(root=ROOT):
    return check_census(root) + check_classes(root) + check_live(root)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--root', type=Path, default=ROOT)
    args = parser.parse_args(argv)
    errors = check(args.root)
    for error in errors:
        print(error)
    if errors:
        print(f'\n样式清单与实现不同步：{len(errors)} 项。清单页说明见 tools/styleguide/index.html 开头。')
        return 1
    print('样式清单与实现同步：census.js 是最新的，样例用到的类都有定义，调用生成的样例导入的函数都在。')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
