import type { CollectionIcon } from "../types";

/** 收藏夹可选的形状（管理员 11-22：五角星/爱心/圆形/正方形）。 */
export const COLLECTION_ICONS: CollectionIcon[] = [
  "star",
  "heart",
  "circle",
  "square",
];

export const COLLECTION_COLORS = [
  "#e8737f",
  "#e8a35c",
  "#e6c85a",
  "#7fc46f",
  "#5fa8e8",
  "#9a7fe8",
  "#e87fb8",
  "#8a919e",
];

interface CollectionGlyphProps {
  icon: CollectionIcon;
  color: string;
  size?: number;
}

/**
 * 收藏夹的纯填充色块图标：无边框、无描边，就是一个实心形状（管理员要求）。
 */
export default function CollectionGlyph({
  icon,
  color,
  size = 16,
}: CollectionGlyphProps) {
  const common = {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: color,
    stroke: "none",
    "aria-hidden": true as const,
  };
  switch (icon) {
    case "star":
      return (
        <svg {...common}>
          <path d="M12 2.6l2.9 5.9 6.5.95-4.7 4.6 1.1 6.5-5.8-3.05-5.8 3.05 1.1-6.5-4.7-4.6 6.5-.95z" />
        </svg>
      );
    case "circle":
      return (
        <svg {...common}>
          <circle cx="12" cy="12" r="8.6" />
        </svg>
      );
    case "square":
      return (
        <svg {...common}>
          <rect x="3.4" y="3.4" width="17.2" height="17.2" rx="2.6" />
        </svg>
      );
    case "heart":
    default:
      return (
        <svg {...common}>
          <path d="M12 20.4S4.5 15.8 4.5 10.4A4.7 4.7 0 0 1 12 6.6a4.7 4.7 0 0 1 7.5 3.8c0 5.4-7.5 10-7.5 10z" />
        </svg>
      );
  }
}
