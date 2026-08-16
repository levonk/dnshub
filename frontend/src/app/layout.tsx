import type { Metadata } from 'next';
import type { ReactNode } from 'react';
import { Nav } from '@/components/Nav';
import './globals.css';

export const metadata: Metadata = {
  title: 'dnshub',
  description: 'dnshub administration dashboard',
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body className="bg-white text-gray-900 dark:bg-gray-950 dark:text-gray-100">
        <div className="flex min-h-screen">
          <Nav />
          <main className="flex-1 overflow-x-auto p-6">{children}</main>
        </div>
      </body>
    </html>
  );
}
