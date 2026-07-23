# Minutes website

The Minutes website is a fully static Next.js export hosted on Cloudflare
Pages. The desktop app, transcription pipeline, Sidekick, and release artifacts
do not run on Cloudflare.

## Local verification

```bash
cd site
npm ci
npm run check:llms
npm run build
```

The deployable site is written to `site/out/`.

## Cloudflare Pages

- Project: `useminutes`
- Production branch: `main`
- Root directory: `site`
- Build command: `npm run build`
- Build output directory: `out`
- Build watch include path: `site/*`

The watch path is deliberate: desktop, CLI, and Sidekick commits do not rebuild
the website unless they also change a generated or hand-written file under
`site/`.

`public/_headers` preserves the plain-text content type and cache policy for
`/llms.txt`. No Pages Functions or Workers runtime is used.

The production domain should remain on its existing host until a Cloudflare
preview has passed route, asset, header, and visual checks.
