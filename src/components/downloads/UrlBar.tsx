import { useLayoutEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { Button } from "@/components/ui/Button";
import { DownloadIcon, LinkIcon } from "@/components/ui/icons";

/**
 * The paste box.
 *
 * A textarea rather than an input because people paste lists — a single-line
 * input silently strips newlines and turns two links into one unparseable
 * string. Enter submits (the common case is one link); Shift+Enter adds a line.
 *
 * Validation is intentionally *not* duplicated here: the Rust side owns the
 * host allowlist, and a second copy in TypeScript would drift from it. This
 * only splits the text and drops obvious duplicates.
 */
export function UrlBar({
  onSubmit,
  busy,
  disabled,
}: {
  onSubmit: (urls: string[]) => void;
  busy: boolean;
  disabled: boolean;
}) {
  const [value, setValue] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);

  // Grow with the content instead of scrolling inside a two-row box, so a
  // pasted list is visible in full before it's submitted.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 168)}px`;
  }, [value]);

  const urls = splitUrls(value);

  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    if (urls.length === 0 || busy || disabled) return;
    onSubmit(urls);
    setValue("");
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };

  return (
    <form className="urlbar" onSubmit={submit}>
      <span className="urlbar__icon">
        <LinkIcon size={15} />
      </span>
      <textarea
        ref={ref}
        className="urlbar__input"
        rows={1}
        autoComplete="off"
        spellCheck={false}
        placeholder="Paste one or more Facebook or TikTok links — one per line"
        aria-label="Video links"
        value={value}
        disabled={disabled}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={onKeyDown}
      />
      <Button
        type="submit"
        loading={busy}
        disabled={disabled || urls.length === 0}
        icon={<DownloadIcon size={15} />}
      >
        {urls.length > 1 ? `Download ${urls.length}` : "Download"}
      </Button>
    </form>
  );
}

/**
 * Pull the links out of pasted text.
 *
 * Split on any whitespace or comma, so a newline-separated list, a
 * space-separated pair and a comma-separated one all work. Duplicates within a
 * single paste are dropped — pasting the same link twice is a slip, not a
 * request for two copies of the file.
 */
export function splitUrls(raw: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const token of raw.split(/[\s,]+/)) {
    const t = token.trim();
    if (!t || seen.has(t)) continue;
    seen.add(t);
    out.push(t);
  }
  return out;
}
