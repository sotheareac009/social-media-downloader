import { ChevronRightIcon, ArrowLeftIcon } from "@/components/ui/icons";

/**
 * Page controls for a long queue.
 *
 * Confirming one playlist can put 133 rows on the page, each with its own
 * progress bar re-rendering several times a second. Paging keeps the DOM small
 * and, more to the point, makes the queue readable — scrolling past a hundred
 * cards to reach the one that failed is not a queue, it's a wall.
 *
 * The range is stated in full ("21–40 of 133") rather than as a page number
 * alone, so it answers "where am I" without arithmetic.
 */
export function QueuePager({
  page,
  pageCount,
  from,
  to,
  total,
  onPage,
}: {
  /** 0-based. */
  page: number;
  pageCount: number;
  /** 1-based, inclusive — the first row shown. */
  from: number;
  /** 1-based, inclusive — the last row shown. */
  to: number;
  total: number;
  onPage: (page: number) => void;
}) {
  return (
    <nav className="pager" aria-label="Queue pages">
      <span className="pager__range">
        {from}–{to} of {total}
      </span>
      <div className="pager__controls">
        <button
          className="btn btn--ghost btn--sm"
          type="button"
          onClick={() => onPage(page - 1)}
          disabled={page === 0}
          aria-label="Previous page"
        >
          <ArrowLeftIcon size={13} />
          Previous
        </button>
        <span className="pager__pos">
          Page <strong>{page + 1}</strong> of {pageCount}
        </span>
        <button
          className="btn btn--ghost btn--sm"
          type="button"
          onClick={() => onPage(page + 1)}
          disabled={page + 1 >= pageCount}
          aria-label="Next page"
        >
          Next
          <ChevronRightIcon size={13} />
        </button>
      </div>
    </nav>
  );
}
