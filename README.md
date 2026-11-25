# Pothos 🪴

Fast, keyboard-driven photo culling for a folder of JPGs. One Rust binary serves a local web UI.
Rotation is lossless (only the EXIF orientation flag changes). Tags live in `.pothos.json`
inside the folder, not in Finder tags. AirDrop goes through a tiny embedded Swift helper.

## Why
I made pothos to be a lightweight, fast way to manage pictures from my Charmera.

- **iPhone:** I can plug an SD card reader straight into the USB-C port, but I can only view one
  image fullscreen at a time unless I first import everything into the camera roll, where rotating images is tedious. 
- **Finder** was too slow for this. I needed fast rotation and fast export to the iPhone camera roll.

pothos shows the whole folder as a grid, rotates with one key, and sends picks to the iPhone with AirDrop.

## Install (macOS)
    cargo install --path .          # builds the Swift AirDrop helper too

## Run
    pothos ~/Pictures/some-folder         # opens http://127.0.0.1:8787
    pothos . --port 9000 --no-open

Alternatively run `pothos` from within the folder with your pictures.

## Shortcuts (press ? in the app)
- Move: arrows or H J K L, G / Shift+G first/last, Enter open, Esc back
- Folders: subfolders show as tiles first. Enter opens one, Esc (or Cmd+Up) goes back up
- Rotate: R right, Shift+R left
- Tags: 1-5 toggle red/yellow/green/blue/purple, 0 clear
- Filter: Shift+1-5 one color, Shift+0 untagged, F all
- Mark: Space, Shift+arrows mark and move, A all, Esc clear
- Reject: X or Backspace moves to `_rejected/`, U or Cmd+Z undo
- Share: S opens the AirDrop picker
- Grid size: + / -

Edits apply to marked photos if any, otherwise the current one.
Each folder keeps its own `.pothos.json` and `_rejected/`. pothos never goes above the folder you start it on.
The server only listens on 127.0.0.1.

## Publish to a photo site (optional)
Press P (or click Publish) to upload every green photo under the folder you opened, subfolders included,
to a Cloudflare R2 bucket, with a `manifest.json` that a website reads. pothos shows what will change and asks first.

- Rotation is baked into the uploaded JPGs, and metadata (EXIF, GPS) is removed. Thumbnails are 540px on the long side.
- Capture time is the EXIF date if there is one, otherwise the file's modified time in the Mac's time zone (the Charmera writes no EXIF).
- Each photo gets the trip whose date range holds its capture date. The shortest matching range wins.
- Photos here that are no longer green, or were rejected, are removed from the site and their files deleted.
  Photos published from other folders stay. Captions added to the manifest are kept.
- File names include a hash of the photo, so a rotated photo gets a new URL and old copies never linger in caches.

Bucket layout: `manifest.json` at the root, full images in `photos/`, thumbnails in `thumbs/`.
Images are uploaded first and the manifest last, so the site never points at a missing file.

### Setup
1. Create an R2 bucket in the Cloudflare dashboard.
2. Make it public, with a custom domain (recommended) or the r2.dev URL.
3. Add a CORS policy so the site can read it:
   ```json
   [{ "AllowedOrigins": ["https://kathas.no", "http://localhost:5173"], "AllowedMethods": ["GET"],
      "AllowedHeaders": ["*"], "MaxAgeSeconds": 3600 }]
   ```
4. Create an R2 API token with Object Read & Write, limited to that bucket. Note the access key ID, the secret,
   and the S3 endpoint `https://<account id>.r2.cloudflarestorage.com`.
5. Save `~/.config/pothos/publish.json` and make it private with `chmod 600 ~/.config/pothos/publish.json`:
   ```json
   {
     "endpoint": "https://<account id>.r2.cloudflarestorage.com",
     "bucket": "photos",
     "accessKeyId": "...",
     "secretAccessKey": "...",
     "trips": [
       { "id": "nagoya", "name": "Nagoya", "start": "2026-09-01", "end": "2027-05-31" },
       { "id": "chengdu", "name": "Chengdu", "start": "2027-03-03", "end": "2027-03-10" }
     ]
   }
   ```
   Set `POTHOS_PUBLISH_CONFIG` to use a different file. The Publish button appears once the file exists.

Deleted images can stay in Cloudflare's cache for a while if they were viewed recently.

## License
MIT, see [LICENSE](LICENSE).
