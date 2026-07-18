import type { ReactNode, SVGProps } from 'react';

/**
 * Inline SVG icons. No icon library is available (CSS Modules + React only), so
 * each icon is a small stroked SVG sized via `currentColor` and `1em`. Every
 * icon exposes an accessible name via a `<title>` element.
 */

type IconProps = SVGProps<SVGSVGElement>;

interface IconWrapperProps extends IconProps {
  title: string;
  children: ReactNode;
}

function Icon({ title, children, ...rest }: IconWrapperProps) {
  return (
    <svg
      width="1em"
      height="1em"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      <title>{title}</title>
      {children}
    </svg>
  );
}

export function PlusIcon(props: IconProps) {
  return (
    <Icon title="Add" {...props}>
      <path d="M12 5v14M5 12h14" />
    </Icon>
  );
}

export function GearIcon(props: IconProps) {
  return (
    <Icon title="Settings" {...props}>
      <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.09a2 2 0 0 1 1 1.74v.5a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.38a2 2 0 0 0-.73-2.73l-.15-.09a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2Z" />
      <circle cx="12" cy="12" r="3" />
    </Icon>
  );
}

export function SendIcon(props: IconProps) {
  return (
    <Icon title="Send" {...props}>
      <path d="m22 2-7 20-4-9-9-4Z" />
      <path d="M22 2 11 13" />
    </Icon>
  );
}

export function StopIcon(props: IconProps) {
  return (
    <Icon title="Stop" {...props}>
      <rect x="6" y="6" width="12" height="12" rx="2" />
    </Icon>
  );
}

export function ChevronIcon(props: IconProps) {
  return (
    <Icon title="Toggle" {...props}>
      <path d="m6 9 6 6 6-6" />
    </Icon>
  );
}

export function CheckIcon(props: IconProps) {
  return (
    <Icon title="Done" {...props}>
      <path d="M20 6 9 17l-5-5" />
    </Icon>
  );
}

export function CopyIcon(props: IconProps) {
  return (
    <Icon title="Copy" {...props}>
      <rect x="8" y="8" width="12" height="12" rx="3" />
      <path d="M16 8V7a3 3 0 0 0-3-3H7a3 3 0 0 0-3 3v6a3 3 0 0 0 3 3h1" />
    </Icon>
  );
}

export function ForkIcon(props: IconProps) {
  return (
    <Icon title="Fork conversation" {...props}>
      <circle cx="6" cy="5" r="2" />
      <circle cx="18" cy="5" r="2" />
      <circle cx="12" cy="19" r="2" />
      <path d="M6 7v2a4 4 0 0 0 4 4h2M18 7v2a4 4 0 0 1-4 4h-2M12 13v4" />
    </Icon>
  );
}

export function CloseIcon(props: IconProps) {
  return (
    <Icon title="Close" {...props}>
      <path d="M18 6 6 18M6 6l12 12" />
    </Icon>
  );
}

export function TerminalIcon(props: IconProps) {
  return (
    <Icon title="Work detail" {...props}>
      <path d="m4 17 6-6-6-6" />
      <path d="M12 19h8" />
    </Icon>
  );
}

export function SunIcon(props: IconProps) {
  return (
    <Icon title="Light theme" {...props}>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
    </Icon>
  );
}

export function MoonIcon(props: IconProps) {
  return (
    <Icon title="Dark theme" {...props}>
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79Z" />
    </Icon>
  );
}

export function GlobeIcon(props: IconProps) {
  return (
    <Icon title="Language" {...props}>
      <circle cx="12" cy="12" r="10" />
      <path d="M2 12h20M12 2a15.3 15.3 0 0 1 0 20M12 2a15.3 15.3 0 0 0 0 20" />
    </Icon>
  );
}

export function MenuIcon(props: IconProps) {
  return (
    <Icon title="Menu" {...props}>
      <path d="M4 7h16M4 12h16M4 17h16" />
    </Icon>
  );
}

export function CompactIcon(props: IconProps) {
  return (
    <Icon title="Compact context" {...props}>
      <path d="m8 3 4 4 4-4M12 7V1M8 21l4-4 4 4M12 17v6" />
      <path d="M4 12h16" />
    </Icon>
  );
}

export function ArrowDownIcon(props: IconProps) {
  return (
    <Icon title="Scroll to bottom" {...props}>
      <path d="M12 5v14M19 12l-7 7-7-7" />
    </Icon>
  );
}

export function ArrowUpIcon(props: IconProps) {
  return (
    <Icon title="Up" {...props}>
      <path d="M12 19V5M5 12l7-7 7 7" />
    </Icon>
  );
}

export function SparkIcon(props: IconProps) {
  return (
    <Icon title="Phi" {...props}>
      <path d="m12 3 1.45 4.55L18 9l-4.55 1.45L12 15l-1.45-4.55L6 9l4.55-1.45Z" />
      <path d="m18.5 15 .7 2.3 2.3.7-2.3.7-.7 2.3-.7-2.3-2.3-.7 2.3-.7Z" />
    </Icon>
  );
}

export function FileIcon(props: IconProps) {
  return (
    <Icon title="File" {...props}>
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Z" />
      <path d="M14 2v6h6" />
    </Icon>
  );
}

export function FolderIcon(props: IconProps) {
  return (
    <Icon title="Folder" {...props}>
      <path d="M3 6.5A2.5 2.5 0 0 1 5.5 4H9l2 2h7.5A2.5 2.5 0 0 1 21 8.5v8A2.5 2.5 0 0 1 18.5 19h-13A2.5 2.5 0 0 1 3 16.5Z" />
    </Icon>
  );
}

export function SearchIcon(props: IconProps) {
  return (
    <Icon title="Search" {...props}>
      <circle cx="11" cy="11" r="8" />
      <path d="m21 21-4.3-4.3" />
    </Icon>
  );
}

export function ProviderIcon(props: IconProps) {
  return (
    <Icon title="Provider" {...props}>
      <path d="m12 2 8 4.5v9L12 20l-8-4.5v-9Z" />
      <path d="m4.3 6.7 7.7 4.4 7.7-4.4M12 11.1V20" />
    </Icon>
  );
}

export function EyeIcon(props: IconProps) {
  return (
    <Icon title="Show value" {...props}>
      <path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6S2 12 2 12Z" />
      <circle cx="12" cy="12" r="2.5" />
    </Icon>
  );
}

export function EditIcon(props: IconProps) {
  return (
    <Icon title="Edit" {...props}>
      <path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" />
      <path d="m15 5 4 4" />
    </Icon>
  );
}

export function WrenchIcon(props: IconProps) {
  return (
    <Icon title="Tool" {...props}>
      <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
    </Icon>
  );
}

export function ListIcon(props: IconProps) {
  return (
    <Icon title="List" {...props}>
      <path d="M8 6h13M8 12h13M8 18h13" />
      <path d="M3 6h.01M3 12h.01M3 18h.01" />
    </Icon>
  );
}

export function LoaderIcon(props: IconProps) {
  return (
    <Icon title="Loading" {...props}>
      <path d="M21 12a9 9 0 1 1-6.22-8.56" />
    </Icon>
  );
}

export function AgentIcon(props: IconProps) {
  return (
    <Icon title="Subagent" {...props}>
      <circle cx="12" cy="8" r="4" />
      <path d="M4 21v-1a7 7 0 0 1 7-7h2a7 7 0 0 1 7 7v1" />
    </Icon>
  );
}

export function PinIcon(props: IconProps) {
  return (
    <Icon title="Pin" {...props}>
      <path d="m14 4 6 6-3 1-4 4-1 5-3-3-5 3 3-5 4-4Z" />
      <path d="m5 19-2 2" />
    </Icon>
  );
}

export function TrashIcon(props: IconProps) {
  return (
    <Icon title="Delete" {...props}>
      <path d="M3 6h18M8 6V4h8v2M19 6l-1 15H6L5 6" />
      <path d="M10 11v6M14 11v6" />
    </Icon>
  );
}

export function ClockIcon(props: IconProps) {
  return (
    <Icon title="Scheduled tasks" {...props}>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5l3 2" />
    </Icon>
  );
}

export function PlayIcon(props: IconProps) {
  return (
    <Icon title="Run" {...props}>
      <path d="m8 5 11 7-11 7Z" />
    </Icon>
  );
}

export function PauseIcon(props: IconProps) {
  return (
    <Icon title="Pause" {...props}>
      <path d="M9 5v14M15 5v14" />
    </Icon>
  );
}
