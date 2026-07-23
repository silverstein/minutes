import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import type { NextConfig } from "next";

const siteRoot = dirname(fileURLToPath(import.meta.url));

const nextConfig: NextConfig = {
  // The website has no server-only routes. Exporting plain files keeps hosting
  // portable and lets Cloudflare Pages serve every request as a free static
  // asset without a Workers runtime.
  output: "export",
  turbopack: {
    root: siteRoot,
  },
};

export default nextConfig;
