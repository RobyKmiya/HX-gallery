import { useCallback, useEffect, useRef, useState } from "react";

/**
 * 指针拖拽排序（替代 HTML5 draggable）。
 *
 * 为什么不用原生拖拽：Tauri 在 Windows 上默认开 `dragDropEnabled`（窗口级
 * 系统拖放，用来接「文件拖进窗口导入」这个真功能），它会在 WebView2 上装
 * 一层 IDropTarget，把 HTML5 的 dragstart/dragover/drop **整层吞掉**——
 * 代码看着没问题，但按住就是拖不动（管理员 2026-11-27 连报两轮：
 * 1.1.86 标签中心排序、1.1.92 工作台标签条）。两者不能同时开，而文件拖入
 * 导入更不能丢，所以排序改走 pointer 事件自己实现。
 *
 * 用法：给可拖动元素加 `data-drag-id`，`onPointerDown` 里调 `begin(event, id)`，
 * 落点判定用 `elementFromPoint().closest('[data-drag-id]')`——所以拖动中的
 * 元素必须 `pointer-events: none`（见 App.css 的 `body.is-pointer-dragging`
 * 与 `.is-dragging` 规则），否则每次都会命中自己。
 */
interface PointerDragOptions {
  /** 越过阈值、真正开始拖动时触发（用来记录拖动源）。 */
  onStart?: (id: string) => void;
  /** 指针停在某个落点上（只用来画高亮，不改顺序）。 */
  onHover?: (id: string | null) => void;
  /** 松手：from 拖到了 to 上。 */
  onDrop?: (from: string, to: string) => void;
  /** 移动多少像素才算拖动，默认 6。 */
  threshold?: number;
  /** 判定方向：横向标签条传 "x"，纵向列表传 "y"，默认两者都算。 */
  axis?: "x" | "y" | "both";
  /** 落点属性名，默认 data-drag-id。 */
  attribute?: string;
}

export interface PointerDrag {
  /** 正在拖的 id（未拖为 null）。 */
  dragId: string | null;
  /** 当前悬停的落点 id（未悬停为 null）。 */
  overId: string | null;
  /** 在可拖动元素的 onPointerDown 里调用。 */
  begin: (event: React.PointerEvent, id: string) => void;
  /** 拖动刚结束时浏览器还会补一个 click——onClick 里先问一句再执行。 */
  shouldIgnoreClick: () => boolean;
}

export function usePointerDrag(options: PointerDragOptions): PointerDrag {
  const [dragId, setDragId] = useState<string | null>(null);
  const [overId, setOverId] = useState<string | null>(null);

  const optionsRef = useRef(options);
  optionsRef.current = options;

  const startRef = useRef<{ id: string; x: number; y: number } | null>(null);
  const movedRef = useRef(false);
  const overRef = useRef<string | null>(null);
  const cleanupRef = useRef<(() => void) | null>(null);
  const ignoreClickRef = useRef(false);

  const detach = useCallback(() => {
    cleanupRef.current?.();
    cleanupRef.current = null;
  }, []);

  useEffect(() => detach, [detach]);

  const begin = useCallback(
    (event: React.PointerEvent, id: string) => {
      if (event.button !== 0) return;
      // 标签上的「关闭 ×」等按钮：按下就是要点击，别当成拖动起点。
      if ((event.target as HTMLElement).closest("button")) return;
      detach();
      startRef.current = { id, x: event.clientX, y: event.clientY };
      movedRef.current = false;
      overRef.current = null;

      const attribute = optionsRef.current.attribute ?? "data-drag-id";
      const threshold = optionsRef.current.threshold ?? 6;
      const axis = optionsRef.current.axis ?? "both";

      const move = (moveEvent: PointerEvent) => {
        const start = startRef.current;
        if (!start) return;
        const dx = moveEvent.clientX - start.x;
        const dy = moveEvent.clientY - start.y;
        if (!movedRef.current) {
          const distance =
            axis === "x" ? Math.abs(dx) : axis === "y" ? Math.abs(dy) : Math.hypot(dx, dy);
          if (distance < threshold) return;
          movedRef.current = true;
          setDragId(start.id);
          document.body.classList.add("is-pointer-dragging");
          optionsRef.current.onStart?.(start.id);
        }
        const element = document.elementFromPoint(
          moveEvent.clientX,
          moveEvent.clientY,
        ) as HTMLElement | null;
        const host = element?.closest(`[${attribute}]`) as HTMLElement | null;
        const target = host?.getAttribute(attribute) ?? null;
        const next = target !== null && target !== start.id ? target : null;
        if (next !== overRef.current) {
          overRef.current = next;
          setOverId(next);
          optionsRef.current.onHover?.(next);
        }
      };

      const finish = () => {
        detach();
        const start = startRef.current;
        const target = overRef.current;
        startRef.current = null;
        overRef.current = null;
        document.body.classList.remove("is-pointer-dragging");
        setDragId(null);
        setOverId(null);
        if (movedRef.current) {
          // 拖动结束后浏览器仍会派一次 click（按下与松开在同一个元素上），
          // 不屏蔽的话「拖着排序」会顺带把那个标签激活。
          ignoreClickRef.current = true;
          window.setTimeout(() => {
            ignoreClickRef.current = false;
          }, 300);
          if (start && target) optionsRef.current.onDrop?.(start.id, target);
        }
        movedRef.current = false;
      };

      const cancel = () => {
        detach();
        startRef.current = null;
        overRef.current = null;
        document.body.classList.remove("is-pointer-dragging");
        setDragId(null);
        setOverId(null);
        movedRef.current = false;
      };

      // 捕获阶段监听：目标元素里若有 webview/输入框先吞掉事件也不影响排序。
      window.addEventListener("pointermove", move, true);
      window.addEventListener("pointerup", finish, true);
      window.addEventListener("pointercancel", cancel, true);
      cleanupRef.current = () => {
        window.removeEventListener("pointermove", move, true);
        window.removeEventListener("pointerup", finish, true);
        window.removeEventListener("pointercancel", cancel, true);
      };
    },
    [detach],
  );

  const shouldIgnoreClick = useCallback(() => ignoreClickRef.current, []);

  return { dragId, overId, begin, shouldIgnoreClick };
}
