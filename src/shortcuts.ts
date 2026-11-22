/**
 * 阅读器快捷键绑定（管理员 11-22：快捷键要能自定义绑定）。
 *
 * 规则：
 * - 只有「动作键」可改；Esc / Space / Ctrl+数字 / Ctrl+H / [ ] 这些是结构键，固定。
 * - 绑定值形如 "F"、"ArrowRight"、"Ctrl+C"、"Ctrl+Enter"、"Shift+PageUp"。
 * - 存在 localStorage，阅读器与设置面板通过 custom event 同步（同标签页 storage 事件不触发）。
 */

export type ShortcutAction =
  | "nextPage"
  | "prevPage"
  | "firstPage"
  | "lastPage"
  | "zoomIn"
  | "zoomOut"
  | "fitPage"
  | "fitWidth"
  | "toggleSpread"
  | "toggleImmersive"
  | "togglePlayback"
  | "toggleBookmark"
  | "openBookmarks"
  | "openDetail"
  | "copyPage"
  | "revealArchive"
  | "toggleThumbs"
  | "prevArchive"
  | "nextArchive"
  | "favoritePage"
  | "favoriteLeft"
  | "favoriteRight";

export const SHORTCUT_LABELS: Record<ShortcutAction, string> = {
  nextPage: "下一页",
  prevPage: "上一页",
  firstPage: "第一页",
  lastPage: "最后一页",
  zoomIn: "放大",
  zoomOut: "缩小",
  fitPage: "适应页面",
  fitWidth: "适应宽度",
  toggleSpread: "双页开合",
  toggleImmersive: "沉浸模式",
  togglePlayback: "GIF 播放",
  toggleBookmark: "添加/移除书签",
  openBookmarks: "书签列表",
  openDetail: "详情浮层",
  copyPage: "复制当前页",
  revealArchive: "资源管理器定位",
  toggleThumbs: "缩略图栏",
  prevArchive: "上一本画集",
  nextArchive: "下一本画集",
  favoritePage: "收藏当前页到散图夹",
  favoriteLeft: "收藏左页到散图夹",
  favoriteRight: "收藏右页到散图夹",
};

export const DEFAULT_SHORTCUTS: Record<ShortcutAction, string> = {
  nextPage: "ArrowRight",
  prevPage: "ArrowLeft",
  firstPage: "Home",
  lastPage: "End",
  zoomIn: "+",
  zoomOut: "-",
  fitPage: "0",
  fitWidth: "",
  toggleSpread: "D",
  toggleImmersive: "F",
  togglePlayback: "Shift+P",
  toggleBookmark: "B",
  openBookmarks: "L",
  openDetail: "I",
  copyPage: "Ctrl+C",
  revealArchive: "Ctrl+Enter",
  toggleThumbs: "T",
  prevArchive: "Alt+ArrowLeft",
  nextArchive: "Alt+ArrowRight",
  favoritePage: "P",
  favoriteLeft: "[",
  favoriteRight: "]",
};

const STORAGE_KEY = "huaxia.shortcuts";
export const SHORTCUTS_CHANGED_EVENT = "huaxia:shortcuts-changed";

export function loadShortcuts(): Record<ShortcutAction, string> {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_SHORTCUTS };
    const parsed = JSON.parse(raw) as Partial<Record<ShortcutAction, string>>;
    const merged = { ...DEFAULT_SHORTCUTS };
    for (const action of Object.keys(DEFAULT_SHORTCUTS) as ShortcutAction[]) {
      if (typeof parsed[action] === "string") merged[action] = parsed[action];
    }
    return merged;
  } catch {
    return { ...DEFAULT_SHORTCUTS };
  }
}

export function saveShortcuts(map: Record<ShortcutAction, string>) {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(map));
  } catch {
    // 存不了就只在本次会话生效
  }
  window.dispatchEvent(new CustomEvent(SHORTCUTS_CHANGED_EVENT));
}

/** 把键盘事件归一成绑定串；纯修饰键按下返回 null（录制时忽略）。 */
export function eventToBinding(event: KeyboardEvent): string | null {
  const key = event.key;
  if (key === "Control" || key === "Shift" || key === "Alt" || key === "Meta") {
    return null;
  }
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  let main = key;
  if (main === " ") main = "Space";
  if (main === "Escape") main = "Esc";
  if (main.length === 1) main = main.toUpperCase();
  parts.push(main);
  return parts.join("+");
}

/** 事件是否命中某个动作（返回动作名，未命中或为空绑定时返回 null）。 */
export function matchShortcut(
  event: KeyboardEvent,
  map: Record<ShortcutAction, string>,
): ShortcutAction | null {
  const binding = eventToBinding(event);
  if (!binding) return null;
  for (const action of Object.keys(map) as ShortcutAction[]) {
    if (map[action] && map[action] === binding) return action;
  }
  return null;
}

/** 绑定串展示成人类可读的样子。 */
export function describeBinding(binding: string): string {
  if (!binding) return "未绑定";
  return binding
    .replace("ArrowRight", "→")
    .replace("ArrowLeft", "←")
    .replace("ArrowUp", "↑")
    .replace("ArrowDown", "↓")
    .replace("PageDown", "PgDn")
    .replace("PageUp", "PgUp")
    .replace("Ctrl+", "Ctrl + ");
}
