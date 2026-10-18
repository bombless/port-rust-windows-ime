//! Identities of this text service and the GUIDs the code needs to name.
//!
//! GUIDs are kept as `u128` (the digits of the textual form, in order) so this
//! module builds everywhere; `win` converts with `GUID::from_u128`.
//!
//! The CLSID and profile GUID are deliberately different from the Go TIP's
//! (`{C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072}` / `{E731B5D4-…}`) so both can be
//! installed side by side.

/// CLSID of the text service. `{32200908-37C6-4149-AED4-664A78DB2082}`
pub const CLSID_TEXT_SERVICE: u128 = 0x32200908_37C6_4149_AED4_664A78DB2082;
/// Language-profile GUID. `{89BD0007-8910-4365-A02A-4B975567138D}`
pub const PROFILE_GUID: u128 = 0x89BD0007_8910_4365_A02A_4B975567138D;
/// Name shown in the language switcher.
pub const SERVICE_NAME: &str = "Rust Chinese Input Method";
/// Simplified Chinese (zh-CN) is the only language this profile registers for.
pub const LANG_ID_ZH_CN: u16 = 0x0804;

/// The Go TIP's identity, kept so installers can refuse to collide with it.
pub const GO_TIP_CLSID: u128 = 0xC2D4A9B1_7F63_4A85_9E20_4B6D8C1F3072;
pub const GO_TIP_PROFILE: u128 = 0xE731B5D4_2A90_4F68_AC13_7D5E9B2046F1;

// Interfaces the log has to be able to name.
pub const IID_IUNKNOWN: u128 = 0x00000000_0000_0000_C000_000000000046;
pub const IID_ICLASSFACTORY: u128 = 0x00000001_0000_0000_C000_000000000046;
pub const IID_ITF_TEXT_INPUT_PROCESSOR: u128 = 0xAA80E7F7_2021_11D2_93E0_0060B067B86E;
pub const IID_ITF_TEXT_INPUT_PROCESSOR_EX: u128 = 0x6E4E2102_F9CD_433D_B496_303CE03A6507;
pub const IID_ITF_KEY_EVENT_SINK: u128 = 0xAA80E7F5_2021_11D2_93E0_0060B067B86E;

// TSF categories this TIP registers under (msctf.h GUID_TFCAT_*).
pub const CAT_TIP_KEYBOARD: u128 = 0x34745C63_B2F0_4784_8B67_5E12C8701A31;
pub const CAT_TIPCAP_COMLESS: u128 = 0x364215D9_75BC_11D7_A6EF_00065B84435C;
pub const CAT_TIPCAP_IMMERSIVE_SUPPORT: u128 = 0x13A016DF_560B_46CD_947A_4C3AF1E0E35D;
pub const CAT_TIPCAP_SECURE_MODE: u128 = 0x49D2F9CE_1F5E_11D7_A6D3_00065B84435C;
pub const CAT_TIPCAP_SYSTRAY_SUPPORT: u128 = 0x25504FB4_7BAB_4BC1_9C69_CF81890F0EF5;
pub const CAT_TIPCAP_UIELEMENT_ENABLED: u128 = 0x49D2F9CF_1F5E_11D7_A6D3_00065B84435C;

pub const TIP_CATEGORIES: [u128; 6] = [
    CAT_TIP_KEYBOARD,
    CAT_TIPCAP_COMLESS,
    CAT_TIPCAP_IMMERSIVE_SUPPORT,
    CAT_TIPCAP_SECURE_MODE,
    CAT_TIPCAP_SYSTRAY_SUPPORT,
    CAT_TIPCAP_UIELEMENT_ENABLED,
];

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, uppercase.
pub fn guid_braced(guid: u128) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}",
        (guid >> 96) as u32,
        (guid >> 80) as u16,
        (guid >> 64) as u16,
        (guid >> 48) as u16,
        guid & 0xFFFF_FFFF_FFFF
    )
}

/// Names the GUIDs that matter and prints the rest: a bare GUID in the log is
/// useless when the question is "which interface did TSF ask for".
pub fn guid_text(guid: u128) -> String {
    let name = match guid {
        CLSID_TEXT_SERVICE => "CLSID_RustTextService",
        IID_IUNKNOWN => "IID_IUnknown",
        IID_ICLASSFACTORY => "IID_IClassFactory",
        IID_ITF_TEXT_INPUT_PROCESSOR => "IID_ITfTextInputProcessor",
        IID_ITF_TEXT_INPUT_PROCESSOR_EX => "IID_ITfTextInputProcessorEx",
        IID_ITF_KEY_EVENT_SINK => "IID_ITfKeyEventSink",
        _ => return guid_braced(guid),
    };
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braced_form_matches_the_registry_spelling() {
        assert_eq!(guid_braced(CLSID_TEXT_SERVICE), "{32200908-37C6-4149-AED4-664A78DB2082}");
        assert_eq!(guid_braced(PROFILE_GUID), "{89BD0007-8910-4365-A02A-4B975567138D}");
        assert_eq!(guid_braced(IID_IUNKNOWN), "{00000000-0000-0000-C000-000000000046}");
    }

    // port of TestGuidTextNamesKnownInterfaces
    #[test]
    fn known_guids_are_named_and_unknown_ones_stay_printable() {
        assert_eq!(guid_text(CLSID_TEXT_SERVICE), "CLSID_RustTextService");
        assert_eq!(guid_text(IID_ITF_TEXT_INPUT_PROCESSOR), "IID_ITfTextInputProcessor");
        assert_eq!(guid_text(IID_ITF_TEXT_INPUT_PROCESSOR_EX), "IID_ITfTextInputProcessorEx");
        assert_eq!(guid_text(IID_ITF_KEY_EVENT_SINK), "IID_ITfKeyEventSink");
        let unknown = guid_text(0x11223344_0000_0000_0000_000000000000);
        assert!(unknown.starts_with("{11223344-"), "got {unknown}");
    }

    #[test]
    fn identity_does_not_collide_with_the_go_tip() {
        assert_ne!(CLSID_TEXT_SERVICE, GO_TIP_CLSID);
        assert_ne!(PROFILE_GUID, GO_TIP_PROFILE);
        assert_eq!(guid_braced(GO_TIP_CLSID), "{C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072}");
    }

    #[test]
    fn categories_are_distinct() {
        let mut all = TIP_CATEGORIES.to_vec();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 6);
    }
}
