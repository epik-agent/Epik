# App icons

The brand mark from `website/brand/brand.json` — four connected nodes, two
accent and two foreground — rasterized onto the dark palette's `bg.root`
tile. `brand.json` stays the source of truth; these are its build artifacts,
committed because they are static and rasterizing them is not worth a
build-time dependency.
