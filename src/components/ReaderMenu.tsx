import { useEffect, useLayoutEffect, useRef, useState } from "react";

export interface ReaderMenuEntry {
  label: string;
  /** 右侧快捷键提示（纯展示）。 */
  hint?: string;
  onClick: () => void;
  disabled?: boolean;
  /** 这一条之前画一条分隔线（分组用）。 */
  dividerBefore?: boolean;
}

interface ReaderMenuProps {
  x: number;
  y: number;
  entries: ReaderMenuEntry[];
  onClose: () => void;
}

/**
 * 阅读器右键菜单（经典看图器风格：翻页/缩放/视图/播放/图片操作分组）。
 * 出现后：点外部 / Esc / 点任意条目都会关。
 */
export default function ReaderMenu({ x, y, entries, onClose }: ReaderMenuProps) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [position, setPosition] = useState({ x, y });

  // 贴边回弹：菜单快出屏时往回收，别让用户够不到。
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const nextX = Math.min(Math.max(4, x), window.innerWidth - rect.width - 4);
    const nextY = Math.min(Math.max(4, y), window.innerHeight - rect.height - 4);
    setPosition({ x: nextX, y: nextY });
  }, [x, y]);

  useEffect(() => {
    const onPointerDown = (event: MouseEvent) => {
      if (!ref.current?.contains(event.target as Node)) onClose();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [onClose]);

  return (
    <div
      ref={ref}
      className="reader-menu"
      style={{ left: position.x, top: position.y }}
      role="menu"
      aria-label="阅读器菜单"
    >
      {entries.map((entry) => (
        <div key={entry.label}>
          {entry.dividerBefore && <hr className="reader-menu-divider" />}
          <button
            type="button"
            className="reader-menu-item"
            role="menuitem"
            disabled={entry.disabled}
            onClick={() => {
              onClose();
              entry.onClick();
            }}
          >
            <span className="reader-menu-label">{entry.label}</span>
            {entry.hint && (
              <span className="reader-menu-hint">{entry.hint}</span>
            )}
          </button>
        </div>
      ))}
    </div>
  );
}
