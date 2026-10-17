//! Minimal COM registry registration for the in-process TSF DLL.
//!
//! This registers the CLSID and Apartment threading model only. TSF language
//! profile/category registration is a separate step and is not implemented yet.

use std::{
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
};

use windows::core::{GUID, HRESULT};

use super::tsf::CLSID_PORT_RUST_IME;

const HKEY_CLASSES_ROOT: isize = 0x8000_0000u32 as isize;
const KEY_SET_VALUE: u32 = 0x0002;
const REG_OPTION_NON_VOLATILE: u32 = 0;
const REG_SZ: u32 = 1;
const ERROR_SUCCESS: i32 = 0;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;
const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x0000_0002;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleExW(flags: u32, module_name: *const u16, module: *mut *mut c_void) -> i32;
    fn GetModuleFileNameW(module: *mut c_void, filename: *mut u16, size: u32) -> u32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        key: isize,
        sub_key: *const u16,
        reserved: u32,
        class: *mut u16,
        options: u32,
        desired_access: u32,
        security_attributes: *mut c_void,
        result: *mut isize,
        disposition: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: isize,
        value_name: *const u16,
        reserved: u32,
        value_type: u32,
        data: *const u8,
        data_size: u32,
    ) -> i32;
    fn RegCloseKey(key: isize) -> i32;
    fn RegDeleteTreeW(key: isize, sub_key: *const u16) -> i32;
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn hresult_from_win32(error: i32) -> HRESULT {
    if error <= 0 {
        HRESULT(error)
    } else {
        HRESULT(((error as u32 & 0x0000_ffff) | 0x8007_0000) as i32)
    }
}

fn clsid_key() -> String {
    format!("CLSID\\{{{}}}", format_guid(CLSID_PORT_RUST_IME))
}

fn format_guid(guid: GUID) -> String {
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )
}

unsafe fn write_string(sub_key: &str, value_name: Option<&str>, value: &str) -> HRESULT {
    let sub_key = wide(sub_key);
    let value_name = value_name.map(wide).unwrap_or_else(|| vec![0]);
    let value = wide(value);
    let mut key = 0isize;
    let mut disposition = 0u32;
    let result = RegCreateKeyExW(
        HKEY_CLASSES_ROOT,
        sub_key.as_ptr(),
        0,
        std::ptr::null_mut(),
        REG_OPTION_NON_VOLATILE,
        KEY_SET_VALUE,
        std::ptr::null_mut(),
        &mut key,
        &mut disposition,
    );
    if result != ERROR_SUCCESS {
        return hresult_from_win32(result);
    }

    let result = RegSetValueExW(
        key,
        value_name.as_ptr(),
        0,
        REG_SZ,
        value.as_ptr().cast(),
        (value.len() * std::mem::size_of::<u16>()) as u32,
    );
    let close_result = RegCloseKey(key);
    if result != ERROR_SUCCESS {
        hresult_from_win32(result)
    } else {
        hresult_from_win32(close_result)
    }
}

unsafe fn register_server() -> HRESULT {
    let address = DllRegisterServer as *const () as *const u16;
    let mut module = std::ptr::null_mut();
    if GetModuleHandleExW(
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        address,
        &mut module,
    ) == 0
    {
        return hresult_from_win32(1);
    }

    let mut path = vec![0u16; 32768];
    let length = GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32);
    if length == 0 || length as usize >= path.len() {
        return hresult_from_win32(1);
    }
    path.truncate(length as usize);
    path.push(0);
    let path = String::from_utf16_lossy(&path[..path.len() - 1]);

    let root = clsid_key();
    let result = write_string(&root, None, "Rust Windows IME");
    if result.is_err() {
        return result;
    }
    let inproc = format!("{root}\\InprocServer32");
    let result = write_string(&inproc, None, &path);
    if result.is_err() {
        return result;
    }
    write_string(&inproc, Some("ThreadingModel"), "Apartment")
}

unsafe fn unregister_server() -> HRESULT {
    let key = wide(&clsid_key());
    let result = RegDeleteTreeW(HKEY_CLASSES_ROOT, key.as_ptr());
    if result == ERROR_SUCCESS || result == ERROR_FILE_NOT_FOUND {
        HRESULT(0)
    } else {
        hresult_from_win32(result)
    }
}

/// Registers the COM CLSID and in-process server path.
///
/// This does not register the TSF language profile; the service will not yet
/// appear as a selectable input method until profile registration is added.
///
/// # Safety
/// Called by COM with the standard zero-argument DLL export ABI.
#[no_mangle]
pub unsafe extern "system" fn DllRegisterServer() -> HRESULT {
    match catch_unwind(AssertUnwindSafe(|| register_server())) {
        Ok(result) => result,
        Err(_) => HRESULT(0x8000_4005u32 as i32),
    }
}

/// Removes the COM CLSID registration created by DllRegisterServer.
///
/// # Safety
/// Called by COM with the standard zero-argument DLL export ABI.
#[no_mangle]
pub unsafe extern "system" fn DllUnregisterServer() -> HRESULT {
    match catch_unwind(AssertUnwindSafe(|| unregister_server())) {
        Ok(result) => result,
        Err(_) => HRESULT(0x8000_4005u32 as i32),
    }
}
