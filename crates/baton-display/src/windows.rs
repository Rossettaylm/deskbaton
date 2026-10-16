use std::mem::size_of;

use baton_core::ids::{DisplayId, InputCode};
use baton_core::ports::{DisplayControl, DisplayInfo};
use baton_core::{Error, Result};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply, PHYSICAL_MONITOR,
    SetVCPFeature,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors,
    GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW};
use windows::core::{BOOL, HSTRING, w};

use crate::edid;
use crate::retry;

const VCP_INPUT_SOURCE: u8 = 0x60;
const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;

/// 经 Dxva2 Monitor Configuration API 读写 DDC。`DisplayId` 为 `\\.\DISPLAYn#序号`，型号和序列号取自注册表中的 EDID。
pub struct WinDdc;

impl DisplayControl for WinDdc {
    fn list(&self) -> Result<Vec<DisplayInfo>> {
        Ok(enumerate()?
            .0
            .iter()
            .map(|(info, _)| info.clone())
            .collect())
    }

    fn get_input(&self, display: &DisplayId) -> Result<InputCode> {
        retry(|| {
            let monitors = enumerate()?;
            let monitor = monitors.find(display)?;
            let mut current = 0u32;
            let mut max = 0u32;
            // SAFETY: 句柄在 `monitors` 存活期间有效；两个输出指针指向本函数的局部变量。
            let ok = unsafe {
                GetVCPFeatureAndVCPFeatureReply(
                    monitor.hPhysicalMonitor,
                    VCP_INPUT_SOURCE,
                    None,
                    &mut current,
                    Some(&mut max),
                )
            };
            if ok == 0 {
                return Err(last_error("读取输入源"));
            }
            // 输入源是 MCCS 非连续值，只有低字节有效；部分显示器会在高字节放非零值。
            Ok(InputCode((current & 0xFF) as u16))
        })
    }

    fn set_input(&self, display: &DisplayId, input: InputCode) -> Result<()> {
        retry(|| {
            let monitors = enumerate()?;
            let monitor = monitors.find(display)?;
            // SAFETY: 句柄在 `monitors` 存活期间有效。
            let ok = unsafe {
                SetVCPFeature(monitor.hPhysicalMonitor, VCP_INPUT_SOURCE, input.0.into())
            };
            if ok == 0 {
                return Err(last_error("切换输入源"));
            }
            Ok(())
        })
    }
}

/// 一次枚举得到的物理显示器；drop 时销毁句柄。
struct Monitors(Vec<(DisplayInfo, PHYSICAL_MONITOR)>);

impl Monitors {
    fn find(&self, display: &DisplayId) -> Result<&PHYSICAL_MONITOR> {
        self.0
            .iter()
            .find(|(info, _)| info.id == *display)
            .map(|(_, monitor)| monitor)
            .ok_or_else(|| Error::Ddc(format!("找不到显示器 {}", display.0)))
    }
}

impl Drop for Monitors {
    fn drop(&mut self) {
        for (_, monitor) in &self.0 {
            // SAFETY: 句柄来自 GetPhysicalMonitorsFromHMONITOR，且只销毁一次。
            let _ = unsafe { DestroyPhysicalMonitors(std::slice::from_ref(monitor)) };
        }
    }
}

fn enumerate() -> Result<Monitors> {
    let mut hmonitors: Vec<HMONITOR> = Vec::new();
    // SAFETY: 回调只在 EnumDisplayMonitors 返回前被调用，期间 `hmonitors` 一直有效。
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut hmonitors as *mut Vec<HMONITOR> as isize),
        )
    }
    .ok()
    .map_err(|e| Error::Ddc(format!("枚举显示器失败：{e}")))?;

    let mut monitors = Monitors(Vec::new());
    for hmonitor in hmonitors {
        let device = device_name(hmonitor)?;
        let edids = monitor_edids(&device);
        let mut count = 0u32;
        // SAFETY: `count` 是局部变量。
        unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hmonitor, &mut count) }
            .map_err(|e| Error::Ddc(format!("获取物理显示器失败：{e}")))?;
        let mut physical = vec![PHYSICAL_MONITOR::default(); count as usize];
        // SAFETY: `physical` 的长度即传给系统的数组大小。
        unsafe { GetPhysicalMonitorsFromHMONITOR(hmonitor, &mut physical) }
            .map_err(|e| Error::Ddc(format!("获取物理显示器失败：{e}")))?;
        for (index, monitor) in physical.into_iter().enumerate() {
            let edid = edids.get(index).cloned().flatten();
            let description = monitor.szPhysicalMonitorDescription;
            let description = utf16(&description);
            let info = DisplayInfo {
                id: DisplayId(format!("{device}#{index}")),
                vendor: edid.as_ref().map(|e| e.vendor.clone()).unwrap_or_default(),
                model: edid
                    .as_ref()
                    .and_then(|e| e.model.clone())
                    .unwrap_or(description),
                serial: edid.and_then(|e| e.serial),
            };
            monitors.0.push((info, monitor));
        }
    }
    Ok(monitors)
}

unsafe extern "system" fn collect_monitor(
    hmonitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` 是 enumerate 传入的 `&mut Vec<HMONITOR>`，回调期间独占访问。
    let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    list.push(hmonitor);
    true.into()
}

fn device_name(hmonitor: HMONITOR) -> Result<String> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: `cbSize` 声明了 MONITORINFOEXW 的大小，系统据此写入扩展字段。
    unsafe {
        GetMonitorInfoW(
            hmonitor,
            &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
        )
    }
    .ok()
    .map_err(|e| Error::Ddc(format!("读取显示器信息失败：{e}")))?;
    Ok(utf16(&info.szDevice))
}

/// 按顺序返回挂在 `device`（`\\.\DISPLAYn`）下的活动显示器的 EDID，与物理显示器的序号对应。
fn monitor_edids(device: &str) -> Vec<Option<edid::Edid>> {
    let device = HSTRING::from(device);
    let mut edids = Vec::new();
    for index in 0.. {
        let mut dd = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: `dd.cb` 已设置为结构体大小。
        let found =
            unsafe { EnumDisplayDevicesW(&device, index, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME) };
        if !found.as_bool() {
            break;
        }
        if dd.StateFlags.0 & DISPLAY_DEVICE_ACTIVE.0 == 0 {
            continue;
        }
        edids.push(
            edid::registry_key(&utf16(&dd.DeviceID))
                .and_then(|key| read_edid(&key))
                .and_then(|bytes| edid::parse(&bytes)),
        );
    }
    edids
}

fn read_edid(key: &str) -> Option<Vec<u8>> {
    let key = HSTRING::from(key);
    let mut len = 0u32;
    // SAFETY: 只查询长度，`len` 是局部变量。
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &key,
            w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            None,
            Some(&mut len),
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: `buf` 有 `len` 字节，系统最多写入这么多。
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &key,
            w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    (rc == ERROR_SUCCESS).then(|| {
        buf.truncate(len as usize);
        buf
    })
}

fn utf16(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn last_error(what: &str) -> Error {
    Error::Ddc(format!(
        "{what}失败：{}",
        windows::core::Error::from_thread()
    ))
}
