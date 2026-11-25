import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement>;

const base: IconProps = {
  width: 18,
  height: 18,
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.8,
  strokeLinecap: "round",
  strokeLinejoin: "round",
  "aria-hidden": true,
};

export function LogoIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H18a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6.5A2.5 2.5 0 0 1 4 18.5z" />
      <path d="M8 3v18M11 8h5M11 12h5" />
    </svg>
  );
}

export function ImportIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M12 3v11" />
      <path d="m8 10 4 4 4-4" />
      <path d="M5 17v2a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-2" />
    </svg>
  );
}

export function FolderIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M3 6.5A2.5 2.5 0 0 1 5.5 4H9l2 2h7.5A2.5 2.5 0 0 1 21 8.5v9A2.5 2.5 0 0 1 18.5 20h-13A2.5 2.5 0 0 1 3 17.5z" />
    </svg>
  );
}

export function RefreshIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M20 7v5h-5" />
      <path d="M4 17v-5h5" />
      <path d="M6.1 9A7 7 0 0 1 18.4 7.4L20 12" />
      <path d="M17.9 15A7 7 0 0 1 5.6 16.6L4 12" />
    </svg>
  );
}

export function SwapIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M7 7h11l-3-3" />
      <path d="M17 17H6l3 3" />
    </svg>
  );
}

export function GridIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <rect x="4" y="4" width="6" height="6" rx="1.5" />
      <rect x="14" y="4" width="6" height="6" rx="1.5" />
      <rect x="4" y="14" width="6" height="6" rx="1.5" />
      <rect x="14" y="14" width="6" height="6" rx="1.5" />
    </svg>
  );
}

/** 画匣浏览器：5 经 3 纬的地球经纬球（左侧导航入口图标）。 */
export function GlobeIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      {/* 球体外圈 */}
      <circle cx="12" cy="12" r="9" />
      {/* 经线 5 条：中间本初子午线 + 左右两对弧 */}
      <path d="M12 3v18" />
      <ellipse cx="12" cy="12" rx="4.5" ry="9" />
      <ellipse cx="12" cy="12" rx="7.5" ry="9" />
      {/* 纬线 3 条：赤道 + 上下一对弧 */}
      <path d="M3 12h18" />
      <ellipse cx="12" cy="12" rx="9" ry="4" />
    </svg>
  );
}

export function BookIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H20v16H6.5A2.5 2.5 0 0 0 4 21.5z" />
      <path d="M4 18.5A2.5 2.5 0 0 1 6.5 16H20" />
    </svg>
  );
}

export function ChevronLeftIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="m15 18-6-6 6-6" />
    </svg>
  );
}

export function ChevronRightIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="m9 18 6-6-6-6" />
    </svg>
  );
}

export function ChevronUpIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="m18 15-6-6-6 6" />
    </svg>
  );
}

export function ChevronDownIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="m6 9 6 6 6-6" />
    </svg>
  );
}

export function FirstPageIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M7 5v14" />
      <path d="m17 6-6 6 6 6" />
    </svg>
  );
}

export function LastPageIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M17 5v14" />
      <path d="m7 6 6 6-6 6" />
    </svg>
  );
}

export function PlayIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M8 5.5v13l10-6.5z" />
    </svg>
  );
}

export function PauseIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M9 5v14M15 5v14" />
    </svg>
  );
}

export function CloseIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="m6 6 12 12M18 6 6 18" />
    </svg>
  );
}

export function StarIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="m12 3 2.7 5.6 6.1.9-4.4 4.3 1 6.1-5.4-2.9-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z" />
    </svg>
  );
}

export function TagIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M3.5 12.6V5a1.5 1.5 0 0 1 1.5-1.5h7.6a1.5 1.5 0 0 1 1.06.44l6.4 6.4a1.5 1.5 0 0 1 0 2.12l-6.1 6.1a1.5 1.5 0 0 1-2.12 0l-6.4-6.4a1.5 1.5 0 0 1-.44-1.06z" />
      <circle cx="7.8" cy="7.8" r="1.3" />
    </svg>
  );
}

export function HeartIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M12 20s-7.5-4.6-7.5-10A4.5 4.5 0 0 1 12 6.5 4.5 4.5 0 0 1 19.5 10c0 5.4-7.5 10-7.5 10z" />
    </svg>
  );
}

export function PlusIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}

export function AlertIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M12 3 2.7 20h18.6z" />
      <path d="M12 9v5M12 17.5v.5" />
    </svg>
  );
}

export function RevealIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M3 6.5A2.5 2.5 0 0 1 5.5 4H9l2 2h7.5A2.5 2.5 0 0 1 21 8.5v9A2.5 2.5 0 0 1 18.5 20h-13A2.5 2.5 0 0 1 3 17.5z" />
      <path d="m10 14 4-4M14 10h-3M14 10v3" />
    </svg>
  );
}

export function SearchIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m16 16 4.5 4.5" />
    </svg>
  );
}

export function FilterIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M4 6h16M7 12h10M10 18h4" />
    </svg>
  );
}

export function GraphIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <circle cx="12" cy="5" r="2.2" />
      <circle cx="5" cy="18" r="2.2" />
      <circle cx="19" cy="18" r="2.2" />
      <path d="M10.4 6.8 6.5 15.6M13.6 6.8l3.9 8.8M7.2 18h9.6" />
    </svg>
  );
}

export function TranslateIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M3.5 5.5h7M7 5.5v1.8c0 3.6-1.8 6.4-4.5 8.2" />
      <path d="M4.5 10.5c1.4 1.9 3.4 3.3 5.7 4.2" />
      <path d="m12.5 20 4-10 4 10M14 16h5" />
    </svg>
  );
}

export function TrashIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M4 7h16M9 7V4h6v3" />
      <path d="m6 7 1 13h10l1-13M10 11v5M14 11v5" />
    </svg>
  );
}

export function CheckIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="m5 12 4 4L19 6" />
    </svg>
  );
}

/** 下载到托盘：向下箭头 + 底部接盘（网络画集入口用）。 */
export function DownloadIcon(props: IconProps) {
  return (
    <svg {...base} {...props}>
      <path d="M12 3v11" />
      <path d="m7.5 10.5 4.5 4.5 4.5-4.5" />
      <path d="M4 17v2.5h16V17" />
    </svg>
  );
}
