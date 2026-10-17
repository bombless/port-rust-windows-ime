use windows::core::GUID;

/// CLSID used by the source Go TSF implementation.
///
/// Keep this identity stable across language ports so an existing installation
/// and the registered language profile continue to refer to the same TIP.
pub const CLSID_PORT_RUST_IME: GUID = GUID::from_u128(0xc2d4a9b1_7f63_4a85_9e20_4b6d8c1f3072);

/// Language-profile GUID used by the source Go TSF implementation.
pub const GUID_PORT_RUST_IME_PROFILE: GUID =
    GUID::from_u128(0xe731b5d4_2a90_4f68_ac13_7d5e9b2046f1);

/// Lifecycle state used while the real COM activation path is being wired.
///
/// This is not itself a COM object and does not indicate that TSF activation
/// currently works. The COM interfaces, key-event sink, and edit sessions
/// still need to be implemented.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ActivationState {
    #[default]
    Inactive,
    Active,
}

/// Temporary lifecycle model. It is intentionally kept separate from the
/// eventual COM implementation so state-machine tests do not require Windows.
#[derive(Debug, Default)]
pub struct TsfAdapter {
    state: ActivationState,
}

impl TsfAdapter {
    pub fn activate(&mut self) {
        self.state = ActivationState::Active;
    }

    pub fn deactivate(&mut self) {
        self.state = ActivationState::Inactive;
    }

    pub fn state(&self) -> ActivationState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_match_the_go_tip_source() {
        assert_eq!(
            CLSID_PORT_RUST_IME,
            GUID::from_u128(0xc2d4a9b1_7f63_4a85_9e20_4b6d8c1f3072)
        );
        assert_eq!(
            GUID_PORT_RUST_IME_PROFILE,
            GUID::from_u128(0xe731b5d4_2a90_4f68_ac13_7d5e9b2046f1)
        );
    }

    #[test]
    fn lifecycle() {
        let mut adapter = TsfAdapter::default();
        assert_eq!(adapter.state(), ActivationState::Inactive);
        adapter.activate();
        assert_eq!(adapter.state(), ActivationState::Active);
        adapter.deactivate();
        assert_eq!(adapter.state(), ActivationState::Inactive);
    }
}
