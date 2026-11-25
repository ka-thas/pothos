# pothos

Keyboard-driven photo culling tool for a folder of small JPGs (around 150 KB each) and its subfolders, on macOS.
It's one Rust binary that serves a local web UI. Sharing to an iPhone goes through AirDrop.

## Stack
- **Rust** (edition 2021), **axum 0.8** + **tokio**, **serde/serde_json**. No other crates: keep it that way unless there's a strong reason.
- **UI**: one file, `web/index.html` (vanilla HTML/CSS/JS, no build step, no framework). Embedded with `include_str!`.
- **Swift helpers**: `airdrop.swift` (AirDrop) and `publish.swift` (image rendering and signed R2 requests via ImageIO, CryptoKit, URLSession), compiled by `build.rs` with `swiftc` (macOS only), embedded with `include_bytes!`, written to `$TMPDIR/pothos-<name>-<version>` at startup. On other OSes empty files are embedded and those features report as unavailable.

## Layout
```
Cargo.toml
build.rs          # compiles airdrop.swift on macOS
airdrop.swift     # NSSharingService(.sendViaAirDrop) helper, exits on share or cancel
publish.swift     # publish helper: info, render, get/put/delete on R2, selftest
src/main.rs       # CLI args, routes, file ops, sidecar, undo
src/exif.rs       # lossless rotation via EXIF Orientation tag (+ unit tests)
src/publish.rs    # publish to R2: walk, trips, ids, manifest merge (+ unit tests)
web/index.html    # entire UI and keyboard shortcuts
README.md         # user-facing install and shortcuts
```

## Commands
```
cargo test                      # exif and publish unit tests
cargo build --release
cargo install --path .          # installs `pothos`
pothos <folder> [--port 8787] [--no-open]
```
For a quick check without a browser, run with `--no-open` and hit the API with curl.

## API (all JSON, localhost only)
Every call takes `dir`, the folder relative to the root (`""` is the root, `day1/morning` a subfolder).
- `GET /` UI, `GET /api/list?dir=` -> `{root, items:[{name, v, tags}], folders:[{name, photos, folders, cover:{name, v}|null}], colors, airdrop}`
- `GET /img?dir=&name=&v=` image bytes. Cached forever: `v` is the file's change time (ctime) in nanos and busts the cache.
- `POST /api/rotate {dir, names, cw}` -> `{name: newVersion}`
- `POST /api/tag {dir, names, color|null}` toggles a color for the whole batch (removes it if all have it, otherwise adds it); `null` clears tags
- `POST /api/reject {dir, names}` moves files to `<dir>/_rejected/`, `POST /api/undo` moves the last batch back and returns `{dir, names}`
- `POST /api/airdrop {dir, names}` spawns the helper without waiting for it
- `POST /api/publish {dryRun}` publishes green photos under the root (ignores `dir`). Returns `{bucket, green, new, changed, unchanged, removed, keptOther, deleted, warnings}`. `GET /api/list` includes `publish: bool` (config file exists)

## Design rules (keep these)
- **Rotation must stay lossless.** Only the EXIF Orientation tag (0x0112) changes, and pixel data is never re-encoded. If the tag is missing from an existing EXIF block, a copy of IFD0 with the new entry is appended and the header repointed, so no existing offsets move. No EXIF means a minimal APP1 is inserted (after JFIF APP0 if present).
- **Writes are atomic**: write a temp file, then rename (images and sidecar).
- **Keep the modified time on rotation.** Cameras like the Charmera write no EXIF, so the file's modified time is the only capture time. Image URL versions use the change time instead.
- **Tags are NOT macOS Finder tags.** Each folder has its own `<folder>/.pothos.json` as `{ "tags": { "file.jpg": ["red", ...] } }`, keyed by file name.
- **Fixed tag set**: `red, yellow, green, blue, purple` (keys 1 to 5). The order in `COLORS` in main.rs is the source of truth and is sent to the UI.
- **Subfolders**: browsed one at a time (folder tiles first, then photos), never flattened. The UI keeps the folder in the URL hash. `_rejected` and dot folders are hidden and cannot be opened.
- **Security**: bind to 127.0.0.1 only. `folder()` resolves `dir` to a canonical path that must stay inside the root (plain names only, no `..`, no dotfiles, not `_rejected`). `safe()` accepts only plain file names (no paths, no dotfiles, must be an existing jpg/jpeg in the folder). Run every name from the client through it.
- **Batch semantics**: the UI applies actions to marked photos if any, otherwise to the current one.
- **Keyboard first**: every feature needs a shortcut, and the help overlay (`?`) must list it. Avoid layout-dependent keys; use `e.code` for Shift+digit.
- **Publishing** (config in `~/.config/pothos/publish.json`, see README): the manifest format is a contract with the kathas.no `/photos` page (`docs/photos-manifest.md` in react.kathas.no). Upload images, then `manifest.json`, then delete files only the old manifest used. Image names are `<id>-<content hash>.jpg` and never reused for different pixels (cached as immutable). Ids are `<YYYYMMDD>-<HHMMSS>-<file stem>` from the capture time. Merge with the bucket's manifest: keep photos from other folders and their captions; drop photos here that are not green or were rejected. Keys go to the helper through env vars, never argv.
- Keep it small and dependency-light. Prefer extending `index.html` over adding a build step.

## Known limitations
- `NSSharingService(named:)` is deprecated since macOS 13 but still works. If AirDrop breaks after a macOS update, fix `airdrop.swift` first.
- Tags are lost if a file is renamed or moved outside the app.
- The undo stack lives in memory and is lost on quit.
- Only JPG/JPEG is supported. Images are served full size (no thumbnails), which is fine for small files.

## Testing notes
- AirDrop can only be tested on macOS, and it needs a real share.
- Publishing: `publish-helper selftest` checks the request signer against AWS's SigV4 test vector. For end-to-end runs, point `POTHOS_PUBLISH_CONFIG` at a config whose endpoint is a local fake S3 server (`http://127.0.0.1:<port>` is allowed).
- To check rotation: pixel data after the SOS marker (`FF DA`) must stay byte-identical, and `ImageOps.exif_transpose` in Pillow should show the rotated size.

## Style
American English, no em dashes in docs or UI text.
