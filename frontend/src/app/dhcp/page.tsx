import Link from 'next/link';

const DHCP_SECTIONS = [
  { href: '/dhcp/leases', label: 'Active Leases', desc: 'v4 + v6 lease table with release/renew' },
  { href: '/dhcp/blocklist', label: 'MAC Blocklist', desc: 'Blocked MACs/OUIs' },
  { href: '/dhcp/audit', label: 'Audit Log', desc: 'Device join/leave events' },
  { href: '/dhcp/pools', label: 'Pools', desc: 'Local + relayed VLAN pools' },
  { href: '/dhcp/pxe', label: 'PXE / BOOTP', desc: 'Bootfile mappings and static BOOTP' },
  { href: '/dhcp/relay', label: 'Relay', desc: 'Trusted agents and Option 82' },
  { href: '/dhcp/rogue', label: 'Rogue Detection', desc: 'Detected rogue DHCP servers' },
];

export default function DhcpIndexPage() {
  return (
    <div>
      <h1 className="mb-4 text-2xl font-bold">DHCP</h1>
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {DHCP_SECTIONS.map((s) => (
          <Link
            key={s.href}
            href={s.href}
            className="block rounded-lg border border-gray-200 p-4 transition-colors hover:border-brand-500 hover:bg-brand-50 dark:border-gray-700 dark:hover:bg-gray-800"
          >
            <div className="font-semibold text-brand-700 dark:text-brand-100">{s.label}</div>
            <div className="mt-1 text-sm text-gray-500 dark:text-gray-400">{s.desc}</div>
          </Link>
        ))}
      </div>
    </div>
  );
}
