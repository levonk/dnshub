'use client';

import { AsyncSection, DataTable, RefreshButton } from '@/components';
import { getRelayAgents, getRelayOption82 } from '@/lib/api';
import type { Option82Mapping, RelayAgent } from '@/lib/types';
import type { ColumnDef } from '@tanstack/react-table';
import { useState } from 'react';

const agentColumns: ColumnDef<RelayAgent, unknown>[] = [
  { accessorKey: 'ip', header: 'IP' },
  { accessorKey: 'interface', header: 'Interface' },
  {
    id: 'trusted',
    header: 'Trusted',
    cell: ({ row }) => (row.original.trusted ? 'yes' : 'no'),
  },
  { accessorKey: 'status', header: 'Status' },
];

const option82Columns: ColumnDef<Option82Mapping, unknown>[] = [
  { accessorKey: 'circuitId', header: 'Circuit ID' },
  { accessorKey: 'profile', header: 'Profile' },
];

export default function DhcpRelayPage() {
  const [version, setVersion] = useState(0);
  return (
    <div>
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-2xl font-bold">DHCP Relay</h1>
        <RefreshButton onRefresh={() => setVersion((v) => v + 1)} />
      </div>
      <section className="mb-8">
        <h2 className="mb-2 text-lg font-semibold">Trusted Relay Agents</h2>
        <AsyncSection fetcher={getRelayAgents} deps={[version]}>
          {(res) => (
            <DataTable
              columns={agentColumns}
              data={res.agents}
              emptyMessage="No relay agents"
            />
          )}
        </AsyncSection>
      </section>
      <section>
        <h2 className="mb-2 text-lg font-semibold">Option 82 Mappings</h2>
        <AsyncSection fetcher={getRelayOption82} deps={[version]}>
          {(res) => (
            <DataTable
              columns={option82Columns}
              data={res.mappings}
              emptyMessage="No Option 82 mappings"
            />
          )}
        </AsyncSection>
      </section>
    </div>
  );
}
