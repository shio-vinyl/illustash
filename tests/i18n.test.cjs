// 翻译一致性（#173、#182）：页面里每个 t('…') 字面量和每段 data-t / data-t-attr 文本都与
// web/static/locales/<code>.json 的每张表对得上；表里没有代码已不再使用的键，占位符、上下文和复数形式与中文相符。
// 还没翻译的文本回退为中文：按语言计数并报告，从不算失败。后端自己的文本（#173 第 3 阶段）是它 Rust 源码
// （native/terms/src、crates/*/src）里的中文字符串字面量；页面用 tb() 翻译，表里也可以收这些字面量。
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {readFileSync, readdirSync} = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const STATIC = path.join(__dirname, '../web/static');
const read = name => readFileSync(path.join(STATIC, name), 'utf8');
// 桌面外壳共用这些表（#173 第 4 阶段）：它的启动页像图库页一样调用 t()，它的 Rust 代码调用
// tr("…") / tr_with("…", …)（desktop/src/i18n.rs）。
const DESKTOP = path.join(__dirname, '../desktop');
const shellPage = readFileSync(path.join(DESKTOP, 'dist/index.html'), 'utf8');
const shellSources = readdirSync(path.join(DESKTOP, 'src')).filter(name => name.endsWith('.rs'))
  .map(name => ['desktop/src/' + name, readFileSync(path.join(DESKTOP, 'src', name), 'utf8')]);
// 与 native/terms/src/app_preferences.rs 的规则相同：文件以语言代码命名。
const CODE = /^[a-z]{2,3}(-[A-Za-z0-9]{1,8})*$/;
const locales = Object.fromEntries(readdirSync(path.join(STATIC, 'locales')).filter(name => name.endsWith('.json'))
  .map(name => [name.slice(0, -5), JSON.parse(read('locales/' + name))]));
const english = locales.en;
// i18n.js 本身只翻译交给它的文本，core/i18n.js 只把它的函数交给各模块。
// 页面的其他模块（core/、ui/；#199）也算脚本。
const scripts = readdirSync(STATIC, {recursive: true}).map(name => name.split(path.sep).join('/'))
  .filter(name => name.endsWith('.js') && !['i18n.js', 'core/i18n.js'].includes(name)).sort();
const placeholders = text => [...text.matchAll(/\{(\w+)\}/g)].map(m => m[1]).sort();
const unescape = (quote, body) => vm.runInNewContext(quote + body + quote);

function scriptKeys(name, source = read(name)) {
  const keys = [], dynamic = [];
  for (const call of source.matchAll(/(?<![\w.$])t\(\s*(['"`]?)/g)) {
    const rest = source.slice(call.index + call[0].length - call[1].length);
    const literal = call[1] && rest.match(/^(['"])((?:\\.|(?!\1)[^\\\n])*)\1\s*[,)]/);
    // t('…', {context: 'x'}) 按 "x\u0004…" 查找（即 gettext 的 msgctxt）。
    const context = literal && rest.slice(literal[0].length - 1).match(/^,\s*\{\s*context:\s*'([\w-]+)'/);
    if (literal) keys.push((context ? context[1] + '\u0004' : '') + unescape(literal[1], literal[2]));
    else dynamic.push(`${name}:${source.slice(0, call.index).split('\n').length}`);
  }
  return {keys, dynamic};
}
function markupKeys(html) {
  const keys = [];
  for (const {0: tag, index} of html.matchAll(/<[a-z][^>]*>/g)) {
    if (/\sdata-t[\s>]/.test(tag)) keys.push(html.slice(index + tag.length).match(/^([^<]*)</)[1]);
    const attrs = tag.match(/\sdata-t-attr="([^"]*)"/);
    if (attrs) for (const name of attrs[1].split(' ')) {
      const value = tag.match(new RegExp(`\\s${name}="([^"]*)"`));
      assert.ok(value, `${tag} has no ${name}`);
      keys.push(value[1].replaceAll('&amp;', '&').replaceAll('&quot;', '"'));
    }
  }
  return keys;
}

// 后端 Rust 源码里测试模块之外的每个中文字符串字面量，取程序里实际持有的值。
const ROOT = path.join(__dirname, '..');
function rustFiles() {
  const out = [];
  const walk = dir => {
    for (const entry of readdirSync(dir, {withFileTypes: true})) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) { if (entry.name !== 'tests') walk(full); }
      else if (entry.name.endsWith('.rs') && entry.name !== 'tests.rs') out.push(full);
    }
  };
  walk(path.join(ROOT, 'native/terms/src'));
  for (const crate of readdirSync(path.join(ROOT, 'crates'))) walk(path.join(ROOT, 'crates', crate, 'src'));
  return out.sort();
}
function rustUnescape(body) {
  return body.replace(/\\(?:\n\s*|u\{([0-9a-fA-F]+)\}|x([0-9a-fA-F]{2})|(.))/g, (all, code, hex, c) =>
    code ? String.fromCodePoint(parseInt(code, 16)) : hex ? String.fromCharCode(parseInt(hex, 16))
      : c === undefined ? '' : ({n: '\n', t: '\t', r: '\r', 0: '\0'})[c] ?? c);
}
// 每个字面量的 [文件, 行号, 文本, 紧挨着的前文代码]；跳过注释和字符字面量。
function rustLiterals(file) {
  let src = readFileSync(file, 'utf8');
  const tests = src.search(/\n#\[cfg\(test\)\]\s*\n\s*(?:pub(?:\(crate\))? )?mod \w+\s*\{/);
  if (tests >= 0) src = src.slice(0, tests);
  const out = [];
  let line = 1;
  for (let i = 0; i < src.length;) {
    const c = src[i], rest = src.slice(i, i + 12);
    if (c === '\n') { line++; i++; continue; }
    if (rest.startsWith('//')) { const j = src.indexOf('\n', i); i = j < 0 ? src.length : j; continue; }
    if (rest.startsWith('/*')) { const j = src.indexOf('*/', i); line += src.slice(i, j).split('\n').length - 1; i = j + 2; continue; }
    const raw = !/[\w]/.test(src[i - 1] || '') && rest.match(/^b?r(#*)"/);
    if (raw) {
      const start = i + raw[0].length, end = src.indexOf('"' + raw[1], start), text = src.slice(start, end);
      out.push([file, line, text, src.slice(Math.max(0, i - 60), i)]);
      line += text.split('\n').length - 1; i = end + 1 + raw[1].length; continue;
    }
    if (c === "'") { const char = rest.match(/^'(?:\\u\{[0-9a-fA-F]+\}|\\.|[^\\'])'/); i += char ? char[0].length : 1; continue; }
    if (c === '"') {
      let j = i + 1;
      while (src[j] !== '"') j += src[j] === '\\' ? 2 : 1;
      const body = src.slice(i + 1, j);
      out.push([file, line, rustUnescape(body), src.slice(Math.max(0, i - 60), i)]);
      line += body.split('\n').length - 1; i = j + 1; continue;
    }
    i++;
  }
  return out.filter(([, , text]) => /\p{Script=Han}/u.test(text));
}
const rust = rustFiles().flatMap(rustLiterals);
// 关于页显示的发布渠道来自构建的元数据，不是字面量。
const application = path.join(ROOT, 'native/terms/application.json');
rust.push([application, 1, JSON.parse(readFileSync(application, 'utf8')).channel, '']);
const backendKeys = new Set(rust.map(([, , text]) => text));
// 不会到达页面的后端文本：命令行、测试 harness、日志和 panic。只有覆盖率报告不计它们；表里仍可以翻译。
const NOT_SHOWN = /^(?:native\/terms\/src\/(?:cli|read_harness)\.rs|native\/terms\/src\/bin\/|crates\/iqdb\/)/;
// SQL 也不计，句子片段（首尾带空格的文本）也不计。
const shownBackend = () => new Set(rust.filter(([file, , text, before]) => !NOT_SHOWN.test(path.relative(ROOT, file).split(path.sep).join('/'))
  && !/(?:e?print(?:ln)?!|panic!|unreachable!|expect|assert\w*!)\(\s*$/.test(before)
  && !/\b(?:CREATE|SELECT|INSERT|UPDATE|DELETE)\b/.test(text) && text.trim() === text).map(([, , text]) => text));

// 外壳 Rust 代码里的 tr("…")：普通字符串字面量，转义按 JSON 读取（\" 和 \\ 含义相同）。
// 注释行和这两个函数自己的定义不算调用。
function rustKeys(name, text) {
  const keys = [], dynamic = [];
  const source = text.split('\n').map(line => /^\s*\/\//.test(line) ? '' : line).join('\n');
  for (const call of source.matchAll(/(?<![\w])(fn\s+)?tr(?:_with)?\(/g)) {
    if (call[1]) continue;
    const rest = source.slice(call.index + call[0].length);
    const literal = rest.match(/^\s*"((?:\\.|[^"\\])*)"\s*[,)]/);
    if (literal) keys.push(JSON.parse(`"${literal[1]}"`));
    else dynamic.push(`${name}:${source.slice(0, call.index).split('\n').length}`);
  }
  return {keys, dynamic};
}
const shellScript = [...shellPage.matchAll(/<script>([\s\S]*?)<\/script>/g)].map(m => m[1]).join('\n');

function usedKeys() {
  const used = new Set(), dynamic = [];
  const add = found => { found.keys.forEach(k => used.add(k)); dynamic.push(...found.dynamic); };
  for (const name of scripts) add(scriptKeys(name));
  // 模块里 ui/el.js 的静态模板（tpl`…`）和 index.html 一样用 data-t / data-t-attr 标出要翻译的文字（#704）。
  for (const name of scripts) for (const m of read(name).matchAll(/\btpl`([^`]*)`/g)) markupKeys(m[1]).forEach(k => used.add(k));
  // i18n.js 只翻译交给它的内容，后端消息的连接符除外。
  used.add('join\u0004；');
  markupKeys(read('index.html')).forEach(k => used.add(k));
  add(scriptKeys('desktop/dist/index.html', shellScript));
  markupKeys(shellPage).forEach(k => used.add(k));
  for (const [name, source] of shellSources) add(rustKeys(name, source));
  return {used, dynamic};
}

test('every translated text in the code is a plain literal', () => {
  const {used, dynamic} = usedKeys();
  // 运行时算出的键无法检查，也无法收集给译者。
  assert.deepEqual(dynamic, [], 't() must be called with a single- or double-quoted literal');
  assert.ok(used.size > 0);
  for (const key of used) {
    const context = key.split('\u0004');
    assert.ok(context.length === 1 || (context.length === 2 && /^[\w-]+$/.test(context[0]) && context[1]), `bad context: ${key}`);
    assert.ok(!key.startsWith('@'), `@ keys are the table's own: ${key}`);
  }
});

// 表项是一个字符串，或者 {n} 的各个复数形式：取该语言 Intl.PluralRules 的类别，总要有 other。
function checkEntry(code, key, value) {
  if (typeof value === 'string') {
    if (value) assert.deepEqual(placeholders(value), placeholders(key), `${code}: placeholders differ: ${key}`);
    return !!value;
  }
  assert.ok(value && typeof value === 'object' && !Array.isArray(value), `${code}: not text or plural forms: ${key}`);
  assert.ok(placeholders(key).includes('n'), `${code}: plural forms need {n}: ${key}`);
  const categories = new Intl.PluralRules(code).resolvedOptions().pluralCategories;
  assert.equal(typeof value.other, 'string', `${code}: plural forms need other: ${key}`);
  for (const [category, form] of Object.entries(value)) {
    assert.ok(categories.includes(category), `${code}: ${category} is not a plural form of this language: ${key}`);
    assert.equal(typeof form, 'string', `${code}: ${key} ${category}`);
    // 某个形式可以不含 {n}（如 "one picture"），但不能带自己的占位符。
    if (form) assert.ok(placeholders(form).every(p => placeholders(key).includes(p)), `${code}: placeholders differ: ${key} ${category}`);
  }
  if (value.other) assert.deepEqual(placeholders(value.other), placeholders(key), `${code}: placeholders differ: ${key} other`);
  return !!value.other;
}

test('every table matches the code: no unused keys, matching placeholders, plural forms of its language', () => {
  const {used} = usedKeys();
  assert.ok(english, 'locales/en.json');
  // 中文是源文本，所以没有表。
  assert.ok(!('zh-CN' in locales), 'zh-CN is the source text, not a table');
  for (const [code, table] of Object.entries(locales)) {
    assert.match(code, CODE);
    assert.equal(typeof table, 'object');
    assert.ok(typeof table['@name'] === 'string' && table['@name'], `${code}.json needs "@name", the language in itself`);
    let translated = 0;
    for (const [key, value] of Object.entries(table)) {
      if (key.startsWith('@')) { assert.equal(key, '@name', `${code}.json: unknown ${key}`); continue; }
      assert.ok(used.has(key) || backendKeys.has(key), `${code}.json has a key no code uses: ${key}`);
      if (checkEntry(code, key, value) && used.has(key)) translated++;
    }
    console.log(`${code} (${table['@name']}): ${translated}/${used.size} texts translated, ${used.size - translated} fall back to Chinese`);
    const backend = [...shownBackend()], done = backend.filter(key => table[key] && (typeof table[key] === 'string' || table[key].other));
    console.log(`${code} (${table['@name']}): backend ${done.length}/${backend.length} texts translated, ${backend.length - done.length} fall back to Chinese`);
  }
});

function page(language, languages = ['zh-CN'], tables = {en: english}) {
  const elements = {text: [{textContent: '设置'}, {textContent: '没有译文的字'}], attrs: [{dataset: {tAttr: 'title aria-label'}, attributes: {title: '任务', 'aria-label': '视图选项'},
    hasAttribute(name) { return name in this.attributes; }, getAttribute(name) { return this.attributes[name]; }, setAttribute(name, value) { this.attributes[name] = value; }}]};
  const document = {documentElement: {lang: 'zh-CN'}, querySelectorAll: selector => selector === '[data-t]' ? elements.text : elements.attrs};
  const names = Object.fromEntries(Object.keys(tables).map(code => [code, tables[code]['@name'] || code]));
  const context = {self: {ILLUSTASH_LANGUAGE: language, ILLUSTASH_LANGUAGES: names, ILLUSTASH_TABLES: tables}, navigator: {languages}, document};
  vm.createContext(context);
  vm.runInContext(read('i18n.js'), context);
  return {context, document, elements, t: (...args) => vm.runInContext('t', context)(...args), I18N: vm.runInContext('I18N', context)};
}

test('the native file picker offers English for every title and file filter', () => {
  const name = 'desktop/src/picker.rs';
  const keys = rustKeys(name, readFileSync(path.join(ROOT, name), 'utf8')).keys;
  assert.ok(keys.length);
  for (const key of keys) assert.ok(english[key], key);
});

test('the language setting picks the table; system follows the first browser language', () => {
  for (const [language, languages, expected] of [['en', ['zh-CN'], 'en'], ['zh-CN', ['en-US'], 'zh-CN'], ['system', ['zh-TW', 'en'], 'zh-CN'],
    ['system', ['zh'], 'zh-CN'], ['system', ['en-GB', 'zh-CN'], 'en'], ['system', ['ja-JP'], 'en'], [undefined, ['fr'], 'en']]) {
    const {context, document} = page(language, languages);
    assert.equal(vm.runInContext('I18N.language', context), expected, `${language} ${languages}`);
    assert.equal(document.documentElement.lang, expected);
  }
});

test('more tables: system takes the first browser language that has one; the settings list names each in itself', () => {
  const tables = {en: {'@name': 'English', '设置': 'Settings'}, fr: {'@name': 'Français', '设置': 'Réglages'}, 'pt-BR': {'@name': 'Português (Brasil)'}};
  for (const [language, languages, expected] of [['system', ['fr-CA', 'en'], 'fr'], ['system', ['ja', 'fr'], 'fr'], ['system', ['ja', 'zh-CN', 'fr'], 'zh-CN'],
    ['system', ['pt-br'], 'pt-BR'], ['system', ['pt-PT'], 'pt-BR'], ['system', ['ja'], 'en'], ['fr', ['en'], 'fr'], ['de', ['fr'], 'fr']]) {
    assert.equal(page(language, languages, tables).I18N.language, expected, `${language} ${languages}`);
  }
  assert.equal(page('fr', [], tables).t('设置'), 'Réglages');
  assert.equal(page('system', ['fr'], {fr: tables.fr}).I18N.language, 'fr');
  // 没有英文时，没人翻译过的语言保持中文。
  assert.equal(page('system', ['ja'], {fr: tables.fr}).I18N.language, 'zh-CN');
  assert.deepEqual(JSON.parse(JSON.stringify(page('system', [], tables).I18N.languages)),
    [['zh-CN', '简体中文'], ['en', 'English'], ['fr', 'Français'], ['pt-BR', 'Português (Brasil)']]);
});

test('plural forms follow the language and {n}; Chinese and plain entries are unchanged', () => {
  const tables = {en: {'{n} 张': {one: '{n} picture', other: '{n} pictures'}, '{n} 组': {one: 'one group', other: '{n} groups'}, '只有 other {n}': {other: 'only {n}'}},
    fr: {'{n} 张': {one: '{n} image', many: '{n} d’images', other: '{n} images'}}};
  const en = page('en', [], tables), fr = page('fr', [], tables), zh = page('zh-CN', [], tables);
  assert.equal(en.t('{n} 张', {n: 1}), '1 picture');
  assert.equal(en.t('{n} 张', {n: '1'}), '1 picture');
  for (const n of [0, 2, '1,234', '1.234', '1 234']) assert.equal(en.t('{n} 张', {n}), `${n} pictures`);
  assert.equal(en.t('{n} 张', {n: '<b>1</b>'}), '<b>1</b> pictures');
  assert.equal(en.t('{n} 张'), '{n} pictures');
  assert.equal(en.t('{n} 组', {n: 1}), 'one group');
  assert.equal(en.t('只有 other {n}', {n: 1}), 'only 1');
  assert.equal(fr.t('{n} 张', {n: 1}), '1 image');
  assert.equal(fr.t('{n} 张', {n: 0}), '0 image');
  assert.equal(fr.t('{n} 张', {n: 2}), '2 images');
  assert.equal(zh.t('{n} 张', {n: 1}), '1 张');
});

test('t() translates, falls back to Chinese and fills placeholders', () => {
  const table = {'设置': 'Settings', '{n} 项进行中': '{n} running', '空译文': ''};
  const en = page('en', [], {en: table}), zh = page('zh-CN', [], {en: table});
  assert.equal(en.t('设置'), 'Settings');
  assert.equal(zh.t('设置'), '设置');
  assert.equal(en.t('没有译文'), '没有译文');
  assert.equal(en.t('空译文'), '空译文');
  assert.equal(en.t('{n} 项进行中', {n: 3}), '3 running');
  assert.equal(zh.t('{n} 项进行中', {n: 3}), '3 项进行中');
  assert.equal(en.t('{a} 和 {b}', {a: 1}), '1 和 {b}');
  assert.equal(en.t('toString'), 'toString');
  const both = {'移除': 'Remove', 'column\u0004移除': 'Removed'};
  assert.equal(page('en', [], {en: both}).t('移除'), 'Remove');
  assert.equal(page('en', [], {en: both}).t('移除', {context: 'column'}), 'Removed');
  assert.equal(page('zh-CN', [], {en: both}).t('移除', {context: 'column'}), '移除');
});

// 页面的判断不能取决于它显示的文字：在英文页面上，拿中文文本做的判断永远不会成立。改用状态字段；后端发来的消息
// 则用 said(text, '模板')：不论页面是什么语言，后端的文本都是中文，它的模板就是它的代码，必须是后端源码里的字面量。
test('stylesheets show no Chinese of their own', () => {
  // CSS 不能调用 t()：生成的文本来自页面填写的属性，写成 content: attr(data-more)。
  for (const name of readdirSync(STATIC).filter(n => n.endsWith('.css')))
    assert.doesNotMatch(read(name), /content:\s*["'][^"']*\p{Script=Han}/u, name);
});
test('no logic tests the Chinese text the page shows', () => {
  for (const name of scripts) {
    read(name).split('\n').forEach((line, i) => {
      if (/^\s*\/\//.test(line)) return;
      for (const m of line.matchAll(/\/((?:\\.|[^/\n])*\p{Script=Han}(?:\\.|[^/\n])*)\/[a-z]*\.test\(|\.(?:includes|startsWith|endsWith|indexOf)\(\s*(['"])([^'"]*\p{Script=Han}[^'"]*)\2/gu)) {
        assert.fail(`${name}:${i + 1} tests Chinese text: ${m[0]}`);
      }
    });
  }
});

test('the backend messages the page tests are templates of the backend', () => {
  let count = 0;
  for (const name of scripts) {
    const source = read(name);
    for (const call of source.matchAll(/(?<![\w.$])said\(/g)) {
      if (/^\s*\/\//.test(source.slice(source.lastIndexOf('\n', call.index) + 1, call.index))) continue;
      const literal = source.slice(call.index).match(/^said\([^,()]*(?:\([^()]*\))?[^,()]*,\s*(['"])((?:\\.|(?!\1)[^\\\n])*)\1\s*\)/);
      assert.ok(literal, `${name}:${source.slice(0, call.index).split('\n').length}: said() takes a quoted template`);
      const template = unescape(literal[1], literal[2]);
      assert.ok(backendKeys.has(template), `${name}: the backend has no such text: ${template}`);
      count++;
    }
  }
  assert.ok(count > 0);
});

test('backend text: an exact entry, else the most specific template; a nested message it holds is translated too', () => {
  const table = {'同步失败：{reason}': 'Sync failed: {reason}', '文件夹不存在或无法访问：{path}': 'Folder missing or unreadable: {path}',
    '已将 {n} 张移出 {name}': {one: 'Removed {n} picture from {name}', other: 'Removed {n} pictures from {name}'},
    '已将 {n} 张移出 {name}（跳过 {skipped} 张）': 'Removed {n} from {name} ({skipped} skipped)', '{a}：{b}': '{a}: {b}',
    '其他': 'Other', '会话已过期，请刷新页面': 'Session expired, reload the page', 'join\u0004；': '; '};
  const en = page('en', [], {en: table}), zh = page('zh-CN', [], {en: table});
  const tb = text => vm.runInContext('tb', en.context)(text), said = (...a) => vm.runInContext('said', en.context)(...a);
  assert.equal(tb('其他'), 'Other');
  assert.equal(tb('会话已过期，请刷新页面'), 'Session expired, reload the page');
  assert.equal(tb('同步失败：文件夹不存在或无法访问：/合成/甲'), 'Sync failed: Folder missing or unreadable: /合成/甲');
  assert.equal(tb('已将 1 张移出 雪'), 'Removed 1 picture from 雪');
  assert.equal(tb('已将 3 张移出 雪'), 'Removed 3 pictures from 雪');
  assert.equal(tb('已将 3 张移出 雪（跳过 1 张）'), 'Removed 3 from 雪 (skipped 1)'.replace('skipped 1', '1 skipped'));
  // 自身没有中文措辞的模板从不匹配；没有模板对得上的文本保持原样。
  assert.equal(tb('甲：乙'), '甲：乙');
  // {n} 只代表数字。
  assert.equal(tb('已将 几 张移出 雪'), '已将 几 张移出 雪');
  assert.equal(tb('没有模板的话'), '没有模板的话');
  assert.equal(tb('Plain English'), 'Plain English');
  assert.equal(tb(null), null);
  assert.equal(vm.runInContext('tb', zh.context)('已将 3 张移出 雪'), '已将 3 张移出 雪');
  // 不论页面是什么语言，said() 都按发来的原样读取后端文本。
  assert.ok(said('会话已过期，请刷新页面', '会话已过期，请刷新页面'));
  assert.ok(said('文件夹不存在或无法访问：/合成/甲', '文件夹不存在或无法访问：{path}'));
  assert.ok(said('同步失败：文件夹不存在或无法访问：/合成/甲', '文件夹不存在或无法访问：{path}'));
  assert.ok(!said('文件夹不存在或无法访问：/合成/甲', '同步失败：{reason}'));
  assert.ok(!said('不是：会话已过期，请刷新页面了', '会话已过期，请刷新页面'));
  // 用 ； 连接的多条消息，逐条翻译。
  assert.equal(tb('其他；已将 1 张移出 雪；没有模板'), 'Other; Removed 1 picture from 雪; 没有模板');
  // 自身带 ； 的消息保持完整。
  const semi = page('en', [], {en: {...table, '甲；乙 {n} 张': 'A; B {n}', '丙 {n} 处': 'C {n}'}}).context;
  assert.equal(vm.runInContext('tb', semi)('丙 2 处；甲；乙 3 张'), 'C 2; A; B 3');
  assert.ok(!said(undefined, '其他'));
});

// #198：后端的文本里也有用户的数据。只有装着后端自己的嵌套消息的占位符才再翻译一次；
// 后端模板的每个占位符都要说明自己是哪一种。
test('approved library errors translate while OS details and paths remain data', () => {
  const {context} = page('en', [], {en: english});
  const tb = text => vm.runInContext('tb', context)(text);
  for (const text of ['图库名称必须是字符串', '整数超出数据库支持的范围', '无法创建图库：名称不能包含空字符（NUL）']) {
    assert.equal(tb(text), english[text]);
  }
  for (const code of [true, false]) for (const destination of [true, false]) {
    const template = `文件操作失败${code ? '（错误号 {code}）' : ''}：{source}（路径：{path}${destination ? ' → {other}' : ''}）`;
    const values = {code: '13', source: '未识别', path: "'合成/未识别'", other: "'合成/目标'"};
    const fill = text => text.replace(/\{(\w+)\}/g, (_, key) => values[key]);
    assert.equal(tb(fill(template)), fill(english[template]));
    assert.equal(tb('读取失败：' + fill(template)), 'Could not load: ' + fill(english[template]));
  }
});

test('artist clues without a count reuse the translated fallback', () => {
  const {context} = page('en', [], {en: english});
  const tb = text => vm.runInContext('tb', context)(text);
  assert.equal(tb('有线索指向 TA'), 'Clues point to them');
  assert.equal(tb('同一原创角色里有 1 张是 TA 画的'), '1 picture of the same original character is by them');
  assert.equal(tb('同一原创角色里有 2 张是 TA 画的'), '2 pictures of the same original character are by them');
});

test('each placeholder of the backend templates is either a nested message or data', () => {
  const {context} = page('en');
  const nested = vm.runInContext('BACKEND_NESTED', context), data = vm.runInContext('BACKEND_DATA', context);
  for (const name of nested) assert.ok(!data.has(name), `${name} is in both lists`);
  const templates = new Set(shownBackend());
  for (const table of Object.values(locales)) for (const key of Object.keys(table)) if (backendKeys.has(key)) templates.add(key);
  const seen = new Set();
  for (const template of templates) {
    // 与 tb() 匹配的模板相同：自身带中文措辞的模板。
    if (!/\p{Script=Han}/u.test(template.replace(/\{\w+\}/g, ''))) continue;
    for (const name of placeholders(template)) {
      seen.add(name);
      assert.ok(nested.has(name) || data.has(name), `{${name}} of "${template}": add it to BACKEND_NESTED or BACKEND_DATA in i18n.js`);
    }
  }
  for (const name of [...nested, ...data]) assert.ok(seen.has(name), `no backend template has {${name}} any more: drop it from i18n.js`);
});

test("the user's names in a backend message stay as they are", () => {
  const en = page('en', [], {en: english}), zh = page('zh-CN', [], {en: english});
  const tb = text => vm.runInContext('tb', en.context)(text), said = (...a) => vm.runInContext('said', en.context)(...a);
  // 名字和表键相同的标签。
  assert.equal(english['未识别'], 'Unidentified');
  assert.equal(tb('已给 3 张加上标签 未识别'), 'Added tag 未识别 to 3 pictures');
  assert.equal(tb('已从 1 张去掉标签 未识别'), english['已从 {n} 张去掉标签 {name}'].one.replace('{n}', '1').replace('{name}', '未识别'));
  // 名字读起来像模板、或含有模板措辞的角色。
  assert.equal(tb('已将 2 张移出 已将 3 张移出 雪'), 'Removed 2 pictures from 已将 3 张移出 雪');
  assert.equal(tb('已将 2 张设为 已将张'), english['已将 {n} 张设为 {name}'].other.replace('{n}', '2').replace('{name}', '已将张'));
  assert.equal(tb('已将 1 张移出 未识别'), 'Removed 1 picture from 未识别');
  // 嵌套消息照样翻译，里面的数据保持原样。
  assert.equal(tb('任务超时：10 分钟没有进展'), english['任务超时：{duration}没有进展'].replace('{duration}', english['{n} 分钟'].other.replace('{n}', '10')));
  assert.equal(tb('冲突：文件夹不存在或无法访问：/合成/未识别'),
    english['冲突：{m}'].replace('{m}', english['文件夹不存在或无法访问：{path}'].replace('{path}', '/合成/未识别')));
  // said() 读取后端的前缀，不会把数据里的 ： 当成前缀。
  assert.ok(said('冲突：会话已过期，请刷新页面', '会话已过期，请刷新页面'));
  assert.ok(said('同步失败：不合法：文件夹不存在或无法访问：/合成/甲', '文件夹不存在或无法访问：{path}'));
  assert.ok(!said('已给 1 张加上 甲：会话已过期，请刷新页面', '会话已过期，请刷新页面'));
  assert.ok(!said('已将 1 张移出 甲：文件夹不存在或无法访问：乙', '文件夹不存在或无法访问：{path}'));
  assert.ok(!said('没有标签「未识别：文件夹不存在或无法访问：乙」', '文件夹不存在或无法访问：{path}'));
  // 中文页面按原样显示后端文本，said() 也同样读取。
  const zhTb = vm.runInContext('tb', zh.context), zhSaid = vm.runInContext('said', zh.context);
  for (const text of ['已给 3 张加上标签 未识别', '已将 2 张移出 已将 3 张移出 雪', '冲突：文件夹不存在或无法访问：/合成/未识别']) assert.equal(zhTb(text), text);
  assert.ok(zhSaid('冲突：会话已过期，请刷新页面', '会话已过期，请刷新页面'));
  assert.ok(!zhSaid('已给 1 张加上 甲：会话已过期，请刷新页面', '会话已过期，请刷新页面'));
});

test('marked page text is translated only on a page in another language', () => {
  const table = {'设置': 'Settings', '任务': 'Tasks', '视图选项': 'View options'};
  const en = page('en', [], {en: table});
  assert.deepEqual(en.elements.text.map(e => e.textContent), ['Settings', '没有译文的字']);
  assert.deepEqual(en.elements.attrs[0].attributes, {title: 'Tasks', 'aria-label': 'View options'});
  const zh = page('zh-CN', [], {en: table});
  assert.deepEqual(zh.elements.text.map(e => e.textContent), ['设置', '没有译文的字']);
  assert.deepEqual(zh.elements.attrs[0].attributes, {title: '任务', 'aria-label': '视图选项'});
});

