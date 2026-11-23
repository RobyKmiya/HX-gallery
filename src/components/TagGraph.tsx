import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import type { TagGraph as TagGraphData } from "../types";
import { CloseIcon, GraphIcon, SearchIcon } from "./Icons";

interface TagGraphProps {
  graph: TagGraphData;
  onClose: () => void;
  onBrowseTag: (tagId: number) => void;
}

interface PlacedNode {
  id: number;
  label: string;
  title: string;
  count: number;
  category: string | null;
  x: number;
  y: number;
  radius: number;
}

type PositionedNode = PlacedNode;

const WIDTH = 980;
const HEIGHT = 620;
const CENTER_X = WIDTH / 2;
const CENTER_Y = HEIGHT / 2;
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));
const MIN_SCALE = 0.35;
const MAX_SCALE = 3.5;
const UNCATEGORIZED_COLOR = "#6b7280";
const CATEGORY_COLORS = [
  "#e8a35c",
  "#7fb3d5",
  "#8fd18a",
  "#c79be0",
  "#e68686",
  "#d9c56b",
  "#6fc7c1",
  "#b0a3e0",
];

interface Viewport {
  scale: number;
  x: number;
  y: number;
}

const RESET_VIEW: Viewport = { scale: 1, x: 0, y: 0 };

function clampScale(value: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, value));
}

function displayLabel(name: string, translatedName: string | null): string {
  const label = translatedName?.trim() || name;
  return label.length > 14 ? `${label.slice(0, 13)}…` : label;
}

function topCategory(category: string | null): string | null {
  if (!category) return null;
  const top = category.split("/")[0]?.trim();
  return top ? top : null;
}

export default function TagGraph({
  graph,
  onClose,
  onBrowseTag,
}: TagGraphProps) {
  const layout = useMemo(() => {
    const maxCount = Math.max(1, ...graph.nodes.map((node) => node.count));
    const sorted = [...graph.nodes].sort(
      (left, right) => right.count - left.count || left.name.localeCompare(right.name),
    );

    const useCircle = sorted.length > 1 && sorted.length <= 8;
    const circleRadius = Math.min(230, 90 + sorted.length * 24);

    const nodes: PlacedNode[] = sorted.map((node, index) => {
      const radius =
        index === 0 ? 24 : Math.min(34, 7 + (Math.sqrt(node.count) / Math.sqrt(maxCount)) * 26);
      const angle = useCircle
        ? -Math.PI / 2 + (index / Math.max(1, sorted.length)) * Math.PI * 2
        : index * GOLDEN_ANGLE + 0.8;
      const distance = useCircle
        ? circleRadius
        : index === 0
          ? 0
          : Math.min(300, 60 + Math.sqrt(index) * 34);
      const x = CENTER_X + Math.cos(angle) * distance * 1.28;
      const y = CENTER_Y + Math.sin(angle) * distance * 0.9;
      const category = topCategory(node.category);
      const titleParts = [
        node.translatedName?.trim() || node.name,
        `${node.count} 个画集`,
      ];
      if (category) titleParts.push(`分类：${category}`);

      return {
        id: node.id,
        label: displayLabel(node.name, node.translatedName),
        title: titleParts.join(" · "),
        count: node.count,
        category,
        x,
        y,
        radius: node.count > 0 ? radius : 7,
      };
    });

    const nodeIds = new Set(nodes.map((node) => node.id));
    const maxWeight = Math.max(1, ...graph.edges.map((edge) => edge.weight));
    const edges = graph.edges.filter(
      (edge) => nodeIds.has(edge.source) && nodeIds.has(edge.target),
    );

    return { nodes, edges, maxWeight };
  }, [graph]);

  const svgRef = useRef<SVGSVGElement | null>(null);
  const panRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    x: number;
    y: number;
    moved: boolean;
  } | null>(null);
  const dragRef = useRef<{
    id: number;
    pointerId: number;
    startClientX: number;
    startClientY: number;
    startX: number;
    startY: number;
    scale: number;
    moved: boolean;
  } | null>(null);
  const suppressClickRef = useRef(false);
  const [view, setView] = useState<Viewport>(RESET_VIEW);
  const [positions, setPositions] = useState<
    Record<number, { x: number; y: number }>
  >({});
  const [hoveredId, setHoveredId] = useState<number | null>(null);
  const [dragging, setDragging] = useState(false);
  const [dragNodeId, setDragNodeId] = useState<number | null>(null);
  const [query, setQuery] = useState("");

  useEffect(() => {
    setView(RESET_VIEW);
    setPositions({});
    setHoveredId(null);
    setQuery("");
    panRef.current = null;
    dragRef.current = null;
  }, [graph]);

  const nodes = useMemo<PositionedNode[]>(
    () =>
      layout.nodes.map((node) => {
        const position = positions[node.id];
        return position ? { ...node, x: position.x, y: position.y } : node;
      }),
    [layout.nodes, positions],
  );

  const nodeMap = useMemo(() => {
    const map = new Map<number, PositionedNode>();
    for (const node of nodes) map.set(node.id, node);
    return map;
  }, [nodes]);

  const edges = useMemo(
    () =>
      layout.edges.flatMap((edge) => {
        const source = nodeMap.get(edge.source);
        const target = nodeMap.get(edge.target);
        if (!source || !target) return [];
        return [{ ...edge, source, target }];
      }),
    [layout.edges, nodeMap],
  );

  const adjacency = useMemo(() => {
    const map = new Map<number, Set<number>>();
    for (const edge of layout.edges) {
      if (!map.has(edge.source)) map.set(edge.source, new Set());
      if (!map.has(edge.target)) map.set(edge.target, new Set());
      map.get(edge.source)?.add(edge.target);
      map.get(edge.target)?.add(edge.source);
    }
    return map;
  }, [layout.edges]);

  const activeIds = useMemo(() => {
    if (hoveredId === null) return null;
    const ids = new Set<number>([hoveredId]);
    adjacency.get(hoveredId)?.forEach((id) => ids.add(id));
    return ids;
  }, [hoveredId, adjacency]);

  const categoryColors = useMemo(() => {
    const names = new Set<string>();
    for (const node of layout.nodes) {
      if (node.category) names.add(node.category);
    }
    const sorted = [...names].sort((left, right) => left.localeCompare(right));
    return new Map(
      sorted.map((name, index) => [
        name,
        CATEGORY_COLORS[index % CATEGORY_COLORS.length],
      ]),
    );
  }, [layout.nodes]);

  const normalizedQuery = query.trim().toLowerCase();
  const searching = normalizedQuery.length > 0;

  const matches = useMemo(() => {
    if (!searching) return new Set<number>();
    return new Set(
      nodes
        .filter((node) => node.title.toLowerCase().includes(normalizedQuery))
        .map((node) => node.id),
    );
  }, [nodes, normalizedQuery, searching]);

  const colorOf = useCallback(
    (category: string | null) =>
      (category && categoryColors.get(category)) || UNCATEGORIZED_COLOR,
    [categoryColors],
  );

  const resetAll = useCallback(() => {
    panRef.current = null;
    dragRef.current = null;
    setView(RESET_VIEW);
    setPositions({});
    setHoveredId(null);
    setQuery("");
  }, []);

  const fitView = useCallback(() => {
    if (nodes.length === 0) return;
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const node of nodes) {
      const labelWidth = node.label.length * 6.4 + 16;
      minX = Math.min(minX, node.x - node.radius);
      maxX = Math.max(maxX, node.x + node.radius + labelWidth);
      minY = Math.min(minY, node.y - node.radius);
      maxY = Math.max(maxY, node.y + node.radius);
    }
    const padding = 48;
    const scale = clampScale(
      Math.min(
        1.2,
        WIDTH / Math.max(1, maxX - minX + padding * 2),
        HEIGHT / Math.max(1, maxY - minY + padding * 2),
      ),
    );
    const centerX = (minX + maxX) / 2;
    const centerY = (minY + maxY) / 2;
    setView({
      scale,
      x: CENTER_X - centerX * scale,
      y: CENTER_Y - centerY * scale,
    });
  }, [nodes]);

  const focusNode = useCallback((node: PositionedNode, scale: number) => {
    const next = clampScale(scale);
    setView({
      scale: next,
      x: CENTER_X - node.x * next,
      y: CENTER_Y - node.y * next,
    });
  }, []);

  const locateFirstMatch = useCallback(() => {
    if (!searching) return;
    const match = nodes.find((node) => matches.has(node.id));
    if (match) focusNode(match, Math.max(view.scale, 1.4));
  }, [searching, nodes, matches, focusNode, view.scale]);

  const zoomAt = useCallback(
    (clientX: number, clientY: number, factor: number) => {
      const svg = svgRef.current;
      if (!svg) return;
      const matrix = svg.getScreenCTM();
      if (!matrix) return;

      const point = new DOMPoint(clientX, clientY).matrixTransform(
        matrix.inverse(),
      );
      setView((previous) => {
        const scale = clampScale(previous.scale * factor);
        const ratio = scale / previous.scale;
        return {
          scale,
          x: point.x - (point.x - previous.x) * ratio,
          y: point.y - (point.y - previous.y) * ratio,
        };
      });
    },
    [],
  );

  const zoomCenter = useCallback(
    (factor: number) => {
      const svg = svgRef.current;
      if (!svg) return;
      const rect = svg.getBoundingClientRect();
      zoomAt(rect.left + rect.width / 2, rect.top + rect.height / 2, factor);
    },
    [zoomAt],
  );

  useEffect(() => {
    const svg = svgRef.current;
    if (!svg) return;

    const handleWheel = (event: WheelEvent) => {
      event.preventDefault();
      zoomAt(event.clientX, event.clientY, Math.exp(-event.deltaY * 0.0015));
    };

    svg.addEventListener("wheel", handleWheel, { passive: false });
    return () => svg.removeEventListener("wheel", handleWheel);
  }, [zoomAt, layout.nodes.length]);

  const handlePointerDown = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (event.button !== 0) return;
    const svg = svgRef.current;
    if (!svg) return;
    panRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      x: view.x,
      y: view.y,
      moved: false,
    };
  };

  const handlePointerMove = (event: ReactPointerEvent<SVGSVGElement>) => {
    const svg = svgRef.current;
    const pan = panRef.current;
    if (!svg || !pan || pan.pointerId !== event.pointerId) return;

    const dx = event.clientX - pan.startX;
    const dy = event.clientY - pan.startY;
    if (!pan.moved && Math.hypot(dx, dy) < 3) return;
    if (!pan.moved) {
      pan.moved = true;
      svg.setPointerCapture(event.pointerId);
      setDragging(true);
    }

    const matrix = svg.getScreenCTM();
    const scaleX = matrix?.a || 1;
    const scaleY = matrix?.d || 1;
    setView((previous) => ({
      ...previous,
      x: pan.x + dx / scaleX,
      y: pan.y + dy / scaleY,
    }));
  };

  const handlePointerUp = (event: ReactPointerEvent<SVGSVGElement>) => {
    const svg = svgRef.current;
    const pan = panRef.current;
    if (svg && pan?.moved && svg.hasPointerCapture(event.pointerId)) {
      svg.releasePointerCapture(event.pointerId);
    }
    if (pan?.moved) {
      suppressClickRef.current = true;
      window.setTimeout(() => {
        suppressClickRef.current = false;
      }, 250);
    }
    panRef.current = null;
    setDragging(false);
  };

  const handleNodePointerDown = (
    event: ReactPointerEvent<SVGGElement>,
    node: PositionedNode,
  ) => {
    if (event.button !== 0) return;
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = {
      id: node.id,
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      startX: node.x,
      startY: node.y,
      scale: view.scale,
      moved: false,
    };
    setHoveredId(node.id);
  };

  const handleNodePointerMove = (event: ReactPointerEvent<SVGGElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const dx = event.clientX - drag.startClientX;
    const dy = event.clientY - drag.startClientY;
    if (!drag.moved && Math.hypot(dx, dy) < 3) return;
    if (!drag.moved) {
      drag.moved = true;
      setDragNodeId(drag.id);
    }
    const matrix = svgRef.current?.getScreenCTM();
    const scale = (matrix?.a || 1) * drag.scale;
    setPositions((previous) => ({
      ...previous,
      [drag.id]: {
        x: drag.startX + dx / scale,
        y: drag.startY + dy / scale,
      },
    }));
  };

  const handleNodePointerUp = (event: ReactPointerEvent<SVGGElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (drag.moved) {
      suppressClickRef.current = true;
      window.setTimeout(() => {
        suppressClickRef.current = false;
      }, 250);
    }
    dragRef.current = null;
    setDragNodeId(null);
  };

  const handleDoubleClick = (event: ReactMouseEvent<SVGSVGElement>) => {
    if ((event.target as Element).closest(".tag-graph-node")) return;
    resetAll();
  };

  const categoryLegend = [...categoryColors.entries()];
  const hasUncategorized = layout.nodes.some((node) => !node.category);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="tag-graph-modal"
        role="dialog"
        aria-modal="true"
        aria-label="标签导图"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="tag-graph-head">
          <div>
            <p className="eyebrow">TAG MAP</p>
            <h2>标签导图</h2>
            <p>
              节点颜色区分一级分类，大小代表画集数量，连线代表标签共同出现在同一画集中。拖动节点可重排，悬停查看相邻标签，点击节点可筛选全库。
            </p>
          </div>
          <button className="icon-button" onClick={onClose} aria-label="关闭导图">
            <CloseIcon />
          </button>
        </header>

        {layout.nodes.length === 0 ? (
          <div className="tag-graph-empty">
            <GraphIcon width={30} height={30} />
            <p>还没有标签。先给画集添加标签，再生成导图。</p>
          </div>
        ) : (
          <div className="tag-graph-canvas">
            <svg
              ref={svgRef}
              className={dragging ? "is-dragging" : ""}
              viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
              role="img"
              aria-label="标签关系图"
              onPointerDown={handlePointerDown}
              onPointerMove={handlePointerMove}
              onPointerUp={handlePointerUp}
              onPointerCancel={handlePointerUp}
              onPointerLeave={() => {
                if (!panRef.current && !dragRef.current) setHoveredId(null);
              }}
              onDoubleClick={handleDoubleClick}
            >
              <g transform={`translate(${view.x} ${view.y}) scale(${view.scale})`}>
                <g className="tag-graph-edges">
                  {edges.map((edge) => {
                    const isActive =
                      hoveredId !== null &&
                      (edge.source.id === hoveredId || edge.target.id === hoveredId);
                    const isDim = hoveredId !== null && !isActive;
                    return (
                      <line
                        key={`${edge.source.id}-${edge.target.id}`}
                        x1={edge.source.x}
                        y1={edge.source.y}
                        x2={edge.target.x}
                        y2={edge.target.y}
                        className={
                          isActive ? "is-active" : isDim ? "is-dim" : undefined
                        }
                        strokeWidth={1 + (edge.weight / layout.maxWeight) * 3}
                        opacity={0.18 + (edge.weight / layout.maxWeight) * 0.45}
                      >
                        <title>共同出现 {edge.weight} 次</title>
                      </line>
                    );
                  })}
                </g>
                <g className="tag-graph-nodes">
                  {nodes.map((node) => {
                    const isMatch = searching && matches.has(node.id);
                    const isMuted = searching
                      ? !isMatch
                      : activeIds !== null && !activeIds.has(node.id);
                    const isNeighbor =
                      !searching &&
                      activeIds !== null &&
                      activeIds.has(node.id) &&
                      hoveredId !== node.id;
                    const className = [
                      "tag-graph-node",
                      isMuted ? "is-dim" : "",
                      isNeighbor ? "is-neighbor" : "",
                      isMatch ? "is-match" : "",
                      dragNodeId === node.id ? "is-moving" : "",
                    ]
                      .filter(Boolean)
                      .join(" ");

                    return (
                      <g
                        key={node.id}
                        className={className}
                        role="button"
                        tabIndex={0}
                        style={
                          {
                            "--node-color": colorOf(node.category),
                          } as CSSProperties
                        }
                        onPointerDown={(event) =>
                          handleNodePointerDown(event, node)
                        }
                        onPointerMove={handleNodePointerMove}
                        onPointerUp={handleNodePointerUp}
                        onPointerCancel={handleNodePointerUp}
                        onPointerEnter={() => setHoveredId(node.id)}
                        onPointerLeave={() => {
                          if (dragRef.current?.id !== node.id) {
                            setHoveredId(null);
                          }
                        }}
                        onFocus={() => setHoveredId(node.id)}
                        onBlur={() => setHoveredId(null)}
                        onClick={() => {
                          if (suppressClickRef.current) {
                            suppressClickRef.current = false;
                            return;
                          }
                          onBrowseTag(node.id);
                        }}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" || event.key === " ") {
                            event.preventDefault();
                            onBrowseTag(node.id);
                          }
                        }}
                      >
                        <title>{node.title}</title>
                        <circle
                          cx={node.x}
                          cy={node.y}
                          r={node.radius}
                          className={node.count === 0 ? "is-unused" : ""}
                        />
                        <text x={node.x + node.radius + 7} y={node.y + 4}>
                          {node.label}
                          {node.count > 0 && (
                            <tspan className="tag-graph-count"> · {node.count}</tspan>
                          )}
                        </text>
                      </g>
                    );
                  })}
                </g>
              </g>
            </svg>

            <label className="tag-graph-search">
              <SearchIcon width={13} height={13} />
              <input
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                  setHoveredId(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    event.stopPropagation();
                    locateFirstMatch();
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    event.stopPropagation();
                    setQuery("");
                    event.currentTarget.blur();
                  }
                }}
                placeholder="查找标签并定位"
                aria-label="查找标签"
              />
              {searching && <small>{matches.size}</small>}
            </label>

            <div className="tag-graph-controls">
              <button
                type="button"
                onClick={() => zoomCenter(1 / 1.2)}
                aria-label="缩小导图"
                title="缩小"
              >
                −
              </button>
              <span>{Math.round(view.scale * 100)}%</span>
              <button
                type="button"
                onClick={() => zoomCenter(1.2)}
                aria-label="放大导图"
                title="放大"
              >
                +
              </button>
              <button
                type="button"
                className="tag-graph-fit"
                onClick={fitView}
                title="缩放到适合窗口"
              >
                适应
              </button>
              <button
                type="button"
                className="tag-graph-reset"
                onClick={resetAll}
                title="重置缩放与节点位置"
              >
                重置
              </button>
            </div>
            <p className="tag-graph-hint">
              滚轮缩放 · 拖动空白平移 · 拖动节点重排 · 双击空白复位
            </p>
          </div>
        )}

        <footer className="tag-graph-legend">
          <span>
            <i className="legend-dot" />
            大小 = 画集数
          </span>
          <span>
            <i className="legend-line" />
            连线 = 共同出现
          </span>
          {categoryLegend.map(([name, color]) => (
            <span key={name}>
              <i className="legend-swatch" style={{ background: color }} />
              {name}
            </span>
          ))}
          {hasUncategorized && (
            <span>
              <i
                className="legend-swatch"
                style={{ background: UNCATEGORIZED_COLOR }}
              />
              未分类
            </span>
          )}
          <span className="tag-graph-legend-count">
            {layout.nodes.length} 个标签
          </span>
        </footer>
      </div>
    </div>
  );
}
