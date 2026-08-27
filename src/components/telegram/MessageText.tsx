import { Fragment, useState, type ReactNode } from "react";
import type { TelegramSpan } from "@/lib/telegram";

/**
 * Render a message's text with Telegram's own formatting.
 *
 * Built as React nodes rather than an HTML string: the text comes from a
 * channel, and handing it to `dangerouslySetInnerHTML` would let a post write
 * markup into the app.
 *
 * Telegram states entity offsets in UTF-16 code units, which is exactly how
 * JavaScript indexes a string, so they are used as-is. Spans may nest (bold
 * inside a link) but never partially overlap, so a simple recursive walk
 * reproduces them faithfully.
 */
export function MessageText({ text, spans }: { text: string; spans: TelegramSpan[] }) {
  if (text === "") return null;
  return <div className="tgmsg__text">{build(text, spans, 0, text.length)}</div>;
}

function build(text: string, spans: TelegramSpan[], from: number, to: number): ReactNode[] {
  // Only spans that start inside this range, longest first, so a nested span is
  // handled by the recursion rather than competing with its parent.
  const here = spans
    .filter((s) => s.offset >= from && s.offset + s.length <= to)
    .sort((a, b) => a.offset - b.offset || b.length - a.length);

  const out: ReactNode[] = [];
  let cursor = from;

  for (let i = 0; i < here.length; i++) {
    const span = here[i];
    if (span.offset < cursor) continue; // already consumed by an outer span
    if (span.offset > cursor) {
      out.push(<Fragment key={`t${cursor}`}>{text.slice(cursor, span.offset)}</Fragment>);
    }
    const end = span.offset + span.length;
    const inner = build(
      text,
      here.filter((s) => s !== span),
      span.offset,
      end,
    );
    out.push(<Styled key={`s${span.offset}-${span.type}`} span={span} text={text} children={inner} />);
    cursor = end;
  }

  if (cursor < to) {
    out.push(<Fragment key={`t${cursor}`}>{text.slice(cursor, to)}</Fragment>);
  }
  return out;
}

function Styled({
  span,
  text,
  children,
}: {
  span: TelegramSpan;
  text: string;
  children: ReactNode;
}) {
  switch (span.type) {
    case "bold":
      return <strong>{children}</strong>;
    case "italic":
      return <em>{children}</em>;
    case "underline":
      return <u>{children}</u>;
    case "strike":
      return <s>{children}</s>;
    case "code":
      return <code className="tgmsg__code">{children}</code>;
    case "pre":
      return <pre className="tgmsg__pre">{children}</pre>;
    case "spoiler":
      return <Spoiler>{children}</Spoiler>;
    case "hashtag":
    case "mention":
      return <span className="tgmsg__ref">{children}</span>;
    case "link": {
      const href = span.url ?? text.substr(span.offset, span.length);
      return (
        // Opens in the system browser rather than navigating the app away from
        // itself, which a webview would otherwise happily do.
        <a
          className="tgmsg__link"
          href={href}
          target="_blank"
          rel="noreferrer noopener"
        >
          {children}
        </a>
      );
    }
    default:
      return <>{children}</>;
  }
}

/** Telegram's hidden text: blurred until clicked, exactly once. */
function Spoiler({ children }: { children: ReactNode }) {
  const [shown, setShown] = useState(false);
  return (
    <span
      className={`tgmsg__spoiler ${shown ? "tgmsg__spoiler--shown" : ""}`.trim()}
      onClick={() => setShown(true)}
      role={shown ? undefined : "button"}
      title={shown ? undefined : "Show hidden text"}
    >
      {children}
    </span>
  );
}
