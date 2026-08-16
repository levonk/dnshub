'use client';

import { AsyncSection, DataTable, RefreshButton, StatusCard } from '@/components';
import { getBlocklistSources, refreshBlocklists } from '@/lib/api';
import type { BlocklistSource } from '@/lib/types';
import type { ColumnDef } from '@tanstack/react-table';
import { useState } from 'react';

const stateFor = (status: BlocklistSource['status']): 'healthy' | 'warning' | 'error' => {
  if (status === 'healthy') return 'healthy';
  if (status === 'stale') return 'warning';
  return 'error';
};

const columns: ColumnDef<BlocklistSource, unknown>[] = [
  { accessorKey: 'name', header: 'Name' },
  { accessorKey: 'category', header: 'Category' },
  { accessorKey: 'url', header: 'URL' },
  {
    accessorKey: 'status',
    header: 'Status',
    cell: ({ getValue }) => {
      const v = getValue() as string;
      const color =
        v === 'healthy'
          ? 'text-emerald-600'
          : v === 'stale'
            ? 'text-amber-600'
            : 'text-rose-600';
      return <span className={color}>{v}</span>;
    },
  },
  {
    id: 'entries',
    header: 'Entries',
    cell: ({ row }) => row.original.entries.toLocaleString(),
  },
  {
    id: 'lastRefresh',
    header: 'Last Refresh',
    cell: ({ row }) => new Date(row.original.lastRefresh).toLocaleString(),
  },
  { accessorKey: 'lastError', header: 'Last Error' },
];

export default function BlocklistsPage() {
  const [version, setVersion] = useState(0);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshMsg, setRefreshMsg] = useState<string | null>(null);

  const handleRefresh = async () => {
    setRefreshing(true);
    setRefreshMsg(null);
    try {
      const result = await refreshBlocklists();
      setRefreshMsg(`Refreshed ${result.refreshed} sources (${result.errors} errors)`);
      setVersion((v) => v + 1);
    } catch (err) {
      setRefreshMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <div>
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-2xl font-bold">Blocklists</h1>
        <button
          type="button"
          className="rounded bg-brand-600 px-3 py-1 text-sm text-white disabled:opacity-40"
          onClick={handleRefresh}
          disabled={refreshing}
        >
          {refreshing ? 'Refreshing…' : 'Refresh All'}
        </button>
        <RefreshButton onRefresh={() => setVersion((v) => v + 1)} label="Reload" />
      </div>
      {refreshMsg ? (
        <div className="mb-4 rounded border border-gray-300 bg-gray-50 p-3 text-sm dark:border-gray-700 dark:bg-gray-800">
          {refreshMsg}
        </div>
      ) : null}
      <AsyncSection fetcher={getBlocklistSources} deps={[version]}>
        {(res) => {
          const total = res.sources.reduce((sum, s) => sum + s.entries, 0);
          const healthy = res.sources.filter((s) => s.status === 'healthy').length;
          return (
            <div className="space-y-6">
              <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
                <StatusCard title="Sources" value={res.sources.length} />
                <StatusCard title="Healthy" value={healthy} state="healthy" />
                <StatusCard title="Total Entries" value={total.toLocaleString()} />
              </div>
              <DataTable
                columns={columns}
                data={res.sources}
                emptyMessage="No blocklist sources configured"
              />
            </div>
          );
        }}
      </AsyncSection>
    </div>
  );
}
