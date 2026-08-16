import type { ReactNode } from 'react';

interface StatusCardProps {
  title: string;
  value: ReactNode;
  hint?: string;
  /** Visual state: healthy (green), warning (amber), error (red). */
  state?: 'healthy' | 'warning' | 'error' | 'neutral';
}

const STATE_COLORS: Record<NonNullable<StatusCardProps['state']>, string> = {
  healthy: 'border-emerald-500 text-emerald-600',
  warning: 'border-amber-500 text-amber-600',
  error: 'border-rose-500 text-rose-600',
  neutral: 'border-gray-300 text-gray-700 dark:border-gray-700 dark:text-gray-200',
};

export function StatusCard({ title, value, hint, state = 'neutral' }: StatusCardProps) {
  return (
    <div
      className={`rounded-lg border-l-4 bg-white p-4 shadow-sm dark:bg-gray-800 ${STATE_COLORS[state]}`}
    >
      <div className="text-xs font-medium uppercase tracking-wide opacity-70">
        {title}
      </div>
      <div className="mt-1 text-2xl font-bold">{value}</div>
      {hint ? (
        <div className="mt-1 text-xs text-gray-500 dark:text-gray-400">{hint}</div>
      ) : null}
    </div>
  );
}
