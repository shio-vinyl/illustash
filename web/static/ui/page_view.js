// 页面样板的两段公共写法（docs/WEB_V2.md「页面样板」第 4、5 条）：这一页的部件只建一次；异步回来先判断还该不该画。
//
//   const page = pageView(scope => notesView(scope));   // 模块顶层，一页一个；build 收到这一页的作用域，返回带 scope 的部件
//   implement({notes: {
//     render: o => page.enter(o).load(state.route),     // 刚进这一页（samePage 为假）或上一份已经释放才建，否则沿用
//     refresh: () => page.get()?.load(state.route),     // 还活着的那一份；离开这一页以后是 null
//   }});
//
//   const begin = latest(scope);                         // 部件里，一条读取线一个（同一页上的几次重读只认最后一次）
//   async function load(route) {
//     const fresh = begin(route, () => nsfw === state.nsfw);   // 每次读之前取一个；第二个参数可省
//     try { … = await read(…); } catch (e) { if (fresh()) error(readError(e)); return; }
//     if (!fresh()) return;                              // 作用域释放了、地址换了、后面又发了一次，或者附加条件不成立
//   }
import {state} from '../core/state.js';

/**
 * 页面部件的单例。build(scope) 返回的部件带 scope 时按它判断死活（角色页的横幅建在子作用域上），否则按传给 build 的作用域。
 * enter({samePage, scope}) 返回要用的那一份；get() 返回还活着的那一份或 null。
 */
export function pageView(build) {
  let view = null, owner = null;
  const alive = () => !!view && (view.scope || owner).alive;
  return {
    enter: ({samePage, scope}) => {
      if (!samePage || !alive()) { owner = scope; view = build(scope); }
      return view;
    },
    get: () => alive() ? view : null,
  };
}

/**
 * 一条读取线的序号。latest(scope) 返回 begin(route, also?)：每调一次发一个新序号，返回的 fresh() 在作用域还活着、
 * state.route 还是这个路由对象、这是最后发出的那次、also() 为真（没给就不看）时为真。route 不给时取现在的 state.route。
 */
export function latest(scope) {
  let last = 0;
  return (route = state.route, also = null) => {
    const seq = ++last;
    return () => scope.alive && state.route === route && seq === last && (!also || !!also());
  };
}
