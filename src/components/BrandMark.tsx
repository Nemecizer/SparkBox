import { useId } from "react";

const NODES: [number, number][] = [[2, 2], [14, 2], [2, 14], [14, 14]];

/**
 * The SparkBox mark: four nodes joined through the fabric by a spark. The same
 * geometry as the app icon (src-tauri/icons/icon-master.svg), drawn for 16px.
 */
export function BrandMark({ size = 16, title, className }: { size?: number; title?: string; className?: string }) {
  const id = useId();
  return (
    <svg
      className={className ? `brand-mark ${className}` : "brand-mark"}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
    >
      <defs>
        <linearGradient id={`${id}n`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#7cb8ff" />
          <stop offset="1" stopColor="#2a74db" />
        </linearGradient>
        <linearGradient id={`${id}s`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#ffd166" />
          <stop offset="1" stopColor="#ff8a2b" />
        </linearGradient>
      </defs>
      <g stroke="#5d9be8" strokeWidth={1.4} strokeLinecap="round">
        {NODES.map(([x, y]) => <line key={`${x}${y}`} x1={x + 4} y1={y + 4} x2={12} y2={12} />)}
      </g>
      <g fill={`url(#${id}n)`}>
        {NODES.map(([x, y]) => <rect key={`${x}${y}`} x={x} y={y} width={8} height={8} rx={2.2} />)}
      </g>
      <path d="M12 6.6L12.95 11.05L17.4 12L12.95 12.95L12 17.4L11.05 12.95L6.6 12L11.05 11.05Z" fill={`url(#${id}s)`} />
    </svg>
  );
}
