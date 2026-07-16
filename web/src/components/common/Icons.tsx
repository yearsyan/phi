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
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9c.14.34.22.7.22 1.06" />
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

export function SparkIcon(props: IconProps) {
  return (
    <Icon title="Phi" {...props}>
      <path d="m12 3 1.45 4.55L18 9l-4.55 1.45L12 15l-1.45-4.55L6 9l4.55-1.45Z" />
      <path d="m18.5 15 .7 2.3 2.3.7-2.3.7-.7 2.3-.7-2.3-2.3-.7 2.3-.7Z" />
    </Icon>
  );
}
