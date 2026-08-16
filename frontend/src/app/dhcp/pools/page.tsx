'use client';

import { AsyncSection, DataTable, RefreshButton } from '@/components';
import { getPools } from '@/lib/api';
import type { Pool } from '@/lib/types';
import type { ColumnDef } from '@tanstack/react-table';
import { useState } from 'react';

const columns: ColumnDef<Pool, unknown>[] = [
  { accessorKey: 'name', header: 'Name' },
  { accessorKey: 'network', header: 'Network' },
  { accessorKey: 'rangeStart', header: 'Start' },
  { accessorKey: 'rangeEnd', header: 'End' },
  {
    accessorKey: 'family',
    header: 'Family',
    cell: ({ getValue }) => `v${getValue() as number}`,
  },
  {
    id: 'relayed',
    header: 'Relayed',
    cell: ({ row }) => (row.original.relayed ? 'yes' : 'no'),
  },
  {
    id: 'vlan',
    header: 'VLAN',
    cell: ({ row }) => row.original.vlan ?? '-',
  },
];

export default function DhcpPoolsPage() {
  const [version, setVersion] = useState(0);
  return (
    <div>
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-2xl font-bold">DHCP Pools</h1>
        <RefreshButton onRefresh={() => setVersion((v) => v + 1)} />
      </div>
      <AsyncSection fetcher={getPools} deps={[version]}>
        {(res) => (
          <DataTable columns={columns} data={res.pools} emptyMessage="No pools configured" />
        )}
      </AsyncSection>
    </div>
  );
}
