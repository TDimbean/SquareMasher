# SquareMasher

SquareMasher is a portable Rust desktop 2048 game for Windows and Linux. It opens an always-on-top game window and remains available from the system tray after that window is closed. Use the tray menu's **Quit** action to stop it.

## Play

- Move with `W`, `A`, `S`, `D`, or the arrow keys.
- Tiles slide smoothly into place on each effective move.
- Newly merged tiles pulse larger, then snap elastically back into place.
- Resize the window freely; the complete interface scales with it.
- Undo the most recent effective move, switch between light and dark themes, or reset after confirmation.
- Equal tiles merge once per move and add their value to the score.
- Reset starts a new board while preserving the high score.
- Board, score, and high score are saved after every effective move.

The tray menu can reopen the game or reset its window to 80% of the original design size and center it on the primary screen.

`SquareMasher.json` is stored beside the executable. Keep the executable in a writable folder and move the JSON file with it to carry the current session to another computer. A small `SquareMasher.lock` file prevents two copies from overwriting the same session; it can remain in the folder after the app exits.

## Build

Install stable Rust, then run:

```powershell
cargo test
cargo build --release
```

Windows output is `target\release\square-masher.exe`. Linux output is `target/release/square-masher`. Each binary is standalone; no installer or app-owned runtime is required.

The Linux build uses X11, including XWayland sessions, because native Wayland does not provide a portable always-on-top request. Its tray uses StatusNotifierItem; desktop environments without a built-in tray, including stock GNOME, need an AppIndicator-compatible shell extension.

## Verification

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```
