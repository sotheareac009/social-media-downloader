import { useState } from "react";
import { ScissorsIcon, SlidersIcon } from "@/components/ui/icons";
import { ConvertTab } from "@/pages/convert/ConvertTab";
import { SplitTab } from "@/pages/convert/SplitTab";

type Tab = "convert" | "split";

/**
 * The two things people do to a file after downloading it: reshape a folder of
 * them, or cut one long one up.
 *
 * They are separate tabs rather than one screen because they take different
 * inputs — a folder versus a single file — and mixing them would mean a table
 * where most columns are blank for most rows.
 *
 * Only the active tab is mounted: both listen for native file drops, and two
 * live listeners would each act on the same drop.
 */
export function ConverterPage() {
  const [tab, setTab] = useState<Tab>("convert");

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
          Reshape a whole folder of videos and photos, or cut one long recording
          into equal parts.
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
      </div>

      {tab === "convert" ? <ConvertTab /> : <SplitTab />}
    </div>
  );
}
