'use client';

import type { ReactNode } from 'react';
import { useEffect, useState } from 'react';

interface AsyncSectionProps<T> {
  /** Fetcher that returns the data to render. */
  fetcher: () => Promise<T>;
  /** Render the loaded data. */
  children: (data: T) => ReactNode;
  /** Rendered while loading (initial load). */
  loading?: ReactNode;
  /** Auto-refresh interval in milliseconds (0 disables). */
  pollMs?: number;
  /** deps to refetch on change. */
  deps?: unknown[];
}

/**
 * Wraps an async data source with loading/error states and optional polling.
 * Used by every page to keep data-fetching boilerplate minimal.
 */
export function AsyncSection<T>({
  fetcher,
  children,
  loading,
  pollMs = 0,
  deps = [],
}: AsyncSectionProps<T>) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [loadingState, setLoadingState] = useState(true);

  useEffect(() => {
    let cancelled = false;
    const run = async () => {
      try {
        const result = await fetcher();
        if (cancelled) return;
        setData(result);
        setError(null);
      } catch (err) {
        if (cancelled) return;
        setError(err instanceof Error ? err : new Error(String(err)));
      } finally {
        if (!cancelled) setLoadingState(false);
      }
    };
    run();
    let interval: ReturnType<typeof setInterval> | undefined;
    if (pollMs > 0) {
      interval = setInterval(run, pollMs);
    }
    return () => {
      cancelled = true;
      if (interval) clearInterval(interval);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  if (loadingState && data === null) {
    return <>{loading ?? <div className="text-gray-500">Loading…</div>}</>;
  }
  if (error) {
    return (
      <div className="rounded border border-rose-400 bg-rose-50 p-4 text-rose-700 dark:bg-rose-900/30 dark:text-rose-300">
        Error: {error.message}
      </div>
    );
  }
  if (data === null) return null;
  return <>{children(data)}</>;
}
