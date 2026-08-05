/* Which design tokens a component may use, and under what name.
 *
 * Every colour here is a custom property from `styles/tokens.css`, never a
 * literal: that indirection is the whole light/dark mechanism, since flipping
 * `data-theme` on the document element rewrites all of them at once and no
 * component has to know a theme exists. A hex code appearing in this file, or
 * in a `view!`, would be a component that only works in one of them.
 *
 * Names read the way markup wants to read them — `bg-root`, `text-secondary`,
 * `border-edge` — rather than the way the brand file is nested.
 */
module.exports = {
  // Tailwind scans text, so class names inside Leptos `view!` macros are found
  // exactly as they would be in HTML.
  content: ["index.html", "src/**/*.rs"],
  theme: {
    extend: {
      colors: {
        // Surfaces.
        root: "var(--bg-root)",
        surface: "var(--bg-surface)",
        raised: "var(--bg-raised)",
        input: "var(--bg-input)",
        bar: "var(--bg-bar)",
        hover: "var(--bg-hover)",
        active: "var(--bg-active)",

        // Type.
        primary: "var(--text-primary)",
        secondary: "var(--text-secondary)",
        muted: "var(--text-muted)",
        faint: "var(--text-faint)",
        inverse: "var(--text-inverse)",

        // The accent, and what is legible on it.
        accent: "var(--accent-base)",
        "accent-hover": "var(--accent-hover)",
        "accent-muted": "var(--accent-muted)",
        "on-accent": "var(--accent-on-accent)",

        // Edges.
        edge: "var(--border-default)",
        "edge-strong": "var(--border-strong)",

        // Meaning.
        error: "var(--semantic-error)",
        "error-muted": "var(--semantic-error-muted)",
        warning: "var(--semantic-warning)",
        "warning-muted": "var(--semantic-warning-muted)",
        success: "var(--semantic-success)",
      },
      fontFamily: {
        // The brand's faces, with fallbacks that need no network: a desktop
        // window should not wait on a font CDN to render its first frame.
        sans: [
          "Geist",
          "ui-sans-serif",
          "system-ui",
          "-apple-system",
          "Segoe UI",
          "sans-serif",
        ],
        mono: ["Geist Mono", "ui-monospace", "SFMono-Regular", "Menlo", "monospace"],
      },
    },
  },
  plugins: [],
};
