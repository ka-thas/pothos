# pothos

Keyboard-driven photo culling tool for a single folder of small JPGs (around 150 KB each) on macOS.
It's one Rust binary that serves a local web UI. Sharing to an iPhone goes through AirDrop.

## Stack
- **Rust** (edition 2021), **axum 0.8** + **tokio**, **serde/serde_json**. No other crates: keep it that way unless there's a strong reason.
- **UI**: one file, `web/index.html` (vanilla HTML/CSS/JS, no build step, no framework). Embedded with `include_str!`.
- **AirDrop**: `airdrop.swift`, compiled by `build.rs` with `swiftc` (macOS only), embedded with `include_bytes!`, written to `$TMPDIR/pothos-airdrop-<version>` at startup. On other OSes an empty file is embedded and AirDrop reports as unavailable.

## Layout
```
Cargo.toml
build.rs          # compiles airdrop.swift on macOS
airdrop.swift     # NSSharingService(.sendViaAirDrop) helper, exits on share or cancel
src/main.rs       # CLI args, routes, file ops, sidecar, undo
src/exif.rs       # lossless rotation via EXIF Orientation tag (+ unit tests)
web/index.html    # entire UI and keyboard shortcuts
README.md         # user-facing install and shortcuts
```

## Commands
```
cargo test                      # exif unit tests
cargo build --release
cargo install --path .          # installs `pothos`
pothos <folder> [--port 8787] [--no-open]
```
For a quick check without a browser, run with `--no-open` and hit the API with curl.

## API (all JSON, localhost only)
- `GET /` UI, `GET /api/list` -> `{folder, items:[{name, v, tags}], colors, airdrop}`
- `GET /img/{name}` image bytes. Cached forever: the UI appends `?v=<mtime nanos>` to bust the cache.
- `POST /api/rotate {names, cw}` -> `{name: newVersion}`
- `POST /api/tag {names, color|null}` toggles a color for the whole batch (removes it if all have it, otherwise adds it); `null` clears tags
- `POST /api/reject {names}` moves files to `_rejected/`, `POST /api/undo` moves the last batch back
- `POST /api/airdrop {names}` spawns the helper without waiting for it

## Design rules (keep these)
- **Rotation must stay lossless.** Only the EXIF Orientation tag (0x0112) changes, and pixel data is never re-encoded. If the tag is missing from an existing EXIF block, a copy of IFD0 with the new entry is appended and the header repointed, so no existing offsets move. No EXIF means a minimal APP1 is inserted (after JFIF APP0 if present).
- **Writes are atomic**: write a temp file, then rename (images and sidecar).
- **Tags are NOT macOS Finder tags.** They live in `<folder>/.pothos.json` as `{ "tags": { "file.jpg": ["red", ...] } }`, keyed by file name.
- **Fixed tag set**: `red, yellow, green, blue, purple` (keys 1 to 5). The order in `COLORS` in main.rs is the source of truth and is sent to the UI.
- **Security**: bind to 127.0.0.1 only. `safe()` accepts only plain file names (no paths, no dotfiles, must be an existing jpg/jpeg in the folder). Run every name from the client through it.
- **Batch semantics**: the UI applies actions to marked photos if any, otherwise to the current one.
- **Keyboard first**: every feature needs a shortcut, and the help overlay (`?`) must list it. Avoid layout-dependent keys; use `e.code` for Shift+digit.
- Keep it small and dependency-light. Prefer extending `index.html` over adding a build step.

## Known limitations
- `NSSharingService(named:)` is deprecated since macOS 13 but still works. If AirDrop breaks after a macOS update, fix `airdrop.swift` first.
- Tags are lost if a file is renamed or moved outside the app.
- The undo stack lives in memory and is lost on quit.
- Only JPG/JPEG is supported. Images are served full size (no thumbnails), which is fine for small files.

## Testing notes
- AirDrop can only be tested on macOS, and it needs a real share.
- To check rotation: pixel data after the SOS marker (`FF DA`) must stay byte-identical, and `ImageOps.exif_transpose` in Pillow should show the rotated size.

## Style
American English, no em dashes in docs or UI text.
