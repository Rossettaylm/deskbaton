use baton_core::ids::{DisplayId, InputCode};
use baton_core::ports::{DisplayControl, DisplayInfo};
use baton_core::{Error, Result};
use baton_macos_sys as sys;

/// Apple Silicon 上经 IOAVService 读写 DDC。
pub struct MacDdc;

impl DisplayControl for MacDdc {
    fn list(&self) -> Result<Vec<DisplayInfo>> {
        Err(Error::Unsupported("macOS 显示器枚举"))
    }

    fn get_input(&self, _display: &DisplayId) -> Result<InputCode> {
        let mut code = 0u16;
        // SAFETY: `code` 在调用期间有效，原生函数只在返回前写入。
        check(unsafe { sys::deskbaton_ddc_get_input(0, &mut code) })?;
        Ok(InputCode(code))
    }

    fn set_input(&self, _display: &DisplayId, input: InputCode) -> Result<()> {
        // SAFETY: 只传值参数。
        check(unsafe { sys::deskbaton_ddc_set_input(0, input.0) })
    }
}

fn check(rc: i32) -> Result<()> {
    match rc {
        sys::OK => Ok(()),
        sys::ERR_UNIMPLEMENTED => Err(Error::Unsupported("macOS DDC/CI")),
        other => Err(Error::Ddc(format!("原生错误码 {other}"))),
    }
}
