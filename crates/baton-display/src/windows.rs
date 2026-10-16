use baton_core::ids::{DisplayId, InputCode};
use baton_core::ports::{DisplayControl, DisplayInfo};
use baton_core::{Error, Result};

/// 经 Dxva2 Monitor Configuration API（`SetVCPFeature` 等）读写 DDC。
pub struct WinDdc;

const WHAT: &str = "Windows DDC/CI";

impl DisplayControl for WinDdc {
    fn list(&self) -> Result<Vec<DisplayInfo>> {
        Err(Error::Unsupported(WHAT))
    }

    fn get_input(&self, _display: &DisplayId) -> Result<InputCode> {
        Err(Error::Unsupported(WHAT))
    }

    fn set_input(&self, _display: &DisplayId, _input: InputCode) -> Result<()> {
        Err(Error::Unsupported(WHAT))
    }
}
