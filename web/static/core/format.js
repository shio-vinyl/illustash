// 各布局共用的格式化（#199）：转义、数字、日期、时间、大小、路径和快捷键名称。不碰 DOM；页面语言取自 I18N（web/static/i18n.js）。
// 同一种东西只在这里写一份（#431 ③-8），不在各模块另写。
import {I18N, t} from './i18n.js';
export const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
// 让静态帮助和动态消息里显示的修饰键保持一致。
// 不分大小写：Chromium 的 userAgentData.platform 在 macOS 上是 "macOS"（#389）。
export const macLike = platform => /mac|iphone|ipad|ipod/i.test(platform || '');
export const isMac = macLike(globalThis.navigator?.userAgentData?.platform || globalThis.navigator?.platform);
export const shortcut = (key, mac = isMac) => mac ? `⌘${key}` : `Ctrl+${key}`;
// 用 ⌘ 写的帮助文字：其他系统上换成 Ctrl（单独的 ⌘ 换成 Ctrl，⌘K 换成 Ctrl+K）。
export const platformText = (text, mac = isMac) => mac ? text : text.replace(/⌘(?=\s|$)/g, 'Ctrl').replaceAll('⌘', 'Ctrl+');
export const number = n => Number(n || 0).toLocaleString(I18N.locale);
// ---------- 日期的键：本机日期的「YYYY-MM-DD」和「YYYY-MM」 ----------
const pad2 = n => String(n).padStart(2, '0');
// 「YYYY-MM-DD」（或「YYYY-MM」，取 1 日）读成本机那一天的零点：按年月日构造，不经字符串解析，不受时区和夏令时影响。
export const parseDay = key => { const [y, m, d] = String(key).split('-').map(Number); return new Date(y, (m || 1) - 1, d || 1); };
export const dayKey = (date = new Date()) => `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;
export const monthKey = (date = new Date()) => dayKey(date).slice(0, 7);
// 按界面语言的 Intl 日期格式，按语言和选项缓存。
const formats = new Map();
const intl = opts => {
  const key = I18N.locale + JSON.stringify(opts);
  if (!formats.has(key)) formats.set(key, new Intl.DateTimeFormat(I18N.locale, opts));
  return formats.get(key);
};
// 按日期键写的几种日期（键见上面）：星期几的短名；完整的年月日（日历格子的读屏标签）；月日和星期（活页条时间轴的悬停卡片）；
// 年和月（时间轴里的一个月）；月的短名（时间轴上那一项的字）；日历的月份标题（中文是「11 月」，别的语言是月份全名）。
export const weekdayShort = key => intl({weekday: 'short'}).format(parseDay(key));
export const dayLong = key => intl({year: 'numeric', month: 'long', day: 'numeric'}).format(parseDay(key));
export const dayWithWeekday = key => intl({month: 'long', day: 'numeric', weekday: 'short'}).format(parseDay(key));
export const monthLong = key => intl({year: 'numeric', month: 'long'}).format(parseDay(key));
export const monthShort = key => intl({month: 'short'}).format(parseDay(key));
export const monthName = key => I18N.language === 'zh-CN' ? t('{m} 月', {m: Number(String(key).slice(5, 7))}) : intl({month: 'long'}).format(parseDay(key));
// 日期（#182）：中文保留自己的空格（「5 月 3 日」）；其他语言照 Intl。
export const dateText = (y, m, d, withYear) => I18N.language === 'zh-CN' ? (withYear ? `${y} 年 ` : '') + `${m} 月 ${d} 日`
  : new Intl.DateTimeFormat(I18N.locale, withYear ? {year: 'numeric', month: 'numeric', day: 'numeric'} : {month: 'numeric', day: 'numeric'}).format(new Date(+y, m - 1, +d));
// 「YYYY-MM-DD」的加入日期：今天、昨天，今年的只写月日，往年的带上年份。
export const dayLabel = day => { const date = parseDay(day), y = date.getFullYear(), m = date.getMonth() + 1, d = date.getDate(), now = new Date();
  const diff = Math.round((new Date(now.getFullYear(), now.getMonth(), now.getDate()) - date) / 864e5);
  return diff === 0 ? t('今天') : diff === 1 ? t('昨天') : dateText(y, m, d, y !== now.getFullYear()); };
export const monthText = (y, m) => I18N.language === 'zh-CN' ? `${y} 年 ${m} 月` : new Intl.DateTimeFormat(I18N.locale, {year: 'numeric', month: 'short'}).format(new Date(+y, m - 1, 1));
// 「2026-03」这样的月份键写成本地的年月（角色页的收集曲线、详情卡片的每月柱子）。
export const monthLabel = key => monthText(Number(String(key).slice(0, 4)), Number(String(key).slice(5, 7)));
export const work = w => (w || '').replaceAll('_', ' ');
// 太长的名字截短（卡片上的芯片、页头的角色筛选、选择栏的按钮）。
export const short = name => name.length > 28 ? name.slice(0, 26) + '…' : name;
// 缩略图档位（#452）：原生端 thumb 长边 400、medium 长边 800。cssPx 是调用处估的显示尺寸（CSS 像素），乘屏幕倍率
// 换成物理像素，超过 260 用 medium。倍率按 1 封底，1 倍屏的结果和只看 CSS 像素时一样。倍率每次画的时候读：窗口
// 拖到倍率不同的屏幕后，下次重画换档。
export const screenScale = (dpr = globalThis.devicePixelRatio) => Math.max(1, Number(dpr) || 1);
export const thumbSize = (cssPx, dpr) => cssPx * screenScale(dpr) > 260 ? 'medium' : 'thumb';
/** 一张图的地址：size 是 thumb、medium、preview 或 full。 */
export const imageUrl = (uuid, size = 'thumb') => `/image?id=${encodeURIComponent(uuid)}&size=${size}`;
// 封面、头像这类可能没有的图：有 uuid 给地址，没有给 null（交给 ui/el.js 的 attr 去掉 src）。
export const coverUrl = (uuid, size = 'thumb') => uuid ? imageUrl(uuid, size) : null;
// 大小按十进制（1 MB = 10⁶ 字节，和 macOS 的访达一致）。bytes 带单位；megabytes 只给数字，接在写好「MB」的文案里，
// 10 MB 以下留一位小数。
export const bytes = n => n >= 1e9 ? (n / 1e9).toFixed(1) + ' GB' : n >= 1e6 ? (n / 1e6).toFixed(1) + ' MB' : Math.round(n / 1e3) + ' KB';
export const megabytes = n => (n / 1e6).toFixed(n < 1e7 ? 1 : 0);
// 时间一律显示成本地时间「MM-DD hh:mm」，按后端给的形式分三种入口：
//   stamp       后端已经按本地时间写好的「YYYY-MM-DD hh:mm…」（同步记录、索引检查），只截取
//   localStamp  Unix 秒数
//   localTime   SQLite 的 UTC 时间戳「YYYY-MM-DD hh:mm:ss」（provenance.sqlite），读不出时为空
const clock = d => { const p = n => String(n).padStart(2, '0'); return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`; };
export const stamp = text => (text || '').replace('T', ' ').slice(5, 16);
export const localStamp = sec => clock(new Date(sec * 1000));
export const pct = (n, total) => total ? `${(n / total * 100).toFixed(1)}%` : '—';
// 后端的 UTC 时间戳（SQLite 的「YYYY-MM-DD hh:mm:ss」，或带时区的 ISO 8601）按本地时区的日期显示；只有日期的
// （人工结论快照导入的）照原样显示；读不出时返回空。
export const localDate = utc => {
  const s = String(utc || ''), day = s.match(/^(\d{4})-(\d\d)-(\d\d)$/);
  if (day) return dateText(day[1], +day[2], +day[3]);
  const d = new Date(/(Z|[+-]\d\d:?\d\d)$/i.test(s) ? s : s.replace(' ', 'T') + 'Z');
  return isNaN(d) ? '' : dateText(d.getFullYear(), d.getMonth() + 1, d.getDate());
};
export const localTime = utc => { const d = new Date(String(utc || '').replace(' ', 'T') + 'Z'); return isNaN(d) ? '' : clock(d); };
// 家目录缩写成 ~：macOS 的 /Users/<名字>，Windows 的 C:\Users\<名字>（#518，截图反馈时不带出用户名）。本机路径都这样显示。
export const home = p => String(p || '').replace(/^(?:\/Users\/[^/]+|[A-Za-z]:[\\/]Users[\\/][^\\/]+)(?=[\\/]|$)/i, '~');
