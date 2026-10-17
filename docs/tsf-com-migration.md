# TSF COM migration notes

This document records source facts verified against `bombless/port-golang-windows-ime/tip/main.go` before implementing the Rust COM ABI. It is not a claim that the Rust DLL is loadable yet.

## Source identity and registration

The Go strategy TIP currently uses:

- CLSID: `{C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072}`
- Language profile GUID: `{E731B5D4-2A90-4F68-AC13-7D5E9B2046F1}`
- Text service display name: `Go Chinese Input Method`
- Language profile: `0x0804` (Simplified Chinese, zh-CN)
- COM in-proc threading model: `Apartment`
- Base service IID: `IID_ITfTextInputProcessor` = `{AA80E7F7-2021-11D2-93E0-0060B067B86E}`
- The Go service also exposes `ITfKeyEventSink` and uses `ITfTextInputProcessorEx` in its strategy implementation.

The current Rust crate has a different placeholder CLSID in `src/windows/tsf.rs`. Do not silently register both identities or change an installed identity without an explicit migration decision. Before shipping, choose and document whether Rust is a side-by-side TIP or a replacement for the Go TIP.

## Required COM lifecycle

1. `DllGetClassObject`: validate pointers and CLSID, initialize the output to null, return a class factory supporting `IClassFactory`, and preserve exact HRESULT semantics.
2. `IClassFactory::CreateInstance`: reject aggregation with `CLASS_E_NOAGGREGATION`; create a fresh per-instance text service; honor the requested IID through QueryInterface.
3. `IClassFactory::LockServer`: maintain a process-wide server-lock count.
4. `DllCanUnloadNow`: report `S_OK` only when object and server-lock counts permit unload. A process-global counter must be paired with correct reference-count transitions.
5. `DllRegisterServer` / `DllUnregisterServer`: register/unregister the in-proc COM class and TSF language profile/categories, with rollback on partial registration failure. Avoid deleting unrelated registry entries.
6. `ITfTextInputProcessor::Activate`: validate the thread manager, retain only required COM references, query `ITfKeystrokeMgr`, and advise the key sink for the supplied client ID. Repeated activation must not double-advise.
7. `Deactivate`: unadvise the sink before dropping references; tolerate partial activation and repeated shutdown.

## Key sink and editing invariants

- `OnTestKeyDown` must call the engine's prediction method only; it must not mutate composition/candidate state or touch the document.
- `OnKeyDown` is the state-changing path and must agree with the test result for supported keys.
- Text changes must happen through `ITfContext::RequestEditSession` and an edit-session implementation, not by writing to the document from an arbitrary callback.
- Composition text, committed text, cancellation, selection replacement, and caret placement need separate, explicit edit-session operations. Failed edit-session requests must not be reported as successful text updates.
- COM callbacks must never unwind a Rust panic across the ABI boundary. Validate out-pointers and input pointers before dereferencing them.
- Every TSF service instance owns its own engine and sink lifecycle; no mutable composition state may be shared globally.

## Implementation / verification status

This is a source-analysis checklist only. The Rust `TsfAdapter` remains a lifecycle placeholder until actual interfaces, class factory, exports, edit sessions, registration, and Windows host tests are implemented. Do not mark this work complete based on this document or non-Windows unit tests alone.
