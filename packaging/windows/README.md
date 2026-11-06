# Windows packaging

`voime-tray.exe` is the per-user desktop process. It owns the notification area
icon, global hotkey, overlay and the managed `voimed.exe` child process.

- Manifest: `voime-tray.manifest` (per-monitor v2 DPI awareness). `build.rs`
  embeds it and `voime.ico` when `rc.exe` from the Windows SDK is available.
- Icons: `../../assets/icons/voime.ico`, `voime-32.png`, `voime-256.png`.
- Installation: `scripts/install-core.ps1` stops a running tray with `WM_CLOSE`
  (falling back to a force stop only if it does not exit), copies the binaries
  to `%LOCALAPPDATA%\Programs\Voime`, and writes the current-user Run key when
  it differs. It never launches the new tray; start it manually or at sign-in.
- GPU workers and runtimes are installed by `scripts/install-gpu.ps1`.
- Unsigned local builds can be flagged by Defender as
  `Behavior:Win32/Persistence.A!ml` because the tray manages a Run-key login
  item. Do not disable Defender or add exclusions for release use; sign the
  binaries and submit false positives to Microsoft. Starting the tray manually
  after installation does not repeat the installer's copy/write/launch chain.
