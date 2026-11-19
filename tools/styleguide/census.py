#!/usr/bin/env python3
"""统计 web/static 的样式（app.css 和 css/ 下的全部文件，#705）实际用到的取值，供样式清单页（tools/styleguide/index.html，#318）使用。

生成 tools/styleguide/census.js（`window.CENSUS = {...}`），页面用普通的 <script> 读它，所以从 file:// 直接
打开也不需要服务器和构建步骤。样式一改就要重跑；`python3 tools/check_styleguide.py` 会拦下没重跑的情况：

    python3 tools/styleguide/census.py          # 改写 census.js
    python3 tools/styleguide/census.py --print  # 只打印摘要，不写文件

统计口径：
  * 一「处」= 一条规则里的一条 `属性:值` 声明（选择器列表只算一次，不按选择器个数算）；
  * 取值只做规范化（去掉 `!important`、`0.5` 写成 `.5`、合并空白），从不按相近程度合并：
    `10.5px` 和 `11px` 是两种取值，清单页要的正是这个；
  * 字号另读 `font:` 简写里的字号；圆角、gap、padding、z-index 读同名属性；
    高度只数 `height` / `min-height` 里写死的 px 和 `var(--h-N)`（不是每个有高度的元素都是控件）；
  * 颜色表读 css/tokens.css 的两个主题块（第一个 `:root{...}` 和 `:root[data-theme=dark]{...}`），块里的注释作为分组标题；
  * 令牌表读 css/tokens.css 里 `/* TOKENS-BEGIN */` 与 `/* TOKENS-END */` 之间的块（#317、#705），亮色在 `:root`，
    暗色另写在 `:root[data-theme=dark]` 的（阴影）记在 dark 里；引用次数是全部样式里 `var(--名字)` 的个数；
  * `files` 是各文件的字节哈希，`sha256` 是它们合起来的哈希，检查脚本和清单页据此判断 census.js 是否过期。

本模块可以被导入（tools/check_styleguide.py 用 `render()` 在内存里重算一遍做比较），导入时不读写任何文件。
"""
import hashlib
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
STATIC = Path('web/static')
CSS_GLOBS = ('app.css', 'css/*.css')  # 相对 web/static
TOKENS = 'css/tokens.css'
OUT = Path('tools/styleguide/census.js')
HEADER = '// 由 tools/styleguide/census.py 从 web/static/app.css 和 web/static/css/ 生成，不要手改。\n'
# 令牌族：按前缀归类，长前缀在前（--fs-display- 要先于 --fs- 匹配）。
TOKEN_FAMILIES = (('fsDisplay', '--fs-display-'), ('fs', '--fs-'), ('radius', '--radius-'), ('space', '--space-'),
                  ('h', '--h-'), ('z', '--z-'), ('fw', '--fw-'), ('shadow', '--shadow-'), ('dur', '--dur-'), ('ease', '--ease'))


def read_files(root=ROOT):
    """{相对 web/static 的路径: 文字}：app.css 和 css/ 下的全部样式，按路径排好。"""
    base = root / STATIC
    return {p.relative_to(base).as_posix(): p.read_bytes().decode('utf-8') for pattern in CSS_GLOBS for p in sorted(base.glob(pattern))}


def strip_comments(css):
    return re.sub(r'/\*.*?\*/', '', css, flags=re.S)


def parse_rules(css, context=''):
    """返回每条样式规则的 [(选择器, 声明体, 所在 at 规则的前导或 '')]，@media / @supports 里的规则也算。"""
    rules, pos, head, n = [], 0, 0, len(css)
    while pos < n:
        if css[pos] != '{':
            pos += 1
            continue
        depth, j = 1, pos + 1
        while depth and j < n:
            depth += {'{': 1, '}': -1}.get(css[j], 0)
            j += 1
        # 前导里可能先有顶层的语句（`@layer a, b;`），只取最后一个 `;` 之后的部分
        prelude, body = css[head:pos].rsplit(';', 1)[-1].strip(), css[pos + 1:j - 1]
        if prelude.startswith(('@media', '@supports', '@layer')):
            rules.extend(parse_rules(body, prelude))
        elif not prelude.startswith('@'):  # @keyframes 的各帧不是组件样式
            rules.append((prelude, body, context))
        pos = head = j
    return rules


def declarations(body):
    """返回 [(属性, 值)]；只在括号和引号之外按 ; 切分（data: URL 里有 ;）。"""
    out, depth, quote, cur = [], 0, '', ''
    for ch in body:
        if quote:
            cur += ch
            if ch == quote:
                quote = ''
        elif ch in '"\'':
            quote = ch
            cur += ch
        elif ch == '(':
            depth += 1
            cur += ch
        elif ch == ')':
            depth -= 1
            cur += ch
        elif ch == ';' and depth == 0:
            out.append(cur)
            cur = ''
        else:
            cur += ch
    out.append(cur)
    result = []
    for d in out:
        if ':' in d:
            p, v = d.split(':', 1)
            result.append((p.strip().lower(), v.strip()))
    return result


def norm(v):
    v = re.sub(r'\s*!important', '', v).strip()
    v = re.sub(r'\s+', ' ', v)
    v = re.sub(r'(?<![\d.])0\.(\d)', r'.\1', v)
    return v


def top_selector(sel):
    return re.sub(r'\s+', ' ', sel.strip())


class Tally:
    """一类属性的取值计数，每种取值另记最多 4 个用到它的选择器作例子。"""

    def __init__(self):
        self.count = Counter()
        self.where = defaultdict(list)

    def add(self, value, selector):
        self.count[value] += 1
        s = top_selector(selector)
        if s not in self.where[value] and len(self.where[value]) < 4:
            self.where[value].append(s if len(s) <= 90 else s[:87] + '…')

    def table(self, key=None):
        items = sorted(self.count.items(), key=key or (lambda kv: (-kv[1], kv[0])))
        return [{'value': v, 'uses': c, 'examples': self.where[v]} for v, c in items]

    def summary(self, key=None, limit=None):
        rows = self.table(key)
        return {'kinds': len(self.count), 'uses': sum(self.count.values()), 'table': rows[:limit] if limit else rows}


def num(v):
    """取值开头的数字，用来排序；var(--x-N) 取 N，取不到数字的排最后。"""
    m = re.match(r'-?[\d.]+', v) or re.match(r'var\(--[a-z]+(?:-[a-z]+)*-(\d+)\)', v)
    if not m:
        return 1e9
    text = m.group(1) if m.lastindex else m.group()
    try:
        return float(text)
    except ValueError:
        return 1e9


def block_after(css, header):
    """`header` 开头的块的花括号内文字；找不到返回 ''。"""
    i = css.find(header)
    if i < 0:
        return ''
    j = css.find('{', i)
    depth, k = 1, j + 1
    while depth and k < len(css):
        depth += {'{': 1, '}': -1}.get(css[k], 0)
        k += 1
    return css[j + 1:k - 1]


def theme_groups(raw, header):
    """主题块里的自定义属性，按块内注释分组：[(标题, {名字: 值})]。注释之前的第一组标题为「基础」。"""
    parts = re.split(r'/\*(.*?)\*/', block_after(raw, header), flags=re.S)
    groups, title = [], '基础'
    for index, part in enumerate(parts):
        if index % 2:
            title = part.strip()
            continue
        found = {p: norm(v) for p, v in declarations(part) if p.startswith('--')}
        if found:
            groups.append((title, found))
    return groups


def token_table(raw, var_uses):
    """TOKENS-BEGIN/END 之间声明的令牌，按族分好：{族: [{name, value, uses[, dark]}]}。暗色另写的值记在 dark 里。"""
    m = re.search(r'/\*\s*TOKENS-BEGIN\s*\*/(.*?)/\*\s*TOKENS-END\s*\*/', raw, flags=re.S)
    tokens = {family: [] for family, _ in TOKEN_FAMILIES}
    if not m:
        return tokens
    found = {}
    for selector, body, context in parse_rules(strip_comments(m.group(1))):
        if context:
            continue  # @media 里的（「减少动态效果」时的时长）不是档位
        dark = 'data-theme=dark' in selector
        for name, value in declarations(body):
            family = next((f for f, prefix in TOKEN_FAMILIES if name.startswith(prefix)), None)
            if not family:
                continue
            if dark and name in found:
                found[name]['dark'] = norm(value)
            elif not dark:
                found[name] = {'name': name, 'value': norm(value), 'uses': var_uses.get(name, 0)}
                tokens[family].append(found[name])
    return tokens


def build(files):
    """由各样式文件的文字（{路径: 文字}）算出 census 数据（dict）。只给一段文字时当作 app.css。"""
    if isinstance(files, str):
        files = {'app.css': files}
    raw = '\n'.join(files.values())
    tokens_raw = files.get(TOKENS, raw)
    css = strip_comments(raw)
    rules = parse_rules(css)
    token_rules = {(sel, body) for sel, body, _ in parse_rules(strip_comments(files.get(TOKENS, '')))}
    fs, rad, gap, pad, hei, zi, lit = Tally(), Tally(), Tally(), Tally(), Tally(), Tally(), Tally()
    dark_overrides = 0
    for sel, body, _ in rules:
        dark = 'data-theme=dark' in sel
        for prop, val in declarations(body):
            v = norm(val)
            if prop == 'font-size':
                fs.add(v, sel)
            elif prop == 'font':
                m = re.search(r'(?<![\w.-])(\d*\.?\d+(?:px|em|rem))(?:/[\d.]+)?\s', v + ' ')
                if m:
                    fs.add(m.group(1) + ' (font 简写)', sel)
            elif prop in ('border-radius', 'border-top-left-radius', 'border-top-right-radius', 'border-bottom-left-radius',
                          'border-bottom-right-radius'):
                rad.add(v, sel)
            elif prop in ('gap', 'row-gap', 'column-gap'):
                gap.add(v, sel)
            elif prop == 'padding':
                pad.add(v, sel)
            elif prop in ('height', 'min-height') and re.fullmatch(r'[\d.]+px|var\(--h-\d+\)', v):
                hei.add(f'{prop}: {v}', sel)  # 只数固定尺寸：100%、auto、0 属于布局
            elif prop == 'z-index':
                zi.add(v, sel)
            if not prop.startswith('--'):
                for h in re.findall(r'#[0-9a-fA-F]{3,8}\b', v):
                    lit.add(h.lower(), sel)
        if dark and top_selector(sel) != ':root[data-theme=dark]' and (sel, body) not in token_rules:  # 变量块本身不算覆盖
            dark_overrides += 1
    # background-image 的 url(...) 里，# 颜色按百分号编码写成 %23，不会被上面数进去，这是对的。
    var_uses = Counter(re.findall(r'var\((--[\w-]+)', css))
    light_groups = theme_groups(tokens_raw, ':root{')
    dark_groups = theme_groups(tokens_raw, ':root[data-theme=dark]{')
    light = {k: v for _, group in light_groups for k, v in group.items()}
    dark = {k: v for _, group in dark_groups for k, v in group.items()}
    by_height = lambda kv: (num(kv[0].split(': ')[1]), kv[0])
    hashes = {path: hashlib.sha256(text.encode('utf-8')).hexdigest() for path, text in files.items()}
    return {
        'source': '、'.join(f'{STATIC.as_posix()}/{path}' for path in files),
        'files': hashes,
        'sha256': hashlib.sha256(''.join(f'{path} {digest}\n' for path, digest in hashes.items()).encode('utf-8')).hexdigest(),
        'lines': sum(text.count('\n') + 1 for text in files.values()),
        'rules': len(rules),
        'themeVars': {'light': light, 'dark': dark, 'uses': dict(var_uses),
                      'groups': [{'title': title, 'names': list(group)} for title, group in light_groups]},
        'tokens': token_table(tokens_raw, var_uses),
        'darkOverrideRules': dark_overrides,
        'literalHexColors': lit.summary(limit=60),
        'rgbaCalls': len(re.findall(r'rgba?\(', css)),
        'fontSize': fs.summary(key=lambda kv: (num(kv[0]), kv[0])),
        'radius': rad.summary(key=lambda kv: (num(kv[0]), kv[0])),
        'gap': gap.summary(key=lambda kv: (num(kv[0]), kv[0])),
        'padding': pad.summary(limit=24),
        'height': hei.summary(key=by_height),
        'zIndex': zi.summary(key=lambda kv: (num(kv[0]), kv[0])),
    }


def render(files):
    """census.js 应有的完整文字。"""
    return HEADER + 'window.CENSUS = ' + json.dumps(build(files), ensure_ascii=False, indent=1) + ';\n'


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    raw = read_files()
    if '--print' in argv:
        data = build(raw)
        for k in ('fontSize', 'radius', 'gap', 'padding', 'height', 'zIndex', 'literalHexColors'):
            print(f"{k}: {data[k]['kinds']} 种取值, {data[k]['uses']} 处")
        print('令牌', {k: len(v) for k, v in data['tokens'].items()}, '亮色变量', len(data['themeVars']['light']),
              '暗色变量', len(data['themeVars']['dark']), 'var() 引用', sum(data['themeVars']['uses'].values()),
              '暗色单独覆盖', data['darkOverrideRules'], 'rgb()/rgba()', data['rgbaCalls'])
        return 0
    out = ROOT / OUT
    out.write_bytes(render(raw).encode('utf-8'))
    print(f'已写入 {OUT}（{out.stat().st_size} 字节）')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
