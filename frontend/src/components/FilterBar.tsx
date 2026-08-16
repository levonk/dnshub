'use client';

import type { ChangeEvent, ReactNode } from 'react';

interface FilterField {
  name: string;
  label: string;
  type?: 'text' | 'select';
  options?: string[];
  placeholder?: string;
}

interface FilterBarProps {
  fields: FilterField[];
  values: Record<string, string | boolean | undefined>;
  onChange: (name: string, value: string | boolean) => void;
  onApply?: () => void;
  onReset?: () => void;
  children?: ReactNode;
}

export function FilterBar({
  fields,
  values,
  onChange,
  onApply,
  onReset,
  children,
}: FilterBarProps) {
  return (
    <div className="mb-4 flex flex-wrap items-end gap-3 rounded-lg border border-gray-200 bg-gray-50 p-3 dark:border-gray-700 dark:bg-gray-800">
      {fields.map((field) => {
        const val = values[field.name];
        if (field.type === 'select') {
          return (
            <label key={field.name} className="flex flex-col text-xs">
              <span className="mb-1 font-medium text-gray-600 dark:text-gray-300">
                {field.label}
              </span>
              <select
                className="rounded border bg-white px-2 py-1 text-sm dark:bg-gray-900"
                value={typeof val === 'string' ? val : ''}
                onChange={(e: ChangeEvent<HTMLSelectElement>) =>
                  onChange(field.name, e.target.value)
                }
              >
                <option value="">Any</option>
                {field.options?.map((opt) => (
                  <option key={opt} value={opt}>{opt}</option>
                ))}
              </select>
            </label>
          );
        }
        return (
          <label key={field.name} className="flex flex-col text-xs">
            <span className="mb-1 font-medium text-gray-600 dark:text-gray-300">
              {field.label}
            </span>
            <input
              type="text"
              className="rounded border bg-white px-2 py-1 text-sm dark:bg-gray-900"
              placeholder={field.placeholder ?? ''}
              value={typeof val === 'string' ? val : ''}
              onChange={(e: ChangeEvent<HTMLInputElement>) =>
                onChange(field.name, e.target.value)
              }
            />
          </label>
        );
      })}
      {children}
      {onApply ? (
        <button
          type="button"
          className="rounded bg-brand-600 px-3 py-1 text-sm text-white hover:bg-brand-700"
          onClick={onApply}
        >
          Apply
        </button>
      ) : null}
      {onReset ? (
        <button
          type="button"
          className="rounded border px-3 py-1 text-sm"
          onClick={onReset}
        >
          Reset
        </button>
      ) : null}
    </div>
  );
}
