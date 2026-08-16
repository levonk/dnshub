'use client';

import { AsyncSection, DataTable, RefreshButton } from '@/components';
import { getBootpStatic, getPxeBootfiles } from '@/lib/api';
import type { BootpStaticEntry, PxeBootfile } from '@/lib/types';
import type { ColumnDef } from '@tanstack/react-table';
import { useState } from 'react';

const bootfileColumns: ColumnDef<PxeBootfile, unknown>[] = [
  { accessorKey: 'arch', header: 'Architecture' },
  { accessorKey: 'bootfile', header: 'Bootfile' },
];

const bootpColumns: ColumnDef<BootpStaticEntry, unknown>[] = [
  { accessorKey: 'mac', header: 'MAC' },
  { accessorKey: 'ip', header: 'IP' },
  { accessorKey: 'bootfile', header: 'Bootfile' },
];

export default function DhcpPxePage() {
  const [version, setVersion] = useState(0);
  return (
    <div>
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-2xl font-bold">PXE / BOOTP</h1>
        <RefreshButton onRefresh={() => setVersion((v) => v + 1)} />
      </div>
      <section className="mb-8">
        <h2 className="mb-2 text-lg font-semibold">Bootfile Mappings</h2>
        <AsyncSection fetcher={getPxeBootfiles} deps={[version]}>
          {(res) => (
            <DataTable
              columns={bootfileColumns}
              data={res.bootfiles}
              emptyMessage="No bootfile mappings"
            />
          )}
        </AsyncSection>
      </section>
      <section>
        <h2 className="mb-2 text-lg font-semibold">Static BOOTP Entries</h2>
        <AsyncSection fetcher={getBootpStatic} deps={[version]}>
          {(res) => (
            <DataTable
              columns={bootpColumns}
              data={res.entries}
              emptyMessage="No static BOOTP entries"
            />
          )}
        </AsyncSection>
      </section>
    </div>
  );
}
