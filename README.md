# port-rust-windows-ime

Rust port target for https://github.com/bombless/port-golang-windows-ime.

Initial migration scaffold: separate a testable composition engine from the Windows TSF COM adapter. This is not yet a production-ready TSF DLL. COM interface implementations, edit-session integration, TIP registration, candidate UI, and complete dictionary/ranking parity remain to be implemented and validated in a real Windows TSF host.

Build and test with `cargo test` and `cargo check`.