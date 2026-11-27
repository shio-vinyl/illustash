// 身份整理和可能认错共用的整对处理。两个答案都是普通按钮，初始焦点在关闭按钮。
import {roleName} from '../core/labels.js';
import {t} from '../core/i18n.js';
import {openDialog} from './dialog.js';
import {text} from './el.js';

const BODY = '<p class="dialog-message"><span></span> <button type="button" class="btn is-small" data-swap></button></p>';
const ACTIONS = '<button value="together" class="btn"></button><button value="apart" class="btn"></button>';

export function chooseIdentityPair(scope, pair) {
  const duplicate = pair.catalog_kind === 'duplicate_of';
  let parent = pair.b.count > pair.a.count ? 'b' : 'a';
  return new Promise(resolve => {
    const d = openDialog({
      title: t('整对处理'), body: BODY, actions: ACTIONS, focus: '[data-close]',
      onClose: d => {
        scope.remove(abandon);
        const answer = d.returnValue === 'together' ? (duplicate ? 'duplicate' : 'form') : d.returnValue;
        resolve(scope.alive && answer ? {answer, parent} : null);
      },
    });
    const abandon = () => { if (d.open) d.close(''); };
    scope.add(abandon);
    const note = d.querySelector('.dialog-message span'), swap = d.querySelector('[data-swap]');
    const update = () => {
      const [base, child] = parent === 'a' ? [pair.a, pair.b] : [pair.b, pair.a];
      text(note, duplicate ? t('合并后保留 {name}。', {name: roleName(base)}) : t('合并后，{child} 是 {name} 的一个造型。', {child: roleName(child), name: roleName(base)}));
    };
    text(swap, duplicate ? t('交换保留项') : t('交换本体'));
    swap.addEventListener('click', () => { parent = parent === 'a' ? 'b' : 'a'; update(); });
    update();
    text(d.querySelector('[value="together"]'), t('合成一个角色'));
    text(d.querySelector('[value="apart"]'), t('分成两个，不再问'));
  });
}
