// 图片列表的取数（#706）：页面样板里 ui/ 不直接发请求（tools/check_frontend.py 规则 22），要读图片列表的地方经这里。
// 现在只有相似组页「判断关系」要的前几张图的 id；网格自己的分页读取还在 ui/grid.js（存量，C2 #708 改）。
import {api} from './session.js';

// 符合这组查询参数（/api/assets 的筛选和 pool）的前 limit 张图的 id。
export const listIds = (params, limit) => api('/api/ids?' + new URLSearchParams({...params, limit}));
