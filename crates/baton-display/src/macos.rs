use baton_core::ids::{DisplayId, InputCode};
use baton_core::ports::{DisplayControl, DisplayInfo};
use baton_core::{Error, Result};
use baton_macos_sys::{self as sys, RawDisplayInfo, RawScreenMode};

use crate::{ScreenMode, mccs, retry};

/// DDC/CI 要求主机发完请求后至少等 40 毫秒再读回包。
const REPLY_DELAY_MS: u32 = 50;
/// P275MV 只收到一次请求时回 Null Message，连发两次才回值；对其它显示器重复请求无副作用。
const WRITE_CYCLES: u32 = 2;

/// Apple Silicon 上经 IOAVService 读写 DDC。`DisplayId` 为 EDID UUID，同一 UUID 的后续显示器追加 `#序号`。
pub struct MacDdc;

impl DisplayControl for MacDdc {
    fn list(&self) -> Result<Vec<DisplayInfo>> {
        Ok(to_display_infos(&list_raw()?))
    }

    fn get_input(&self, display: &DisplayId) -> Result<InputCode> {
        let index = index_of(display)?;
        let request = mccs::get_vcp_request(mccs::VCP_INPUT_SOURCE);
        retry(|| {
            let mut reply = [0u8; mccs::VCP_REPLY_LEN];
            transact(index, &request, &mut reply)?;
            match mccs::classify_vcp_reply(mccs::VCP_INPUT_SOURCE, &reply)? {
                // 输入源是 MCCS 非连续值，只有低字节有效；部分显示器会在高字节放非零值。
                mccs::VcpReply::Ready(value) => Ok(InputCode(value & 0xFF)),
                mccs::VcpReply::NotReady => {
                    Err(Error::Ddc("显示器回了空消息，没有给出当前输入源".into()))
                }
            }
        })
    }

    fn set_input(&self, display: &DisplayId, input: InputCode) -> Result<()> {
        let index = index_of(display)?;
        let request = mccs::set_vcp_request(mccs::VCP_INPUT_SOURCE, input.0);
        retry(|| write(index, &request))
    }
}

pub(crate) fn screen_mode(display: &DisplayId) -> Result<ScreenMode> {
    let index = index_of(display)?;
    let mut raw = RawScreenMode::default();
    // SAFETY: `raw` 是局部变量，原生函数只写入这一个结构体。
    match unsafe { sys::deskbaton_screen_mode(index, &mut raw) } {
        sys::OK => Ok(ScreenMode {
            width: raw.width,
            height: raw.height,
            pixel_width: raw.pixel_width,
            pixel_height: raw.pixel_height,
            refresh_hz: (raw.refresh_hz > 0.0).then_some(raw.refresh_hz),
        }),
        sys::ERR_NOT_FOUND => Err(Error::Ddc(format!(
            "系统显示列表里找不到显示器 {}",
            display.0
        ))),
        rc => Err(Error::Ddc(format!("读取显示模式失败（原生错误码 {rc}）"))),
    }
}

fn index_of(display: &DisplayId) -> Result<u32> {
    to_display_infos(&list_raw()?)
        .iter()
        .position(|d| d.id == *display)
        .map(|i| i as u32)
        .ok_or_else(|| Error::Ddc(format!("找不到显示器 {}", display.0)))
}

fn list_raw() -> Result<Vec<RawDisplayInfo>> {
    let mut buf = vec![RawDisplayInfo::EMPTY; 4];
    loop {
        let capacity = buf.len() as u32;
        // SAFETY: `buf` 有 `capacity` 个元素，原生函数最多写入这么多项。
        let count = check(unsafe { sys::deskbaton_ddc_list(buf.as_mut_ptr(), capacity) })? as usize;
        if count <= buf.len() {
            buf.truncate(count);
            return Ok(buf);
        }
        buf.resize(count, RawDisplayInfo::EMPTY);
    }
}

fn to_display_infos(raw: &[RawDisplayInfo]) -> Vec<DisplayInfo> {
    raw.iter()
        .enumerate()
        .map(|(i, r)| {
            let uuid = text(&r.edid_uuid);
            let earlier = raw[..i]
                .iter()
                .filter(|p| p.edid_uuid == r.edid_uuid)
                .count();
            let id = match earlier {
                0 => uuid,
                n => format!("{uuid}#{}", n + 1),
            };
            let serial = text(&r.serial);
            DisplayInfo {
                id: DisplayId(id),
                vendor: text(&r.vendor),
                model: text(&r.model),
                serial: (!serial.is_empty()).then_some(serial),
            }
        })
        .collect()
}

fn text(field: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = field
        .iter()
        .map(|&c| c as u8)
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn write(index: u32, data: &[u8]) -> Result<()> {
    // SAFETY: `data` 在调用期间有效，原生函数只读取 `data.len()` 字节。
    check(unsafe {
        sys::deskbaton_ddc_write(index, data.as_ptr(), data.len() as u32, WRITE_CYCLES)
    })
    .map(drop)
}

/// 每次调用都重新发请求：不重发而再读，读到的不是回包而是总线上的残留数据。
fn transact(index: u32, request: &[u8], reply: &mut [u8]) -> Result<()> {
    // SAFETY: `request` 只被读取 `request.len()` 字节，`reply` 最多被写入 `reply.len()` 字节，两者在调用期间有效。
    check(unsafe {
        sys::deskbaton_ddc_transact(
            index,
            request.as_ptr(),
            request.len() as u32,
            WRITE_CYCLES,
            REPLY_DELAY_MS,
            reply.as_mut_ptr(),
            reply.len() as u32,
        )
    })
    .map(drop)
}

fn check(rc: i32) -> Result<u32> {
    match rc {
        sys::ERR_UNIMPLEMENTED => Err(Error::Unsupported("macOS DDC/CI")),
        sys::ERR_NOT_FOUND => Err(Error::Ddc("显示器已断开".into())),
        sys::ERR_IO => Err(Error::Ddc("I2C 读写失败".into())),
        rc if rc >= 0 => Ok(rc as u32),
        other => Err(Error::Ddc(format!("原生错误码 {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(uuid: &str, model: &str, serial: &str) -> RawDisplayInfo {
        let mut r = RawDisplayInfo::EMPTY;
        for (dst, src) in [
            (&mut r.edid_uuid[..], uuid),
            (&mut r.model[..], model),
            (&mut r.serial[..], serial),
        ] {
            for (d, s) in dst.iter_mut().zip(src.bytes()) {
                *d = s as std::ffi::c_char;
            }
        }
        r
    }

    #[test]
    fn duplicate_uuids_get_suffixed_ids() {
        let infos = to_display_infos(&[
            raw("AAAA", "HP D27k", "S1"),
            raw("BBBB", "HP D27k", ""),
            raw("AAAA", "HP D27k", "S2"),
        ]);
        let ids: Vec<&str> = infos.iter().map(|d| d.id.0.as_str()).collect();
        assert_eq!(ids, ["AAAA", "BBBB", "AAAA#2"]);
        assert_eq!(infos[0].serial.as_deref(), Some("S1"));
        assert_eq!(infos[1].serial, None);
    }
}
