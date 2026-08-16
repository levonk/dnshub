import type { Config } from 'tailwindcss';

const config: Config = {
  content: [
    './src/app/**/*.{ts,tsx}',
    './src/components/**/*.{ts,tsx}',
  ],
  theme: {
    extend: {
      colors: {
        // Tremor-aligned palette for status cards.
        brand: {
          50: '#eff6ff',
          100: '#dbeafe',
          500: '#3b82f6',
          600: '#2563eb',
          700: '#1d4ed8',
        },
      },
    },
  },
  // Tremor ships its own plugin; keep the safelist minimal for static export.
  safelist: [
    'bg-emerald-500',
    'bg-amber-500',
    'bg-rose-500',
    'text-emerald-500',
    'text-amber-500',
    'text-rose-500',
  ],
  plugins: [],
};

export default config;
