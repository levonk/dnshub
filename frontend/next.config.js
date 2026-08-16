/** @type {import('next').NextConfig} */
const nextConfig = {
  // Produce a fully static SPA in `out/` — no Node.js runtime in production.
  // The Rust binary (or Traefik) serves these files from /etc/dnshub/frontend/.
  output: 'export',
  // Sub-path deployment: the Rust API serves the frontend under /ui/.
  basePath: '/ui',
  assetPrefix: '/ui',
  // Static export cannot use Image Optimization (requires a server).
  images: {
    unoptimized: true,
  },
  // Treat type errors and lint errors as build failures.
  typescript: {
    ignoreBuildErrors: false,
  },
  eslint: {
    ignoreDuringBuilds: false,
  },
  // Trailing slashes so static file serving works without rewrites.
  trailingSlash: true,
};

export default nextConfig;
