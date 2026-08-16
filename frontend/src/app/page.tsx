'use client';

import { AsyncSection, StatusCard } from '@/components';
import { getStatus } from '@/lib/api';
import type { Status } from '@/lib/types';

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3600);
  const mins = Math.floor((seconds % 3600) / 60);
  const parts: string[] = [];
  if (days > 0) parts.push(`${days}d`);
  if (hours > 0) parts.push(`${hours}h`);
  parts.push(`${mins}m`);
  return parts.join(' ');
}

function tierState(healthy: boolean): 'healthy' | 'error' {
  return healthy ? 'healthy' : 'error';
}

export default function DashboardPage() {
  return (
    <div>
      <h1 className="mb-4 text-2xl font-bold">Dashboard</h1>
      <AsyncSection<Status> fetcher={getStatus} pollMs={5000}>
        {(status) => (
          <div className="space-y-6">
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
              <StatusCard
                title="Uptime"
                value={formatUptime(status.uptime)}
                hint={`v${status.version}`}
                state="neutral"
              />
              <StatusCard
                title="Cache Hit Ratio"
                value={`${(status.cacheHitRatio * 100).toFixed(1)}%`}
                state={status.cacheHitRatio > 0.8 ? 'healthy' : 'warning'}
              />
              <StatusCard
                title="Query Rate"
                value={`${status.queryRate.toFixed(1)}/s`}
                hint={`${status.queriesTotal} total`}
                state="neutral"
              />
              <StatusCard
                title="Blocklist Entries"
                value={status.blocklists.entries.toLocaleString()}
                hint={`${status.blocklists.sources} sources`}
                state="neutral"
              />
            </div>

            <section>
              <h2 className="mb-2 text-lg font-semibold">Upstream Tiers</h2>
              <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
                {status.tiers.map((tier) => (
                  <StatusCard
                    key={tier.name}
                    title={tier.name}
                    value={tier.healthy ? 'Healthy' : 'Unhealthy'}
                    hint={`${tier.latencyMs}ms${tier.lastError ? ` · ${tier.lastError}` : ''}`}
                    state={tierState(tier.healthy)}
                  />
                ))}
              </div>
            </section>
          </div>
        )}
      </AsyncSection>
    </div>
  );
}
