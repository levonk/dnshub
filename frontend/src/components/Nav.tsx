'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';

interface NavItem {
  href: string;
  label: string;
}

const NAV_ITEMS: NavItem[] = [
  { href: '/', label: 'Dashboard' },
  { href: '/status', label: 'Status' },
  { href: '/config', label: 'Config' },
  { href: '/dhcp/leases', label: 'DHCP Leases' },
  { href: '/dhcp/blocklist', label: 'MAC Blocklist' },
  { href: '/dhcp/audit', label: 'Audit Log' },
  { href: '/dhcp/pools', label: 'DHCP Pools' },
  { href: '/dhcp/pxe', label: 'PXE / BOOTP' },
  { href: '/dhcp/relay', label: 'DHCP Relay' },
  { href: '/dhcp/rogue', label: 'Rogue DHCP' },
  { href: '/blocklists', label: 'Blocklists' },
  { href: '/query-log', label: 'Query Log' },
];

export function Nav() {
  const pathname = usePathname();
  return (
    <nav className="flex h-full w-56 flex-col border-r border-gray-200 bg-gray-50 p-3 dark:border-gray-800 dark:bg-gray-900">
      <Link
        href="/"
        className="mb-4 text-lg font-bold text-brand-600"
      >
        dnshub
      </Link>
      <ul className="flex flex-col gap-1">
        {NAV_ITEMS.map((item) => {
          const active =
            item.href === '/'
              ? pathname === '/'
              : pathname?.startsWith(item.href);
          return (
            <li key={item.href}>
              <Link
                href={item.href}
                className={`block rounded px-3 py-2 text-sm transition-colors ${
                  active
                    ? 'bg-brand-100 text-brand-700 dark:bg-brand-700 dark:text-white'
                    : 'text-gray-700 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-800'
                }`}
              >
                {item.label}
              </Link>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
