//! Invokes the COM registration exports of the TSF DLL.
//!
//! Port of `cmd/registerhost`. The Go DLL could not be registered with
//! `regsvr32` (it crashed on return, a Go runtime problem); this tool called the
//! export directly. The Rust DLL has no such problem, but the installer keeps
//! using this so registration behaves identically and reports the raw HRESULT.
//!
//! usage: registerhost register|unregister <dll>

#[cfg(not(windows))]
fn main() {
    eprintln!("registerhost only runs on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    use std::ffi::CString;
    use windows::core::{HSTRING, PCSTR};
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: registerhost register|unregister <dll>");
        std::process::exit(2);
    }
    let export = match args[1].as_str() {
        "register" => "DllRegisterServer",
        "unregister" => "DllUnregisterServer",
        other => {
            eprintln!("unknown action: {other}");
            std::process::exit(2);
        }
    };

    let module = match unsafe { LoadLibraryW(&HSTRING::from(args[2].as_str())) } {
        Ok(module) => module,
        Err(error) => {
            eprintln!("LoadLibraryW failed: {error}");
            std::process::exit(1);
        }
    };
    let name = CString::new(export).expect("export names have no NUL");
    let Some(address) = (unsafe { GetProcAddress(module, PCSTR(name.as_ptr().cast())) }) else {
        eprintln!("GetProcAddress({export}) failed: {}", windows::core::Error::from_thread());
        std::process::exit(1);
    };
    // SAFETY: DllRegisterServer / DllUnregisterServer are `HRESULT (STDAPICALLTYPE *)(void)`.
    let call: unsafe extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
    let hr = unsafe { call() };
    eprintln!("{export} hr 0x{:08X}", hr as u32);
    unsafe {
        let _ = FreeLibrary(module);
    }
    // Anything but S_OK counts as failure, S_FALSE included.
    std::process::exit(if hr != 0 { 1 } else { 0 });
}
