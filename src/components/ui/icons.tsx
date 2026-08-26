/** Inline stroke icons. Inline so the CSP never has to allow a remote asset. */
type P = { size?: number; className?: string };

/** Exported so callers can hold an icon in a data structure. */
export type IconProps = P;

const base = (size: number) => ({
  width: size,
  height: size,
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.75,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  "aria-hidden": true,
});

export const CheckIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M20 6 9 17l-5-5" />
  </svg>
);

export const AlertIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 8v4.5M12 16h.01" />
  </svg>
);

export const XIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M18 6 6 18M6 6l12 12" />
  </svg>
);

export const LinkIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M9.5 14.5 14.5 9.5" />
    <path d="M12.5 7.5 14 6a4.24 4.24 0 0 1 6 6l-1.5 1.5" />
    <path d="M11.5 16.5 10 18a4.24 4.24 0 0 1-6-6L5.5 10.5" />
  </svg>
);

export const ShieldIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M12 3l7 3v5.5c0 4.3-2.9 8.2-7 9.5-4.1-1.3-7-5.2-7-9.5V6z" />
  </svg>
);

export const UsersIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M15.5 20v-1.6a3.4 3.4 0 0 0-3.4-3.4H6.9A3.4 3.4 0 0 0 3.5 18.4V20" />
    <circle cx="9.5" cy="8" r="3.2" />
    <path d="M20.5 20v-1.6a3.4 3.4 0 0 0-2.6-3.3M15.8 5a3.2 3.2 0 0 1 0 6.1" />
  </svg>
);

export const DownloadIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M12 3v11m0 0 4-4m-4 4-4-4" />
    <path d="M4 17v2.5A1.5 1.5 0 0 0 5.5 21h13a1.5 1.5 0 0 0 1.5-1.5V17" />
  </svg>
);

export const SlidersIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
    <circle cx="15" cy="7" r="2" />
    <circle cx="9" cy="17" r="2" />
  </svg>
);

export const SunIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
  </svg>
);

export const MoonIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5z" />
  </svg>
);

export const BoltIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M13 2 4.5 13.5H11l-1 8.5 8.5-11.5H12z" />
  </svg>
);

/** A playlist: three lines with a play triangle, as YouTube draws it. */
export const ListIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M4 6h11M4 11h11M4 16h6" />
    <path d="m15 14 5 3-5 3Z" />
  </svg>
);

/** Scissors: the converter's cut. */
export const ScissorsIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <circle cx="6" cy="6" r="2.5" />
    <circle cx="6" cy="18" r="2.5" />
    <path d="M8 7.5 20 18M20 6 8 16.5" />
  </svg>
);

export const FolderIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" />
  </svg>
);

export const TrashIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M4 7h16M9 7V5h6v2M6 7l1 12h10l1-12M10 11v5M14 11v5" />
  </svg>
);

export const StopIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <rect x="6" y="6" width="12" height="12" rx="2.5" />
  </svg>
);

export const ClockIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 7v5.2l3.2 2" />
  </svg>
);

export const TerminalIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <rect x="3" y="4" width="18" height="16" rx="2.5" />
    <path d="M7.5 9.5 10 12l-2.5 2.5M12.5 15H17" />
  </svg>
);

export const GlobeIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <circle cx="12" cy="12" r="9" />
    <path d="M3 12h18M12 3c2.5 2.6 3.8 5.7 3.8 9S14.5 18.4 12 21c-2.5-2.6-3.8-5.7-3.8-9S9.5 5.6 12 3Z" />
  </svg>
);

export const HomeIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M4 10.5 12 4l8 6.5V19a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 19Z" />
    <path d="M9.5 20.5v-6h5v6" />
  </svg>
);

export const SendIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M21.5 4.5 3 11.2l6.4 2.4M21.5 4.5 18 20l-8.6-6.4M21.5 4.5 9.4 13.6M9.4 13.6V19l3-3.2" />
  </svg>
);

// A solid Telegram paper plane, for the Telegram page header and connected tile.
export const TelegramIcon = ({ size = 16, className }: P) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" className={className} aria-hidden>
    <path d="M21.94 4.4 18.9 19.1c-.23 1.02-.84 1.27-1.7.79l-4.7-3.47-2.27 2.19c-.25.25-.46.46-.94.46l.34-4.8L18.4 6.9c.38-.34-.08-.53-.6-.19L6.98 13.7l-4.64-1.45c-1.01-.32-1.03-1.01.21-1.5l18.15-7c.84-.3 1.58.2 1.24 1.65Z" />
  </svg>
);

export const UploadIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M12 15V4M8 8l4-4 4 4M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3" />
  </svg>
);

export const ChevronRightIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="m9 6 6 6-6 6" />
  </svg>
);

export const ArrowLeftIcon = ({ size = 16, className }: P) => (
  <svg {...base(size)} className={className}>
    <path d="M19 12H5" />
    <path d="m12 19-7-7 7-7" />
  </svg>
);
