// 页面上的数据库检查（#212）：检查发现损坏时的提示（设置里「数据库检查」那一行在 ui/settings_data.js），损坏在 source.sqlite
// 时带「抢救重建」（#228）。检查本身在 core/integrity.js。
import {t, tb} from '../core/i18n.js';
import {esc} from '../core/format.js';
import {state} from '../core/state.js';
import {onIntegrity, rescueNow} from '../core/integrity.js';
import {loadLibraries} from '../core/settings.js';
import {$, toast} from './dom.js';
import {openLibrary} from './settings_libraries.js';
// source.sqlite 损坏：抢救的按钮，然后是它的进度，或它没有完成的原因（可以再次开始）。
function rescueLine(rescue) {
  if (rescue?.running) return `<p class="damage-line">${esc(t('正在抢救：{stage}，{percent}%', {stage: tb(rescue.running.stage), percent: Math.floor(rescue.running.done * 100)}))}</p>`;
  if (rescue?.ready) return `<p class="damage-line">${esc(t('抢救完成，正在重新打开图库…'))}</p>`;
  const failed = rescue?.error ? `<p class="damage-line">${esc(t('抢救没有完成：{error}', {error: tb(rescue.error)}))}</p>` : '';
  return `${failed}<p class="damage-line"><button class="small" id="rescueNow">${t('抢救重建')}</button></p>`;
}
function showDamage() {
  const g = state.integrity, damaged = g?.damaged;
  const rescuable = damaged?.database === 'source.sqlite';
  $('damageBanner').innerHTML = damaged ? esc(tb(damaged.message)) + (rescuable ? rescueLine(g.rescue) : '') : '';
  $('damageBanner').hidden = !damaged;
  if ($('rescueNow')) $('rescueNow').onclick = async () => {
    $('rescueNow').disabled = true;
    try { await rescueNow(); } catch (e) { toast(e.message); $('rescueNow') && ($('rescueNow').disabled = false); }
  };
}
// 新的 source.sqlite 在图库重新打开时换上；重新打开的页面会说明抢救做了什么。
let reopening = false;
async function reopenRescued() {
  if (reopening || !state.integrity?.rescue?.ready) return;
  reopening = true;
  await loadLibraries();
  const lib = state.libraries;
  if (lib?.current) await openLibrary(lib.current, lib.info?.name || '');
  reopening = false;
}
export function wireIntegrity() {
  onIntegrity(() => { showDamage(); reopenRescued(); });
}
