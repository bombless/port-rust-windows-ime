//! COM server and TSF profile registration.
//!
//! Port of the C `registerComServer` / `registerTip` / `runRegistration` helpers.
//! Writing `HKEY_CLASSES_ROOT\CLSID` and the TSF profile database needs
//! administrator rights, exactly as it did for the Go DLL.

use windows::core::{Error, Result, HRESULT, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, MAX_PATH, RPC_E_CHANGED_MODE, S_OK};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CLASSES_ROOT, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, ITfCategoryMgr, ITfInputProcessorProfiles,
};

use super::{guid, this_module};
use crate::guids::{guid_braced, CLSID_TEXT_SERVICE, LANG_ID_ZH_CN, PROFILE_GUID, SERVICE_NAME, TIP_CATEGORIES};
use crate::session::utf16_nul_terminated;
use crate::tip_log;

fn clsid_key() -> String {
    format!("CLSID\\{}", guid_braced(CLSID_TEXT_SERVICE))
}

/// Path of this DLL, as the registry has to record it.
fn module_path() -> Result<String> {
    let module = this_module()?;
    let mut buffer = vec![0u16; MAX_PATH as usize * 4];
    let length = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(Error::from_thread());
    }
    Ok(String::from_utf16_lossy(&buffer[..length]))
}

/// Sets a `REG_SZ` value, creating the key. `name` of `None` is the default value.
fn write_string(root: HKEY, sub_key: &str, name: Option<&str>, value: &str) -> Result<()> {
    let sub_key = HSTRING::from(sub_key);
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            root,
            &sub_key,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    }
    .ok()?;
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) };
    let name = name.map(HSTRING::from);
    let value_name = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let result = unsafe { RegSetValueExW(key, value_name, None, REG_SZ, Some(bytes)) };
    unsafe {
        let _ = RegCloseKey(key);
    }
    result.ok()
}

fn register_com_server() -> Result<()> {
    let path = module_path()?;
    let key = clsid_key();
    write_string(HKEY_CLASSES_ROOT, &key, None, SERVICE_NAME)?;
    let inproc = format!("{key}\\InprocServer32");
    write_string(HKEY_CLASSES_ROOT, &inproc, None, &path)?;
    write_string(HKEY_CLASSES_ROOT, &inproc, Some("ThreadingModel"), "Apartment")
}

fn unregister_com_server() -> Result<()> {
    let result = unsafe { RegDeleteTreeW(HKEY_CLASSES_ROOT, &HSTRING::from(clsid_key())) };
    // Nothing to delete is as good as deleted.
    if result == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        result.ok()
    }
}

/// Registers the text service and its zh-CN profile, then the categories.
fn register_tip() -> Result<()> {
    let clsid = guid(CLSID_TEXT_SERVICE);
    let profile = guid(PROFILE_GUID);
    let profiles: ITfInputProcessorProfiles =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) }?;
    unsafe { profiles.Register(&clsid) }?;

    // The `windows` wrapper for AddLanguageProfile takes the icon file as a slice
    // and would pass a dangling pointer for an empty one. The Go/C registration
    // passed NULL with a zero length; calling the vtable entry directly keeps
    // that exactly.
    // NUL-terminated even though the length is passed too: TSF reads the name up
    // to a NUL regardless (see `utf16_nul_terminated`).
    let name = utf16_nul_terminated(SERVICE_NAME).expect("the service name has no NUL");
    let hr = unsafe {
        use windows::core::Interface;
        (Interface::vtable(&profiles).AddLanguageProfile)(
            Interface::as_raw(&profiles),
            &clsid,
            LANG_ID_ZH_CN,
            &profile,
            PCWSTR(name.as_ptr()),
            (name.len() - 1) as u32,
            PCWSTR::null(),
            0,
            0,
        )
    };
    hr.ok()?;

    let categories: ITfCategoryMgr = unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) }?;
    // Every category is attempted; the first failure is what gets reported.
    let mut first_error = None;
    for category in TIP_CATEGORIES {
        if let Err(error) = unsafe { categories.RegisterCategory(&clsid, &guid(category), &clsid) } {
            tip_log!("RegisterCategory {} failed hr=0x{:08x}", guid_braced(category), error.code().0 as u32);
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn unregister_tip() -> Result<()> {
    let clsid = guid(CLSID_TEXT_SERVICE);
    if let Ok(categories) =
        unsafe { CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) }
    {
        for category in TIP_CATEGORIES {
            // A category that was never registered is not an error worth stopping for.
            let _ = unsafe { categories.UnregisterCategory(&clsid, &guid(category), &clsid) };
        }
    }
    let profiles: ITfInputProcessorProfiles =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) }?;
    unsafe { profiles.Unregister(&clsid) }
}

/// Entry point for `DllRegisterServer` / `DllUnregisterServer`.
pub(crate) fn run_registration(unregister: bool) -> HRESULT {
    // COM may already be initialised in another mode (regsvr32 and registerhost
    // differ); that is fine, we just must not uninitialise what we did not init.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if init.is_err() && init != RPC_E_CHANGED_MODE {
        return init;
    }
    let result = if unregister {
        unregister_tip().and_then(|()| unregister_com_server())
    } else {
        let registered = register_com_server().and_then(|()| register_tip());
        if registered.is_err() {
            // Do not leave a half-registered service behind: undo whatever part of the
            // TSF profile and the COM class got written. Both only touch this
            // service's own CLSID, so unrelated registrations are never at risk.
            let _ = unregister_tip();
            let _ = unregister_com_server();
        }
        registered
    };
    if init.is_ok() {
        unsafe { CoUninitialize() };
    }
    let hr = result.map_or_else(|error| error.code(), |()| S_OK);
    tip_log!("{} hr=0x{:08x}", if unregister { "DllUnregisterServer" } else { "DllRegisterServer" }, hr.0 as u32);
    hr
}
