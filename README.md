# pothos 🪴

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
    xcode-select --install          # once, if you don't have the Command Line Tools
    cargo install --path .          # builds the Swift AirDrop helper too

## Run
    pothos ~/Pictures/some-folder         # opens http://127.0.0.1:8787
    pothos . --port 9000 --no-open

## Shortcuts (press ? in the app)
- Move: arrows or H J K L, G / Shift+G first/last, Enter open, Esc back
- Rotate: R right, Shift+R left
- Tags: 1-5 toggle red/yellow/green/blue/purple, 0 clear
- Filter: Shift+1-5 one color, Shift+0 untagged, F all
- Mark: Space, Shift+arrows mark and move, A all, Esc clear
- Reject: X or Backspace moves to `_rejected/`, U or Cmd+Z undo
- Share: S opens the AirDrop picker
- Grid size: + / -

Edits apply to marked photos if any, otherwise the current one.
The server only listens on 127.0.0.1.

## License
MIT, see [LICENSE](LICENSE).
