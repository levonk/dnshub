'use client';

import { AsyncSection, DataTable, FilterBar, RefreshButton } from '@/components';
import { exportQueryLog, getQueryLog } from '@/lib/api';
import type { QueryLogEntry, QueryLogFilters, QueryLogResponse } from '@/lib/types';
import type { ColumnDef } from '@tanstack/react-table';
import { useCallback, useMemo, useState } from 'react';

const POLL_MS = 2000;
const PAGE_SIZE = 50;

const columns: ColumnDef<QueryLogEntry, unknown>[] = [
  { accessorKey: 'time', header: 'Time' },
  { accessorKey: 'client', header: 'Client' },
  { accessorKey: 'domain', header: 'Domain' },
  { accessorKey: 'type', header: 'Type' },
  { accessorKey: 'status', header: 'Status' },
  {
    id: 'blocked',
    header: 'Blocked',
    cell: ({ row }) =>
      row.original.blocked ? (
        <span className="text-rose-600">yes</span>
      ) : (
        <span className="text-emerald-600">no</span>
      ),
  },
  { accessorKey: 'category', header: 'Category' },
  { accessorKey: 'tier', header: 'Tier' },
  {
    id: 'latencyMs',
    header: 'Latency',
    cell: ({ row }) => `${row.original.latencyMs}ms`,
  },
];

export default function QueryLogPage() {
  const [filters, setFilters] = useState<Record<string, string | boolean | undefined>>({});
  const [applied, setApplied] = useState<QueryLogFilters>({});
  const [page, setPage] = useState(1);
  const [polling, setPolling] = useState(true);

  const activeFilters = useMemo<QueryLogFilters>(
    () => ({ ...applied, page, limit: PAGE_SIZE }),
    [applied, page],
  );

  const fetcher = useCallback(() => getQueryLog(activeFilters), [activeFilters]);

  const handleApply = () => {
    setApplied({
      client: typeof filters.client === 'string' ? filters.client : undefined,
      blocked: filters.blocked === true ? true : undefined,
      category: typeof filters.category === 'string' ? filters.category : undefined,
      domain: typeof filters.domain === 'string' ? filters.domain : undefined,
    });
    setPage(1);
  };

  const handleReset = () => {
    setFilters({});
    setApplied({});
    setPage(1);
  };

  const handleExport = async () => {
    const blob = await exportQueryLog(applied);
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'query-log.csv';
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div>
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-2xl font-bold">Query Log</h1>
        <button
          type="button"
          className="rounded border px-3 py-1 text-sm"
          onClick={() => setPolling((p) => !p)}
        >
          {polling ? 'Pause Live' : 'Resume Live'}
        </button>
        <RefreshButton onRefresh={() => setPage((p) => p)} label="Refresh" />
        <button
          type="button"
          className="rounded border px-3 py-1 text-sm"
          onClick={handleExport}
        >
          Export CSV
        </button>
      </div>

      <FilterBar
        fields={[
          { name: 'client', label: 'Client', placeholder: '192.168.1.20' },
          { name: 'domain', label: 'Domain', placeholder: 'example.com' },
          {
            name: 'category',
            label: 'Category',
            type: 'select',
            options: ['ads', 'social', 'malware', 'tracker', 'adult'],
          },
        ]}
        values={filters}
        onChange={(name, value) => setFilters((f) => ({ ...f, [name]: value }))}
        onApply={handleApply}
        onReset={handleReset}
      >
        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            checked={filters.blocked === true}
            onChange={(e) =>
              setFilters((f) => ({ ...f, blocked: e.target.checked }))
            }
          />
          Blocked only
        </label>
      </FilterBar>

      <AsyncSection<QueryLogResponse>
        fetcher={fetcher}
        pollMs={polling ? POLL_MS : 0}
        deps={[activeFilters, polling]}
      >
        {(res) => (
          <div className="space-y-3">
            <div className="flex items-center gap-4 text-sm text-gray-500">
              <span>{res.total} total</span>
              <span>
                Page {res.page} of {Math.max(1, Math.ceil(res.total / res.limit))}
              </span>
              <div className="ml-auto flex gap-2">
                <button
                  type="button"
                  className="rounded border px-2 py-0.5 disabled:opacity-40"
                  onClick={() => setPage((p) => Math.max(1, p - 1))}
                  disabled={page <= 1}
                >
                  ‹ Prev
                </button>
                <button
                  type="button"
                  className="rounded border px-2 py-0.5 disabled:opacity-40"
                  onClick={() => setPage((p) => p + 1)}
                  disabled={res.entries.length < res.limit}
                >
                  Next ›
                </button>
              </div>
            </div>
            <DataTable
              columns={columns}
              data={res.entries}
              pageSize={PAGE_SIZE}
              emptyMessage="No queries match the current filters"
            />
          </div>
        )}
      </AsyncSection>
    </div>
  );
}
