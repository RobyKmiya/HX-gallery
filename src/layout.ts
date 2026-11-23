import { useCallback, useState } from "react";

/**
 * 记住布局尺寸：拖动分界线改的值存进 localStorage，下次开窗口原样恢复。
 *
 * 存进去之前先按 [min, max] 夹一次——库/版本换了、或者有人手改了 localStorage，
 * 都不至于把侧栏拖成 0 宽或者比屏幕还宽。
 */
export function usePersistentSize(
  key: string,
  initial: number,
  min: number,
  max: number,
): [number, (next: number) => void, () => void] {
  const clamp = useCallback(
    (value: number) => Math.min(max, Math.max(min, Math.round(value))),
    [max, min],
  );

  const [value, setValue] = useState(() => {
    try {
      const raw = localStorage.getItem(key);
      const parsed = raw === null ? Number.NaN : Number(raw);
      return Number.isFinite(parsed) ? clamp(parsed) : initial;
    } catch {
      return initial;
    }
  });

  const set = useCallback(
    (next: number) => {
      const clamped = clamp(next);
      setValue(clamped);
      try {
        localStorage.setItem(key, String(clamped));
      } catch {
        // 存不了就只在本次生效
      }
    },
    [clamp, key],
  );

  const reset = useCallback(() => set(initial), [set, initial]);

  return [value, set, reset];
}

/** 记住一个开关（例如阅读器的沉浸模式）。 */
export function usePersistentFlag(
  key: string,
  initial: boolean,
): [boolean, (next: boolean) => void] {
  const [value, setValue] = useState(() => {
    try {
      const raw = localStorage.getItem(key);
      return raw === null ? initial : raw === "1";
    } catch {
      return initial;
    }
  });

  const set = useCallback(
    (next: boolean) => {
      setValue(next);
      try {
        localStorage.setItem(key, next ? "1" : "0");
      } catch {
        // 同上
      }
    },
    [key],
  );

  return [value, set];
}
