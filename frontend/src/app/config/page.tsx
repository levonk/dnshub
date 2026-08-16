'use client';

import { AsyncSection, RefreshButton } from '@/components';
import { getConfig, updateConfig } from '@/lib/api';
import type { Config } from '@/lib/types';
import { useState } from 'react';

export default function ConfigPage() {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<Config | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const startEdit = (cfg: Config) => {
    setDraft(structuredClone(cfg));
    setEditing(true);
  };

  const save = async () => {
    if (!draft) return;
    setSaving(true);
    setSaveError(null);
    try {
      await updateConfig(draft);
      setEditing(false);
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div>
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-2xl font-bold">Configuration</h1>
        <RefreshButton onRefresh={() => getConfig()} label="Reload" />
      </div>
      <AsyncSection<Config> fetcher={getConfig}>
        {(cfg) =>
          editing && draft ? (
            <div className="space-y-4">
              <pre
                className="overflow-x-auto rounded border border-gray-300 bg-gray-50 p-4 text-xs dark:border-gray-700 dark:bg-gray-800"
                contentEditable
                suppressContentEditableWarning
                onBlur={(e) => {
                  try {
                    setDraft(JSON.parse(e.currentTarget.textContent ?? '{}'));
                  } catch {
                    // keep draft as-is on parse error
                  }
                }}
              >
                {JSON.stringify(draft, null, 2)}
              </pre>
              {saveError ? (
                <div className="text-rose-600">{saveError}</div>
              ) : null}
              <div className="flex gap-2">
                <button
                  type="button"
                  className="rounded bg-brand-600 px-3 py-1 text-sm text-white disabled:opacity-40"
                  onClick={save}
                  disabled={saving}
                >
                  {saving ? 'Saving…' : 'Save'}
                </button>
                <button
                  type="button"
                  className="rounded border px-3 py-1 text-sm"
                  onClick={() => setEditing(false)}
                >
                  Cancel
                </button>
              </div>
            </div>
          ) : (
            <div className="space-y-4">
              <div className="flex gap-2">
                <button
                  type="button"
                  className="rounded bg-brand-600 px-3 py-1 text-sm text-white"
                  onClick={() => startEdit(cfg)}
                >
                  Edit
                </button>
              </div>
              <pre className="overflow-x-auto rounded border border-gray-300 bg-gray-50 p-4 text-xs dark:border-gray-700 dark:bg-gray-800">
                {JSON.stringify(cfg, null, 2)}
              </pre>
            </div>
          )
        }
      </AsyncSection>
    </div>
  );
}
