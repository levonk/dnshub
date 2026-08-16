'use client';

import { AsyncSection, StatusCard } from '@/components';
import { getStatus } from '@/lib/api';
import type { Status } from '@/lib/types';

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3600);
  const mins = Math.floor((seconds % 3600) / 60);
  return [`${days}d`, `${hours}h`, `${mins}m`].join(' ');
}

export default function StatusPage() {
  return (
    <div>
      <h1 className="mb-4 text-2xl font-bold">Service Status</h1>
      <AsyncSection<Status> fetcher={getStatus} pollMs={2000}>
        {(status) => (
          <div className="space-y-6">
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
              <StatusCard title="Uptime" value={formatUptime(status.uptime)} />
              <StatusCard
                title="Cache Hit Ratio"
                value={`${(status.cacheHitRatio * 100).toFixed(1)}%`}
                state={status.cacheHitRatio > 0.8 ? 'healthy' : 'warning'}
              />
              <StatusCard
                title="Query Rate"
                value={`${status.queryRate.toFixed(1)}/s`}
              />
              <StatusCard
                title="Total Queries"
                value={status.queriesTotal.toLocaleString()}
              />
            </div>

            <section>
              <h2 className="mb-2 text-lg font-semibold">Upstream Tier Status</h2>
              <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
                {status.tiers.map((tier) => (
                  <StatusCard
                    key={tier.name}
                    title={tier.name}
                    value={tier.healthy ? 'Healthy' : 'Unhealthy'}
                    hint={`${tier.latencyMs}ms${tier.lastError ? ` · ${tier.lastError}` : ''}`}
                    state={tier.healthy ? 'healthy' : 'error'}
                  />
                ))}
              </div>
            </section>

            <section>
              <h2 className="mb-2 text-lg font-semibold">Blocklists</h2>
              <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
                <StatusCard
                  title="Sources"
                  value={status.blocklists.sources}
                />
                <StatusCard
                  title="Entries"
                  value={status.blocklists.entries.toLocaleString()}
                />
                <StatusCard
                  title="Last Refresh"
                  value={new Date(status.blocklists.lastRefresh).toLocaleString()}
                />
              </div>
            </section>
          </div>
        )}
      </AsyncSection>
    </div>
  );
}
