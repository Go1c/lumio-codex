/**
 * 构建期预渲染在 Node 里跑 `renderToString`，没有 window / document / navigator。
 *
 * 需要按环境分叉的组件统一问这里：预渲染要产出**爬虫可读的静态正文**，所以
 * 服务端分支一律选「内容已经在 HTML 里」的那一支——不要 loading 文案、不要
 * 初始 opacity:0、不要依赖设备探测的结果。
 */
export function isServerRender(): boolean {
  return typeof window === "undefined";
}
