import { useEffect, useState } from "react";
import { uploadVideoThumbnail } from "@/lib/upload";

/**
 * A poster frame for a local video file.
 *
 * Every thumbnail is one FFmpeg process, so two things stop a list of forty
 * clips from launching forty of them at once:
 *
 *   * a cache keyed by path, because rows re-render constantly while a batch
 *     runs and the frame never changes;
 *   * a small queue, because the point of a thumbnail is to make the list
 *     readable, not to compete with the conversion for cores.
 */
const cache = new Map<string, string | null>();

/** Paths waiting for a slot, oldest first. */
const queue: (() => void)[] = [];
let running = 0;
const MAX_CONCURRENT = 3;

function pump() {
  while (running < MAX_CONCURRENT && queue.length > 0) {
    const next = queue.shift();
    if (!next) return;
    running++;
    next();
  }
}

/** Resolve a poster frame, at most `MAX_CONCURRENT` at a time. */
function loadThumb(path: string): Promise<string | null> {
  const hit = cache.get(path);
  if (hit !== undefined) return Promise.resolve(hit);

  return new Promise((resolve) => {
    queue.push(() => {
      uploadVideoThumbnail(path)
        .then((data) => {
          cache.set(path, data);
          resolve(data);
        })
        // A frame that cannot be read is not an error worth showing — the row
        // simply keeps its placeholder.
        .catch(() => {
          cache.set(path, null);
          resolve(null);
        })
        .finally(() => {
          running--;
          pump();
        });
    });
    pump();
  });
}

export function VideoThumb({
  path,
  className = "thumb",
}: {
  path: string;
  className?: string;
}) {
  const [src, setSrc] = useState<string | null>(() => cache.get(path) ?? null);

  useEffect(() => {
    let alive = true;
    // Straight from the cache when it is there, so a re-render never flickers
    // back to the placeholder.
    const cached = cache.get(path);
    if (cached !== undefined) {
      setSrc(cached);
      return;
    }
    setSrc(null);
    void loadThumb(path).then((data) => {
      if (alive) setSrc(data);
    });
    return () => {
      alive = false;
    };
  }, [path]);

  return (
    <span className={className}>
      {src ? (
        // Decorative: the filename is right beside it in every use.
        <img src={src} alt="" loading="lazy" />
      ) : (
        <span className="thumb__ph" />
      )}
    </span>
  );
}
