'use client';

import type { ReactNode } from 'react';
import { useState } from 'react';

interface RefreshButtonProps {
  onRefresh: () => Promise<void> | void;
  label?: string;
  children?: ReactNode;
}

export function RefreshButton({ onRefresh, label = 'Refresh', children }: RefreshButtonProps) {
  const [loading, setLoading] = useState(false);
  const handle = async () => {
    setLoading(true);
    try {
      await onRefresh();
    } finally {
      setLoading(false);
    }
  };
  return (
    <button
      type="button"
      className="rounded border px-3 py-1 text-sm disabled:opacity-40"
      onClick={handle}
      disabled={loading}
    >
      {loading ? 'Refreshing…' : label}
      {children}
    </button>
  );
}
