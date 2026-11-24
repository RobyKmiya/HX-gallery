/**
 * 模态让位总线（P1 工作台）：原生子 webview 盖不住 React 模态，
 * 任何模态打开/关闭时广播一次，内建浏览器 host 收到后整体隐藏/恢复。
 * 单独成文件避免 SettingsDialog → WorkspaceTabs → LibraryView 的环形依赖。
 */
export const EVENT_MODAL = "huaxia:modal-visibility";

export function notifyModalOpen() {
  window.dispatchEvent(new CustomEvent(EVENT_MODAL, { detail: { open: true } }));
}

export function notifyModalClose() {
  window.dispatchEvent(new CustomEvent(EVENT_MODAL, { detail: { open: false } }));
}
