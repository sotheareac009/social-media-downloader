type Tone = "success" | "warning" | "danger" | "muted" | "active";

/**
 * The status dot used by devices, accounts and jobs alike.
 *
 * One component so a "connected" account and an "online" device can never end
 * up different shades of green in the same list.
 */
export function StatusDot({ tone, pulse = false }: { tone: Tone; pulse?: boolean }) {
  return (
    <span
      className={`sdot sdot--${tone} ${pulse ? "sdot--pulse" : ""}`.trim()}
      aria-hidden
    />
  );
}

export function StatusBadge({
  tone,
  children,
  pulse = false,
}: {
  tone: Tone;
  children: React.ReactNode;
  pulse?: boolean;
}) {
  return (
    <span className={`sbadge sbadge--${tone}`}>
      <StatusDot tone={tone} pulse={pulse} />
      {children}
    </span>
  );
}
