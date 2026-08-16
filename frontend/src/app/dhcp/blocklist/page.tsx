'use client';

import { AsyncSection, DataTable, RefreshButton } from '@/components';
import { addMacBlock, deleteMacBlock, getMacBlocklist } from '@/lib/api';
import type { MacBlockEntry } from '@/lib/types';
import type { ColumnDef } from '@tanstack/react-table';
import { useState } from 'react';

export default function DhcpBlocklistPage() {
  const [version, setVersion] = useState(0);
  const [pattern, setPattern] = useState('');
  const [kind, setKind] = useState<'mac' | 'oui'>('mac');
  const [reason, setReason] = useState('');
  const [error, setError] = useState<string | null>(null);

  const handleAdd = async () => {
    setError(null);
    try {
      await addMacBlock({ pattern, kind, reason, addedAt: new Date().toISOString() });
      setPattern('');
      setReason('');
      setVersion((v) => v + 1);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const columns: ColumnDef<MacBlockEntry, unknown>[] = [
    { accessorKey: 'pattern', header: 'Pattern' },
    { accessorKey: 'kind', header: 'Kind' },
    { accessorKey: 'reason', header: 'Reason' },
    { accessorKey: 'addedAt', header: 'Added' },
    {
      id: 'actions',
      header: '',
      cell: ({ row }) => (
        <button
          type="button"
          className="rounded border px-2 py-0.5 text-xs text-rose-600 hover:bg-rose-50"
          onClick={async () => {
            await deleteMacBlock(row.original);
            setVersion((v) => v + 1);
          }}
        >
          Remove
        </button>
      ),
    },
  ];

  return (
    <div>
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-2xl font-bold">DHCP MAC Blocklist</h1>
        <RefreshButton onRefresh={() => setVersion((v) => v + 1)} />
      </div>
      {error ? (
        <div className="mb-4 rounded border border-rose-400 bg-rose-50 p-3 text-rose-700">
          {error}
        </div>
      ) : null}
      <div className="mb-4 flex flex-wrap items-end gap-3 rounded-lg border p-3">
        <label className="flex flex-col text-xs">
          <span className="mb-1 font-medium">Pattern</span>
          <input
            type="text"
            className="rounded border px-2 py-1 text-sm"
            placeholder="aa:bb:cc:dd:ee:ff"
            value={pattern}
            onChange={(e) => setPattern(e.target.value)}
          />
        </label>
        <label className="flex flex-col text-xs">
          <span className="mb-1 font-medium">Kind</span>
          <select
            className="rounded border px-2 py-1 text-sm"
            value={kind}
            onChange={(e) => setKind(e.target.value as 'mac' | 'oui')}
          >
            <option value="mac">mac</option>
            <option value="oui">oui</option>
          </select>
        </label>
        <label className="flex flex-col text-xs">
          <span className="mb-1 font-medium">Reason</span>
          <input
            type="text"
            className="rounded border px-2 py-1 text-sm"
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
        </label>
        <button
          type="button"
          className="rounded bg-brand-600 px-3 py-1 text-sm text-white"
          onClick={handleAdd}
        >
          Add
        </button>
      </div>
      <AsyncSection fetcher={getMacBlocklist} deps={[version]}>
        {(res) => (
          <DataTable
            columns={columns}
            data={res.entries}
            emptyMessage="No blocked MACs"
          />
        )}
      </AsyncSection>
    </div>
  );
}
