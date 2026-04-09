import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import type { NextConfig } from "next";

const siteRoot = dirname(fileURLToPath(import.meta.url));

const nextConfig: NextConfig = {
  // NOTE: do not set `outputFileTracingRoot` here. Codex added it in the
  // Next.js 16 upgrade (c45cd33) as a monorepo pattern, but it caused Vercel's
  // GitHub integration post-build step to compute the manifest path relative
  // to the repo root instead of the site/ root, resulting in:
  //   ENOENT: no such file or directory, lstat
  //   '/vercel/path0/.next/routes-manifest-deterministic.json'
  // The Vercel KB article
  // (https://vercel.com/kb/guide/missing-routes-manifest-or-output-turborepo-nx)
  // calls out removing `outputFileTracingRoot` as the fix. It is safe to omit
  // here because this site is fully static (no serverless functions), so the
  // file trace doesn't affect the shipped output. CLI deploys
  // (`vercel deploy` from site/) worked fine with it set because the CLI
  // upload path skips the broken post-build step.
  turbopack: {
    root: siteRoot,
  },
  async headers() {
    return [
      {
        source: "/llms.txt",
        headers: [
          { key: "Content-Type", value: "text/plain; charset=utf-8" },
          { key: "Cache-Control", value: "public, max-age=3600" },
        ],
      },
    ];
  },
};

export default nextConfig;
