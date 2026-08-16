'use client';

import { AsyncSection, DataTable, RefreshButton, StatusCard } from '@/components';
import { getDhcpLeases, getDhcpStatic, releaseLease } from '@/lib/api';
import type { DhcpLease, StaticLease } from '@/lib/types';
import type { ColumnDef } from '@tanstack/react-table';
import { useState } from 'react';

const leaseColumns = (
  onRelease: (lease: DhcpLease) => void,
): ColumnDef<DhcpLease, unknown>[] => [
  { accessorKey: 'ip', header: 'IP' },
  { accessorKey: 'mac', header: 'MAC' },
  { accessorKey: 'hostname', header: 'Hostname' },
  {
    accessorKey: 'family',
    header: 'Family',
    cell: ({ getValue }) => `v${getValue() as number}`,
  },
  { accessorKey: 'leaseStart', header: 'Start' },
  { accessorKey: 'leaseEnd', header: 'End' },
  {
    id: 'static',
    header: 'Static',
    cell: ({ row }) => (row.original.static ? 'yes' : 'no'),
  },
  {
    id: 'actions',
    header: '',
    cell: ({ row }) => (
      <button
        type="button"
        className="rounded border px-2 py-0.5 text-xs text-rose-600 hover:bg-rose-50"
        onClick={() => onRelease(row.original)}
      >
        Release
      </button>
    ),
  },
];

const staticColumns: ColumnDef<StaticLease, unknown>[] = [
  { accessorKey: 'mac', header: 'MAC' },
  { accessorKey: 'ip', header: 'IP' },
  { accessorKey: 'hostname', header: 'Hostname' },
];

export default function DhcpLeasesPage() {
  const [version, setVersion] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const handleRelease = async (lease: DhcpLease) => {
    setError(null);
    try {
      await releaseLease({ ip: lease.ip });
      setVersion((v) => v + 1);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div>
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-2xl font-bold">DHCP Leases</h1>
        <RefreshButton onRefresh={() => setVersion((v) => v + 1)} />
      </div>
      {error ? (
        <div className="mb-4 rounded border border-rose-400 bg-rose-50 p-3 text-rose-700">
          {error}
        </div>
      ) : null}

      <AsyncSection fetcher={getDhcpLeases} deps={[version]}>
        {(res) => {
          const v4 = res.leases.filter((l) => l.family === 4);
          const v6 = res.leases.filter((l) => l.family === 6);
          return (
            <div className="space-y-6">
              <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
                <StatusCard title="Total Leases" value={res.leases.length} />
                <StatusCard title="IPv4" value={v4.length} />
                <StatusCard title="IPv6" value={v6.length} />
              </div>
              <section>
                <h2 className="mb-2 text-lg font-semibold">Active Leases</h2>
                <DataTable
                  columns={leaseColumns(handleRelease)}
                  data={res.leases}
                  emptyMessage="No active leases"
                />
              </section>
            </div>
          );
        }}
      </AsyncSection>

      <section className="mt-8">
        <h2 className="mb-2 text-lg font-semibold">Static Assignments</h2>
        <AsyncSection fetcher={getDhcpStatic} deps={[version]}>
          {(res) => (
            <DataTable
              columns={staticColumns}
              data={res.statics}
              emptyMessage="No static leases"
            />
          )}
        </AsyncSection>
      </section>
    </div>
  );
}
