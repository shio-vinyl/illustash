// 「更多像这张的」的结果页（#449）：标题、例图的小缩略图、「按画面 / 按角色」，以及例图没有特征时怎么说。结果本身是
// 普通的图库网格（core/pages.js 的 like 视图），地址和后端参数在 core/like.js；入口按钮在 ui/detail.js（详情栏、
// 大图）和 ui/grid.js（选择栏）。#427 的「看过了」开关放在标题右侧的操作区（likeActions）。
import {esc, number, thumbSize} from '../core/format.js';
import {said, t} from '../core/i18n.js';
import {likeIds, withExamples} from '../core/like.js';
import {implement, viewOf} from '../core/pages.js';
import {go} from '../core/route.js';
import {api} from '../core/session.js';
import {state} from '../core/state.js';
import {$, helpTip, wireHelpTip} from './dom.js';
import {inspect} from './detail.js';

// 例图没有特征：后端回 400「这张图还没有特征，识别完成后再试」（native/terms/src/http_example_search.rs）。
const missingFeatures = e => said(e?.said, '这张图还没有特征，识别完成后再试');
// 标题里例图缩略图的显示高度和宽度（CSS 像素，app.css 的 .like-example img），按高度选缩略图档位。
const THUMB_H = 40, THUMB_W = 32;
// 标题旁最多显示几张例图。
const SHOWN = 5;

// 每张例图有没有这种特征：键是「比较方式|uuid」，值是 {has, epoch}，has 为 true、false，或还在问（一个 promise）。
// 有特征的一直算有；没有的在读取轮次（state.readEpoch，写入和同步之后变）变了以后再问，识别完成后就能问到。
const known = new Map();
// 只用一张例图问一次数量：例图没有特征时后端回 400。只在例图有两张以上时问；只有一张时网格的读取本身就说明了。
function probe(by, id) {
  const key = `${by}|${id}`, epoch = state.readEpoch, seen = known.get(key);
  if (seen && (seen.has !== false || seen.epoch === epoch)) return seen.has;
  const params = new URLSearchParams({like: JSON.stringify([id]), ...(by ? {like_by: by} : {})});
  const asking = api('/api/assets/count?' + params).then(() => true, e => !missingFeatures(e))
    .then(has => { known.set(key, {has, epoch}); return has; });
  known.set(key, {has: asking, epoch});
  return asking;
}
// 这几张里已知没有特征的；还没问完的先不算，问完再画一次标题。
function missingOf(r, ids) {
  if (ids.length < 2) return [];
  const answers = ids.map(id => probe(r.by, id));
  const waiting = answers.filter(a => a instanceof Promise);
  if (waiting.length) Promise.all(waiting).then(() => { if (state.route === r) drawHeading($('heading'), r); });
  return ids.filter((id, i) => answers[i] === false);
}

function drawHeading(h, r) {
  const ids = likeIds(r), missing = missingOf(r, ids), many = ids.length > 1;
  const n = state.totalReady ? number(state.total) : '…';
  const sub = [t('{n} 张', {n}), missing.length ? t('{n} 张例图没有特征', {n: number(missing.length)}) : ''].filter(Boolean).join(' · ');
  // 「?」里是这一页的说明，再接上图库的操作提示，和 ui/shell.js renderHeading 的 helpHint 一样一个「·」一行。
  const hints = (viewOf(r).hint?.(r) || '').split(' · ').filter(Boolean).map(x => esc(x.charAt(0).toLocaleUpperCase() + x.slice(1)));
  const [helpButton, helpPop] = helpTip([t('在符合筛选条件的图里，取和例图最像的 500 张。'), t('按画面比较画面标签，按角色比较角色特征。'), ...hints]);
  // 标题旁只放得下几张：没有特征的排在前面，其余写「+N」，悬停写总数。
  const shown = [...missing, ...ids.filter(id => !missing.includes(id))].slice(0, SHOWN), more = ids.length - shown.length;
  const examples = shown.map(id => {
    const off = missing.includes(id);
    return `<span class="like-example${off ? ' off' : ''}"><button data-like-show="${esc(id)}" title="${off ? t('还没有特征，没有算进去') : t('查看这张')}">`
      + `<img src="/image?id=${encodeURIComponent(id)}&size=${thumbSize(THUMB_H)}" alt="" loading="lazy"></button>`
      + (many ? `<button class="like-drop" data-like-drop="${esc(id)}" title="${t('去掉这张例图')}" aria-label="${t('去掉这张例图')}">×</button>` : '') + '</span>';
  }).join('') + (more ? `<span class="like-more" title="${t('共 {n} 张例图', {n: number(ids.length)})}">+${number(more)}</span>` : '');
  const modes = [['', t('按画面')], ['ccip', t('按角色')]].map(([v, label]) => {
    const on = (r.by || '') === v;
    return `<button data-like-by="${v}" class="${on ? 'on' : ''}" aria-pressed="${on}">${label}</button>`;
  }).join('');
  // 缩略图那一段的宽度（--w）给排版用：位置不够时先裁它，再截短标题，「按画面｜按角色」不缩（app.css 的 .like-examples）。
  const width = shown.length * (THUMB_W + 4) + (more ? 28 : 0);
  h.innerHTML = `<div class="heading-main like-head"><h1 class="title-page">${many ? t('更多像这些的') : t('更多像这张的')}${helpButton}</h1><p class="page-sub">${sub}</p>${helpPop}</div>`
    + `<div class="like-examples" style="--w:${width}px">${examples}</div>${likeActions(r)}<div class="segmented like-by" role="group" aria-label="${t('比较方式')}">${modes}</div>`;
  wireHelpTip();
  for (const b of h.querySelectorAll('[data-like-show]')) b.onclick = () => inspect(b.dataset.likeShow);
  for (const b of h.querySelectorAll('[data-like-drop]')) b.onclick = () => go(withExamples(r, ids.filter(id => id !== b.dataset.likeDrop)));
  for (const b of h.querySelectorAll('[data-like-by]')) b.onclick = () => { if ((r.by || '') !== b.dataset.likeBy) go({...r, by: b.dataset.likeBy}); };
}
// 标题右侧给以后的开关留的位置（#427 的「看过了」）；现在没有。
const likeActions = () => '';

implement({like: {
  drawHeading,
  // 例图全都没有特征：网格空着，说清楚为什么，不弹错误条。
  readFailed: e => missingFeatures(e) ? t('例图还没有特征，识别完成后再试。') : '',
}});
