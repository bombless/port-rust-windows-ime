# TSF COM implementation notes

This document records the source-level compatibility decisions and the current implementation boundary for the Rust TSF service. It is not a substitute for Windows host testing.

## Source identity and registration

The Go source implementation in `bombless/port-golang-windows-ime/tip/main.go` uses:

- CLSID: `{C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072}`
- Language profile GUID: `{E731B5D4-2A90-4F68-AC13-7D5E9B2046F1}`
- Display name in the Rust registration: `Rust Chinese Input Method`
- Language profile: `0x0804` (Simplified Chinese, zh-CN)
- COM in-process threading model: `Apartment`
- Text-service interfaces currently implemented: `ITfTextInputProcessor` and `ITfKeyEventSink`

The Rust implementation exports a COM class factory and registration entry points, registers the language profile and TSF categories, advises the key-event sink on activation, and unadvises it on deactivation. It also requests synchronous TSF edit sessions for composition updates and uses `ITfRange::SetText` to update text.

## Behavioral invariants

- `OnTestKeyDown` uses the engine prediction path and must not mutate composition or candidate state.
- `OnKeyDown` is the state-changing path and requests a TSF edit session before modifying document text.
- Text updates are performed within the edit-session callback using the supplied edit cookie.
- The service tracks the composition range for subsequent updates and clears it on commit, cancellation, or deactivation.
- Failed edit-session requests restore the previous engine state so the engine does not claim a document update that did not occur.
- COM callbacks must not unwind a Rust panic across the ABI boundary; pointer arguments and HRESULT results must be handled carefully.
- Engine and composition state are owned per service instance. The candidate popup is currently process-global and remains a prototype, so multiple simultaneous service instances and full UI-thread lifecycle are not yet supported.

## Current verification boundary

The repository CI checks formatting, unit tests, Clippy, and a Windows MSVC x64 release build. Passing CI does not validate raw COM vtable calls at runtime, registration behavior, profile visibility, service activation, or document editing in a real TSF host.

Before treating the service as usable, validate on Windows:

1. Register and unregister the DLL and confirm profile visibility.
2. Activate the service in a standard text host such as Notepad.
3. Test composition updates, candidate selection, commit, cancel, Backspace, and arrow keys.
4. Exercise repeated activation/deactivation, focus changes, multiple applications, and cleanup.
5. Review COM vtable indices/signatures against the Windows SDK and verify failure/rollback behavior.

The current implementation is an initial development version. Report automated build results separately from real-host validation.
