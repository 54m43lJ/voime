# Windows packaging

`voime-tray.exe` is the per-user desktop process. It owns the notification area
icon, global hotkey, overlay and the managed `voimed.exe` child process.

- Manifest: `voime-tray.manifest` (per-monitor v2 DPI awareness).
- Icons: `../../assets/icons/voime.ico`, `voime-32.png`, `voime-256.png`.
- Installation: `scripts/install-core.ps1` copies the binaries to
  `%LOCALAPPDATA%\Programs\Voime` and writes the current-user Run key.
- GPU workers and runtimes are installed by `scripts/install-gpu.ps1`.
