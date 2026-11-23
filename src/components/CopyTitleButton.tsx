import { useEffect, useRef, useState } from "react";
import { CheckIcon } from "./Icons";

interface CopyTitleButtonProps {
  text: string;
  /** 右键标题也能复制：这里只渲染按钮，右键在宿主元素上绑。 */
  className?: string;
}

/**
 * 标题旁的复制小按钮。
 *
 * 取舍（管理员 11-22 让考究「小按钮 vs 右键」）：
 * - 纯右键：零视觉噪音，但**不可发现**——没人会去右键标题试试；
 * - 小按钮：占 6px 视觉、可发现、鼠标/触屏都能点；
 * 结论：按钮为主（这里是它），同时给标题本身也挂右键复制当补充，两条路都能走。
 */
export default function CopyTitleButton({ text, className }: CopyTitleButtonProps) {
  const [done, setDone] = useState(false);
  const timerRef = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
    },
    [],
  );

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setDone(true);
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
      timerRef.current = window.setTimeout(() => setDone(false), 1200);
    } catch {
      // 剪贴板被拒时静默：按钮图标不变，用户可再点一次
    }
  };

  return (
    <button
      type="button"
      className={`icon-button copy-title${done ? " is-done" : ""}${
        className ? ` ${className}` : ""
      }`}
      onClick={(event) => {
        event.stopPropagation();
        void copy();
      }}
      title={done ? "已复制" : "复制标题"}
      aria-label="复制标题"
    >
      {done ? (
        <CheckIcon width={13} height={13} />
      ) : (
        <svg
          width={13}
          height={13}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={1.8}
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <rect x="9" y="9" width="11" height="11" rx="2" />
          <path d="M6 15V5.5A1.5 1.5 0 0 1 7.5 4H15" />
        </svg>
      )}
    </button>
  );
}
