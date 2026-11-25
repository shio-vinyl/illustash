// 身份整理和可能认错共用的整对处理。两个答案都是普通按钮，初始焦点在关闭按钮。
import {t} from '../core/i18n.js';
import {openDialog} from './dialog.js';
import {text} from './el.js';

const BODY = '<label class="field"><span></span><select class="work-choice" name="parent"><option value="a"></option><option value="b"></option></select></label>'
  + '<p class="muted-note dialog-message"></p>';
const ACTIONS = '<button value="together" class="btn"></button><button value="apart" class="btn"></button>';

export function chooseIdentityPair(scope, pair) {
  const duplicate = pair.catalog_kind === 'duplicate_of';
  return new Promise(resolve => {
    const d = openDialog({
      title: t('整对处理'), body: BODY, actions: ACTIONS, focus: '[data-close]',
      onClose: d => {
        scope.remove(abandon);
        const answer = d.returnValue === 'together' ? (duplicate ? 'duplicate' : 'form') : d.returnValue;
        resolve(scope.alive && answer ? {answer, parent: d.querySelector('select').value} : null);
      },
    });
    const abandon = () => { if (d.open) d.close(''); };
    scope.add(abandon);
    text(d.querySelector('label span'), duplicate ? t('合成后的角色') : t('合成时的本体'));
    text(d.querySelector('option[value="a"]'), pair.a.name);
    text(d.querySelector('option[value="b"]'), pair.b.name);
    const select = d.querySelector('select'), note = d.querySelector('.dialog-message');
    select.value = pair.b.count > pair.a.count ? 'b' : 'a';
    const update = () => {
      const [base, child] = select.value === 'a' ? [pair.a, pair.b] : [pair.b, pair.a];
      text(note, duplicate ? t('合并后保留 {name}。', {name: base.name}) : t('合并后，{child} 是 {name} 的一个造型。', {child: child.name, name: base.name}));
    };
    select.addEventListener('change', update);
    update();
    text(d.querySelector('[value="together"]'), t('合成一个角色'));
    text(d.querySelector('[value="apart"]'), t('分成两个，不再问'));
  });
}
