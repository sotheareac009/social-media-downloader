import { useEffect, useState } from "react";
import type { ConverterRoute } from "@/App";
import { BoltIcon, ScissorsIcon, SlidersIcon } from "@/components/ui/icons";
import { ConvertTab } from "@/pages/convert/ConvertTab";
import { MergeTab } from "@/pages/convert/MergeTab";
import { SplitTab } from "@/pages/convert/SplitTab";

type Tab = "convert" | "split" | "merge";

/** The sidebar entry each tab answers to. */
const ROUTE_FOR: Record<Tab, ConverterRoute> = {
  convert: "convert",
  split: "convert-split",
  merge: "convert-merge",
};

const TAB_FOR: Record<ConverterRoute, Tab> = {
  convert: "convert",
  "convert-split": "split",
  "convert-merge": "merge",
};

/**
 * The three things people do to a file after downloading it: reshape a folder
 * of them, cut one long one up, or join several together.
 *
 * They are separate tabs rather than one screen because they take different
 * inputs — a folder, a single file, an ordered list — and mixing them would
 * mean a table where most columns are blank for most rows.
 *
 * Each tab is mounted on first visit and then kept alive, hidden rather than
 * unmounted: a tab holds real work — a scanned table, a running batch, a
 * finished result — and switching away must not throw it out. Only the visible
 * one listens for file drops, which is the one thing that genuinely cannot be
 * shared.
 *
 * Only the active tab is mounted: both listen for native file drops, and two
 * live listeners would each act on the same drop.
 */
export function ConverterPage({
  route,
  onNavigate,
}: {
  route: ConverterRoute;
  onNavigate: (route: ConverterRoute) => void;
}) {
  // The route is the source of truth, so the sidebar and the tab strip can
  // never disagree about which tool is open.
  const tab = TAB_FOR[route];
  const setTab = (next: Tab) => onNavigate(ROUTE_FOR[next]);
  // Which tabs have ever been opened. Mounting all three up front would run
  // three FFmpeg capability probes at once for screens nobody has looked at.
  const [visited, setVisited] = useState<Set<Tab>>(new Set(["convert"]));
  useEffect(() => {
    setVisited((prev) => (prev.has(tab) ? prev : new Set(prev).add(tab)));
  }, [tab]);

  return (
    <div className="page">
      <header className="page__header rise">
        <span className="page__eyebrow">
          <SlidersIcon size={12} />
          Converter
        </span>
        <h1 className="page__title">
          Media <span className="up-accent">converter</span>
        </h1>
        <p className="page__lede">
          Reshape a whole folder of videos and photos, cut one long recording
          into equal parts, or join clips into one.
        </p>
      </header>

      <div className="tabs rise" role="tablist">
        <button
          className={`tab ${tab === "convert" ? "tab--active" : ""}`.trim()}
          type="button"
          role="tab"
          aria-selected={tab === "convert"}
          onClick={() => setTab("convert")}
        >
          <SlidersIcon size={14} />
          Convert
        </button>
        <button
          className={`tab ${tab === "split" ? "tab--active" : ""}`.trim()}
          type="button"
          role="tab"
          aria-selected={tab === "split"}
          onClick={() => setTab("split")}
        >
          <ScissorsIcon size={14} />
          Split
        </button>
        <button
          className={`tab ${tab === "merge" ? "tab--active" : ""}`.trim()}
          type="button"
          role="tab"
          aria-selected={tab === "merge"}
          onClick={() => setTab("merge")}
        >
          <BoltIcon size={14} />
          Merge
        </button>
      </div>

      {visited.has("convert") && (
        <div hidden={tab !== "convert"}>
          <ConvertTab active={tab === "convert"} />
        </div>
      )}
      {visited.has("split") && (
        <div hidden={tab !== "split"}>
          <SplitTab active={tab === "split"} />
        </div>
      )}
      {visited.has("merge") && (
        <div hidden={tab !== "merge"}>
          <MergeTab active={tab === "merge"} />
        </div>
      )}
    </div>
  );
}
