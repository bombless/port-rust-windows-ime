# port-rust-windows-ime

A Rust port of [port-golang-windows-ime](https://github.com/bombless/port-golang-windows-ime), implemented as a Windows Text Services Framework (TSF) text service.

## Status

The project contains a Rust COM/TSF implementation and a prototype input engine.

**Manual Windows validation:** The project maintainer has installed and tried the built DLL on a real Windows machine. This confirms that the DLL has been exercised in a real Windows environment; it does not by itself claim exhaustive coverage of all TSF hosts, input scenarios, failure paths, or edge cases.

Implemented:

- A Rust composition engine with unit tests and a bundled Luna Pinyin dictionary.
- Source-compatible TSF CLSID and language-profile GUID constants.
- COM class factory and DLL exports: `DllGetClassObject`, `DllCanUnloadNow`, `DllRegisterServer`, and `DllUnregisterServer`.
- Registration of the in-process COM server, zh-CN language profile, and six TSF categories.
- Initial `ITfTextInputProcessor` activation/deactivation and `ITfKeyEventSink` registration.
- A synchronous `ITfEditSession` path that requests a TSF write lock and updates text through `ITfRange::SetText`, with explicit caret placement and tracked composition range.
- A first-pass native candidate popup showing the composition and up to nine candidates.

Further validation and hardening:

- Broader coverage across TSF hosts, applications, selection/range conditions, and input scenarios.
- Edit-session rollback and caret behavior when TSF denies or fails an edit.
- COM ABI/vtable correctness, failure paths, repeated activation/deactivation, and complete resource cleanup.
- Candidate UI caret anchoring, monitor/work-area bounds, per-monitor DPI, candidate interaction, and UI-thread ownership.
- Full behavior parity with the Go implementation, including dictionary loading/ranking and input edge cases.
- A supported uninstall workflow.

## Build and test

Install a Rust toolchain and the `x86_64-pc-windows-msvc` target. Run the following checks:

```powershell
cargo fmt --all -- --check
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release --target x86_64-pc-windows-msvc
```

GitHub Actions runs formatting, tests, and Clippy on Ubuntu and Windows, and builds the Windows x64 release DLL. These automated checks complement the maintainer's manual Windows testing; they do not replace broader compatibility and edge-case testing. The automatic GitHub Release workflow is triggered by pushing a tag matching `v*`; its end-to-end release run should be confirmed after the workflow is introduced.

## COM identity

- CLSID: `C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072`
- Language profile GUID: `E731B5D4-2A90-4F68-AC13-7D5E9B2046F1`
- In-process threading model: `Apartment`

The identifiers were checked against the native declarations in `tip/main.go` in the Go source repository.

## Register the x64 DLL (experimental)

Registration currently supports 64-bit Windows. Open PowerShell **as Administrator** and provide the built or downloaded DLL path:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\install.ps1 .\port_rust_windows_ime.dll
```

For a GitHub Actions build, download and extract the Windows x64 DLL artifact, then pass the extracted `port_rust_windows_ime.dll` to the script. The script invokes the native x64 `regsvr32.exe` and verifies the registered DLL path and `Apartment` threading model.

This is a registration helper, not a production installer. There is no companion uninstall script yet.
