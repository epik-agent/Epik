# Vendored syntax grammars

The pruned grammar set for code-block highlighting. `build.rs` compiles
this folder into the SyntaxSet dump the highlighter embeds, so
regeneration is just `cargo build`; adding a language is dropping a
syntect-compatible `.sublime-syntax` file here and listing it in
`LANGUAGES` in `src/highlight.rs` (the test suite fails if the two
drift).

Provenance, fetched 2026-08-15:

- Most grammars come from [sublimehq/Packages] at revision
  `1ba99a47ee234311f7e87fc9d6153d2d9e4d4d93` — the revision the syntect
  crate pins for its own default set, so syntect is known to parse
  them: Rust, Python, JavaScript, Go, Java, C, C++, Bash, JSON, YAML,
  TOML, CSS, Markdown, Haskell. License:
  `LICENSE-sublimehq-packages.txt`.
- `HTML.sublime-syntax` and `SQL.sublime-syntax` from the same
  repository at revision `759d6eed9b4beed87e602a23303a121c3a6c2fb3` —
  the revision [sharkdp/bat] pins — because the newer versions use
  `extends`/version-2 constructs syntect cannot parse. Same license.
- `TypeScript.sublime-syntax` from [sharkdp/bat]
  (`assets/syntaxes/02_Extra/TypeScript.sublime-syntax`, master,
  2026-08-15), bat's syntect-compatible conversion of
  [Microsoft/TypeScript-Sublime-Plugin] (Apache-2.0).
- `Lean.sublime-syntax` from [LexouDuck/SublimeText-Lean] (master,
  2026-08-15), MIT. License: `LICENSE-sublimetext-lean.txt`.

Recapture any sublimehq file with:

```sh
curl -sf "https://raw.githubusercontent.com/sublimehq/Packages/<revision>/<Dir>/<Name>.sublime-syntax" \
  -o grammars/<Name>.sublime-syntax
```

[sublimehq/Packages]: https://github.com/sublimehq/Packages
[sharkdp/bat]: https://github.com/sharkdp/bat
[Microsoft/TypeScript-Sublime-Plugin]: https://github.com/Microsoft/TypeScript-Sublime-Plugin
[LexouDuck/SublimeText-Lean]: https://github.com/LexouDuck/SublimeText-Lean
