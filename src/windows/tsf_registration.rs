//! TSF language-profile and category registration matching the Go runRegistration path.
//!
//! The interfaces are called through their documented COM vtables so this module
//! does not depend on generated windows-rs method wrappers. Windows execution is
//! still required to validate registration against the system TSF implementation.

use std::ffi::c_void;
use windows::core::{GUID, HRESULT};

use super::tsf::{CLSID_PORT_RUST_IME, GUID_PORT_RUST_IME_PROFILE};

const S_OK: HRESULT = HRESULT(0);
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const CLSCTX_INPROC_SERVER: u32 = 0x1;
const LANGID_ZH_CN: u16 = 0x0804;

const CLSID_TF_INPUT_PROCESSOR_PROFILES: GUID =
    GUID::from_u128(0x33c53a50_f456_4884_b049_85fd643ecfed);
const IID_ITF_INPUT_PROCESSOR_PROFILES: GUID =
    GUID::from_u128(0x1f02b6c5_7842_4ee6_8a0b_9a24183a95ca);
const CLSID_TF_CATEGORY_MGR: GUID =
    GUID::from_u128(0xa4b544a1_438d_4b41_9325_869523e2d6c7);
const IID_ITF_CATEGORY_MGR: GUID =
    GUID::from_u128(0xc3acefb5_f69d_4905_938f_fcadcf4be830);

const CAT_TIP_KEYBOARD: GUID =
    GUID::from_u128(0x34745c63_b2f0_4784_8b67_5e12c8701a31);
const CAT_TIP_COMLESS: GUID =
    GUID::from_u128(0x364215d9_75bc_11d7_a6ef_00065b84435c);
const CAT_TIP_IMMERSIVE: GUID =
    GUID::from_u128(0x13a016df_560b_46cd_947a_4c3af1e0e35d);
const CAT_TIP_SECURE: GUID =
    GUID::from_u128(0x49d2f9ce_1f5e_11d7_a6d3_00065b84435c);
const CAT_TIP_SYSTRAY: GUID =
    GUID::from_u128(0x25504fb4_7bab_4bc1_9c69_cf81890f0ef5);
const CAT_TIP_UI_ELEMENT: GUID =
    GUID::from_u128(0x49d2f9cf_1f5e_11d7_a6d3_00065b84435c);

const CATEGORIES: [GUID; 6] = [
    CAT_TIP_KEYBOARD,
    CAT_TIP_COMLESS,
    CAT_TIP_IMMERSIVE,
    CAT_TIP_SECURE,
    CAT_TIP_SYSTRAY,
    CAT_TIP_UI_ELEMENT,
];

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, co_init: u32) -> HRESULT;
    fn CoUninitialize();
    fn CoCreateInstance(
        class: *const GUID,
        outer: *mut c_void,
        context: u32,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> HRESULT;
}

type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
type RegisterFn = unsafe extern "system" fn(*mut c_void, *const GUID) -> HRESULT;
type UnregisterFn = unsafe extern "system" fn(*mut c_void, *const GUID) -> HRESULT;
type AddLanguageProfileFn = unsafe extern "system" fn(
    *mut c_void, *const GUID, u16, *const GUID, *const u16, u32, *const u16, u32, u16,
) -> HRESULT;
type RemoveLanguageProfileFn =
    unsafe extern "system" fn(*mut c_void, *const GUID, u16, *const GUID);
type CategoryFn =
    unsafe extern "system" fn(*mut c_void, *const GUID, *const GUID, *const GUID) -> HRESULT;

unsafe fn method<T: Copy>(object: *mut c_void, index: usize) -> T {
    let vtable = *(object as *mut *mut *mut c_void);
    std::mem::transmute_copy(&*vtable.add(index))
}

struct ComPtr(*mut c_void);
impl Drop for ComPtr {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let release: ReleaseFn = method(self.0, 2);
                release(self.0);
            }
        }
    }
}

fn succeeded(hr: HRESULT) -> bool {
    hr.0 >= 0
}

unsafe fn create(class: &GUID, iid: &GUID) -> ResultCom<ComPtr> {
    let mut object = std::ptr::null_mut();
    let hr = CoCreateInstance(
        class,
        std::ptr::null_mut(),
        CLSCTX_INPROC_SERVER,
        iid,
        &mut object,
    );
    if succeeded(hr) && !object.is_null() {
        Ok(ComPtr(object))
    } else if succeeded(hr) {
        Err(HRESULT(0x80004003u32 as i32))
    } else {
        Err(hr)
    }
}

type ResultCom<T> = std::result::Result<T, HRESULT>;

unsafe fn register_tip_inner() -> HRESULT {
    let profiles = match create(
        &CLSID_TF_INPUT_PROCESSOR_PROFILES,
        &IID_ITF_INPUT_PROCESSOR_PROFILES,
    ) {
        Ok(value) => value,
        Err(hr) => return hr,
    };
    let register: RegisterFn = method(profiles.0, 3);
    let unregister: UnregisterFn = method(profiles.0, 4);
    let add_profile: AddLanguageProfileFn = method(profiles.0, 5);
    let remove_profile: RemoveLanguageProfileFn = method(profiles.0, 6);

    let mut hr = register(profiles.0, &CLSID_PORT_RUST_IME);
    if !succeeded(hr) {
        return hr;
    }

    // Clean up profiles left under older language IDs by previous builds.
    remove_profile(profiles.0, &CLSID_PORT_RUST_IME, 0x0409, &GUID_PORT_RUST_IME_PROFILE);
    remove_profile(profiles.0, &CLSID_PORT_RUST_IME, 0x0411, &GUID_PORT_RUST_IME_PROFILE);

    let name: Vec<u16> = "Go Chinese Input Method".encode_utf16().collect();
    hr = add_profile(
        profiles.0,
        &CLSID_PORT_RUST_IME,
        LANGID_ZH_CN,
        &GUID_PORT_RUST_IME_PROFILE,
        name.as_ptr(),
        name.len() as u32,
        std::ptr::null(),
        0,
        0,
    );
    if !succeeded(hr) {
        let _ = unregister(profiles.0, &CLSID_PORT_RUST_IME);
        return hr;
    }

    let categories = match create(&CLSID_TF_CATEGORY_MGR, &IID_ITF_CATEGORY_MGR) {
        Ok(value) => value,
        Err(error) => {
            let _ = unregister(profiles.0, &CLSID_PORT_RUST_IME);
            return error;
        }
    };
    let register_category: CategoryFn = method(categories.0, 3);
    for category in CATEGORIES.iter() {
        let category_hr = register_category(
            categories.0,
            &CLSID_PORT_RUST_IME,
            category,
            &CLSID_PORT_RUST_IME,
        );
        if !succeeded(category_hr) {
            // Keep the first failure visible to regsvr32; unregister the profile
            // below rather than claiming registration succeeded partially.
            let _ = unregister(profiles.0, &CLSID_PORT_RUST_IME);
            return category_hr;
        }
    }
    S_OK
}

unsafe fn unregister_tip_inner() -> HRESULT {
    // Category cleanup is best effort, matching the source implementation.
    if let Ok(categories) = create(&CLSID_TF_CATEGORY_MGR, &IID_ITF_CATEGORY_MGR) {
        let unregister_category: CategoryFn = method(categories.0, 4);
        for category in CATEGORIES.iter() {
            let _ = unregister_category(
                categories.0,
                &CLSID_PORT_RUST_IME,
                category,
                &CLSID_PORT_RUST_IME,
            );
        }
    }

    let profiles = match create(
        &CLSID_TF_INPUT_PROCESSOR_PROFILES,
        &IID_ITF_INPUT_PROCESSOR_PROFILES,
    ) {
        Ok(value) => value,
        Err(hr) => return hr,
    };
    let unregister: UnregisterFn = method(profiles.0, 4);
    unregister(profiles.0, &CLSID_PORT_RUST_IME)
}

/// Register the TIP with COM initialized on this thread. RPC_E_CHANGED_MODE is
/// accepted because callers may already have initialized COM in another mode.
pub unsafe fn register_tip() -> HRESULT {
    let init = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
    if init.0 < 0 && init.0 != 0x80010106u32 as i32 {
        return init;
    }
    let hr = register_tip_inner();
    if succeeded(init) {
        CoUninitialize();
    }
    hr
}

/// Unregister language profiles and TSF categories before removing the CLSID.
pub unsafe fn unregister_tip() -> HRESULT {
    let init = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
    if init.0 < 0 && init.0 != 0x80010106u32 as i32 {
        return init;
    }
    let hr = unregister_tip_inner();
    if succeeded(init) {
        CoUninitialize();
    }
    hr
}
