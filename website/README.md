# Epik website

Static site: one page, `index.html`, with a "Download Epik" section. Its
buttons link to the latest GitHub Release's installers
(`https://github.com/epik-agent/Epik/releases/latest/download/<file>`), so
their file names must match the ones
[`.github/workflows/release.yml`](../.github/workflows/release.yml) uploads:
`Epik.dmg`, `Epik.msi`, `Epik_amd64.AppImage` and `Epik_amd64.deb`. A short
script highlights the visitor's platform; the buttons work without it. The `deploy-website` job in
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) deploys it to
GitHub Pages on every push to `main` that passes CI. No build step: the
workflow uploads this directory as-is.

Brand assets and the palette definition live in [`brand/`](brand/)
(`brand.json` is the source of truth for colors, fonts, and the logo).
