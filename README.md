# Pothos 🪴

Fast, keyboard-driven photo culling for a folder of JPGs. One Rust binary with a local web UI.

## Why
I made pothos to be a lightweight, fast way to manage pictures from my Charmera.

- iPhone: I can plug an SD card reader straight into the USB-C port, but I can only view one
  image fullscreen at a time unless I first import everything into the camera roll, where rotating images is tedious. 
- Finder on mac was too slow for this. I needed fast rotation and fast export to the iPhone camera roll.

pothos shows the whole folder as a grid, rotates with one key, and sends picks to the iPhone with AirDrop.

## Install (macOS)
    cargo install --path .

## Run
    pothos ~/Pictures/some-folder     # opens http://127.0.0.1:8787
    pothos . --port 9000 --no-open

Or run `pothos` inside the folder.

## Shortcuts (? in the app)
- Move: arrows or H J K L, G / Shift+G first/last, Enter open, Esc back
- Folders: Enter opens, Esc or Cmd+Up goes up
- Rotate: R right, Shift+R left (lossless, only the EXIF flag changes)
- Tags: 1-5 red/yellow/green/blue/purple, 0 clear
- Filter: Shift+1-5 one color, Shift+0 untagged, F all
- Mark: Space, Shift+arrows, A all, Esc clear
- Reject: X or Backspace moves to `_rejected/`, U or Cmd+Z undo
- Share: S for AirDrop
- Publish: P
- Grid size: + / -

Actions apply to marked photos, otherwise the current one.
Tags are stored per folder in `.pothos.json`, not Finder tags. The server only listens on 127.0.0.1.

## Publish (optional)
P uploads every green photo under the open folder to a Cloudflare R2 bucket, plus a `manifest.json` for a website.
It shows the changes and asks first.

- Rotation is baked in, metadata is stripped, thumbnails are 540px.
- Capture time is the EXIF date, or the file's modified time.
- Each photo gets the shortest trip whose dates contain it.
- Photos no longer green, or rejected, are removed. Photos from other folders and captions are kept.

Bucket layout: `manifest.json`, `photos/`, `thumbs/`.

### Setup
1. Create a public R2 bucket (custom domain or r2.dev).
2. Add a CORS policy:
   ```json
   [{ "AllowedOrigins": ["https://kathas.no", "http://localhost:5173"], "AllowedMethods": ["GET"],
      "AllowedHeaders": ["*"], "MaxAgeSeconds": 3600 }]
   ```
3. Create an R2 API token (Object Read & Write, this bucket only).
4. Save `~/.config/pothos/publish.json` and run `chmod 600` on it:
   ```json
   {
     "endpoint": "https://<account id>.r2.cloudflarestorage.com",
     "bucket": "photos",
     "accessKeyId": "...",
     "secretAccessKey": "...",
     "trips": [
       { "id": "nagoya", "name": "Nagoya", "start": "2026-09-01", "end": "2027-05-31" }
     ]
   }
   ```
   `POTHOS_PUBLISH_CONFIG` overrides the path. The Publish button shows once the file exists.

## License
MIT, see [LICENSE](LICENSE).
