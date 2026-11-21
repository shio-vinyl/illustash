// 角色资料包（#602；原来分开的 #550 WD 角色扩展和 #559 CCIP 长尾包合成一个包）：包的下载、导入和删除，本图库的启用、
// 换包、撤回，以及签名目录里的新版本。不碰 DOM。状态是 GET /api/packs 的回复（形状见 native/terms/src/http_host_get.rs
// 的 `packs`），存在 state.pack，只由这里写入；下载、导入或启用、撤回进行期间，以及等着默认启用时每秒读一次，读取失败时退避
// 着接着读。onPack(fn) 在每次读到或改动后调用 fn(before)，before 是改动前的状态。两个组成部分各自的旧路由
// （/api/wd-extension、/api/ccip-longtail）页面不再读。
import {session, api} from './session.js';
import {state} from './state.js';
import {listeners} from './events.js';
import {poller} from './poll.js';
const changed = listeners();
export const onPack = changed.on;

export const PACK_ROUTE = '/api/packs';
export const packDownloading = x => x?.download?.status === 'running';
export const packRunning = x => x?.task?.status === 'running';
// 等着默认启用（#630）：包已就位，宿主等后台任务结束就会开始启用，页面接着读，直到看到启用任务。上次的启用失败或取消了时
// 宿主这次打开不再自动重试，也就不用读。
export const packAwaitingDefault = x => x?.library?.prompt === 'pending' && x?.current?.status === 'ready'
  && (!x.task || x.task.status === 'running');
// 下载、导入、启用或撤回进行中，或者等着默认启用：每秒读一次。
const watched = x => packDownloading(x) || packRunning(x) || packAwaitingDefault(x);
// 自己排下一次的读取（core/poll.js，#648）。读取失败时只要状态还要看着就照样排下一次，1、2、4、8 秒退避，最多 10 秒：
// 宿主短暂忙或者一次网络抖动，进度不会停在那里等用户切页。换了会话就停。
let failures = 0;
const tick = poller(async fresh => {
  const token = session.token;
  try { await loadPack(); } catch {
    if (!fresh() || token !== session.token) return 0;
    failures += 1;
    return watched(state.pack) ? Math.min(10000, 1000 * 2 ** (failures - 1)) : 0;
  }
  failures = 0;
  return fresh() && token === session.token && watched(state.pack) ? 1000 : 0;
}, {retry: 0});
// 几次读取会重叠（设置页渲染、任务面板、轮询）：只采用最后发出的那次；换了会话的回应丢掉（#34）。
let poll = 0;
async function adopt(value, mine, token) {
  if (mine !== poll || token !== session.token) return state.pack;
  const before = state.pack;
  state.pack = value;
  tick.later(watched(value) ? 1000 : 0);
  await changed.emit(before);
  return value;
}
export function loadPack() {
  const mine = ++poll, token = session.token;
  return api(PACK_ROUTE).then(value => adopt(value, mine, token));
}
// 没有副作用的 POST：不经 post()，免得启动一次任务就被当成图库写入。回复都是完整的状态；不带路径的导入先打开系统的选择框，
// 关掉时回复 {cancelled}。
async function send(body) {
  const mine = ++poll, token = session.token;
  const value = await api(PACK_ROUTE, {method: 'POST', body: JSON.stringify(body)});
  return value.cancelled ? value : adopt(value, mine, token);
}
// download（带 version 时下载目录里的那个版本）、import、cancel（下载或导入）、discard、delete。
export const packAction = (action, version) => send(version ? {action, version} : {action});
// 启用和重新检查是同一个动作；带 version 时换成那个版本。
export const enablePack = version => send(version ? {action: 'enable', version} : {action: 'enable'});
export const withdrawPack = () => send({action: 'withdraw'});
// 已有图库在默认启用的提示里选了不用。
export const declinePack = () => send({action: 'decline'});
// 只在扫描阶段有效；写入阶段后端回 409。
export const cancelPack = (id = state.pack?.task?.id) => send({action: 'cancel', id});
// 状态没变、页面要按它重画时通知一次（ui/packs.js 登记了这一页发起的导入以后）：before 就是现在的状态，不算结束了什么。
export const packChanged = () => changed.emit(state.pack);
// 卡片上「有新版本」的那个版本（没有时为 null）。
export const packOffer = (x = state.pack) => x?.offer || null;

// 刚结束的启用、重新检查、自动检查或撤回：同一个任务从运行变成别的状态时返回一次，否则为 null。
export function packFinished(before, after) {
  const was = before?.task, now = after?.task;
  return was?.status === 'running' && now && now.id === was.id && now.status !== 'running' ? now : null;
}
// 刚结束的下载或导入。
export const packDownloadFinished = (before, after) => packDownloading(before) && after?.download && !packDownloading(after) ? after.download : null;
