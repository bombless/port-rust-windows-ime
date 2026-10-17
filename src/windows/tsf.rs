use windows::core::GUID;

/// Stable CLSID reserved for the Rust TSF input method.
pub const CLSID_PORT_RUST_IME: GUID = GUID::from_u128(0x5f70b5e6_8a7c_4e3c_9d1a_0f6c2b7a4101);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ActivationState { #[default] Inactive, Active }

/// Lifecycle placeholder; COM interfaces and TSF edit-session calls are not wired yet.
#[derive(Debug, Default)]
pub struct TsfAdapter { state: ActivationState }
impl TsfAdapter {
    pub fn activate(&mut self) { self.state = ActivationState::Active; }
    pub fn deactivate(&mut self) { self.state = ActivationState::Inactive; }
    pub fn state(&self) -> ActivationState { self.state }
}

#[cfg(test)]
mod tests { use super::*; #[test] fn lifecycle() { let mut a=TsfAdapter::default(); assert_eq!(a.state(),ActivationState::Inactive); a.activate(); assert_eq!(a.state(),ActivationState::Active); a.deactivate(); assert_eq!(a.state(),ActivationState::Inactive); } }
