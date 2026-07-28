# Epik

Manager-mode feature development on GitHub: converge on a design, author the
feature's issue graph, and launch autonomous builds on Claude Code on the web.

## Design

Design and theory documents live in `docs/design-history/`. Read the latest
entry before making significant design decisions.

## Working conventions

**Always work on a feature branch.** Never commit directly to `main` — not
for features, not for fixes, not for docs. Create a branch, open a pull
request, let CI pass, then merge. This applies to humans, interactive Claude
sessions, and headless builders alike.
