'use client';

import { AsyncSection, DataTable, FilterBar } from '@/components';
import { getAuditLog } from '@/lib/api';
import type { AuditEvent } from '@/lib/types';
import type { ColumnDef } from '@tanstack/react-table';
import { useState } from 'react';

const columns: ColumnDef<AuditEvent, unknown>[] = [
  { accessorKey: 'timestamp', header: 'Time' },
  { accessorKey: 'mac', header: 'MAC' },
  { accessorKey: 'ip', header: 'IP' },
  { accessorKey: 'hostname', header: 'Hostname' },
  { accessorKey: 'event', header: 'Event' },
];

export default function DhcpAuditPage() {
  const [filters, setFilters] = useState<Record<string, string | boolean | undefined>>({});
  const [applied, setApplied] = useState<Record<string, string | boolean | undefined>>({});

  return (
    <div>
      <h1 className="mb-4 text-2xl font-bold">DHCP Audit Log</h1>
      <FilterBar
        fields={[
          { name: 'mac', label: 'MAC', placeholder: 'aa:bb:cc:..' },
          { name: 'event', label: 'Event', placeholder: 'join / leave' },
          { name: 'from', label: 'From', placeholder: 'ISO timestamp' },
          { name: 'to', label: 'To', placeholder: 'ISO timestamp' },
        ]}
        values={filters}
        onChange={(name, value) => setFilters((f) => ({ ...f, [name]: value }))}
        onApply={() => setApplied({ ...filters })}
        onReset={() => {
          setFilters({});
          setApplied({});
        }}
      />
      <AsyncSection
        fetcher={() =>
          getAuditLog({
            mac: typeof applied.mac === 'string' ? applied.mac : undefined,
            event: typeof applied.event === 'string' ? applied.event : undefined,
            from: typeof applied.from === 'string' ? applied.from : undefined,
            to: typeof applied.to === 'string' ? applied.to : undefined,
          })
        }
        deps={[applied]}
      >
        {(res) => (
          <div className="space-y-2">
            <div className="text-sm text-gray-500">{res.total} events</div>
            <DataTable columns={columns} data={res.events} emptyMessage="No audit events" />
          </div>
        )}
      </AsyncSection>
    </div>
  );
}
