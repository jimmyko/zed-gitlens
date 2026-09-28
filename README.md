# GitLens for Zed

GitLens-style git features for [Zed](https://zed.dev), built as a Zed extension.

Zed already has inline blame, a blame gutter, commit hovers, File History and a
Git Graph. This extension adds what's missing:

- **Line history.** Select some lines, open code actions (`cmd-.` / `ctrl-.`) and
  pick **Show line history**. It runs `git log -L` over the selection and opens
  the result as a `.diff` file.
- **Authorship code lens.** A lens at the top of each file shows who changed it
  last, when, and how many people have touched it. Clicking it opens that last
  commit's diff for the file.

## How it works

Zed extensions can't add panels, commands or editor decorations, so the
extension launches a small language server, `gitlens-ls`, and uses standard LSP
features that Zed already renders: code actions, code lens,
`workspace/executeCommand` and `window/showDocument`.

```
extension.toml, src/lib.rs   Zed extension (Rust -> wasm32-wasip2); finds or downloads gitlens-ls
server/                      gitlens-ls language server (native Rust, shells out to git)
```

## Setup

Code lens is off by default in Zed. Turn it on in `settings.json`:

```json
{ "code_lens": "on" }
```

Optional: bind line history to a key by binding the code actions menu, e.g.

```json
{ "context": "Editor", "bindings": { "alt-h": "editor::ToggleCodeActions" } }
```

## Development

Requirements: Rust via rustup, and `git` on your `PATH`.

1. Build and install the server so the extension finds it on `PATH`:
   ```sh
   cargo install --path server
   ```
2. In Zed, run `zed: install dev extension` and pick this folder.
3. Open a file in a git repo, select a few lines and run code actions.

Logs: `zed: open log`. Language server traffic: `dev: open language server logs`.

Tests:

```sh
(cd server && cargo test)
cargo build --release --target wasm32-wasip2
```

## Releasing

Push a `v*` tag. The Release workflow builds `gitlens-ls` for macOS, Linux and
Windows and attaches `gitlens-ls-<target>.tar.gz|zip` to the GitHub release.
Users without `gitlens-ls` on their `PATH` get the latest release downloaded
automatically.

## Limitations

- The language server is attached to the languages listed in `extension.toml`.
  Add more there if you need them.
- Results open as static `.diff` files, not an interactive history list; Zed
  has no extension UI API yet
  ([RFC #53403](https://github.com/zed-industries/zed/discussions/53403)).
- History is for the committed file; uncommitted edits can shift line numbers.
