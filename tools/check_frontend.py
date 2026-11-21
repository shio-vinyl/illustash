"""web/static 前端结构检查（#320，属于 #316）。

纯 Python，不依赖 Node 工具，也不引入构建步骤。把现状里的坏模式冻结在基线文件里，只拦新增，做法同
tools/check_architecture.py 对 crate 依赖方向的检查。规则（说明见 AGENTS.md「前端结构约定」）：

  1 import-core-ui       core/ 不得导入 ui/（硬规则，没有基线）
  1 import-cycle         ui/ 循环导入里的导入边；现有的登记为基线，不得新增边或新的环
  2 undeclared-state     `state.<字段>` 必须在 core/state.js 声明
  3 document-listener    不得新增 document/window/body 级的 keydown、keyup、click、mousedown、pointerdown、change 监听
  4 dialog-element       ui/dialog.js 以外不得 `document.createElement('dialog')`，也不得在字符串里拼 `<dialog`（硬规则，没有基线）
  5 long-line            不得新增超过 200 字符的行（app.js、core/、ui/）
  6 page-name            页面名只能在 core/pages.js（页面注册表，#342）里比较（硬规则，没有基线）
  7 state-writer         state 字段只能由 core/state.js 组标题写明的写入者文件赋值（#353；硬规则，没有基线）
  8 css-literal          app.css 和 css/ 下的样式不得新增写死的字号、圆角、gap/padding、z-index 和颜色，改用 css/tokens.css 的令牌（#349、#705）
  9 ime-composing        每个 keydown/keyup/keypress 处理函数先问 core/keys.js 的 composing(e)；别处不得直接读 isComposing / keyCode（#388）
 10 inner-html           innerHTML / outerHTML / insertAdjacentHTML 的右边只能是不带插值的字符串字面量，数据经 ui/el.js 写（#704）
 11 animation-entry      requestAnimationFrame 只在 ui/frame.js、ui/motion.js；.animate( 和 startViewTransition 只在 ui/motion.js（#704）
 12 safari17-js          Safari 17.0（macOS 14 的 WKWebView）没有的 JS 能力只在登记的增强处；Safari 没有的不用（#704；硬规则，没有基线；
                         也扫经典脚本 i18n.js、splash.js、theme.js）
 13 css-scale            字重、阴影、动效的时长和缓动只用三档令牌；字号只用 11/12/13/20/28/40，圆角不用冻结给旧样式的 2/6/10/14/16（#705）
 14 css-selector         不写类名子串选择器（[class*=] 一类）和后代形式的 :has()（:has(.x)；:has(> .x)、:has(+ .x) 可以）（#469）
 15 css-var              var(--名) 必须有定义：在 css/tokens.css，或在 JS_VARIABLES 登记的「由 JS 写的变量」里；只有 app.css 还认同一个文件里的
                         旧局部变量（--gutter）。css/ 下不认局部变量，所以 `--x: 13px` 绕不过规则 8、13（硬规则）
 16 backdrop-filter      backdrop-filter 只写在 css/base.css 的玻璃表面（.glass、.glass-bar、对话框背后的 .glass-scrim::backdrop）
 17 css-compat           Safari 17.0 没有的 CSS 能力只写在登记的增强块里：探测它的 @supports（测不出来的写成 @starting-style 这类块本身），
                         块前面紧挨着写明退路的注释（@supports not (…) 不算）；CSS 嵌套、相对颜色语法和 ::backdrop 里的 var() 不用（硬规则）
 18 css-layer            css/ 下的文件整个写在自己的 @layer 里（硬规则）：tokens.css 在 tokens，base.css 在 base，overlays.css 在 overlays，其余在 pages
 19 css-order            层的先后靠样式表的顺序：index.html（和样式清单页）第一个样式表是 css/tokens.css，tokens.css 的第一条是 @layer 先后
                         声明；index.html 引了 css/ 下的每个文件（硬规则）
 20 tint-writer          --tint 只经 ui/tint.js 的 setTint() 写（它同时加 .tint）；别的模块不出现 --tint，样式里也不声明 --tint
                         （tokens.css 里 :root 的中性默认值除外）（硬规则）
 21 ui-file-length       ui/ 下的文件不超过 600 行；现有的大文件按超出的行数冻结为基线，只能缩小（#703「可维护性」，#706）
 22 ui-request           ui/ 不直接发请求：不从 core/session.js 导入 api、post、shared，也不调 fetch()、sendBeacon，不建 EventSource、XMLHttpRequest、
                         WebSocket；请求写在 core/ 的取数层里
                         （docs/WEB_V2.md「页面样板」，#706）

基线（tools/frontend_baseline.json）按「文件 + 内容特征」登记（导入边、字段名、每个文件的数量），不按行号，
所以文件里别处的改动不会让它失效。基线只能缩小：`--shrink` 只调低、不新增；`--base-ref REF` 在基线比 REF
上的大时报错；`--strict-stale` 把「基线里已不再出现的项」也算作失败，逼着改好违规的人同时收紧基线。
tools/check_all.sh 两个都带上。新规则就是 new_rules() 里多一个 Rule；带基线的新规则用 `--init-rule <规则名>` 登记一次存量，
和规则在同一个 PR 里提交（`--init` 在基线已存在时拒绝；手工往基线里加会被 `--base-ref` 当成基线变大）。`--base-ref` 把
「基准上的基线和检查脚本里都没有」的规则认作首次登记，不算变大，但登记的数量不能多于现状。

规则 8 按声明扫描（选择器 + 属性 + 值），基线键是「属性: 字面量」（如 `background: #e0e0e6`），不含选择器，改名、挪位置
不受影响；css/ 下的文件键前加文件名（如 `css/base.css: color: #fff`），所以从 app.css 挪过去的字面量算新增。规则 13–17 的基线键
同样带文件名。令牌定义处（css/tokens.css 里的 `--名:值`）不查；按 #339 的登记，margin 和不在档位上的
18px 以上 padding、gap 不查；z-index 的 0、长度的 0 不算。颜色查十六进制、rgb()/hsl() 等颜色函数（通道里用了 var() 的
不算，如 `rgba(var(--sh),.1)`），颜色名只在颜色类属性里查。别处声明的自定义属性（`.x{--gutter:26px}`）只查颜色。

规则 9 只认「处理函数里出现了 composing(」，认不出它是不是真的放在最前面、是不是对着同一个事件；处理函数交给变量、
再从别处传进来（`addEventListener('keydown', handlers[kind])`）时找不到函数体，按没有调用算。

规则 10 认的是赋值号或括号右边的整段表达式：只有一个单引号、双引号字符串，或没有 `${` 的模板字符串，才算静态；
`a ? '' : '<b>'` 这种由字面量拼成的也算带变量。`el.innerHTML = …` 和 `el['innerHTML'] = …` 都认；右边到分号、逗号、
外层括号为止，不写分号时到行尾（下一行不以运算符开头、这一行不以运算符结尾）。另外按名字报（不看参数）：
setHTMLUnsafe、parseHTMLUnsafe、createContextualFragment、parseFromString、给 srcdoc 赋值，以及 ui/el.js 以外的 tpl( 调用
（tpl 只能写成 tpl`…`）。ui/el.js 解析的是 tpl`…` 的静态模板（它在运行时拒绝插值和直接调用），放行。
规则 11 的 `.animate(` 不算 `motion.animate(`（`import * as motion from './motion.js'` 的写法）。startViewTransition 在
模块里只由规则 11 管，规则 12 只在经典脚本里管它，不重复报。
规则 12 只按名字认（`Promise.withResolvers`、`Object.groupBy` 这类），认不出经变量间接调用的；字符串里的名字（`'requestIdleCallback' in window`）不算。
迭代器方法只认紧跟在 `.values()`、`.keys()`、`.entries()` 后面的调用和 `Iterator.from`，存进变量再调的认不出。
规则 22 按从 core/session.js 的导入认 api、post、shared（改名导入、import * as 都认），fetch、sendBeacon 按调用认（window.fetch、globalThis.fetch?.( 也认），EventSource、XMLHttpRequest、WebSocket 按 new 认；
认不出经别的模块转手的请求函数（那个模块在 ui/ 里时，它自己的导入会被报）。

已知盲区（文本级检查做不到）：规则 2 认不出 `state[key]`、`Object.assign(state, …)` 和解构写法，名为 state
的函数参数会被误报；规则 10 认不出 `Object.assign(el, {innerHTML: html})`、`el[name] = html` 这类经变量或对象写的属性，
也认不出把 tpl 存进变量再调；正则字面量与除号靠前一个字符判断，`if (ok) /re/.test(s)` 这种写法会被当成除号。规则 8 不查
JS 里拼的内联样式、index.html 的 style 属性和 var() 的回退值；同一个字面量在别处删掉一处、新增一处，数量不变，查不出来。
规则 8 在 css/ 下给的改法只在新代码能用的档位里找（字号 11/12/13/20/28/40，圆角 4/8/12/18），和规则 13 一致。
规则 13 只看 font-weight、font、font-size、border-radius、box-shadow、transition、animation 这几类声明，别处声明的自定义属性
（`.x{--shadow:…}`）不查；box-shadow 里不带模糊的描边环不算阴影。规则 17 按能力清单（COMPAT_FEATURES）找，清单之外的新能力查不出来。
"""
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
import argparse
import bisect
import functools
import json
import posixpath
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
STATIC = Path('web/static')
BASELINE = Path('tools/frontend_baseline.json')
# 规则 3 的放行项：浮层管理器是 document 级 keydown / click 监听的唯一入口（Esc 只关栈顶一个、点外关闭都靠它在一处判断）。
# 用户 2026-11-12 在 #343 批准，只此一个文件；别的文件新增全局监听仍然报错，也不要往基线里加。
GLOBAL_LISTENER_ALLOWED = frozenset({'ui/overlay.js'})
# 规则 4 的放行项：ui/dialog.js 的 openDialog 是唯一生成 <dialog> 的地方，别处的对话框都建在它上面。
# 用户 2026-11-15 批准（#470），只此一个文件；别的文件拼 <dialog> 仍然报错。
DIALOG_ALLOWED = frozenset({'ui/dialog.js'})
MAX_LINE = 200
# 规则 21：ui/ 下一个文件的行数上限。
MAX_UI_LINES = 600
# 规则 22：ui/ 从 core/session.js 导入的发请求的函数（按导入认：改了名、传来传去、包一层都认得出，也不会把同名的方法定义当成调用），
# 以及直接调 fetch（含 window.、globalThis.、self. 前缀和 ?.()）、navigator.sendBeacon，和 new EventSource / XMLHttpRequest / WebSocket。
REQUEST_NAMES = frozenset({'api', 'post', 'shared'})
SESSION_IMPORT_RE = re.compile(r'''\bimport\s*(?:\{([^}]*)\}|\*\s*as\s+(\w+))\s*from\s*['"]([^'"]*core/session\.js)['"]''')
FETCH_RE = re.compile(r'(?<![\w$.])(?:(?:window|globalThis|self|navigator)\s*\.\s*)?(?:fetch|sendBeacon)\s*(?:\?\.\s*)?\(|\bnew\s+(?:EventSource|XMLHttpRequest|WebSocket)\b')
# 规则 10 的放行项：ui/el.js 把 tpl`…` 的静态模板交给 <template> 解析，是唯一把字符串变成节点的地方（#704）。
HTML_SINK_ALLOWED = frozenset({'ui/el.js'})
# 规则 11 的放行项（#704）：每帧调度和动效各有唯一入口。ui/motion.js 由 #705 新建，先写进名单。
FRAME_ALLOWED = frozenset({'ui/frame.js', 'ui/motion.js'})
ANIMATE_ALLOWED = frozenset({'ui/motion.js'})
# 规则 12：Safari 17.0 没有的 JS 能力（docs/WEB_V2.md「兼容底线」）。值是 (正则, 从哪个版本起有)；None 表示 Safari 正式版没有，哪里都不用。
# 版本按 MDN browser-compat-data 8.1.9（2026-11-12）的 safari 一栏核对（https://github.com/mdn/browser-compat-data，
# 条目见每行注释）；加新项时照样查 BCD 或 WebKit 的发布说明，把条目写上。
SAFARI17_JS = {
    'import 属性（with {type}）': (r'''\bimport\b[^;]*?['"`]\s*with\s*\{|\bimport\s*\([^;]*?\{\s*with\s*:''', 'Safari 17.2'),  # javascript.statements.import.import_attributes
    'Promise.withResolvers': (r'\bPromise\s*\.\s*withResolvers\b', 'Safari 17.4'),  # javascript.builtins.Promise.withResolvers
    'Object.groupBy': (r'\bObject\s*\.\s*groupBy\b', 'Safari 17.4'),  # javascript.builtins.Object.groupBy
    'Map.groupBy': (r'\bMap\s*\.\s*groupBy\b', 'Safari 17.4'),  # javascript.builtins.Map.groupBy
    'AbortSignal.any': (r'\bAbortSignal\s*\.\s*any\b', 'Safari 17.4'),  # api.AbortSignal.any_static
    'checkVisibility': (r'\.\s*checkVisibility\s*\(', 'Safari 17.4'),  # api.Element.checkVisibility
    'startViewTransition': (r'\bstartViewTransition\b', 'Safari 18.0'),  # api.Document.startViewTransition
    'URL.parse': (r'\bURL\s*\.\s*parse\b', 'Safari 18.0'),  # api.URL.parse_static
    'Promise.try': (r'\bPromise\s*\.\s*try\b', 'Safari 18.2'),  # javascript.builtins.Promise.try
    '迭代器方法': (r'\.\s*(?:values|keys|entries)\s*\(\s*\)\s*\.\s*(?:map|filter|take|drop|flatMap|reduce|toArray|forEach|some|every|find)\s*\('
              r'|\bIterator\s*\.\s*from\b', 'Safari 18.4'),  # javascript.builtins.Iterator.map / toArray / from
    'setHTMLUnsafe': (r'\.\s*setHTMLUnsafe\s*\(|\bparseHTMLUnsafe\b', 'Safari 26'),  # api.Element.setHTMLUnsafe
    'Navigation API': (r'\bwindow\s*\.\s*navigation\b|(?<![\w$.])navigation\s*\.\s*(?:navigate|currentEntry|entries|traverseTo|'
                       r'updateCurrentEntry|transition|back|forward|reload)\b|\bNavigateEvent\b', 'Safari 26.2'),  # api.Window.navigation
    'Temporal': (r'\bTemporal\s*\.', 'Safari 27.3'),  # javascript.builtins.Temporal
    'CloseWatcher': (r'\bCloseWatcher\b', 'Safari 27.3'),  # api.CloseWatcher
    '显式资源管理（using、Symbol.dispose）': (r'\bSymbol\s*\.\s*(?:asyncDispose|dispose)\b|\b(?:Async)?DisposableStack\b'
                                       r'|(?<![\w$.])(?:await\s+)?using\s+[A-Za-z_$][\w$]*\s*=', 'Safari 27.3'),  # javascript.statements.using
    'requestIdleCallback': (r'\brequestIdleCallback\b', None),  # api.Window.requestIdleCallback：只在技术预览里带开关
    'scheduler（postTask、yield）': (r'\bscheduler\s*\.\s*[A-Za-z_$]', None),  # api.Scheduler：Safari 没有
    'Element.moveBefore': (r'\.\s*moveBefore\s*\(', None),  # api.Element.moveBefore：Safari 没有
}
# 规则 12 登记的增强处：文件 -> 允许作增强的能力（旁边要写明 Safari 17.0 上的退路）。新增登记先在 Issue 里提。
SAFARI17_ENHANCEMENTS = {'ui/motion.js': frozenset({'startViewTransition'})}
# 规则检查的模块文件。i18n.js、splash.js、theme.js 是经典脚本，locales/ 是数据，都不在范围内。
MODULE_GLOBS = ('app.js', 'core/*.js', 'ui/*.js')
# 规则 12 另外扫的经典脚本：同样在 macOS 14 的 WKWebView 里跑，i18n.js、splash.js 还在启动页上，出错就是白屏。
CLASSIC_SCRIPTS = ('i18n.js', 'splash.js', 'theme.js')


# ---------------------------------------------------------------------------------------------------------- scanning
_BLANK = re.compile(r'[^\n]')
_TEMPLATE_RUN = re.compile(r'[^\\`$]+|\$(?!\{)')
_REGEX_KEYWORDS = {'return', 'typeof', 'case', 'in', 'of', 'void', 'delete', 'throw', 'new', 'else', 'do', 'yield', 'await'}


@functools.lru_cache(maxsize=1024)
def strip_comments(src, strings=True):
    """把注释替换成空白（保留换行，行号不变）；字符串、模板字符串文字和正则字面量原样保留。
    strings=False 时引号字符串和模板字符串的文字也一并抹成空白（`${…}` 里的代码保留）。"""
    keep = (lambda text: text) if strings else (lambda text: _BLANK.sub(' ', text))
    out, i, n = [], 0, len(src)
    stack = []  # 'b' = 代码里打开的花括号，'t' = 模板字符串的 `${`
    template = False
    tail = ''  # 最近输出的几个字符，用来区分正则字面量和除号

    def emit(piece):
        nonlocal tail
        out.append(piece)
        tail = (tail + piece)[-64:]

    while i < n:
        c = src[i]
        if template:
            if c == '\\':
                emit(keep(src[i:i + 2])); i += 2
            elif c == '`':
                emit(c); i += 1; template = False
            elif src.startswith('${', i):
                emit('${'); i += 2; stack.append('t'); template = False
            else:
                m = _TEMPLATE_RUN.match(src, i)
                emit(keep(m.group(0))); i = m.end()
        elif c in '\'"':
            j = i + 1
            while j < n and src[j] != c and src[j] != '\n':
                j += 2 if src[j] == '\\' else 1
            emit(c + keep(src[i + 1:j]) + src[j:j + 1]); i = j + 1
        elif c == '`':
            emit(c); i += 1; template = True
        elif src.startswith('//', i):
            j = src.find('\n', i)
            j = n if j < 0 else j
            emit(' ' * (j - i)); i = j
        elif src.startswith('/*', i):
            j = src.find('*/', i + 2)
            j = n if j < 0 else j + 2
            emit(_BLANK.sub(' ', src[i:j])); i = j
        elif c == '/' and _regex_allowed(tail):
            j, in_class = i + 1, False
            while j < n and src[j] != '\n':
                if src[j] == '\\':
                    j += 2; continue
                if src[j] == '[':
                    in_class = True
                elif src[j] == ']':
                    in_class = False
                elif src[j] == '/' and not in_class:
                    break
                j += 1
            emit(src[i:j + 1]); i = j + 1
        else:
            if c == '{':
                stack.append('b')
            elif c == '}' and stack and stack.pop() == 't':
                template = True
            emit(c); i += 1
    if template or stack:
        raise ValueError('unbalanced string/template/brace while scanning')
    return ''.join(out)


def _regex_allowed(before):
    """`/` 出现在运算符或 `(` 之后是正则字面量；出现在值（名字、数字、`)`、`]`）之后是除号。"""
    tail = before.rstrip()
    if not tail:
        return True
    if tail[-1] in '(,=:[!&|?{};+-*%<>~^':
        return True
    m = re.search(r'([A-Za-z_$][\w$]*)$', tail)
    return bool(m and m.group(1) in _REGEX_KEYWORDS)


@dataclass
class Source:
    path: str  # 相对 web/static 的 posix 路径
    raw: str
    code: str  # 注释已抹空
    bare: str = ''  # 注释和字符串、模板文字都已抹空（用来匹配标识符）

    def line_of(self, offset):
        return self.code.count('\n', 0, offset) + 1

    def raw_line(self, number):
        return self.raw.splitlines()[number - 1].strip()


def load_sources(root, patterns=MODULE_GLOBS):
    base = root / STATIC
    found = {}
    for pattern in patterns:
        for path in sorted(base.glob(pattern)):
            raw = path.read_text(encoding='utf-8')
            rel = path.relative_to(base).as_posix()
            try:
                found[rel] = Source(rel, raw, strip_comments(raw), strip_comments(raw, strings=False))
            except ValueError as error:
                raise SystemExit(f'check_frontend: cannot scan {rel}: {error}')
    return found


IMPORT_RE = re.compile(r'''\b(?:import|export)\b[^'"`;]*?\bfrom\s*(['"])(.+?)\1|\bimport\s*\(?\s*(['"])(.+?)\3''', re.S)


def imports(source, known):
    """`source` 静态或动态导入的模块（相对 web/static）；不是本目录模块的路径忽略。"""
    result = []
    for m in IMPORT_RE.finditer(source.code):
        spec = m.group(2) or m.group(4)
        if spec.startswith('/'):
            target = posixpath.normpath(spec.lstrip('/'))
        elif spec.startswith('.'):
            target = posixpath.normpath(posixpath.join(posixpath.dirname(source.path), spec))
        else:
            continue
        if target in known:
            result.append((target, source.line_of(m.start())))
    return result


def strongly_connected(graph):
    """Tarjan 算法；返回多于一个节点（或带自环）的强连通分量。"""
    index, low, on, stack, out, counter = {}, {}, set(), [], [], [0]

    def visit(v):
        index[v] = low[v] = counter[0]; counter[0] += 1
        stack.append(v); on.add(v)
        for w in graph.get(v, ()):
            if w not in index:
                visit(w); low[v] = min(low[v], low[w])
            elif w in on:
                low[v] = min(low[v], index[w])
        if low[v] == index[v]:
            comp = []
            while True:
                w = stack.pop(); on.discard(w); comp.append(w)
                if w == v:
                    break
            if len(comp) > 1 or v in graph.get(v, ()):
                out.append(comp)

    for v in sorted(graph):
        if v not in index:
            visit(v)
    return out


# -------------------------------------------------------------------------------------------------------------- rules
@dataclass
class Rule:
    id: str
    number: str
    title: str
    convention: str
    fix: str
    baselined: bool = True
    found: Counter = field(default_factory=Counter)
    where: dict = field(default_factory=dict)  # 键 -> ["文件:行号: 内容", ...]

    def add(self, key, place):
        self.found[key] += 1
        self.where.setdefault(key, []).append(place)


def new_rules():
    return {r.id: r for r in [
        Rule('import-core-ui', '1', 'core/ 导入了 ui/',
             'core/ 不碰 DOM、不依赖 ui/（任何布局都能复用，Node 里能直接导入）。',
             '把 core/ 需要的东西留在 core/；要通知界面就用 core/events.js 的监听（onPost / onJob 那一类），由 app.js 把界面接上。',
             baselined=False),
        Rule('import-cycle', '1', '新增了 ui/ 模块循环依赖里的导入边',
             'ui/ 现有的循环导入已冻结为基线；不得新增新的环，也不得往已有的环里再加边。',
             '把两边共用的函数/常量下沉到更底层的模块（core/ 或没有回头导入的 ui/ 模块），或改成由调用方传入回调；'
             '不要为了“顺手”再互相 import。拆掉一条环内边后，运行 `python tools/check_frontend.py --shrink` 把它从基线里删掉。'),
        Rule('undeclared-state', '2', 'state 上使用了未在 core/state.js 声明的字段',
             'state 上的字段必须先在 core/state.js 的 state 对象里声明（并写初始值），现有未声明字段已冻结为基线。',
             '在 core/state.js 里加上这个字段和初始值；如果它只是某个模块自己的局部状态，改成该模块里的 let/const，不要挂在 state 上。'),
        Rule('document-listener', '3', '新增了 document/window/body 级的键盘、点击或 change 监听',
             '全局键盘、点击监听（keydown、keyup、click、mousedown、pointerdown）让 Esc 的先后、点外关闭各写一套（#316）；'
             '表单的 change 同样挂在控件所在的面板上，不挂在 document 上（#431）。现有的已冻结为基线，不得新增。',
             '把监听挂在具体元素或 <dialog> 上（dialog 用 cancel/close 事件，点外关闭在 dialog 自己的 click 里判断）；'
             '要新开浮层就向 ui/overlay.js 登记（createLayer / openModal），它是唯一放行的入口；确需别的全局监听时，在 Issue 里提出请用户决定，由用户改本检查的规则，不要往基线里添加。'),
        Rule('dialog-element', '4', '新增了运行时拼出的 <dialog>（createElement 或字符串里的 <dialog）',
             '运行时拼出的 <dialog> 各自复制同一份外壳（#316）。ui/dialog.js 的 openDialog 是唯一放行的入口（#470），没有基线。',
             '复用 ui/dialog.js 里的 openDialog / confirmAction / chooseWork；不够用就在 ui/dialog.js 里扩展参数，不要再在别处拼一份 dialog 外壳。',
             baselined=False),
        Rule('long-line', '5', f'新增了超过 {MAX_LINE} 字符的行',
             f'一行不超过 {MAX_LINE} 个字符（代理和评审都读不动一行 1000 字符）。现有的超长行已冻结为基线，不得新增。',
             '把这行拆成多行：长模板字符串按块换行，长条件/链式调用拆成中间变量；顺手拆开的话，用 `--shrink` 更新基线。'),
        Rule('page-name', '6', '在页面注册表以外的位置新增了页面名判断',
             '页面是哪一类（点击语义、主区、侧栏高亮、工具栏、pool 参数、重画方式……）统一声明在 core/pages.js（#342），页面名只在那里比较，没有基线。',
             '把页面的性质写进 core/pages.js 的声明，用 `viewOf(route).<字段>` 或 `pageOf(route).<字段>` 读；'
             '异步回应回来时判断「还在不在这一页」用 `onPage(route, \'名\')`；要登记渲染入口、标题、重画方式用 `implement()`。'
             '见 docs/WEB_UI_MAP.md「新增一个页面要做什么」。',
             baselined=False),
        Rule('state-writer', '7', '在声明的写入者以外给 state 字段赋值',
             'core/state.js 把 state 字段按归属分组，每组只有一个写入者（组标题「// ---- 说明：写入者文件（备注） ----」写明，#344）；'
             '别的模块只读，要改就调用写入者模块的函数。没有基线。',
             '改成调用写入者模块里写这个字段的函数（组标题括号里列了别处该用的，如 setCursor、setChar、setRoute）；'
             '没有合适的函数，就在写入者模块里加一个导出函数，由它来赋值。确实应当换写入者或拆组时，改 core/state.js 的组标题，'
             '并在 PR 里写明理由；不要为了通过检查在组标题里列两个文件。',
             baselined=False),
        Rule('css-literal', '8', '样式里新增了写死的字号、圆角、间距、z-index 或颜色',
             'web/static/css/tokens.css 的 CSS 变量是外观令牌的唯一来源（docs/WEB_V2.md「样式令牌」，#317、#349、#705）：app.css 和 css/ 下的'
             '样式里，font-size（含 font 简写）、border-radius、gap 和 padding、z-index、颜色都写成 var(--…)；tokens.css 里的变量声明就是令牌定义，不算。'
             f'按 #339 的登记，margin 和不在档位上的 {LARGE_SPACING}px 以上 padding、gap 不检查。app.css 现有的字面量已冻结为基线，不得新增；css/ 下没有基线。',
             '按每处箭头后的提示改用令牌；没有对应档位的，取最近的一档，确需新档位先在 Issue 里问（新档位成对加在 css/tokens.css，'
             '并改 docs/WEB_V2.md 那一节），不要往基线里添加。改好存量的一处后运行 `python tools/check_frontend.py --shrink`。'),
        Rule('ime-composing', '9', '键盘处理函数没有先判断输入法是否在组字，或者绕开 core/keys.js 直接读 isComposing / keyCode',
             '输入法组字时的回车、方向键、数字键、Esc 是在选词、挑候选、取消组字，不是页面的操作（#388）。WebKit 里结束组字的那次 keydown 的 '
             'isComposing 已经是 false、keyCode 仍是 229，只判断 isComposing 不够。所以每个 keydown / keyup / keypress 处理函数都要先用 '
             'core/keys.js 的 composing(e) 判断，判断只在那一处写。没有基线。',
             "在处理函数开头加 `if (composing(e)) return;`（`import {composing} from '../core/keys.js'`），不要自己读 e.isComposing 或 e.keyCode；"
             '处理函数交给变量再传进来时，改成直接写在 addEventListener 里，或让函数体里出现 composing(。见 docs/WEB_V2.md「输入框的键盘处理」。',
             baselined=False),
        Rule('inner-html', '10', '新增了把数据拼进 innerHTML / outerHTML / insertAdjacentHTML 的写法',
             '数据不拼进 HTML 字符串（#703「底座设计」，#704）：转义不靠调用处记得，节点建一次、之后只改变了的地方。'
             'innerHTML、outerHTML、insertAdjacentHTML 的右边只能是不带插值的字符串字面量（清空写 `el.textContent = \'\'`）；'
             '方括号写法（el[\'innerHTML\']）同样算；setHTMLUnsafe、parseHTMLUnsafe、createContextualFragment、DOMParser 的 parseFromString、'
             '给 srcdoc 赋值、把 tpl 当函数调都按名字报。ui/el.js 解析 tpl`…` 的静态模板，是唯一放行的文件。现有的按文件计数冻结为基线，不得新增。',
             '改用 ui/el.js：静态结构写成模块顶层的 tpl`…` 模板（不带插值），数据用 text() / attr() / cls() 写进 data-ref 的节点；'
             '带链接或加粗的译文用 rich() 拼节点，小的动态节点用 make()；列表用 ui/keyed.js。写法见 docs/WEB_V2.md「渲染底座」。'
             '改掉存量的一处后运行 `python tools/check_frontend.py --shrink`。'),
        Rule('animation-entry', '11', '在 ui/frame.js、ui/motion.js 以外调了 requestAnimationFrame、.animate( 或 startViewTransition',
             '每帧的读写和动效各有唯一入口（#704）：requestAnimationFrame 只在 ui/frame.js 和 ui/motion.js 里调，'
             'el.animate、document.startViewTransition 只在 ui/motion.js 里调。这样「先量后改」、减少动态效果、'
             'Safari 17 上的退路都只在一处写。现有的按文件计数冻结为基线，不得新增。',
             '要在下一帧量或改，用 ui/frame.js 的 read(fn, key) / write(fn, key)（滚动、缩放的处理函数只排队）；'
             '要动画，从 ui/motion.js 导入 animate / enter / exit / flip 和共享元素过渡（具名导入，或 import * as motion 后写 motion.animate(…)）。'
             '改掉存量的一处后运行 `python tools/check_frontend.py --shrink`。'),
        Rule('safari17-js', '12', '用了 Safari 17.0（macOS 14 的 WKWebView）没有的 JS 能力',
             'macOS 14.0 的 WKWebView 按 Safari 17.0 算（docs/WEB_V2.md「兼容底线」）：17.0 以后才有的能力只能写在登记的增强处'
             '（SAFARI17_ENHANCEMENTS，现在只有 ui/motion.js 的 startViewTransition），旁边写明退路；Safari 正式版没有的'
             '（requestIdleCallback、scheduler.postTask / yield、Element.moveBefore）哪里都不用。模块和经典脚本（i18n.js、'
             'splash.js、theme.js）都查。没有基线。',
             '换成 Safari 17.0 就有的写法：Promise.withResolvers() → `new Promise((resolve, reject) => …)` 把两个函数存出来；'
             'Object.groupBy / Map.groupBy → 一个循环往 Map 里放；requestIdleCallback → ui/frame.js 或 setTimeout；'
             'checkVisibility → 量 getBoundingClientRect（放进 frame.read）；AbortSignal.any → 自己建一个 AbortController，在每个来源信号上'
             '听 abort；迭代器方法 → 先 [...iter] 或 Array.from(iter) 成数组；moveBefore → ui/keyed.js 挪完放回焦点的做法。确实要作增强时写在登记的增强处、先判断有没有、写好退路，'
             '新的增强处在 Issue 里提出，由用户同意后登记进 tools/check_frontend.py 的 SAFARI17_ENHANCEMENTS。',
             baselined=False),
        Rule('css-scale', '13', '样式里用了三档以外的字重、阴影、时长或缓动，或者新代码不该用的字号、圆角档位',
             '0.4 的视觉语言（#685、#705，docs/WEB_V2.md「前端整体升级」）：字重三档 --fw-400 / --fw-600 / --fw-750；阴影三档 --shadow-hover / '
             '--shadow-pop / --shadow-dialog（不带模糊的描边环不算阴影）；时长三档 --dur-1 / --dur-2 / --dur-3，缓动 --ease 和只给强调动作的 '
             '--ease-spring（循环动画 infinite 不查）；字号只用 11 / 12 / 13 / 20 / 28 / 40；圆角 2 / 6 / 10 / 14 / 16 冻结给旧样式。'
             'app.css 里的旧档位已冻结为基线，不得新增；css/ 下没有基线。',
             '按每处箭头后的提示改用对应的令牌，不要往基线里添加。改好存量的一处后运行 `python tools/check_frontend.py --shrink`。'),
        Rule('css-selector', '14', '样式里新增了类名子串选择器或后代形式的 :has()',
             '[class*=…]、[class^=…]、[class$=…] 这类按类名子串匹配的选择器和 :has(.x) 这种查整棵子树的 :has()，每次改 DOM 都要重算大片样式：'
             'VS Code 在 3.7k 个节点上实测，前者让样式重算慢 2.4 倍（#469）。:has() 只查直接子节点和相邻节点（:has(> .x)、:has(+ .x)、:has(~ .x)）可以。'
             'app.css 现有的已冻结为基线，不得新增。',
             '给要选的元素加一个专门的类（或 data- 特性）直接选；:has() 改成 :has(> …) 这种只看子节点、相邻节点的写法，或由 JS 在父节点上写一个状态类。'),
        Rule('css-var', '15', '样式里用了没有定义的 CSS 变量',
             'var(--名) 引用的变量必须有定义（#705）：在 css/tokens.css 里，或在 tools/check_frontend.py 的 JS_VARIABLES 里登记为「由 JS 写的变量」'
             '（如 --tint、--p）。没定义的变量取不到值，整条声明失效，又不会报错（#317 时发现过从未定义的 --panel）。css/ 下不认文件里自己声明的局部变量：'
             '`--x: 13px` 这种声明规则 8、13 都不查，借它就绕过了三档令牌。只有 app.css 还认同一个文件里的旧局部变量（--gutter）。没有基线。',
             '拼错了就改对；是令牌就成对加进 css/tokens.css（先在 Issue 里问）；由 JS 写的，加进 JS_VARIABLES 并写明哪个文件写它；'
             '部件里想用的局部变量直接写成令牌，或者把那个值直接写在属性上（规则 8、13 会查它）。',
             baselined=False),
        Rule('backdrop-filter', '16', 'backdrop-filter 写在了玻璃表面以外',
             '磨砂玻璃只给浮层（#685、#705）：每个带 backdrop-filter 的元素各占一块显存，卡片上的模糊在长列表里会把显存吃满（#683）。所以 backdrop-filter '
             '只写在 css/base.css 的玻璃表面（.glass、.glass-bar）。app.css 现有的已冻结为基线，不得新增。',
             '浮层用 css/base.css 的 .glass（整块玻璃）或 .glass-bar（浮在图上的按钮组），不要自己写 backdrop-filter；卡片、芯片上不用模糊，'
             '用不透明或半透明的底色。'),
        Rule('css-compat', '17', 'Safari 17.0 没有的 CSS 能力写在了登记的增强块以外',
             'macOS 14.0 的 WKWebView 按 Safari 17.0 算（docs/WEB_V2.md「兼容底线」）。没有的能力只作增强：写在探测它的 @supports 里'
             '（@starting-style 这类测不出来的，块本身就是增强块），块前面紧挨着一段写明退路的注释（含「退路」二字），退路就是块外面已经写好的样子。'
             '`@supports not (…)` 不算增强块。CSS 嵌套（17.2）、相对颜色语法（rgb(from …)，Safari 18 之前和规范不一致）和 ::backdrop 里的 var()'
             '（Safari 17.4 之前 ::backdrop 不从元素继承自定义属性）不用。没有基线。',
             '把用到新能力的声明挪进 `@supports (能力) { … }`，前面加 `/* 增强：…（Safari 版本）。退路：… */`；块外面写好不支持时的样子。'
             '嵌套的规则展开成完整的选择器；相对颜色改用 color-mix()；::backdrop 里写死色值（或在对话框外层元素上画遮罩）。'
             '能力清单在 tools/check_frontend.py 的 COMPAT_FEATURES。',
             baselined=False),
        Rule('css-layer', '18', 'css/ 下的样式写在了自己的 @layer 外面',
             '层的先后由 css/tokens.css 第一行的 `@layer legacy, tokens, base, pages, overlays;` 定（#705）。没有层的样式比所有层都强，'
             '会盖过新部件，也让 legacy 层的旧样式失去「最低」的位置。所以 css/ 下每个文件整个写在一个 @layer 里：tokens.css 在 tokens，'
             'base.css 在 base，overlays.css 在 overlays，其余各页的文件在 pages；层的顺序只在 tokens.css 第一行声明。没有基线。',
             '把规则挪进文件里那一个 `@layer 名{ … }`；要新的层先在 Issue 里问，加在 tokens.css 第一行的声明里，并改 docs/WEB_V2.md「样式文件与层」。',
             baselined=False),
        Rule('css-order', '19', '样式表的先后不对：层的先后会变',
             '层的先后按每个层名第一次出现的顺序定（#705）。css/tokens.css 第一条是 `@layer legacy, tokens, base, pages, overlays;`，'
             '它必须是页面上第一个样式表、并且这条声明在 tokens.css 里任何块之前；否则先出现的层排在前面，旧样式反过来盖住新部件，规则 18 查不出来。'
             'css/ 下新加的文件也要在 web/static/index.html 里引上。没有基线。',
             '把 `<link rel="stylesheet" href="/css/tokens.css">` 放在 index.html 所有样式表的最前面（样式清单页 tools/styleguide/index.html 同样）；'
             'tokens.css 的层声明挪回第一条；新文件在 index.html 里 base.css 之后照现有顺序加一行 <link>。',
             baselined=False),
        Rule('tint-writer', '20', '在 ui/tint.js 以外写了 --tint',
             '图的主色只经 ui/tint.js 的 setTint(el, color) 写（#705）：它写 --tint 并给元素加 .tint，派生的底色才会在那个元素上重算；'
             '只写 --tint、漏了 .tint 不会报错，只会静悄悄地退回中性色。样式里同样不声明 --tint（tokens.css 里 :root 的中性默认值除外）。没有基线。',
             "改成 `import {setTint} from './tint.js'`（core/ 不碰 DOM，交给 ui/ 写）后 `setTint(el, color)`；去掉颜色用 `setTint(el, null)`。",
             baselined=False),
        Rule('ui-file-length', '21', f'ui/ 下的文件超过了 {MAX_UI_LINES} 行',
             f'0.4 的页面样板（#703「可维护性」，docs/WEB_V2.md「页面样板」）：一页一个模块，一个 ui/ 文件不超过 {MAX_UI_LINES} 行；'
             '写得长了多半是把几块界面、取数和计算塞进了同一个文件。现有的大文件按超出的行数冻结为基线，只能缩小。',
             '按页面样板拆：取数和不碰 DOM 的计算挪进 core/ 的对应模块，一页里自成一块的部件（卡片、浮层、工具栏）拆成自己的 ui/ 文件，'
             '模块开头写清它负责什么。缩短了存量的大文件后运行 `python tools/check_frontend.py --shrink`。'),
        Rule('ui-request', '22', 'ui/ 里直接发了请求',
             '页面样板（docs/WEB_V2.md「页面样板」，#706）：ui/ 只画，不发请求。ui/ 不从 core/session.js 导入 api、post、shared，也不直接调 fetch()；'
             '请求写在 core/ 的取数层里（每页一个 core/<页>.js，返回数据或抛错），所以请求的参数、并发合并和过期判断只写在一处，Node 里能直接测。'
             '现有的按文件计数（导入的名字和 fetch 调用各算一处）冻结为基线，不得新增。',
             '把请求挪进 core/ 的取数函数（已有的就复用，例如 core/galleries.js、core/catalog.js、core/assets.js），参数里带上作用域的 signal，'
             'ui/ 里 await 它的结果再画；异步回来先判断还在不在这一页：`if (!scope.alive || state.route !== route) return`（页面样板第 5 条）。'
             '改掉存量的一处后运行 `python tools/check_frontend.py --shrink`。'),
    ]}


LISTENER_EVENTS = 'keydown|keyup|click|mousedown|pointerdown|change'
LISTENER_RE = re.compile(r'''\b(?:document(?:\s*\.\s*(?:body|documentElement))?|window)\s*\.\s*(?:addEventListener\s*\(\s*(['"`])(%s)\1|on(%s)\s*=(?!=))''' % (LISTENER_EVENTS, LISTENER_EVENTS))
DIALOG_RE = re.compile(r'''createElement\s*\(\s*(['"`])dialog\1\s*\)|<dialog\b''')
# 页面名判断：`page === '…'`、`route.page !== '…'`，以及 `[…].includes(route.page)`。
PAGE_NAME_RE = re.compile(r'''(?:\b(?:route|r)\.page|\bpage)\s*[!=]==\s*['"`]|\.includes\(\s*(?:state\.)?(?:route|r)\.page\s*\)''')
# 规则 9：键盘事件的处理函数。`el.addEventListener('keydown', fn` 和 `el.onkeydown = fn`，fn 写在原地，或是同一个文件里的具名函数。
KEY_HANDLER_RE = re.compile(r'''\.\s*addEventListener\s*\(\s*(['"`])(?:keydown|keyup|keypress)\1\s*,\s*|\.\s*on(?:keydown|keyup|keypress)\s*=(?!=)\s*''')
COMPOSING_CALL_RE = re.compile(r'(?<![\w$.])composing\s*\(')
RAW_COMPOSING_RE = re.compile(r'\.\s*(?:isComposing|keyCode)\b')
COMPOSING_HOME = 'core/keys.js'


def _expression_end(bare, pos):
    """从 pos 起的一个表达式（箭头函数、function 表达式或名字）结束的位置：括号配平，遇到更外层的 `)`、顶层的 `,` 或 `;` 为止。"""
    depth, i = 0, pos
    while i < len(bare):
        c = bare[i]
        if c in '([{':
            depth += 1
        elif c in ')]}':
            if depth == 0:
                break
            depth -= 1
        elif c in ',;' and depth == 0:
            break
        i += 1
    return i


def _function_end(bare, pos):
    """`function 名(` 之后：返回函数体花括号配平后的位置。"""
    i = bare.index('{', pos)
    depth = 0
    while i < len(bare):
        depth += {'{': 1, '}': -1}.get(bare[i], 0)
        i += 1
        if depth == 0:
            break
    return i


def key_handlers(source):
    """这个文件里每个键盘处理函数：[(行号, 函数体文字)]。写成名字的，取同一文件里 `function 名(` 或 `const 名 =` 的定义；找不到时函数体是空串。"""
    found = []
    for m in KEY_HANDLER_RE.finditer(source.code):
        end = _expression_end(source.bare, m.end())
        body = source.code[m.end():end]
        name = re.fullmatch(r'\s*([A-Za-z_$][\w$]*)\s*', body)
        if name:
            n = re.escape(name.group(1))
            d = re.search(r'\bfunction\s+%s\s*\(|\b(?:const|let|var)\s+%s\s*=(?!=)' % (n, n), source.bare)
            if not d:
                body = ''
            elif d.group(0).startswith('function'):
                body = source.code[d.end():_function_end(source.bare, d.end())]
            else:
                body = source.code[d.end():_expression_end(source.bare, d.end())]
        found.append((source.line_of(m.start()), body))
    return found


# 规则 10：往 HTML 里写的入口。赋值（= 和 +=，点号或方括号）取右边的整段表达式，insertAdjacentHTML 取第二个参数；
# 下面 HTML_NAMED_RE 的几种不看参数，按名字报。
HTML_ASSIGN_RE = re.compile(r'''(?:\.\s*(?:innerHTML|outerHTML)|\[\s*(['"`])(?:innerHTML|outerHTML)\1\s*\])\s*\+?=(?![=>])''')
HTML_INSERT_RE = re.compile(r'\.\s*insertAdjacentHTML\s*\(')
HTML_NAMED_RE = re.compile(r'\.\s*(?:setHTMLUnsafe|createContextualFragment|parseFromString)\s*\(|\bparseHTMLUnsafe\s*\('
                           r'|\.\s*srcdoc\s*\+?=(?![=>])|(?<![\w$.])tpl\s*\(')
# 不写分号时：这一行以运算符结尾、或下一行以运算符开头，表达式就接着往下读；否则行尾就是表达式的结尾。
_CONTINUES_AFTER = re.compile(r'[-+*/%&|^?:=,(<>!~.]\s*$')
_CONTINUES_BEFORE = re.compile(r'\s*(?:[-+*/%&|^?:,.)\]}]|\?\?|&&|\|\|)')


def _rhs_end(bare, pos):
    """赋值号右边的表达式在哪里结束：同 _expression_end，另外在括号外的换行处，前后都不像续行时结束。"""
    depth, i = 0, pos
    while i < len(bare):
        c = bare[i]
        if c in '([{':
            depth += 1
        elif c in ')]}':
            if depth == 0:
                break
            depth -= 1
        elif c in ',;' and depth == 0:
            break
        elif c == '\n' and depth == 0 and bare[pos:i].strip() and bare.count('`', pos, i) % 2 == 0:  # 不在多行模板字符串里
            if not _CONTINUES_AFTER.search(bare[pos:i]) and not _CONTINUES_BEFORE.match(bare, i + 1):
                break
        i += 1
    return i
# 静态的右边：一个引号字符串，或一个不带 `${` 的模板字符串。
STATIC_HTML_RE = re.compile(r'''(['"])(?:\\.|(?!\1)[^\\\n])*\1|`(?:\\.|\$(?!\{)|[^`\\$])*`''', re.S)


def html_sinks(source):
    """这个文件里每处往 HTML 写的地方：[(起点, 右边的代码文字)]。"""
    found = []
    for m in HTML_ASSIGN_RE.finditer(source.code):
        found.append((m.start(), source.code[m.end():_rhs_end(source.bare, m.end())]))
    for m in HTML_INSERT_RE.finditer(source.code):
        comma = _expression_end(source.bare, m.end())
        if comma < len(source.bare) and source.bare[comma] == ',':
            found.append((m.start(), source.code[comma + 1:_expression_end(source.bare, comma + 1)]))
    return found


# 规则 11：(名字, 正则, 放行的文件)。在注释和字符串都抹空的代码里找。
ANIMATION_ENTRIES = (
    ('requestAnimationFrame', re.compile(r'\brequestAnimationFrame\b'), FRAME_ALLOWED),
    ('.animate(', re.compile(r'(?<!motion)\.\s*animate\s*\('), ANIMATE_ALLOWED),
    ('startViewTransition', re.compile(r'\bstartViewTransition\b'), ANIMATE_ALLOWED),
)

STATE_RE = re.compile(r'(?<![\w$.])state\s*\.\s*([A-Za-z_$][\w$]*)')


def declared_state(sources):
    """core/state.js 里 `state` 对象字面量的顶层字段。"""
    code = sources['core/state.js'].bare
    start = code.index('export const state = {') + len('export const state = ')
    depth, keys = 0, set()
    for m in re.finditer(r'''[{}\[\]()]|'[^'\n]*'|"[^"\n]*"|`[^`]*`|([A-Za-z_$][\w$]*)\s*:(?!:)''', code[start:]):
        s = m.group(0)
        if s in '{[(':
            depth += 1
        elif s in '}])':
            depth -= 1
            if depth == 0:
                break
        elif m.group(1) and depth == 1:
            keys.add(m.group(1))
    return keys


# 规则 7：组标题「// ---- 说明：写入者文件（备注） ----」，冒号和括号之间是写入者文件。
GROUP_RE = re.compile(r'^\s*//\s*----\s*(.*?)\s*----\s*$')
WRITER_FILE_RE = re.compile(r'(?:core|ui)/[\w.-]+\.js|app\.js')
# 给 `state.<字段>` 赋值：直接赋值和复合赋值、++ / --，也包括写进字段的值里面（`state.sel.ids = …`、`state.items[i] = …`），
# 以及 `delete state.<字段>…`、`Object.assign(state.<字段>, …)`。认不出：调用方法改容器（`state.pending.add()`）、
# 解构赋值、`Object.assign(state, …)` 和 `state[key] = …`。
_CHAIN = r'(?:\s*(?:\??\.\s*[A-Za-z_$][\w$]*|\[[^\]\n]*\]))*'
STATE_WRITE_RE = re.compile(
    r'(?<![\w$.])state\s*\.\s*([A-Za-z_$][\w$]*)' + _CHAIN
    + r'\s*(?:(?:\*\*|<<|>>>?|&&|\|\||\?\?|[-+*/%&|^])?=(?![=>])|\+\+|--)'
    r'|(?:\+\+|--)\s*state\s*\.\s*([A-Za-z_$][\w$]*)'
    r'|\bdelete\s+state\s*\.\s*([A-Za-z_$][\w$]*)'
    r'|\bObject\s*\.\s*assign\s*\(\s*state\s*\.\s*([A-Za-z_$][\w$]*)')


def state_writers(sources):
    """core/state.js 里每个 state 字段的写入者：返回 ({字段: 写入者文件}, [声明格式的问题])。"""
    src = sources['core/state.js']
    marker = 'export const state = {'
    start = src.bare.index(marker) + len(marker) - 1
    first_line = src.line_of(start)
    headings, problems = [], []
    for number, text in enumerate(src.raw.splitlines(), 1):
        m = GROUP_RE.match(text) if number > first_line else None
        if not m:
            continue
        title, colon, rest = m.group(1).partition('：')
        files = WRITER_FILE_RE.findall(re.split(r'[（(]', rest, maxsplit=1)[0]) if colon else []
        headings.append((number, files))
        if len(files) != 1:
            problems.append((number, f'组标题要写明一个写入者文件，这里写了 {len(files)} 个：{text.strip()[:90]}'))
    writers, depth = {}, 0
    for m in re.finditer(r"""[{}\[\]()]|'[^'\n]*'|"[^"\n]*"|`[^`]*`|([A-Za-z_$][\w$]*)\s*:(?!:)""", src.bare[start:]):
        token = m.group(0)
        if token in '{[(':
            depth += 1
        elif token in '}])':
            depth -= 1
            if depth == 0:
                break
        elif m.group(1) and depth == 1:
            line = src.line_of(start + m.start())
            owner = [files for number, files in headings if number < line]
            if not owner:
                problems.append((line, f'字段 {m.group(1)} 不在任何组标题下面，没有写明写入者'))
            elif len(owner[-1]) == 1:
                writers[m.group(1)] = owner[-1][0]
    return writers, problems


# ------------------------------------------------------------------------------------------------- 规则 8：CSS 字面量
CSS_FILE = 'app.css'  # 相对 web/static；0.4 之前的旧样式，整个在 legacy 层
TOKENS_FILE = 'css/tokens.css'  # 令牌定义处：这个文件里的变量声明（`--名:值`）就是令牌本身，不算违规（#705）
CSS_GLOBS = (CSS_FILE, 'css/*.css')
# 令牌表（给出「该用哪个令牌」的提示）只读 tokens.css 里这两个选择器的声明：亮色值和暗色值。
TOKEN_SELECTORS = frozenset({':root', ':root[data-theme=dark]'})
# 按 #339 的登记（docs/WEB_V2.md「样式令牌」）不进令牌的：页面级的大间距（18px 以上、不在档位上的 padding、gap）和 margin。
LARGE_SPACING = 18
SPACING_PROPS = frozenset({'gap', 'row-gap', 'column-gap', 'grid-gap', 'grid-row-gap', 'grid-column-gap'})
RADIUS_PROP_RE = re.compile(r'^border(?:-[a-z]+)*-radius$')
# 会带颜色名（white、black…）的属性；十六进制和 rgb()/hsl() 这类颜色函数在任何属性里都算。
COLOR_PROP_RE = re.compile(r'color|background|border|outline|shadow|fill|stroke|text-decoration|column-rule|filter')
NAMED_COLORS = frozenset('''aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue blueviolet brown
    burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan darkblue darkcyan darkgoldenrod darkgray
    darkgreen darkgrey darkkhaki darkmagenta darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen darkslateblue
    darkslategray darkslategrey darkturquoise darkviolet deeppink deepskyblue dimgray dimgrey dodgerblue firebrick floralwhite
    forestgreen fuchsia gainsboro ghostwhite gold goldenrod gray green greenyellow grey honeydew hotpink indianred indigo ivory khaki
    lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan lightgoldenrodyellow lightgray lightgreen lightgrey
    lightpink lightsalmon lightseagreen lightskyblue lightslategray lightslategrey lightsteelblue lightyellow lime limegreen linen
    magenta maroon mediumaquamarine mediumblue mediumorchid mediumpurple mediumseagreen mediumslateblue mediumspringgreen
    mediumturquoise mediumvioletred midnightblue mintcream mistyrose moccasin navajowhite navy oldlace olive olivedrab orange
    orangered orchid palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff peru pink plum powderblue purple
    rebeccapurple red rosybrown royalblue saddlebrown salmon sandybrown seagreen seashell sienna silver skyblue slateblue slategray
    slategrey snow springgreen steelblue tan teal thistle tomato turquoise violet wheat white whitesmoke yellow yellowgreen'''.split())
NAMED_COLOR_RE = re.compile(r'(?<![\w-])(%s)(?![\w-])' % '|'.join(sorted(NAMED_COLORS, key=len, reverse=True)), re.I)
HEX_RE = re.compile(r'(?<![\w&])#([0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})(?![\w])')
COLOR_FN_RE = re.compile(r'(?<![\w-])(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(', re.I)
LENGTH_RE = re.compile(r'(?<![\w#.\-])-?(?:\d+\.?\d*|\.\d+)(px|rem|em|pt|%)(?![\w%])')
FONT_SIZE_IN_SHORTHAND_RE = re.compile(r'(?<![\w#.\-/])(?:\d+\.?\d*|\.\d+)(?:px|rem|em|pt|%)(?=\s*/|\s|$)')


@dataclass
class Declaration:
    selector: str  # 最内层的选择器（或 @keyframes 里的 from / 50%）
    context: tuple  # 外层到内层的全部前导（@media …、选择器）
    prop: str
    value: str
    line: int


def line_finder(text):
    """返回「偏移 → 行号」的函数（二分查换行的位置；逐个 text.count 在大文件上是平方级的）。"""
    breaks = [i for i, c in enumerate(text) if c == '\n']
    return lambda offset: bisect.bisect_left(breaks, offset) + 1


@functools.lru_cache(maxsize=64)
def css_declarations(raw):
    """按顺序列出 CSS 里的每条声明；注释抹空、字符串和括号里的 `;`、`{` 不当作分隔。行号是声明开头所在的行。
    同一份文字的结果会缓存（规则 8、13、15、20 各读一遍）；返回的是元组，调用方不要改它。"""
    text = re.sub(r'/\*.*?\*/', lambda m: _BLANK.sub(' ', m.group(0)), raw, flags=re.S)
    out, stack, start, depth, quote = [], [], 0, 0, ''
    line_of = line_finder(text)

    def emit(end):
        piece = text[start:end]
        body = piece.strip()
        if not body or not stack or ':' not in body:
            return
        prop, _, value = body.partition(':')
        offset = start + len(piece) - len(piece.lstrip())
        out.append(Declaration(stack[-1], tuple(stack), prop.strip().lower(), value.strip(), line_of(offset)))

    for i, c in enumerate(text):
        if quote:
            if c == quote and text[i - 1] != '\\':
                quote = ''
        elif c in '\'"':
            quote = c
        elif c == '(':
            depth += 1
        elif c == ')':
            depth = max(depth - 1, 0)
        elif depth:
            continue
        elif c == '{':
            stack.append(' '.join(text[start:i].split()))
            start = i + 1
        elif c == ';':
            emit(i)
            start = i + 1
        elif c == '}':
            emit(i)
            if stack:
                stack.pop()
            start = i + 1
    return tuple(out)


def _calls(text, opener):
    """`text` 里以 `opener` 正则开头、括号配平的函数调用：[(起点, 终点)]。"""
    spans, pos = [], 0
    while (m := opener.search(text, pos)):
        depth, j = 0, m.end() - 1
        while j < len(text):
            depth += {'(': 1, ')': -1}.get(text[j], 0)
            j += 1
            if depth == 0:
                break
        spans.append((m.start(), j))
        pos = j
    return spans


_VAR_RE = re.compile(r'\bvar\(')


def _clean_value(value):
    """去掉字符串、url(…) 的内容和 `!important`，把 var(…) 整个换成 VAR（变量里的回退值不算字面量）。"""
    value = re.sub(r'''(['"])(?:\\.|(?!\1).)*\1''', "''", value)
    value = re.sub(r'\burl\([^)]*\)', 'url()', value, flags=re.I)
    value = re.sub(r'!\s*important', '', value, flags=re.I)
    for begin, end in reversed(_calls(value, _VAR_RE)):
        value = value[:begin] + 'VAR' + value[end:]
    return value.strip()


def css_tokens(declarations):
    """令牌定义处的变量：返回 ({亮色变量: 值}, {暗色变量: 值})。"""
    light, dark = {}, {}
    for d in declarations:
        if d.prop.startswith('--') and d.selector in TOKEN_SELECTORS:
            (dark if 'dark' in d.selector else light)[d.prop] = d.value
    return light, dark


def _px_tokens(light, prefix):
    """形如 `--fs-12: 12px` 的数值令牌：{12.0: '--fs-12'}。"""
    found = {}
    for name, value in light.items():
        m = re.fullmatch(r'(\d+(?:\.\d+)?)px', value)
        if name.startswith(prefix) and m:
            found.setdefault(float(m.group(1)), name)
    return found


# 第二段（#339）按用户答复定下的归并（docs/WEB_V2.md「样式令牌」），优先于下面「最近的一档」的推算。
MERGED = {'--fs-': {9.5: 10, 10.5: 11, 11.5: 11, 12.5: 12, 13.5: 13},
          '--radius-': {3: 4, 5: 6, 7: 6, 9: 10, 13: 14},
          '--space-': {3: 4, 5: 6, 7: 8, 9: 8, 11: 10, 13: 12}}


def _nearest(scale, value, uses, prefix):
    """最近的一档；两档一样近时取用得更多的一档（docs/WEB_V2.md「样式令牌」）。"""
    if MERGED[prefix].get(value) in scale:
        return scale[MERGED[prefix][value]]
    return scale[min(scale, key=lambda v: (abs(v - value), -uses[scale[v]], -v))]


def _normal_color(text):
    """颜色字面量的规范写法：十六进制小写并展开成 6 / 8 位，函数去掉空白、`0.5` 写成 `.5`。"""
    if text.startswith('#'):
        digits = text[1:].lower()
        return '#' + (''.join(c * 2 for c in digits) if len(digits) in (3, 4) else digits)
    return re.sub(r'(?<![\d.])0\.', '.', re.sub(r'\s+', '', text.lower()))


class CssAdvisor:
    """按 app.css 顶部的令牌，给每个字面量写出该用哪个令牌（没有对应档位时说明要先问）。"""

    def __init__(self, declarations, raw):
        self.uses = Counter(re.findall(r'var\((--[\w-]+)\)', raw))
        light, self.dark = css_tokens(declarations)
        self.display = _px_tokens(light, '--fs-display-')
        self.font = {v: n for v, n in _px_tokens(light, '--fs-').items() if not n.startswith('--fs-display-')}
        self.radius = _px_tokens(light, '--radius-')
        self.space = _px_tokens(light, '--space-')
        self.z = {int(v): n for n, v in light.items() if n.startswith('--z-') and re.fullmatch(r'-?\d+', v)}
        self.colors = {}
        for name, value in light.items():
            if HEX_RE.fullmatch(value) or COLOR_FN_RE.match(value):
                self.colors.setdefault(_normal_color(value), []).append(name)

    def restrict(self):
        """只在新代码能用的档位里给建议（css/ 下的文件，和规则 13 一致）。"""
        self.font = {v: n for v, n in self.font.items() if NEW_FONT_SIZES.get(int(v)) == n}
        self.display = {v: n for v, n in self.display.items() if NEW_FONT_SIZES.get(int(v)) == n}
        self.radius = {v: n for v, n in self.radius.items() if int(v) in NEW_RADII}
        self.restricted = True
        return self

    @staticmethod
    def _px(literal):
        m = re.fullmatch(r'-?(\d+\.?\d*|\.\d+)px', literal)
        return float(m.group(1)) if m and not literal.startswith('-') else None

    @staticmethod
    def _scale(scale):
        return ' / '.join(f'{v:g}' for v in sorted(scale))

    def _ask(self, what, scale, value, prefix):
        near = f'最近的一档是 var({_nearest(scale, value, self.uses, prefix)})；' if scale else ''
        tiers = '新写的界面能用的档位' if getattr(self, 'restricted', False) else '现有档位'
        return f'没有 {what} 这一档（{tiers} {self._scale(scale)}）：{near}确需这个值，先在 Issue 里问是否新增档位'

    def font_size(self, literal):
        px = self._px(literal)
        if px in self.font:
            return f'改用 var({self.font[px]})'
        if px in self.display:
            return f'改用 var({self.display[px]})（展示字号，只给大标题、空状态、启动页这类一次性的大字）'
        if px is None:
            return f'字号令牌都是 px 档位，没有 {literal} 这种写法的档；改用 var(--fs-N)，或先在 Issue 里问是否新增档位'
        return self._ask(literal, self.font, px, '--fs-')

    def radius_of(self, literal):
        px = self._px(literal)
        if literal == '50%':
            return '改用 var(--radius-round)'
        if px in self.radius:
            return f'改用 var({self.radius[px]})'
        if px is not None and px >= 999:
            return '改用 var(--radius-pill)'
        if px is None:
            return f'圆角令牌是 px 档位、--radius-pill 和 --radius-round（50%），没有 {literal}；先在 Issue 里问是否新增档位'
        return self._ask(literal, self.radius, px, '--radius-')

    def spacing(self, literal):
        px = self._px(literal)
        if px in self.space:
            return f'改用 var({self.space[px]})'
        if px is None:
            return f'间距令牌都是 px 档位，没有 {literal} 这种写法的档；改用 var(--space-N)，或先在 Issue 里问是否新增档位'
        return self._ask(literal, self.space, px, '--space-')

    def z_index(self, literal):
        value = int(literal)
        if value in self.z:
            return f'改用 var({self.z[value]})'
        order = ' < '.join(name for _, name in sorted(self.z.items()))
        return f'没有 z-index {literal} 这一层（由低到高：{order}）：按它该在哪两层之间，先在 Issue 里问是否新增档位（只加档位，不改先后）'

    def color(self, literal):
        names = self.colors.get(_normal_color(literal), [])
        if names:
            pairs = '、'.join(f'var({n})（暗色下是 {self.dark.get(n, "同一个值")}）' for n in names[:3])
            return (f'亮色下等于 {pairs}：确认这个语义在暗色下也合适就改用它；'
                    '不合适就在 :root 和 :root[data-theme=dark] 两块里成对新增一个语义色（先在 Issue 里问）')
        return ('没有这个颜色的令牌：改用 :root 顶部现有的语义色，或在 :root 和 :root[data-theme=dark] 两块里成对新增一个'
                '（先在 Issue 里问；新增暗色取值按 docs/WEB_V2.md 的对比度要求核对）')


def css_literals(files):
    """各样式文件里写死的字号、圆角、间距、z-index 和颜色：[(文件, 基线键, 声明, 字面量, 怎么改)]。
    `files` 是 {相对 web/static 的路径: 文字}；只给一段文字时当作 app.css。"""
    if isinstance(files, str):
        files = {CSS_FILE: files}
    parsed = {path: css_declarations(raw) for path, raw in files.items()}
    token_decls = parsed.get(TOKENS_FILE) or [d for ds in parsed.values() for d in ds]
    legacy = CssAdvisor(token_decls, '\n'.join(files.values()))
    current = CssAdvisor(token_decls, '\n'.join(files.values())).restrict()
    found = []
    for path, declarations in parsed.items():
        prefix = '' if path == CSS_FILE else f'{path}: '
        advisor = legacy if path == CSS_FILE else current
        for d in declarations:
            custom = d.prop.startswith('--')
            if custom and path == TOKENS_FILE:
                continue  # 令牌定义处
            value = _clean_value(d.value)
            hits = []
            if d.prop == 'font-size':
                hits += [(m.group(0), advisor.font_size) for m in LENGTH_RE.finditer(value)]
            elif d.prop == 'font':
                hits += [(m.group(0), advisor.font_size) for m in FONT_SIZE_IN_SHORTHAND_RE.finditer(value)][:1]
            elif RADIUS_PROP_RE.match(d.prop):
                hits += [(m.group(0), advisor.radius_of) for m in LENGTH_RE.finditer(value)]
            elif d.prop in SPACING_PROPS or d.prop.startswith('padding'):
                for m in LENGTH_RE.finditer(value):
                    px = advisor._px(m.group(0))
                    if px is not None and px >= LARGE_SPACING and px not in advisor.space:
                        continue  # 页面级的大间距，按 #339 的登记不进令牌
                    if m.group(1) in ('px', 'rem', 'em', 'pt'):
                        hits.append((m.group(0), advisor.spacing))
            elif d.prop == 'z-index' and re.fullmatch(r'-?\d+', value) and int(value) != 0:
                hits.append((value, advisor.z_index))
            hits = [(lit, how) for lit, how in hits if float(re.match(r'-?(\d+\.?\d*|\.\d+)', lit).group(1)) != 0]
            hits += [(m.group(0), advisor.color) for m in HEX_RE.finditer(value)]
            hits += [(value[b:e], advisor.color) for b, e in _calls(value, COLOR_FN_RE) if 'VAR' not in value[b:e]]
            if not custom and COLOR_PROP_RE.search(d.prop):
                hits += [(m.group(0), advisor.color) for m in NAMED_COLOR_RE.finditer(value)]
            for literal, how in hits:
                shown = _normal_color(literal) if how == advisor.color else literal
                found.append((path, f'{prefix}{d.prop}: {shown}', d, literal, how(literal)))
    return found


# ------------------------------------------------------------------------------------- 规则 13：三档令牌和新代码的档位
WEIGHTS = ('--fw-400', '--fw-600', '--fw-750')
SHADOWS = ('--shadow-hover', '--shadow-pop', '--shadow-dialog')
NEW_FONT_SIZES = {11: '--fs-11', 12: '--fs-12', 13: '--fs-13', 20: '--fs-20', 28: '--fs-display-28', 40: '--fs-display-40'}
NEW_RADII = {4: '40px 以下的小元素', 8: '缩略图、格子', 12: '卡片、分组、弹出层', 18: '内容页、对话框、活页条标签'}
FROZEN_RADII = frozenset({2, 6, 10, 14, 16})
CSS_WIDE = frozenset({'inherit', 'initial', 'unset', 'revert', 'revert-layer'})
VAR_NAME_RE = re.compile(r'var\(\s*(--[\w-]+)')
TIME_RE = re.compile(r'(?<![\w.\-])-?(?:\d+\.?\d*|\.\d+)m?s(?![\w%])')
EASING_RE = re.compile(r'(?<![\w-])(?:cubic-bezier|steps|linear)\([^)]*\)|(?<![\w-])(?:ease-in-out|ease-in|ease-out|ease|linear|step-start|step-end)(?![\w(-])', re.I)
WEIGHT_WORD_RE = re.compile(r'(?<![\w-])(?:[1-9]00|bold|bolder|lighter)(?![\w-])', re.I)
SCALE_FIX = {
    'weight': '字重只有三档：正文 var(--fw-400)、强调和节标题 var(--fw-600)、展示字号 var(--fw-750)',
    'shadow': '阴影只有三档：悬停 var(--shadow-hover)、弹出层 var(--shadow-pop)、对话框 var(--shadow-dialog)；不带模糊的描边环（0 0 0 2px var(--accent)）可以照写',
    'duration': '时长只有三档：出场、悬停 var(--dur-1)（140ms）；进场、展开、换页 var(--dur-2)（240ms）；共享元素、活页条标签 var(--dur-3)（380ms）',
    'easing': '缓动用 var(--ease)；带回弹的 var(--ease-spring) 只给活页条标签和「弹一下」这类强调动作',
}


def split_top(value, sep=','):
    """按不在括号里的分隔符切开。"""
    parts, depth, cur = [], 0, ''
    for c in value:
        depth += {'(': 1, ')': -1}.get(c, 0)
        if c == sep and depth == 0:
            parts.append(cur)
            cur = ''
        else:
            cur += c
    return parts + [cur]


def _weight_fix(word):
    number = {'bold': 700, 'bolder': 700, 'lighter': 300, 'normal': 400}.get(word.lower()) or int(word)
    nearest = min((400, 600, 750), key=lambda w: (abs(w - number), w))
    return f'{SCALE_FIX["weight"]}；{word} 最近的是 var(--fw-{nearest})'


def _size_fix(name):
    m = re.search(r'(\d+)$', name)
    nearest = min(NEW_FONT_SIZES, key=lambda v: (abs(v - int(m.group(1))), v)) if m else 13
    return (f'新写的界面字号只用 11 / 12 / 13 / 20（var(--fs-N)）和 28 / 40（var(--fs-display-N)）：'
            f'改用最近的一档 var({NEW_FONT_SIZES[nearest]})')


def _radius_fix(name):
    px = int(re.search(r'(\d+)$', name).group(1))
    nearest = min(NEW_RADII, key=lambda v: (abs(v - px), v))
    tiers = '；'.join(f'{use} var(--radius-{v})' for v, use in NEW_RADII.items())
    return f'2 / 6 / 10 / 14 / 16 只留给旧样式（{tiers}，能点的控件 var(--radius-pill)，人 var(--radius-round)）：最近的是 var(--radius-{nearest})'


def scale_issues(declarations):
    """规则 13：[(基线键里属性之后的部分, 声明, 怎么改)]。"""
    found = []
    for d in declarations:
        raw = re.sub(r'!\s*important', '', d.value, flags=re.I).strip()
        value = _clean_value(d.value)
        names = VAR_NAME_RE.findall(raw)
        if d.prop == 'font-weight':
            if value.lower() not in CSS_WIDE and not (value == 'VAR' and names and names[0] in WEIGHTS):
                shown = f'var({names[0]})' if value == 'VAR' and names else value
                how = _weight_fix(value) if WEIGHT_WORD_RE.fullmatch(value) or value.lower() == 'normal' else SCALE_FIX['weight']
                found.append((f'font-weight: {shown}', d, how))
        if d.prop == 'font':
            size = FONT_SIZE_IN_SHORTHAND_RE.search(value)
            before = value[:size.start()] if size else value.split('/')[0]
            for m in WEIGHT_WORD_RE.finditer(before):
                found.append((f'font-weight: {m.group(0)}', d, _weight_fix(m.group(0))))
            found += [(f'font-weight: var({n})', d, SCALE_FIX['weight']) for n in names if n.startswith('--fw-') and n not in WEIGHTS]
        if d.prop in ('font', 'font-size'):
            for n in names:
                px = re.fullmatch(r'--fs-(?:display-)?(\d+)', n)
                if px and NEW_FONT_SIZES.get(int(px.group(1))) != n:
                    found.append((f'font-size: var({n})', d, _size_fix(n)))
        if RADIUS_PROP_RE.match(d.prop):
            for n in names:
                px = re.fullmatch(r'--radius-(\d+)', n)
                if px and int(px.group(1)) in FROZEN_RADII:
                    found.append((f'border-radius: var({n})', d, _radius_fix(n)))
        if d.prop == 'box-shadow':
            for layer in split_top(raw):
                layer = layer.strip()
                if not layer or layer.lower() in CSS_WIDE | {'none'}:
                    continue
                inner = VAR_NAME_RE.findall(layer)
                cleaned = _clean_value(layer)
                if cleaned == 'VAR':
                    if inner[0] not in SHADOWS:
                        found.append((f'box-shadow: var({inner[0]})', d, SCALE_FIX['shadow']))
                    continue
                for b, e in reversed(_calls(cleaned, COLOR_FN_RE)):
                    cleaned = cleaned[:b] + cleaned[e:]
                lengths = re.findall(r'(?<![\w#.\-])-?(?:\d+\.?\d*|\.\d+)(?:px|rem|em)?(?![\w%(])', cleaned)
                if len(lengths) >= 3 and float(re.match(r'-?([\d.]+)', lengths[2]).group(1)) != 0:
                    found.append((f'box-shadow: {" ".join(lengths)}', d, SCALE_FIX['shadow']))
        if d.prop in ('transition', 'animation', 'transition-duration', 'animation-duration'):
            shorthand = d.prop in ('transition', 'animation')
            for raw_layer in split_top(raw):
                layer = _clean_value(raw_layer)
                if d.prop.startswith('animation') and re.search(r'(?<![\w-])infinite(?![\w-])', layer):
                    continue  # 循环动画（转圈、呼吸）不是进出场，不查
                times = TIME_RE.findall(layer)
                if shorthand and '--dur-' in raw_layer:
                    times = []  # 时长已经是令牌，写死的时间是延迟，不查
                for t in times[:1] if shorthand else times:
                    if float(re.match(r'-?([\d.]+)', t).group(1)) != 0:
                        found.append((f'{d.prop.split("-")[0]}-duration: {t}', d, SCALE_FIX['duration']))
        if d.prop in ('transition', 'animation', 'transition-timing-function', 'animation-timing-function'):
            for layer in split_top(value):
                if d.prop.startswith('animation') and re.search(r'(?<![\w-])infinite(?![\w-])', layer):
                    continue
                for m in EASING_RE.finditer(layer):
                    easing = re.sub(r'\s+', '', m.group(0))
                    found.append((f'{d.prop.split("-")[0]}-timing-function: {easing}', d, SCALE_FIX['easing']))
    return found


# ------------------------------------------------------------- 规则 14、15、16、17、18、19、20：样式文件的结构
# 规则 18（层）：css/ 下各文件所在的层；层的先后只在 tokens.css 第一行声明。
LAYER_ORDER = ('legacy', 'tokens', 'base', 'pages', 'overlays')
FILE_LAYERS = {TOKENS_FILE: 'tokens', 'css/base.css': 'base', 'css/overlays.css': 'overlays'}
# 规则 15：由 JS 写进元素 style 的变量（样式里只引用，不在 CSS 里定义）。每项写明谁写它；不再有人写时删掉。
JS_VARIABLES = {
    '--tint': '各视图按图的主色写（#717 的数据到了以后），见 css/tokens.css 的 .tint',
    '--p': 'ui/catalog.js 最近 30 天的圆环；css/base.css 带进度圆环的头像',
    '--dup-hue': 'ui/grid.js 重复组的色调',
    '--depth': 'ui/catalog.js 作品树的缩进层级',
    '--w': 'ui/like.js 例图条的宽度',
    '--at': 'ui/rail.js 活页条标签停在第几个一级入口（css/rail.css 的 .rail-tab）',
}
# 规则 16：可以写 backdrop-filter 的地方：base.css 里落在这几种玻璃表面上的选择器（最后一节是它们，前面可以带
# `:root[data-reduce-transparency]` 这类条件，降低透明度时关掉模糊用）。.glass-scrim::backdrop 是对话框背后的模糊（#715）。
GLASS_FILE = 'css/base.css'
GLASS_SELECTORS = frozenset({'.glass', '.glass-bar', '.glass-scrim::backdrop'})


# 玻璃选择器前面只允许这一个前缀：桌面外壳注入的「降低透明度」根属性（#755）。别的前缀（`.card .glass`）不放行。
GLASS_PREFIXES = frozenset({':root[data-reduce-transparency]'})


def _glass_selector(prelude):
    """选择器列表里每一项都是玻璃表面本身，或前缀是「降低透明度」的根属性。"""
    def ok(part):
        bits = part.split()
        return bool(bits) and bits[-1] in GLASS_SELECTORS and (len(bits) == 1 or (len(bits) == 2 and bits[0] in GLASS_PREFIXES))
    return all(ok(part) for part in prelude.split(',') if part.strip())
# 规则 14
CLASS_SUBSTRING_RE = re.compile(r'\[\s*class\s*[*^$|]=')
HAS_RE = re.compile(r':has\(\s*(.)')
# 规则 17：Safari 17.0 没有的能力：(名字, 在哪里找, 正则, @supports 条件里要出现的字样)。'at' 找块的前导，'prop' 找属性名，
# 'value' 找值，'selector' 找选择器。探测字样是 None 的（@supports 测不出来）只能写成那个块本身，前面写退路。
COMPAT_FEATURES = (
    ('@starting-style（Safari 17.5，@supports 测不出来）', 'at', r'^@starting-style\b', None),
    ('@scope（Safari 17.4）', 'at', r'^@scope\b', None),
    ('@view-transition（Safari 18.2）', 'at', r'^@view-transition\b', None),
    ('@position-try（锚点定位）', 'at', r'^@position-try\b', None),
    ('transition-behavior（Safari 17.4）', 'prop', r'^transition-behavior$', 'transition-behavior'),
    ('content-visibility（Safari 18.0）', 'prop', r'^content-visibility$', 'content-visibility'),
    ('View Transitions（Safari 18.0）', 'prop', r'^view-transition-(?:name|class)$', 'view-transition'),
    ('滚动驱动动画（Safari 26）', 'prop', r'^(?:animation-timeline|animation-range(?:-start|-end)?|scroll-timeline(?:-name|-axis)?|view-timeline(?:-name|-axis|-inset)?|timeline-scope)$', 'timeline'),
    ('锚点定位', 'prop', r'^(?:anchor-name|anchor-scope|position-anchor|position-area|position-try(?:-fallbacks|-order)?|position-visibility)$', 'anchor'),
    ('field-sizing', 'prop', r'^field-sizing$', 'field-sizing'),
    ('interpolate-size', 'prop', r'^interpolate-size$', 'interpolate-size'),
    ('text-box', 'prop', r'^text-box(?:-trim|-edge)?$', 'text-box'),
    ('linear() 缓动（Safari 17.2）', 'value', r'(?<![\w-])linear\(', 'linear('),
    ('allow-discrete（Safari 17.4）', 'value', r'(?<![\w-])allow-discrete(?![\w-])', 'transition-behavior'),
    ('light-dark()（Safari 17.5）', 'value', r'(?<![\w-])light-dark\(', 'light-dark'),
    ('calc-size()', 'value', r'(?<![\w-])calc-size\(', 'calc-size'),
    ('anchor() / anchor-size()', 'value', r'(?<![\w-])anchor(?:-size)?\(', 'anchor'),
    ('text-wrap: balance / pretty（Safari 17.5 / 26）', 'text-wrap', r'(?<![\w-])(?:balance|pretty)(?![\w-])', 'text-wrap'),
    ('View Transitions 的伪元素（Safari 18.0）', 'selector', r'::view-transition', 'view-transition'),
    (':state()（Safari 17.4）', 'selector', r':state\(', ':state'),
    ('::details-content（Safari 18.4）', 'selector', r'::details-content', 'details-content'),
)
RELATIVE_COLOR_RE = re.compile(r'(?<![\w-])(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(\s*from\s', re.I)


@dataclass
class Block:
    prelude: str
    line: int
    start: int  # 前导在文字里的起点
    parent: int  # 外层块在列表里的下标，顶层是 -1


def css_blocks(raw):
    """样式表的块结构：(blocks, statements, decls)。blocks 是每个 `{` 的 Block；statements 是顶层以 `;` 结束的语句
    [(文字, 行号)]；decls 是 [(所在块的下标, 属性, 值, 行号)]。注释抹空，字符串和括号里的符号不算。"""
    text = re.sub(r'/\*.*?\*/', lambda m: _BLANK.sub(' ', m.group(0)), raw, flags=re.S)
    blocks, statements, decls, stack = [], [], [], []
    start, depth, quote = 0, 0, ''
    line_of = line_finder(text)

    def head(end):
        piece = text[start:end]
        offset = start + len(piece) - len(piece.lstrip())
        return piece.strip(), offset, line_of(offset)

    for i, c in enumerate(text):
        if quote:
            if c == quote and text[i - 1] != '\\':
                quote = ''
        elif c in '\'"':
            quote = c
        elif c == '(':
            depth += 1
        elif c == ')':
            depth = max(depth - 1, 0)
        elif depth:
            continue
        elif c == '{':
            body, offset, line = head(i)
            blocks.append(Block(' '.join(body.split()), line, offset, stack[-1] if stack else -1))
            stack.append(len(blocks) - 1)
            start = i + 1
        elif c in ';}':
            body, offset, line = head(i)
            if body:
                if not stack:
                    statements.append((' '.join(body.split()), line))
                elif ':' in body:
                    prop, _, value = body.partition(':')
                    decls.append((stack[-1], prop.strip().lower(), value.strip(), line))
            if c == '}' and stack:
                stack.pop()
            start = i + 1
    return blocks, statements, decls


def _chain(blocks, index):
    """从最外层到 index 的块。"""
    out = []
    while index >= 0:
        out.append(blocks[index])
        index = blocks[index].parent
    return out[::-1]


def _comment_before(raw, offset):
    """紧挨在 offset 之前（中间只有空白）的那段注释的文字；没有时是 ''。"""
    before = raw[:offset].rstrip()
    if not before.endswith('*/'):
        return ''
    begin = before.rfind('/*')
    return before[begin:] if begin >= 0 else ''


def structure_issues(path, raw):
    """规则 14、16、17、18（层）、19（tokens.css 里声明的位置）：[(规则, 基线键, 行号, 内容, 怎么改)]。"""
    blocks, statements, decls = css_blocks(raw)
    found = []
    in_css = path.startswith('css/')
    # 规则 18（层）
    if in_css:
        want = FILE_LAYERS.get(path, 'pages')
        declaration = f'@layer {", ".join(LAYER_ORDER)}'
        for text, line in statements:
            if not (path == TOKENS_FILE and text.replace(' ', '') == declaration.replace(' ', '') and line == min(l for _, l in statements)):
                found.append(('css-layer', f'{path}: {text[:40]}', line, text,
                              f'这里只能有 `@layer 名{{ … }}` 一块；层的顺序只在 {TOKENS_FILE} 第一行写成 `{declaration};`'))
        if path == TOKENS_FILE and not any(t.replace(' ', '') == declaration.replace(' ', '') for t, _ in statements):
            found.append(('css-layer', f'{path}: 没有层的声明', 1, '', f'第一行写 `{declaration};`'))
        # 规则 19：层的声明要在任何块之前，否则先出现的块的层排到前面
        lines = [l for t, l in statements if t.replace(' ', '') == declaration.replace(' ', '')]
        if path == TOKENS_FILE and lines and blocks and blocks[0].line <= lines[0]:
            found.append(('css-order', f'{path}: 层的声明不在最前', lines[0], declaration,
                          f'把 `{declaration};` 挪到 {path} 的第一条，在 `@layer tokens{{` 之前'))
        for b in blocks:
            if b.parent == -1 and b.prelude != f'@layer {want}':
                found.append(('css-layer', f'{path}: {b.prelude[:40]}', b.line, b.prelude,
                              f'{path} 的规则整个写在 `@layer {want}{{ … }}` 里'))
            elif b.parent != -1 and b.prelude.startswith('@layer'):
                found.append(('css-layer', f'{path}: 嵌套的 {b.prelude[:30]}', b.line, b.prelude,
                              f'不要在层里再开层（会变成 {want}.子层）：规则直接写在 `@layer {want}{{ … }}` 里'))
    # 规则 14
    for b in blocks:
        if b.prelude.startswith('@'):
            continue
        for m in CLASS_SUBSTRING_RE.finditer(b.prelude):
            found.append(('css-selector', f'{path}: 类名子串选择器', b.line, b.prelude, '给元素加专门的类直接选'))
        for m in HAS_RE.finditer(b.prelude):
            if m.group(1) not in '>+~':
                found.append(('css-selector', f'{path}: 后代形式的 :has()', b.line, b.prelude, '改成 :has(> …)、:has(+ …)，或由 JS 在父节点上写状态类'))
    # 规则 16
    for index, prop, value, line in decls:
        if prop in ('backdrop-filter', '-webkit-backdrop-filter'):
            selector = blocks[index].prelude
            if not (path == GLASS_FILE and _glass_selector(selector)):
                found.append(('backdrop-filter', f'{path}: backdrop-filter', line, f'{selector} {{ {prop}: {value[:40]} }}',
                              '改用 css/base.css 的 .glass、.glass-bar（对话框背后用 .glass-scrim）'))
    # 规则 17
    def enhanced(chain, probe):
        """chain 里有探测 probe 的 @supports（或 probe 为 None 时 chain 的某一块就是那个能力的块），并且那一块前面写了退路。"""
        for b in chain:
            positive = b.prelude.startswith('@supports') and not re.match(r'@supports\s+not\b', b.prelude)
            hit = (positive and probe and probe.lower() in b.prelude.lower()) or (probe is None and b is chain[-1])
            if hit and '退路' in _comment_before(raw, b.start):
                return True
        return False

    for index, b in enumerate(blocks):
        parent = blocks[b.parent] if b.parent >= 0 else None
        if parent and not parent.prelude.startswith('@'):
            found.append(('css-compat', f'{path}: CSS 嵌套', b.line, f'{parent.prelude} {{ {b.prelude} {{ … }} }}',
                          'CSS 嵌套要 Safari 17.2：展开成完整的选择器写在外面'))
        chain = _chain(blocks, index)
        for name, where, pattern, probe in COMPAT_FEATURES:
            target = b.prelude if (where == 'at' and b.prelude.startswith('@')) or (where == 'selector' and not b.prelude.startswith('@')) else None
            if target is not None and re.search(pattern, target) and not enhanced(chain, probe):
                found.append(('css-compat', f'{path}: {name}', b.line, b.prelude, _compat_fix(name, probe)))
    for index, prop, value, line in decls:
        chain = _chain(blocks, index)
        if 'var(' in value and any('::backdrop' in b.prelude for b in chain):
            found.append(('css-compat', f'{path}: ::backdrop 里的 var()', line, f'{prop}: {value[:60]}',
                          'Safari 17.4 之前 ::backdrop 不继承自定义属性，var() 取不到值：写死色值，或在对话框外层元素上画遮罩'))
        if RELATIVE_COLOR_RE.search(value):
            found.append(('css-compat', f'{path}: 相对颜色语法', line, f'{prop}: {value[:60]}',
                          '相对颜色语法在 Safari 18 之前和规范不一致：改用 color-mix()'))
        for name, where, pattern, probe in COMPAT_FEATURES:
            hit = ((where == 'prop' and re.search(pattern, prop)) or (where == 'value' and re.search(pattern, value))
                   or (where == 'text-wrap' and prop in ('text-wrap', 'text-wrap-style') and re.search(pattern, value)))
            if hit and not enhanced(chain, probe):
                found.append(('css-compat', f'{path}: {name}', line, f'{prop}: {value[:60]}', _compat_fix(name, probe)))
    return found


def _compat_fix(name, probe):
    if probe is None:
        return f'{name} 本身就是增强块：块外面写好不支持时的样子，块前面紧挨着写 `/* 增强：…。退路：… */`'
    return f'挪进 `@supports ({probe}…) {{ … }}`，块外面写好不支持时的样子，块前面紧挨着写 `/* 增强：…。退路：… */`'


def undefined_variables(files):
    """规则 15：[(文件, 变量名, 行号)]。定义处：css/tokens.css、JS_VARIABLES；app.css 另外认同一个文件里的声明。"""
    shared = set(JS_VARIABLES)
    local = {}
    for path, raw in files.items():
        names = {d.prop for d in css_declarations(raw) if d.prop.startswith('--')}
        local[path] = names if path == CSS_FILE else set()
        if path == TOKENS_FILE:
            shared |= names
    found = []
    for path, raw in files.items():
        text = re.sub(r'/\*.*?\*/', lambda m: _BLANK.sub(' ', m.group(0)), raw, flags=re.S)
        line_of = line_finder(text)
        for m in VAR_NAME_RE.finditer(text):
            if m.group(1) not in shared and m.group(1) not in local[path]:
                found.append((path, m.group(1), line_of(m.start())))
    return found


# 规则 19：引样式表的页面（相对仓库根）和每个页面里第一个样式表应有的地址。
STYLE_PAGES = {'web/static/index.html': '/css/tokens.css', 'tools/styleguide/index.html': '../../web/static/css/tokens.css'}
LINK_RE = re.compile(r'<link\b[^>]*\brel\s*=\s*["\']?stylesheet\b["\']?[^>]*>', re.I)
HREF_RE = re.compile(r'\bhref\s*=\s*(?:"([^"]*)"|\'([^\']*)\'|([^\s"\'>]+))', re.I)
TINT_HOME = 'ui/tint.js'


def load_pages(root):
    """{相对仓库根的路径: 文字}：引样式表的页面（不存在的跳过）。"""
    return {rel: (root / rel).read_text(encoding='utf-8') for rel in STYLE_PAGES if (root / rel).exists()}


def stylesheet_order(pages, css_files):
    """规则 19：[(页面, 基线键, 内容, 怎么改)]。"""
    found = []
    for rel, html in pages.items():
        hrefs = [next(g for g in h.groups() if g is not None) for m in LINK_RE.finditer(re.sub(r'<!--.*?-->', '', html, flags=re.S))
                 if (h := HREF_RE.search(m.group(0)))]
        want = STYLE_PAGES[rel]
        if not hrefs or hrefs[0] != want:
            found.append((rel, f'{rel}: 第一个样式表不是 tokens.css', f'第一个样式表是 {hrefs[0] if hrefs else "（没有）"}',
                          f'把 <link rel="stylesheet" href="{want}"> 放在所有样式表的最前面'))
        if rel == 'web/static/index.html':
            for path in sorted(css_files):
                if path.startswith('css/') and f'/{path}' not in hrefs:
                    found.append((rel, f'{rel}: 没有引 {path}', f'没有 <link href="/{path}">',
                                  f'在 base.css 之后照现有顺序加一行 <link rel="stylesheet" href="/{path}">'))
    return found


def load_css(root):
    """{相对 web/static 的路径: 文字}：app.css 和 css/ 下的全部样式。"""
    base = root / STATIC
    if not (base / CSS_FILE).exists():
        raise SystemExit(f'check_frontend: 缺少 {STATIC}/{CSS_FILE}')
    return {path.relative_to(base).as_posix(): path.read_text(encoding='utf-8')
            for pattern in CSS_GLOBS for path in sorted(base.glob(pattern))}


def safari17(rules, p, s, classic=False):
    """规则 12：一个文件里 Safari 17.0 没有的 JS 能力。"""
    for what, (pattern, since) in SAFARI17_JS.items():
        if since and what in SAFARI17_ENHANCEMENTS.get(p, ()):
            continue
        if what == 'startViewTransition' and not classic:
            continue  # 模块里由规则 11 管（只在 ui/motion.js），不重复报
        for m in re.finditer(pattern, s.bare):
            line = s.line_of(m.start())
            when = f'{since} 才有' if since else 'Safari 正式版没有'
            rules['safari17-js'].add(f'{p}: {what}（{when}）', f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')


def run_rules(sources, css=None, classic=None, pages=None):
    rules = new_rules()
    for p, s in (classic or {}).items():
        safari17(rules, p, s, classic=True)
    # 规则 19：页面里样式表的先后
    for rel, key, text, how in stylesheet_order(pages or {}, css or {}):
        rules['css-order'].add(key, f'{rel}: {text}  → {how}')
    # 规则 20：--tint 只经 ui/tint.js 写
    for p, s in sources.items():
        if p == TINT_HOME:
            continue
        for m in re.finditer(r'--tint(?![\w-])', s.code):
            line = s.line_of(m.start())
            rules['tint-writer'].add(p, f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
    if isinstance(css, str):
        css = {CSS_FILE: css}
    for path, raw in (css or {}).items():
        where = f'{STATIC}/{path}'
        # 规则 13：三档令牌和新代码的档位（键带文件名）
        for key, d, how in scale_issues(css_declarations(raw)):
            selector = d.selector if len(d.selector) <= 60 else d.selector[:59] + '…'
            rules['css-scale'].add(f'{path}: {key}', f'{where}:{d.line}: {selector} {{ {d.prop}: {d.value[:70]} }}  → {how}')
        # 规则 14、16、17、18（层）、19（tokens.css 里声明的位置）
        for rid, key, line, text, how in structure_issues(path, raw):
            rules[rid].add(key, f'{where}:{line}: {text[:90]}  → {how}')
    if css:
        # 规则 8：写死的字面量
        for path, key, d, literal, how in css_literals(css):
            selector = d.selector if len(d.selector) <= 60 else d.selector[:59] + '…'
            rules['css-literal'].add(key, f'{STATIC}/{path}:{d.line}: {selector} {{ {d.prop}: {d.value[:70]} }}  → {how}')
        # 规则 15：没定义的变量
        for path, name, line in undefined_variables(css):
            rules['css-var'].add(f'{path}: {name}', f'{STATIC}/{path}:{line}: var({name})')
        # 规则 20：样式里也不直接声明 --tint（tokens.css 里 :root 的中性默认值除外）
        for path, raw in css.items():
            for d in css_declarations(raw):
                if d.prop == '--tint' and not (path == TOKENS_FILE and d.selector in TOKEN_SELECTORS):
                    rules['tint-writer'].add(f'{path}: --tint', f'{STATIC}/{path}:{d.line}: {d.selector} {{ --tint: {d.value[:40]} }}')
    graph = {p: [t for t, _ in imports(s, sources)] for p, s in sources.items()}
    edges = {(p, t): s.raw_line(line) for p, s in sources.items() for t, line in imports(s, sources)}
    # 规则 1：core -> ui，硬规则
    for (p, t), text in sorted(edges.items()):
        if p.startswith('core/') and (t.startswith('ui/') or t == 'app.js'):
            rules['import-core-ui'].add(f'{p} -> {t}', f'{STATIC}/{p}: {text}')
    # 规则 1：环内的导入边
    for comp in strongly_connected({p: [t for t in ts] for p, ts in graph.items()}):
        members = set(comp)
        for (p, t), text in sorted(edges.items()):
            if p in members and t in members:
                rules['import-cycle'].add(f'{p} -> {t}', f'{STATIC}/{p}: {text}  (in a cycle of {len(members)} files: '
                                          + ', '.join(sorted(members)) + ')')
    # 规则 2：state 字段
    declared = declared_state(sources)
    for p, s in sources.items():
        for m in STATE_RE.finditer(s.bare):
            if m.group(1) not in declared:
                line = s.line_of(m.start())
                rules['undeclared-state'].add(m.group(1), f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
    # 规则 7：state 字段的写入者
    writers, problems = state_writers(sources)
    for line, text in problems:
        rules['state-writer'].add(f'core/state.js:{line}: {text}', f'{STATIC}/core/state.js:{line}: {sources["core/state.js"].raw_line(line)[:110]}')
    for p, s in sources.items():
        for m in STATE_WRITE_RE.finditer(s.bare):
            name = next(g for g in m.groups() if g)
            owner = writers.get(name)
            if owner and owner != p:
                line = s.line_of(m.start())
                rules['state-writer'].add(f'{p}: state.{name}（声明的写入者是 {owner}）', f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
    # 规则 3、4、5
    for p, s in sources.items():
        for m in LISTENER_RE.finditer(s.code):
            if p in GLOBAL_LISTENER_ALLOWED:
                continue
            line = s.line_of(m.start())
            rules['document-listener'].add(f'{p}::{m.group(2) or m.group(3)}', f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
        for m in DIALOG_RE.finditer(s.code):
            if p in DIALOG_ALLOWED:
                continue
            line = s.line_of(m.start())
            rules['dialog-element'].add(p, f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
        # 规则 9
        if p != COMPOSING_HOME:
            for m in RAW_COMPOSING_RE.finditer(s.bare):
                line = s.line_of(m.start())
                rules['ime-composing'].add(f'{p}: 直接读 {m.group(0).lstrip(". ")}', f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
            for line, body in key_handlers(s):
                if not COMPOSING_CALL_RE.search(body):
                    rules['ime-composing'].add(f'{p}: 键盘处理函数没有调用 composing()', f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
        # 规则 10、11、12
        if p not in HTML_SINK_ALLOWED:
            for start, rhs in html_sinks(s):
                if not STATIC_HTML_RE.fullmatch(rhs.strip()):
                    line = s.line_of(start)
                    rules['inner-html'].add(p, f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
            for m in HTML_NAMED_RE.finditer(s.bare):
                line = s.line_of(m.start())
                rules['inner-html'].add(p, f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
        for what, pattern, allowed in ANIMATION_ENTRIES:
            if p in allowed:
                continue
            for m in pattern.finditer(s.bare):
                line = s.line_of(m.start())
                rules['animation-entry'].add(f'{p}: {what}', f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
        safari17(rules, p, s)
        if p != 'core/pages.js':
            for m in PAGE_NAME_RE.finditer(s.code):
                line = s.line_of(m.start())
                rules['page-name'].add(p, f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
        for number, text in enumerate(s.raw.splitlines(), 1):
            if len(text) > MAX_LINE:
                rules['long-line'].add(p, f'{STATIC}/{p}:{number}: ({len(text)} chars) {text.strip()[:70]}…')
        # 规则 21、22：只管 ui/
        if p.startswith('ui/'):
            lines = s.raw.count('\n') + (0 if s.raw.endswith('\n') else 1)
            # 每超出一行记一次：基线是超出的行数，所以加了行就超出基线，缩短了才能收紧
            for _ in range(MAX_UI_LINES, lines):
                rules['ui-file-length'].add(p, f'{STATIC}/{p}: 共 {lines} 行，超出 {lines - MAX_UI_LINES} 行')
            for m in SESSION_IMPORT_RE.finditer(s.code):
                names = ['*'] if m.group(2) else [n.split(' as ')[0].strip() for n in m.group(1).split(',')]
                line = s.line_of(m.start())
                for name in names:
                    if name == '*' or name in REQUEST_NAMES:
                        rules['ui-request'].add(p, f'{STATIC}/{p}:{line}: 从 core/session.js 导入 {name}')
            for m in FETCH_RE.finditer(s.bare):
                line = s.line_of(m.start())
                rules['ui-request'].add(p, f'{STATIC}/{p}:{line}: {s.raw_line(line)[:110]}')
    return rules


# ----------------------------------------------------------------------------------------------------------- baseline
def snapshot(rules):
    return {r.id: dict(sorted(r.found.items())) for r in rules.values() if r.baselined}


def load_baseline(root):
    path = root / BASELINE
    if not path.exists():
        return None
    return json.loads(path.read_text(encoding='utf-8')).get('rules', {})


def write_baseline(root, data):
    doc = {'_note': '前端结构检查的基线（tools/check_frontend.py，#320）：现状冻结，只能缩小。改好一处就运行 '
                    '`python tools/check_frontend.py --shrink` 删掉对应项；新增违规不要往这里添加，按报错的“改法”修代码。',
           'rules': data}
    (root / BASELINE).write_text(json.dumps(doc, ensure_ascii=False, indent=1) + '\n', encoding='utf-8')


def compare(rules, baseline):
    """返回 (errors, stale)：超出基线的新违规，以及基线里已不再出现的项。"""
    errors, stale = [], []
    for rule in rules.values():
        base = baseline.get(rule.id, {}) if rule.baselined else {}
        unregistered = (f'\n  基线里还没有规则 {rule.id}：新规则首次登记存量用 `python tools/check_frontend.py --init-rule {rule.id}`。'
                        if rule.baselined and rule.id not in baseline else '')
        for key, count in sorted(rule.found.items()):
            allowed = base.get(key, 0)
            if count > allowed:
                places = rule.where[key]
                shown = places if len(places) <= 8 else places[:8] + [f'… 另有 {len(places) - 8} 处']
                what = f'{key}（现有 {count} 处，基线允许 {allowed} 处）' if rule.baselined else key
                errors.append(f'[规则 {rule.number} · {rule.id}] {rule.title}: {what}\n  '
                              + '\n  '.join(shown)
                              + f'\n  约定：{rule.convention}\n  改法：{rule.fix}' + unregistered)
        for key, allowed in sorted(base.items()):
            if rule.found.get(key, 0) < allowed:
                stale.append(f'[{rule.id}] {key}: 基线 {allowed}，现有 {rule.found.get(key, 0)}')
    return errors, stale


def baseline_growth(current, reference, introduced=frozenset()):
    """`current` 里比 `reference`（基准 ref 上的基线）多出来或数量更大的项。
    `introduced` 是在基准上还不存在的规则（见 introduced_rules）：它们的整份基线是首次登记，不算变大。"""
    grown = []
    for rule, entries in current.items():
        if rule in introduced:
            continue
        for key, count in entries.items():
            if count > reference.get(rule, {}).get(key, 0):
                grown.append(f'[{rule}] {key}: {reference.get(rule, {}).get(key, 0)} -> {count}')
    return grown


def introduced_rules(current, reference, reference_script, rules):
    """`current` 里在基准上还不存在的规则：基准的基线里没有这一项，基准的检查脚本里也没有定义它。
    只看基线不够：基准脚本里已有的规则不会缺席基线（--init 和 --shrink 都会留下这一项，哪怕是空的），
    缺席说明有人删了它，这时照常按「基线变大」处理。"""
    return frozenset(rid for rid in current if rid in rules and rules[rid].baselined and rid not in reference
                     and f"Rule('{rid}'" not in reference_script)


def file_at(root, ref, path):
    result = subprocess.run(['git', '-C', str(root), 'show', f'{ref}:{path}'], capture_output=True, text=True)
    return result.stdout if result.returncode == 0 else None


def baseline_at(root, ref):
    text = file_at(root, ref, BASELINE.as_posix())
    return None if text is None else json.loads(text).get('rules', {})


def check(root=ROOT, baseline=None):
    """返回 (errors, stale)。传入 `baseline` 时覆盖基线文件（测试用）。"""
    rules = run_rules(load_sources(root), load_css(root), load_sources(root, CLASSIC_SCRIPTS), load_pages(root))
    if baseline is None:
        baseline = load_baseline(root)
    if baseline is None:
        return ['缺少 tools/frontend_baseline.json；首次建立基线用 `python tools/check_frontend.py --init`。'], []
    return compare(rules, baseline)


def init_rule(root, rules, baseline, rid):
    """--init-rule：只给一条还没登记的规则按现状写基线，其他规则原样保留。"""
    rule = rules.get(rid)
    if rule is None:
        print(f'没有规则 {rid}；带基线的规则有：' + '、'.join(r.id for r in rules.values() if r.baselined), file=sys.stderr)
        return 1
    if not rule.baselined:
        print(f'规则 {rid} 是硬规则，没有基线；违规要改代码。', file=sys.stderr)
        return 1
    if rid in baseline:
        print(f'规则 {rid} 已经登记过基线；之后只能用 --shrink 缩小，新增违规要改代码。', file=sys.stderr)
        return 1
    added = dict(sorted(rule.found.items()))
    order = [r for r in rules if r in baseline or r == rid] + [r for r in baseline if r not in rules]
    write_baseline(root, {r: added if r == rid else baseline[r] for r in order})
    print(f'已在 {BASELINE} 里登记规则 {rid} 的存量：{len(added)} 项，共 {sum(added.values())} 处。和新规则在同一个 PR 里提交。')
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--init', action='store_true', help='按当前文件树建立基线（仅在基线不存在时）')
    parser.add_argument('--init-rule', metavar='RULE',
                        help='只给基线里还没有的一条新规则按现状登记存量（新加一条带基线的规则时用一次，和新规则在同一个 PR 里提交）；'
                             '已登记的规则拒绝，其他规则的基线不动。--base-ref 认得出这是首次登记：基准上的基线和脚本里都没有这条规则')
    parser.add_argument('--shrink', action='store_true', help='调低或删除已不再出现的基线项；从不新增')
    parser.add_argument('--base-ref', metavar='REF', help='基线比这个 git ref 上的大时报错')
    parser.add_argument('--strict-stale', action='store_true', help='把基线里已不再出现的项算作失败')
    args = parser.parse_args(argv)
    root = args.root
    rules = run_rules(load_sources(root), load_css(root), load_sources(root, CLASSIC_SCRIPTS), load_pages(root))
    if args.init:
        if (root / BASELINE).exists():
            print('基线已存在；只能用 --shrink 缩小。', file=sys.stderr)
            return 1
        write_baseline(root, snapshot(rules))
        print(f'已写入 {BASELINE}')
        return 0
    baseline = load_baseline(root)
    if baseline is None:
        print('缺少 tools/frontend_baseline.json；首次建立基线用 `python tools/check_frontend.py --init`。')
        return 1
    if args.init_rule:
        return init_rule(root, rules, baseline, args.init_rule)
    if args.shrink:
        shrunk = {rid: {k: min(v, rules[rid].found.get(k, 0)) for k, v in entries.items() if rules[rid].found.get(k, 0)}
                  for rid, entries in baseline.items() if rid in rules}
        write_baseline(root, shrunk)
        print(f'已按现状收紧 {BASELINE}（只删减，不新增；新增违规仍会报错）')
        baseline = shrunk
    errors, stale = compare(rules, baseline)
    if args.base_ref:
        reference = baseline_at(root, args.base_ref)
        if reference is None:
            print(f'注意：{args.base_ref} 上没有基线文件，跳过“基线变大”检查。')
        else:
            introduced = introduced_rules(baseline, reference, file_at(root, args.base_ref, 'tools/check_frontend.py') or '', rules)
            for rid in sorted(introduced):
                print(f'注意：规则 {rid} 在 {args.base_ref} 上还没有，这份基线（{sum(baseline[rid].values())} 处）是首次登记，不算基线变大。')
                over = [f'{key}: 基线 {count}，现有 {rules[rid].found.get(key, 0)}' for key, count in sorted(baseline[rid].items())
                        if count > rules[rid].found.get(key, 0)]
                if over:
                    errors.append(f'[首次登记多于现状] 规则 {rid}：' + '；'.join(over[:5])
                                  + f'\n  首次登记只能照现状登记：删掉 {BASELINE} 里这一项，重新运行 `python tools/check_frontend.py --init-rule {rid}`。')
            for line in baseline_growth(baseline, reference, introduced):
                errors.append(f'[基线变大] {line}\n  基线只能缩小：新增违规要改代码，不能往 {BASELINE} 里添加。')
    for error in errors:
        print(error)
    if stale:
        label = '错误' if args.strict_stale else '提示'
        print(f'\n{label}：基线里有 {len(stale)} 项已不再出现，可收紧（`python tools/check_frontend.py --shrink`）：')
        for line in stale[:20]:
            print('  ' + line)
        if len(stale) > 20:
            print(f'  … 另有 {len(stale) - 20} 项')
        if args.strict_stale:
            errors.append('stale baseline')
    if not errors:
        total = {r.id: sum(r.found.values()) for r in rules.values() if r.baselined}
        print('frontend structure valid: no new violations beyond baseline ('
              + ', '.join(f'{k} {v}' for k, v in total.items()) + ')')
    return 1 if errors else 0


if __name__ == '__main__':
    raise SystemExit(main())
