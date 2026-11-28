/**
 * 画廊选择工具（M4 · Windows 化）。
 *
 * 纯函数，不依赖 React：宿主持有锚点，卡片列表按当前展示顺序算区间。
 * 后续「空白拖拽框选」也可以复用这里的约定（顺序 = DOM 顺序）。
 */

/**
 * Shift 区间选择：按 `order` 取 anchor → target（含两端）。
 *
 * 任一端不在当前列表（比如翻页后锚点已经滚出去了）就安全退化成只选 target，
 * 不会因为找不到锚点而把整段乱选上。
 */
export function rangeSelection(
  order: readonly number[],
  anchor: number | null,
  target: number,
): number[] {
  const targetIndex = order.indexOf(target);
  if (targetIndex < 0) return [target];
  const anchorIndex = anchor === null ? -1 : order.indexOf(anchor);
  if (anchorIndex < 0) return [target];
  const from = Math.min(anchorIndex, targetIndex);
  const to = Math.max(anchorIndex, targetIndex);
  return order.slice(from, to + 1);
}
