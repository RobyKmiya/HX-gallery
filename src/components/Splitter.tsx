import { useRef } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";

interface SplitterProps {
  /** 当前尺寸（px），只用于 aria 播报与键盘步进。 */
  value: number;
  min: number;
  max: number;
  onChange: (next: number) => void;
  /** 双击 / 回车复位到默认宽度。 */
  onReset: () => void;
  /**
   * 拖动方向：1 = 往右拖变大（左侧面板）；-1 = 往右拖变小（右侧抽屉）。
   */
  direction?: 1 | -1;
  label: string;
}

/**
 * 面板之间的可拖动分界线。
 *
 * 除了拖动，还支持键盘（←/→ 步进 8px、回车复位）与双击复位——
 * 分界线是纯装饰的话，键盘用户就没法调整布局了。
 */
export default function Splitter({
  value,
  min,
  max,
  onChange,
  onReset,
  direction = 1,
  label,
}: SplitterProps) {
  const dragRef = useRef<{ startX: number; startValue: number } | null>(null);
  const clamp = (next: number) => Math.min(max, Math.max(min, Math.round(next)));

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { startX: event.clientX, startValue: value };
    document.body.classList.add("is-splitting");
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const state = dragRef.current;
    if (!state) return;
    const delta = (event.clientX - state.startX) * direction;
    const next = clamp(state.startValue + delta);
    if (next !== value) onChange(next);
  };

  const endDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!dragRef.current) return;
    dragRef.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    document.body.classList.remove("is-splitting");
  };

  return (
    <div
      className="splitter splitter-v"
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      title={`拖动调整${label}宽度，双击复位`}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onDoubleClick={onReset}
      onKeyDown={(event) => {
        if (event.key === "ArrowLeft") {
          event.preventDefault();
          onChange(clamp(value - 8 * direction));
        } else if (event.key === "ArrowRight") {
          event.preventDefault();
          onChange(clamp(value + 8 * direction));
        } else if (event.key === "Enter") {
          event.preventDefault();
          onReset();
        }
      }}
    />
  );
}
