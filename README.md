# port-rust-windows-ime

Rust port of [port-golang-windows-ime](https://github.com/bombless/port-golang-windows-ime), targeting a real Windows Text Services Framework (TSF) text service.

## Current status

This is an early implementation and is **not yet a usable production IME**.

Implemented in the current development branch:

- A platform-independent composition engine with unit tests.
- Go-compatible TSF CLSID and language profile GUID constants.
- Initial windows-rs COM class factory (`IClassFactory`) and DLL exports (`DllGetClassObject`, `DllCanUnloadNow`).
- First `DllRegisterServer` / `DllUnregisterServer` exports to register/remove the CLSID, `InprocServer32` path, and `Apartment` threading model.
- Initial `ITfTextInputProcessor` activation/deactivation and `ITfKeyEventSink` registration/unregistration.
- An `ITfEditSession` / `ITfContext::RequestEditSession` synchronous write-lock path. `DoEditSession` now uses the edit cookie with `ITfContext::GetSelection` and `ITfRange::SetText` to update the tracked composition range.
- Key-event sink prediction calls `Engine::test_key`; actual key handling calls `Engine::handle_key` and routes update/commit/cancel outcomes through edit sessions.

Not implemented yet:

- TSF edit-session behavior has been connected to the engine but is not yet confirmed by the current Windows CI run or a real TSF host. Selection/range tracking, edit-session failure rollback, and caret behavior still need validation.
- TSF language-profile and category registration are still missing. The COM registry exports alone do not make the IME appear in Windows language/input settings.
- Candidate UI, full dictionary/ranking parity, and complete Go behavior migration.
- Real Windows host activation and typing acceptance.

## Build and test

On a Rust toolchain with the target installed:

```powershell
cargo fmt --all -- --check
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release --target x86_64-pc-windows-msvc
```

The repository CI runs portable checks on Ubuntu and a Windows MSVC x64 build. The latest Windows run exposed incorrect assumptions about windows-rs 0.58 wrapper signatures; fixes have been committed, but the current head has not yet been confirmed by CI. A successful compile is not proof that TSF profile registration, activation, or text insertion works in a real Windows host.

## COM identity

- CLSID: `C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072`
- Language profile GUID: `E731B5D4-2A90-4F68-AC13-7D5E9B2046F1`
- Intended in-process threading model: `Apartment`

These identifiers were compared with the native C declarations in `tip/main.go` in the Go source repository.

## Validation boundary

Do not register this build as a daily-use input method yet. Until registration is implemented and the edit-session path is validated in Windows CI and a real TSF host, this remains a partial COM/TSF implementation. Windows CI and real Windows TSF host validation must be reported separately.
