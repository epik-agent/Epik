# Epik website

Static placeholder site. The `deploy-website` job in
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) deploys it to
GitHub Pages on every push to `main` that passes CI. No build step: the
workflow uploads this directory as-is.

Brand assets and the palette definition live in [`brand/`](brand/)
(`brand.json` is the source of truth for colors, fonts, and the logo).
