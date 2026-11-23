/**
 * 阅读器鼠标手势与点击分区（管理员 11-23 需求）。
 *
 * - 六个手势槽位：单击左区 / 单击中区（目录域）/ 单击右区 / 双击画面 /
 *   Ctrl+滚轮上 / Ctrl+滚轮下，各自绑定一个动作，可在设置里改。
 * - 分区边界：左右两条竖线可拖动，位置按比例存本机；单击按区域触发。
 * - 缩放只走 Ctrl+滚轮（鼠标侧），不带 Ctrl 的滚轮不再改缩放。
 */
import type { ShortcutAction } from "./shortcuts";

export type GestureSlot =
  | "clickLeft"
  | "clickCenter"
  | "clickRight"
  | "doubleClick"
  | "ctrlWheelUp"
  | "ctrlWheelDown";

export type GestureAction = ShortcutAction | "none";

export const GESTURE_LABELS: Record<GestureSlot, string> = {
  clickLeft: "单击左区",
  clickCenter: "单击中区（目录域）",
  clickRight: "单击右区",
  doubleClick: "双击画面",
  ctrlWheelUp: "Ctrl+滚轮上滚",
  ctrlWheelDown: "Ctrl+滚轮下滚",
};

export const DEFAULT_GESTURES: Record<GestureSlot, GestureAction> = {
  clickLeft: "prevPage",
  clickCenter: "toggleThumbs",
  clickRight: "nextPage",
  doubleClick: "toggleImmersive",
  ctrlWheelUp: "zoomIn",
  ctrlWheelDown: "zoomOut",
};

const STORAGE_KEY = "huaxia.gestures";
export const GESTURES_CHANGED_EVENT = "huaxia:gestures-changed";

export function loadGestures(): Record<GestureSlot, GestureAction> {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    const merged = { ...DEFAULT_GESTURES };
    if (!raw) return merged;
    const parsed = JSON.parse(raw) as Partial<Record<GestureSlot, GestureAction>>;
    for (const slot of Object.keys(DEFAULT_GESTURES) as GestureSlot[]) {
      if (typeof parsed[slot] === "string") merged[slot] = parsed[slot];
    }
    return merged;
  } catch {
    return { ...DEFAULT_GESTURES };
  }
}

export function saveGestures(map: Record<GestureSlot, GestureAction>) {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(map));
  } catch {
    // 存不了就只在本次会话生效
  }
  window.dispatchEvent(new CustomEvent(GESTURES_CHANGED_EVENT));
}

/** 点击分区边界（0~1 的横向比例）。 */
export interface ZoneFractions {
  left: number;
  right: number;
}

export const DEFAULT_ZONES: ZoneFractions = { left: 0.28, right: 0.72 };
const ZONES_KEY = "huaxia.readerZones";
export const ZONES_CHANGED_EVENT = "huaxia:zones-changed";

export function loadZones(): ZoneFractions {
  try {
    const raw = window.localStorage.getItem(ZONES_KEY);
    if (!raw) return { ...DEFAULT_ZONES };
    const parsed = JSON.parse(raw) as Partial<ZoneFractions>;
    const left = typeof parsed.left === "number" ? parsed.left : DEFAULT_ZONES.left;
    const right = typeof parsed.right === "number" ? parsed.right : DEFAULT_ZONES.right;
    // 夹到 [0.06, 0.94] 且保证中区至少 8%，避免拖到贴边后点不着。
    const clampedLeft = Math.min(0.86, Math.max(0.06, left));
    const clampedRight = Math.max(clampedLeft + 0.08, Math.min(0.94, right));
    return { left: clampedLeft, right: clampedRight };
  } catch {
    return { ...DEFAULT_ZONES };
  }
}

export function saveZones(zones: ZoneFractions) {
  try {
    window.localStorage.setItem(ZONES_KEY, JSON.stringify(zones));
  } catch {
    // 同上
  }
  window.dispatchEvent(new CustomEvent(ZONES_CHANGED_EVENT));
}
