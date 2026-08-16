'use client';

import { AsyncSection, DataTable, StatusCard } from '@/components';
import { getRogueStatus } from '@/lib/api';
import type { RogueDhcpServer } from '@/lib/types';
import type { ColumnDef } from '@tanstack/react-table';

const columns: ColumnDef<RogueDhcpServer, unknown>[] = [
  { accessorKey: 'ip', header: 'IP' },
  { accessorKey: 'mac', header: 'MAC' },
  { accessorKey: 'interface', header: 'Interface' },
  { accessorKey: 'detectedAt', header: 'Detected' },
];

export default function DhcpRoguePage() {
  return (
    <div>
      <h1 className="mb-4 text-2xl font-bold">Rogue DHCP Detection</h1>
      <AsyncSection fetcher={getRogueStatus} pollMs={5000}>
        {(status) => (
          <div className="space-y-6">
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
              <StatusCard
                title="Detection"
                value={status.enabled ? 'Enabled' : 'Disabled'}
                state={status.enabled ? 'healthy' : 'warning'}
              />
              <StatusCard
                title="Rogue Servers"
                value={status.rogues.length}
                state={status.rogues.length > 0 ? 'error' : 'healthy'}
              />
              <StatusCard
                title="Last Scan"
                value={new Date(status.lastScan).toLocaleString()}
              />
            </div>
            <section>
              <h2 className="mb-2 text-lg font-semibold">Detected Rogue Servers</h2>
              <DataTable
                columns={columns}
                data={status.rogues}
                emptyMessage="No rogue DHCP servers detected"
              />
            </section>
          </div>
        )}
      </AsyncSection>
    </div>
  );
}
