use std::mem::size_of;

use baton_core::ids::{DisplayId, InputCode};
use baton_core::ports::{DisplayControl, DisplayInfo};
use baton_core::{Error, Result};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply, PHYSICAL_MONITOR,
    SetVCPFeature,
};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{ERROR_SUCCESS, GetLastError, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DEVMODEW, DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICEW, DeleteDC, ENUM_CURRENT_SETTINGS,
    EnumDisplayDevicesW, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, STGM_READ,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW};
use windows::Win32::UI::ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetThreadDpiAwarenessContext,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

use crate::levels::{self, Backend, DimCurve, EndpointVolume, GammaTable};
use crate::vcp::{self, VcpRead, VcpValue};
use crate::{ScreenMode, edid, retry};

const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;
/// `winerror.h`：显示器不支持这个 VCP。
const ERROR_GRAPHICS_DDCCI_VCP_NOT_SUPPORTED: u32 = 0xC026_2584;
/// GDI gamma 表：红、绿、蓝各 256 项。
const RAMP_ENTRIES: usize = 256;
/// 写入后回读允许的误差，驱动保留的位数可能少于 16 位。
const RAMP_TOLERANCE: u16 = 256;
const GAMMA_ATTEMPTS: u64 = 3;

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
        match get_vcp(display, vcp::INPUT_SOURCE)? {
            // 输入源是 MCCS 非连续值，只有低字节有效；部分显示器会在高字节放非零值。
            VcpRead::Value(value) => Ok(InputCode(value.current & 0xFF)),
            VcpRead::Unsupported => Err(Error::Ddc("显示器不支持 VCP 0x60".into())),
        }
    }

    fn set_input(&self, display: &DisplayId, input: InputCode) -> Result<()> {
        set_vcp(display, vcp::INPUT_SOURCE, input.0)
    }
}

fn get_vcp(display: &DisplayId, code: u8) -> Result<VcpRead> {
    retry(|| {
        let monitors = enumerate()?;
        let monitor = monitors.find(display)?;
        let mut current = 0u32;
        let mut max = 0u32;
        // SAFETY: 句柄在 `monitors` 存活期间有效；两个输出指针指向本函数的局部变量。
        let ok = unsafe {
            GetVCPFeatureAndVCPFeatureReply(
                monitor.hPhysicalMonitor,
                code,
                None,
                &mut current,
                Some(&mut max),
            )
        };
        if ok == 0 {
            // SAFETY: 紧接着失败的调用读取本线程的错误码。
            let error = unsafe { GetLastError() };
            if error.0 == ERROR_GRAPHICS_DDCCI_VCP_NOT_SUPPORTED {
                return Ok(VcpRead::Unsupported);
            }
            return Err(Error::Ddc(format!(
                "读取 VCP 0x{code:02X} 失败：{}",
                windows::core::Error::from(error.to_hresult())
            )));
        }
        Ok(VcpRead::Value(VcpValue {
            current: u16::try_from(current).unwrap_or(u16::MAX),
            max: u16::try_from(max).unwrap_or(u16::MAX),
        }))
    })
}

fn set_vcp(display: &DisplayId, code: u8, value: u16) -> Result<()> {
    retry(|| {
        let monitors = enumerate()?;
        let monitor = monitors.find(display)?;
        // SAFETY: 句柄在 `monitors` 存活期间有效。
        let ok = unsafe { SetVCPFeature(monitor.hPhysicalMonitor, code, value.into()) };
        if ok == 0 {
            return Err(last_error(&format!("写入 VCP 0x{code:02X}")));
        }
        Ok(())
    })
}

/// 软件调光经 GDI gamma 表；系统音量经 Core Audio 的端点音量。
pub struct WinLevels;

impl Backend for WinLevels {
    fn get_vcp(&self, display: &DisplayId, code: u8) -> Result<VcpRead> {
        get_vcp(display, code)
    }

    fn set_vcp(&self, display: &DisplayId, code: u8, value: u16) -> Result<()> {
        set_vcp(display, code, value)
    }

    fn gamma(&self, display: &DisplayId) -> Option<GammaTable> {
        let dc = DisplayDc::open(display)?;
        let ramp = dc.read()?;
        let channel = |i: usize| {
            ramp[i * RAMP_ENTRIES..(i + 1) * RAMP_ENTRIES]
                .iter()
                .map(|&v| f32::from(v) / 65535.0)
                .collect()
        };
        Some(GammaTable {
            red: channel(0),
            green: channel(1),
            blue: channel(2),
        })
    }

    fn set_gamma(&self, display: &DisplayId, table: &GammaTable) -> Result<()> {
        let mut ramp = [0u16; 3 * RAMP_ENTRIES];
        for (i, channel) in [&table.red, &table.green, &table.blue]
            .into_iter()
            .enumerate()
        {
            if channel.len() != RAMP_ENTRIES {
                return Err(Error::Levels("gamma 表项数不是 256".into()));
            }
            for (dst, &v) in ramp[i * RAMP_ENTRIES..].iter_mut().zip(channel) {
                *dst = (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
            }
        }
        write_ramp(display, &ramp)
    }

    fn dim_curve(&self) -> DimCurve {
        DimCurve::PeakPinned
    }

    fn endpoint(&self, matcher: &str) -> Result<Option<EndpointVolume>> {
        let _com = Com::init();
        let Some(volume) = find_endpoint(matcher)? else {
            return Ok(None);
        };
        // SAFETY: `volume` 是刚激活的有效接口。
        let (level, muted) = unsafe { (volume.GetMasterVolumeLevelScalar(), volume.GetMute()) };
        Ok(Some(EndpointVolume {
            level: level.map_err(|e| audio_error("读取音量", e))?,
            muted: muted.map_err(|e| audio_error("读取静音", e))?.as_bool(),
        }))
    }

    fn set_endpoint_level(&self, matcher: &str, level: f32) -> Result<()> {
        let _com = Com::init();
        let volume = find_endpoint(matcher)?.ok_or_else(missing_endpoint)?;
        // SAFETY: `volume` 是刚激活的有效接口；事件上下文传空表示不区分来源。
        unsafe { volume.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null()) }
            .map_err(|e| audio_error("设置音量", e))
    }

    fn set_endpoint_mute(&self, matcher: &str, muted: bool) -> Result<()> {
        let _com = Com::init();
        let volume = find_endpoint(matcher)?.ok_or_else(missing_endpoint)?;
        // SAFETY: 同上。
        unsafe { volume.SetMute(muted, std::ptr::null()) }.map_err(|e| audio_error("设置静音", e))
    }
}

/// 恢复为恒等 gamma 表；之后系统的校准加载器可能再改，不影响。
pub(crate) fn reset_gamma(display: &DisplayId) {
    let mut ramp = [0u16; 3 * RAMP_ENTRIES];
    for (i, v) in ramp.iter_mut().enumerate() {
        *v = ((i % RAMP_ENTRIES) * 257) as u16;
    }
    if let Err(e) = write_ramp(display, &ramp) {
        log::warn!("复原 {} 的 gamma 失败：{e}", display.0);
    }
}

/// 过暗的表会被静默拒绝（返回成功但不生效），所以写后回读校验。
fn write_ramp(display: &DisplayId, ramp: &[u16; 3 * RAMP_ENTRIES]) -> Result<()> {
    for attempt in 1..=GAMMA_ATTEMPTS {
        if let Some(dc) = DisplayDc::open(display) {
            // SAFETY: `ramp` 为 3 × 256 个 WORD，正是 SetDeviceGammaRamp 要求的布局。
            let set = unsafe { SetDeviceGammaRamp(dc.0, ramp.as_ptr().cast()) }.as_bool();
            if set && dc.read().is_some_and(|applied| ramps_match(ramp, &applied)) {
                return Ok(());
            }
        }
        if attempt < GAMMA_ATTEMPTS {
            std::thread::sleep(std::time::Duration::from_millis(20 * attempt));
        }
    }
    Err(Error::Levels("软件调光没有生效（HDR 下不可用）".into()))
}

fn ramps_match(expected: &[u16], actual: &[u16]) -> bool {
    expected
        .iter()
        .zip(actual)
        .all(|(&a, &b)| a.abs_diff(b) <= RAMP_TOLERANCE)
}

/// `\\.\DISPLAYn` 的设备上下文；同一个 `DISPLAYn` 下的物理显示器共用一张 gamma 表。
struct DisplayDc(HDC);

impl DisplayDc {
    fn open(display: &DisplayId) -> Option<Self> {
        let device = HSTRING::from(display.0.split('#').next().unwrap_or_default());
        // SAFETY: 驱动名与设备名都是有效的宽字符串，其余参数为空。
        let dc = unsafe { CreateDCW(w!("DISPLAY"), &device, PCWSTR::null(), None) };
        (!dc.is_invalid()).then_some(Self(dc))
    }

    fn read(&self) -> Option<[u16; 3 * RAMP_ENTRIES]> {
        let mut ramp = [0u16; 3 * RAMP_ENTRIES];
        // SAFETY: `ramp` 为 3 × 256 个 WORD，系统最多写入这么多。
        unsafe { GetDeviceGammaRamp(self.0, ramp.as_mut_ptr().cast()) }
            .as_bool()
            .then_some(ramp)
    }
}

impl Drop for DisplayDc {
    fn drop(&mut self) {
        // SAFETY: 句柄来自 CreateDCW，且只释放一次。
        let _ = unsafe { DeleteDC(self.0) };
    }
}

/// 在当前线程进入多线程套间；线程已在单线程套间时直接沿用，不反初始化。
struct Com(bool);

impl Com {
    fn init() -> Self {
        // SAFETY: 保留参数为空；成功（含已初始化）时由 drop 配对 CoUninitialize。
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: 与 init 中成功的 CoInitializeEx 配对。
            unsafe { CoUninitialize() };
        }
    }
}

/// 名称包含 `matcher`（不区分大小写）的活动输出端点；零个或多个时为 `None`。
fn find_endpoint(matcher: &str) -> Result<Option<IAudioEndpointVolume>> {
    // SAFETY: 以下均为 COM 调用，接口在本函数内有效；调用方已初始化 COM。
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| audio_error("打开音频设备列表", e))?;
        let devices = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| audio_error("列出音频设备", e))?;
        let count = devices
            .GetCount()
            .map_err(|e| audio_error("列出音频设备", e))?;
        let mut names = Vec::new();
        let mut endpoints = Vec::new();
        for i in 0..count {
            let Ok(device) = devices.Item(i) else {
                continue;
            };
            let Ok(name) = device
                .OpenPropertyStore(STGM_READ)
                .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
            else {
                continue;
            };
            names.push(name.to_string());
            endpoints.push(device);
        }
        let Some(device) = levels::unique_match(&names, matcher).and_then(|i| endpoints.get(i))
        else {
            log::info!("名称包含「{matcher}」的音频输出设备不是恰好一个：{names:?}");
            return Ok(None);
        };
        device
            .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            .map(Some)
            .map_err(|e| audio_error("打开端点音量", e))
    }
}

fn missing_endpoint() -> Error {
    Error::Levels("找不到这台显示器的音频设备".into())
}

fn audio_error(what: &str, e: windows::core::Error) -> Error {
    Error::Levels(format!("{what}失败：{e}"))
}

/// 与「设置 → 显示」一致：物理分辨率加系统缩放比例。
pub(crate) fn screen_mode(display: &DisplayId) -> Result<ScreenMode> {
    let device = display.0.split('#').next().unwrap_or_default();
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: `dmSize` 已设置为结构体大小，系统据此写入 `mode`。
    let found =
        unsafe { EnumDisplaySettingsW(&HSTRING::from(device), ENUM_CURRENT_SETTINGS, &mut mode) };
    if !found.as_bool() {
        return Err(Error::Ddc(format!("读取 {device} 的显示设置失败")));
    }
    // 0 和 1 表示使用硬件默认刷新率，具体值未知。
    let refresh = mode.dmDisplayFrequency;
    Ok(ScreenMode {
        width: mode.dmPelsWidth,
        height: mode.dmPelsHeight,
        pixel_width: mode.dmPelsWidth,
        pixel_height: mode.dmPelsHeight,
        refresh_hz: (refresh > 1).then_some(f64::from(refresh)),
        scale_percent: monitor_dpi(device).map(|dpi| (dpi * 100 + 48) / 96),
    })
}

fn monitor_dpi(device: &str) -> Option<u32> {
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
    .ok()?;
    let hmonitor = hmonitors
        .into_iter()
        .find(|&h| device_name(h).is_ok_and(|name| name == device))?;
    let (mut x, mut y) = (0u32, 0u32);
    // 未声明 DPI 感知的线程只能读到 96，读之前临时切换，读完恢复，不影响调用方。
    // SAFETY: 参数是系统预定义的上下文常量；返回值为切换前的上下文。
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // SAFETY: 两个输出指针指向本函数的局部变量。
    let read = unsafe { GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) };
    // 失败时返回空指针；不能用 `is_invalid`，它把 -1（即 UNAWARE）也当作无效。
    if !previous.0.is_null() {
        // SAFETY: `previous` 是上一次调用返回的有效上下文。
        let _ = unsafe { SetThreadDpiAwarenessContext(previous) };
    }
    read.ok()?;
    (x > 0).then_some(x)
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
