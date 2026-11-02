# Bundled syntax definitions

These grammars add languages that syntect's built-in set lacks. `build.rs` merges
them with the built-in set into a precompiled bundle at build time.

| File | Source | License |
|------|--------|---------|
| `TypeScript.sublime-syntax` | [bat](https://github.com/sharkdp/bat) `assets/syntaxes/02_Extra/TypeScript.sublime-syntax` (blob `68c9b4a`), converted from Microsoft's [TypeScript-Sublime-Plugin](https://github.com/microsoft/TypeScript-Sublime-Plugin) (commit `ba45efd`) | Apache-2.0, see `licenses/TypeScript-LICENSE.txt` |
| `TOML.sublime-syntax` | [sublime_toml_highlighting](https://github.com/jasonwilliams/sublime_toml_highlighting) (commit `f5a57e8`) | MIT, see `licenses/TOML-LICENSE.txt` |
