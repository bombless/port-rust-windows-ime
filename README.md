# port-rust-windows-ime

A standalone [Text Services Framework](https://learn.microsoft.com/windows/win32/tsf/text-services-framework)
(TSF) pinyin input method for Windows, written in Rust. It is a port of
[`bombless/port-golang-windows-ime`](https://github.com/bombless/port-golang-windows-ime):
the same input method, the same dictionary, the same key behaviour, built as a
`cdylib` on the [`windows`](https://crates.io/crates/windows) crate instead of Go + cgo.

The DLL is loaded by Windows into whatever application you type into. There is no
host process, no pipe and no companion UI program: the dictionary is embedded, the
engine runs in-process, and the candidate window is drawn by the DLL itself.

> **Status: experimental.** The engine is verified against the Go original and the
> COM/edit-session logic is tested against a fake TSF host in CI, but the DLL has
> **not yet been exercised in a real application**. See [PLAN.md](PLAN.md) for the
> acceptance checklist. Do not register it on a machine you cannot recover.

## What it does

- Type pinyin; a numbered candidate list (up to 9) follows the caret.
- `Space` / `Enter` commit the selected candidate, `1`–`9` pick one, arrows move
  the selection, `Backspace` / `Delete` edit, `Esc` cancels.
- Ctrl-, Alt- and Win-shortcuts are never touched; they go to the application.
- Multi-syllable input is segmented into dictionary words (`nihao` → 你好,
  `zhongguo` → 中国), and half-typed input shows prefix matches.

## Layout

| Path | What |
|---|---|
| `src/engine.rs` | Dictionary parser, candidate generation, key reducer. Pure. |
| `src/candidate_ui.rs` | Candidate view model and the `Presenter` seam. Pure. |
| `src/layout.rs` | Candidate-window text and placement. Pure. |
| `src/session.rs` | Per-service state shared by the key path. Pure. |
| `src/diag.rs` | Log file, panic guard, in-flight callback tracking, dump requests. |
| `src/guids.rs` | CLSID, profile and category identities. |
| `src/win/` | Everything Windows: COM text service, class factory and DLL exports, edit sessions, registration, the candidate window. |
| `src/win/fake_tsf.rs` | Test-only fake TSF host (document, ranges, compositions, context). |
| `src/bin/registerhost.rs` | Calls `DllRegisterServer` / `DllUnregisterServer` and prints the raw HRESULT. |
| `data/pinyin_simp.dict.yaml` | The embedded dictionary, unchanged from the Go repo. |
| `tools/differential/` | Runs the original Go engine against this one. |
| `install-dll.ps1`, `uninstall.ps1` | Build, register and remove the input method. |

Everything outside `src/win/` builds and tests on any OS.

## Build and test

```powershell
cargo test                      # everything, including the COM tests on Windows
cargo build --release           # target\release\port_rust_windows_ime.dll + registerhost.exe
```

On Linux or macOS `cargo test` runs the portable half (engine, model, session,
diagnostics, layout). The differential test needs `git`, `go` and `python3`:

```sh
tools/differential/run.sh       # Go original vs Rust: candidates and key sequences must match exactly
```

CI (`.github/workflows/ci.yml`) runs formatting, the portable tests and the
differential test on Linux, and on Windows: clippy, the full test suite, and an
end-to-end *register → `CoCreateInstance` → unregister* check against the real
registry and COM.

## Install (experimental)

From an elevated or non-elevated PowerShell on 64-bit Windows with a Rust toolchain:

```powershell
.\install-dll.ps1               # builds, copies a versioned DLL to install\, registers it, restarts ctfmon
.\uninstall.ps1                 # unregisters it and removes install\PortRustWindowsIME-*.dll
```

The installer asks for elevation itself (TSF registration writes `HKLM`). Afterwards
restart the target application and pick **Rust Chinese Input Method** in the
language switcher.

### Identity: side by side with the Go TIP

This port deliberately has its **own** CLSID and profile GUID, so it can be
installed next to the Go input method and the two compared on the same machine:

| | Go TIP | This crate |
|---|---|---|
| Name | Go Chinese Input Method | Rust Chinese Input Method |
| CLSID | `{C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072}` | `{32200908-37C6-4149-AED4-664A78DB2082}` |
| Profile | `{E731B5D4-2A90-4F68-AC13-7D5E9B2046F1}` | `{89BD0007-8910-4365-A02A-4B975567138D}` |
| Language | zh-CN `0x0804` | zh-CN `0x0804` |
| Threading | Apartment | Apartment |

To make it a drop-in *replacement* for the Go TIP instead, set the same GUIDs in
`src/guids.rs` and `install-dll.ps1` (and drop the Go identity from the collision
lists there). Never register both identities: they would overwrite each other.

## Diagnostics

All of these are environment variables of the *application hosting the DLL*.

| Variable | Effect |
|---|---|
| `PortRustWindowsIMELogPath` | Log file; default `%TEMP%\PortRustWindowsIME.log`. |
| `PortRustWindowsIMELogText=1` | Log composition and committed text verbatim. **Off by default**: the log would otherwise record everything typed. |
| `PortRustWindowsIMEPauseAt` | `Activate` or `Deactivate`: freeze the calling thread there so a dump can be taken. Hangs the host on purpose. |

Create `%TEMP%\PortRustWindowsIME.dump` and, within a second, the first process with
the DLL loaded that notices it appends a *dump* to the log and deletes the file (create
it again to ask the next process): the TSF callbacks currently running (a callback
stuck on something shows up with a large elapsed time), the most recent finished ones,
and a stack of the dumping thread. A panic inside any callback is
caught, logged with a backtrace, and also produces a dump. The candidate window logs
metadata only (never text) to `%LOCALAPPDATA%\PortRustWindowsIME\candidate-debug.log`.

## Differences from the Go version

See [MIGRATION.md](MIGRATION.md): what was ported, what was deliberately left behind,
every intentional behaviour change, and the quirks inherited unchanged.

## Third-party data

The dictionary's provenance is recorded in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
