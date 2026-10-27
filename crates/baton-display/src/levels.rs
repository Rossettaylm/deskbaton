//! 显示器亮度、音量与静音：优先经 DDC 调显示器本身，显示器明确回「不支持」时，亮度改用软件调光，
//! 音量改调系统里这台显示器的音频端点。读写失败只报错，不切换路径。

use std::collections::HashMap;

use baton_core::ids::DisplayId;
use baton_core::ports::DisplayInfo;
use baton_core::{Error, Result};

use crate::vcp::{self, VcpRead};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Ddc,
    Gamma,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slider {
    pub percent: u8,
    pub source: Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LevelsView {
    /// `None`：DDC 不支持，且读不到这台显示器的 gamma 表。
    pub brightness: Option<Slider>,
    /// `None`：DDC 不支持，系统里也没有唯一对应的音频端点。
    pub volume: Option<Slider>,
    /// `None`：没有可用的静音开关。
    pub muted: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Brightness,
    Volume,
    Mute,
}

/// 归一化到 0–1 的 gamma 表，三个通道等长。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GammaTable {
    pub red: Vec<f32>,
    pub green: Vec<f32>,
    pub blue: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EndpointVolume {
    /// 0–1，系统音量滑条的刻度。
    pub level: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DimCurve {
    /// 整体乘一个系数，0% 对应 `floor`。
    #[cfg_attr(all(target_os = "windows", not(test)), allow(dead_code))]
    Linear { floor: f32 },
    /// Windows 静默拒绝过暗的表：峰值不低于 70%，更暗时只压中间调。
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    PeakPinned,
}

/// 平台相关的读写；调用方负责串行化。
pub(crate) trait Backend: Send {
    fn get_vcp(&self, display: &DisplayId, code: u8) -> Result<VcpRead>;
    fn set_vcp(&self, display: &DisplayId, code: u8, value: u16) -> Result<()>;
    /// 读不到这台显示器的 gamma 表时为 `None`。
    fn gamma(&self, display: &DisplayId) -> Option<GammaTable>;
    /// 写入后系统没有采用时返回错误。
    fn set_gamma(&self, display: &DisplayId, table: &GammaTable) -> Result<()>;
    fn dim_curve(&self) -> DimCurve;
    /// 名称包含 `matcher`（不区分大小写）的音频输出端点恰好一个时返回它的音量。
    fn endpoint(&self, matcher: &str) -> Result<Option<EndpointVolume>>;
    fn set_endpoint_level(&self, matcher: &str, level: f32) -> Result<()>;
    fn set_endpoint_mute(&self, matcher: &str, muted: bool) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrightnessPath {
    Ddc { max: u16 },
    Gamma,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VolumePath {
    Ddc { max: u16 },
    System,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MutePath {
    Ddc,
    System,
    Unsupported,
}

struct Dimmed {
    base: GammaTable,
    written: GammaTable,
    percent: u8,
}

/// 面板进程内唯一一份，持有路径判定结果和软件调光的基准表。
pub struct Levels {
    backend: Box<dyn Backend>,
    audio_device: Option<String>,
    brightness: HashMap<DisplayId, BrightnessPath>,
    volume: HashMap<DisplayId, VolumePath>,
    mute: HashMap<DisplayId, MutePath>,
    dimmed: HashMap<DisplayId, Dimmed>,
}

impl Levels {
    /// `audio_device` 为找音频端点用的名称子串，`None` 时用显示器型号。
    pub fn new(audio_device: Option<String>) -> Self {
        Self::with_backend(crate::platform_levels_backend(), audio_device)
    }

    pub(crate) fn with_backend(backend: Box<dyn Backend>, audio_device: Option<String>) -> Self {
        Self {
            backend,
            audio_device: audio_device.filter(|s| !s.trim().is_empty()),
            brightness: HashMap::new(),
            volume: HashMap::new(),
            mute: HashMap::new(),
            dimmed: HashMap::new(),
        }
    }

    pub fn read(&mut self, display: &DisplayInfo) -> Result<LevelsView> {
        let brightness = self.read_brightness(&display.id)?;
        let (volume, endpoint) = self.read_volume(display)?;
        let muted = self.read_mute(&display.id, endpoint)?;
        Ok(LevelsView {
            brightness,
            volume,
            muted,
        })
    }

    /// `value`：亮度、音量为 0–100 的百分比，静音为 0 / 1。
    pub fn set(&mut self, display: &DisplayInfo, control: Control, value: u8) -> Result<()> {
        let id = &display.id;
        if !(self.brightness.contains_key(id)
            && self.volume.contains_key(id)
            && self.mute.contains_key(id))
        {
            self.read(display)?;
        }
        let percent = value.min(100);
        match control {
            Control::Brightness => match self
                .brightness
                .get(id)
                .copied()
                .unwrap_or(BrightnessPath::Unsupported)
            {
                BrightnessPath::Ddc { max } => {
                    self.backend
                        .set_vcp(id, vcp::BRIGHTNESS, to_raw(percent, max, false))
                }
                BrightnessPath::Gamma => self.dim(id, percent),
                BrightnessPath::Unsupported => Err(Error::Unsupported("调这台显示器的亮度")),
            },
            Control::Volume => match self
                .volume
                .get(id)
                .copied()
                .unwrap_or(VolumePath::Unsupported)
            {
                VolumePath::Ddc { max } => {
                    self.backend
                        .set_vcp(id, vcp::VOLUME, to_raw(percent, max, true))
                }
                VolumePath::System => self
                    .backend
                    .set_endpoint_level(&self.matcher(display), f32::from(percent) / 100.0),
                VolumePath::Unsupported => Err(Error::Unsupported("调这台显示器的音量")),
            },
            Control::Mute => match self.mute.get(id).copied().unwrap_or(MutePath::Unsupported) {
                MutePath::Ddc => {
                    let code = if value != 0 {
                        vcp::MUTE_ON
                    } else {
                        vcp::MUTE_OFF
                    };
                    self.backend.set_vcp(id, vcp::MUTE, code)
                }
                MutePath::System => self
                    .backend
                    .set_endpoint_mute(&self.matcher(display), value != 0),
                MutePath::Unsupported => Err(Error::Unsupported("这台显示器的静音")),
            },
        }
    }

    /// 下次 `read` 重新判定各项走哪条路径；不影响软件调光。
    pub fn forget(&mut self) {
        self.brightness.clear();
        self.volume.clear();
        self.mute.clear();
    }

    /// 当前由本进程调暗的显示器。
    pub fn dimmed(&self) -> Vec<DisplayId> {
        let mut ids: Vec<DisplayId> = self.dimmed.keys().cloned().collect();
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        ids
    }

    /// 复原本进程调过的所有 gamma；已被系统或其它应用改掉的不动。
    pub fn restore_all(&mut self) {
        let dimmed: Vec<(DisplayId, Dimmed)> = self.dimmed.drain().collect();
        for (id, dimmed) in dimmed {
            if let Err(e) = self.restore(&id, &dimmed) {
                log::warn!("复原 {} 的软件调光失败：{e}", id.0);
            }
        }
    }

    fn matcher(&self, display: &DisplayInfo) -> String {
        self.audio_device
            .clone()
            .unwrap_or_else(|| display.model.trim().to_owned())
    }

    fn read_brightness(&mut self, id: &DisplayId) -> Result<Option<Slider>> {
        let path = match self.brightness.get(id) {
            Some(BrightnessPath::Unsupported) => return Ok(None),
            Some(BrightnessPath::Gamma) => return Ok(Some(self.gamma_slider(id))),
            _ => self.backend.get_vcp(id, vcp::BRIGHTNESS)?,
        };
        let (path, slider) = match path {
            VcpRead::Value(value) => {
                let max = effective_max(value.max);
                let slider = Slider {
                    percent: to_percent(value.current, max),
                    source: Source::Ddc,
                };
                (BrightnessPath::Ddc { max }, Some(slider))
            }
            VcpRead::Unsupported if self.backend.gamma(id).is_some() => {
                (BrightnessPath::Gamma, Some(self.gamma_slider(id)))
            }
            VcpRead::Unsupported => (BrightnessPath::Unsupported, None),
        };
        self.brightness.insert(id.clone(), path);
        Ok(slider)
    }

    fn read_volume(
        &mut self,
        display: &DisplayInfo,
    ) -> Result<(Option<Slider>, Option<EndpointVolume>)> {
        let id = &display.id;
        let read = match self.volume.get(id) {
            Some(VolumePath::Unsupported) => return Ok((None, None)),
            Some(VolumePath::System) => VcpRead::Unsupported,
            _ => self.backend.get_vcp(id, vcp::VOLUME)?,
        };
        let (path, slider, endpoint) = match read {
            VcpRead::Value(value) => {
                let max = effective_max(value.max);
                let slider = Slider {
                    percent: to_percent(value.current, max),
                    source: Source::Ddc,
                };
                (VolumePath::Ddc { max }, Some(slider), None)
            }
            VcpRead::Unsupported => match self.system_endpoint(display)? {
                Some(endpoint) => {
                    let slider = Slider {
                        percent: (endpoint.level.clamp(0.0, 1.0) * 100.0).round() as u8,
                        source: Source::System,
                    };
                    (VolumePath::System, Some(slider), Some(endpoint))
                }
                None => (VolumePath::Unsupported, None, None),
            },
        };
        self.volume.insert(id.clone(), path);
        Ok((slider, endpoint))
    }

    fn system_endpoint(&self, display: &DisplayInfo) -> Result<Option<EndpointVolume>> {
        let matcher = self.matcher(display);
        if matcher.is_empty() {
            return Ok(None);
        }
        self.backend.endpoint(&matcher)
    }

    fn read_mute(
        &mut self,
        id: &DisplayId,
        endpoint: Option<EndpointVolume>,
    ) -> Result<Option<bool>> {
        let volume = self
            .volume
            .get(id)
            .copied()
            .unwrap_or(VolumePath::Unsupported);
        let (path, muted) = match volume {
            VolumePath::Unsupported => (MutePath::Unsupported, None),
            VolumePath::System => (MutePath::System, endpoint.map(|e| e.muted)),
            VolumePath::Ddc { .. } if self.mute.get(id) == Some(&MutePath::Unsupported) => {
                (MutePath::Unsupported, None)
            }
            VolumePath::Ddc { .. } => match self.backend.get_vcp(id, vcp::MUTE)? {
                VcpRead::Value(value) => (MutePath::Ddc, Some(value.current == vcp::MUTE_ON)),
                VcpRead::Unsupported => (MutePath::Unsupported, None),
            },
        };
        self.mute.insert(id.clone(), path);
        Ok(muted)
    }

    /// 回读到的表已不是我们最后一次写的，说明被系统或其它应用改了：放手，按 100% 显示。
    fn gamma_slider(&mut self, id: &DisplayId) -> Slider {
        let percent = match self.dimmed.get(id) {
            None => 100,
            Some(dimmed) => match self.backend.gamma(id) {
                Some(current) if tables_close(&current, &dimmed.written) => dimmed.percent,
                _ => {
                    self.dimmed.remove(id);
                    100
                }
            },
        };
        Slider {
            percent,
            source: Source::Gamma,
        }
    }

    fn dim(&mut self, id: &DisplayId, percent: u8) -> Result<()> {
        if percent >= 100 {
            return match self.dimmed.remove(id) {
                Some(dimmed) => self.restore(id, &dimmed),
                None => Ok(()),
            };
        }
        let current = self
            .backend
            .gamma(id)
            .ok_or_else(|| Error::Levels("读不到这台显示器的 gamma 表".into()))?;
        let base = match self.dimmed.get(id) {
            Some(dimmed) if tables_close(&current, &dimmed.written) => dimmed.base.clone(),
            _ => current,
        };
        let written = dim_table(&base, percent, self.backend.dim_curve());
        self.backend.set_gamma(id, &written)?;
        self.dimmed.insert(
            id.clone(),
            Dimmed {
                base,
                written,
                percent,
            },
        );
        Ok(())
    }

    fn restore(&self, id: &DisplayId, dimmed: &Dimmed) -> Result<()> {
        match self.backend.gamma(id) {
            Some(current) if tables_close(&current, &dimmed.written) => {
                self.backend.set_gamma(id, &dimmed.base)
            }
            _ => Ok(()),
        }
    }
}

/// 不少显示器报的最大值不可信：截到 100，报 0 时按 100 算。
/// 名称包含 `matcher`（不区分大小写）的恰好一个时返回它的下标。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) fn unique_match<S: AsRef<str>>(names: &[S], matcher: &str) -> Option<usize> {
    let needle = matcher.to_lowercase();
    let mut hits = names
        .iter()
        .enumerate()
        .filter(|(_, name)| name.as_ref().to_lowercase().contains(&needle))
        .map(|(i, _)| i);
    match (hits.next(), hits.next()) {
        (Some(i), None) => Some(i),
        _ => None,
    }
}

fn effective_max(max: u16) -> u16 {
    if max == 0 { 100 } else { max.min(100) }
}

fn to_percent(current: u16, max: u16) -> u8 {
    let (current, max) = (u32::from(current.min(max)), u32::from(max));
    ((current * 100 + max / 2) / max) as u8
}

/// 部分显示器音量写 0 会出问题，`keep_audible` 时非 0 百分比至少写 1。
fn to_raw(percent: u8, max: u16, keep_audible: bool) -> u16 {
    let raw = ((u32::from(percent) * u32::from(max) + 50) / 100) as u16;
    if keep_audible && percent > 0 {
        raw.max(1)
    } else {
        raw
    }
}

fn dim_table(base: &GammaTable, percent: u8, curve: DimCurve) -> GammaTable {
    let p = f32::from(percent.min(100)) / 100.0;
    let map: Box<dyn Fn(f32) -> f32> = match curve {
        DimCurve::Linear { floor } => {
            let scale = floor + (1.0 - floor) * p;
            Box::new(move |v| v * scale)
        }
        DimCurve::PeakPinned => {
            const PEAK_FLOOR: f32 = 0.70;
            let requested = 0.20 + 0.80 * p;
            let peak = requested.max(PEAK_FLOOR);
            let exponent = if requested < PEAK_FLOOR {
                (requested * 0.5 / peak).ln() / 0.5f32.ln()
            } else {
                1.0
            };
            Box::new(move |v| (peak * v.max(0.0).powf(exponent)).min(1.0))
        }
    };
    let apply = |channel: &[f32]| channel.iter().map(|&v| map(v)).collect();
    GammaTable {
        red: apply(&base.red),
        green: apply(&base.green),
        blue: apply(&base.blue),
    }
}

/// 两张表在 17 个等距采样点上逐通道相差都不超过 0.01；项数不同时按线性插值比较。
fn tables_close(a: &GammaTable, b: &GammaTable) -> bool {
    const POINTS: usize = 17;
    const TOLERANCE: f32 = 0.01;
    [(&a.red, &b.red), (&a.green, &b.green), (&a.blue, &b.blue)]
        .iter()
        .all(|(x, y)| {
            (0..POINTS).all(|i| {
                let t = i as f32 / (POINTS - 1) as f32;
                match (sample(x, t), sample(y, t)) {
                    (Some(p), Some(q)) => (p - q).abs() <= TOLERANCE,
                    _ => false,
                }
            })
        })
}

fn sample(channel: &[f32], t: f32) -> Option<f32> {
    let last = channel.len().checked_sub(1)?;
    let pos = t * last as f32;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(last);
    let frac = pos - lo as f32;
    Some(channel[lo] + (channel[hi] - channel[lo]) * frac)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::vcp::VcpValue;

    #[derive(Clone, Copy)]
    enum FakeVcp {
        Value(u16, u16),
        Unsupported,
        Fail,
    }

    struct FakeState {
        vcp: HashMap<u8, FakeVcp>,
        vcp_reads: Vec<u8>,
        vcp_writes: Vec<(u8, u16)>,
        gamma: Option<GammaTable>,
        gamma_writes: usize,
        reject_gamma: bool,
        endpoint: Option<EndpointVolume>,
        matchers: Vec<String>,
        endpoint_writes: Vec<String>,
    }

    #[derive(Clone)]
    struct Fake(Arc<Mutex<FakeState>>);

    impl Fake {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(FakeState {
                vcp: HashMap::new(),
                vcp_reads: Vec::new(),
                vcp_writes: Vec::new(),
                gamma: Some(identity(4)),
                gamma_writes: 0,
                reject_gamma: false,
                endpoint: None,
                matchers: Vec::new(),
                endpoint_writes: Vec::new(),
            })))
        }

        fn vcp(self, code: u8, value: FakeVcp) -> Self {
            self.state().vcp.insert(code, value);
            self
        }

        fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
            self.0.lock().unwrap()
        }

        fn levels(&self, audio_device: Option<&str>) -> Levels {
            Levels::with_backend(Box::new(self.clone()), audio_device.map(str::to_owned))
        }
    }

    impl Backend for Fake {
        fn get_vcp(&self, _: &DisplayId, code: u8) -> Result<VcpRead> {
            let mut s = self.state();
            s.vcp_reads.push(code);
            match s.vcp.get(&code).copied().unwrap_or(FakeVcp::Unsupported) {
                FakeVcp::Value(current, max) => Ok(VcpRead::Value(VcpValue { current, max })),
                FakeVcp::Unsupported => Ok(VcpRead::Unsupported),
                FakeVcp::Fail => Err(Error::Ddc("I2C 读写失败".into())),
            }
        }
        fn set_vcp(&self, _: &DisplayId, code: u8, value: u16) -> Result<()> {
            self.state().vcp_writes.push((code, value));
            Ok(())
        }
        fn gamma(&self, _: &DisplayId) -> Option<GammaTable> {
            self.state().gamma.clone()
        }
        fn set_gamma(&self, _: &DisplayId, table: &GammaTable) -> Result<()> {
            let mut s = self.state();
            if s.reject_gamma {
                return Err(Error::Levels("软件调光没有生效".into()));
            }
            s.gamma = Some(table.clone());
            s.gamma_writes += 1;
            Ok(())
        }
        fn dim_curve(&self) -> DimCurve {
            DimCurve::Linear { floor: 0.15 }
        }
        fn endpoint(&self, matcher: &str) -> Result<Option<EndpointVolume>> {
            let mut s = self.state();
            s.matchers.push(matcher.to_owned());
            Ok(s.endpoint)
        }
        fn set_endpoint_level(&self, _: &str, level: f32) -> Result<()> {
            self.state()
                .endpoint_writes
                .push(format!("level {level:.2}"));
            Ok(())
        }
        fn set_endpoint_mute(&self, _: &str, muted: bool) -> Result<()> {
            self.state().endpoint_writes.push(format!("mute {muted}"));
            Ok(())
        }
    }

    fn identity(n: usize) -> GammaTable {
        let channel: Vec<f32> = (0..n).map(|i| i as f32 / (n - 1) as f32).collect();
        GammaTable {
            red: channel.clone(),
            green: channel.clone(),
            blue: channel,
        }
    }

    fn display() -> DisplayInfo {
        DisplayInfo {
            id: DisplayId("A".into()),
            vendor: "HPN".into(),
            model: "HP D27k".into(),
            serial: None,
        }
    }

    fn slider(percent: u8, source: Source) -> Option<Slider> {
        Some(Slider { percent, source })
    }

    #[test]
    fn ddc_paths_when_display_supports_everything() {
        let fake = Fake::new()
            .vcp(vcp::BRIGHTNESS, FakeVcp::Value(62, 100))
            .vcp(vcp::VOLUME, FakeVcp::Value(30, 100))
            .vcp(vcp::MUTE, FakeVcp::Value(vcp::MUTE_ON, 2));
        let view = fake.levels(None).read(&display()).unwrap();
        assert_eq!(
            view,
            LevelsView {
                brightness: slider(62, Source::Ddc),
                volume: slider(30, Source::Ddc),
                muted: Some(true),
            }
        );
    }

    #[test]
    fn hp_d27k_falls_back_for_volume_only_when_endpoint_exists() {
        let fake = Fake::new().vcp(vcp::BRIGHTNESS, FakeVcp::Value(59, 100));
        let view = fake.levels(None).read(&display()).unwrap();
        assert_eq!(view.brightness, slider(59, Source::Ddc));
        assert_eq!(view.volume, None);
        assert_eq!(view.muted, None);
        assert_eq!(fake.state().matchers, ["HP D27k"]);
    }

    #[test]
    fn unsupported_volume_uses_unique_system_endpoint() {
        let fake = Fake::new().vcp(vcp::BRIGHTNESS, FakeVcp::Value(50, 100));
        fake.state().endpoint = Some(EndpointVolume {
            level: 0.42,
            muted: true,
        });
        let mut levels = fake.levels(Some("Speakers (HP)"));
        let view = levels.read(&display()).unwrap();
        assert_eq!(view.volume, slider(42, Source::System));
        assert_eq!(view.muted, Some(true));
        assert_eq!(fake.state().matchers, ["Speakers (HP)"]);

        levels.set(&display(), Control::Volume, 70).unwrap();
        levels.set(&display(), Control::Mute, 0).unwrap();
        assert_eq!(fake.state().endpoint_writes, ["level 0.70", "mute false"]);
        assert!(fake.state().vcp_writes.is_empty());
    }

    #[test]
    fn mute_unsupported_hides_switch_but_keeps_ddc_volume() {
        let fake = Fake::new()
            .vcp(vcp::BRIGHTNESS, FakeVcp::Value(50, 100))
            .vcp(vcp::VOLUME, FakeVcp::Value(10, 100));
        let mut levels = fake.levels(None);
        let view = levels.read(&display()).unwrap();
        assert_eq!(view.volume, slider(10, Source::Ddc));
        assert_eq!(view.muted, None);
        assert!(levels.set(&display(), Control::Mute, 1).is_err());
    }

    #[test]
    fn io_failure_reports_error_without_caching_or_falling_back() {
        let fake = Fake::new().vcp(vcp::BRIGHTNESS, FakeVcp::Fail);
        let mut levels = fake.levels(None);
        assert!(levels.read(&display()).is_err());
        assert_eq!(fake.state().gamma_writes, 0);

        fake.state()
            .vcp
            .insert(vcp::BRIGHTNESS, FakeVcp::Value(40, 100));
        let view = levels.read(&display()).unwrap();
        assert_eq!(view.brightness, slider(40, Source::Ddc));
    }

    #[test]
    fn unsupported_paths_are_cached_until_forget() {
        let fake = Fake::new();
        fake.state().gamma = None;
        let mut levels = fake.levels(None);
        let view = levels.read(&display()).unwrap();
        assert_eq!(view, LevelsView::default());
        levels.read(&display()).unwrap();
        assert_eq!(fake.state().vcp_reads, [vcp::BRIGHTNESS, vcp::VOLUME]);

        levels.forget();
        levels.read(&display()).unwrap();
        assert_eq!(fake.state().vcp_reads.len(), 4);
        assert!(levels.set(&display(), Control::Brightness, 50).is_err());
    }

    #[test]
    fn ddc_writes_scale_to_reported_max() {
        let fake = Fake::new()
            .vcp(vcp::BRIGHTNESS, FakeVcp::Value(25, 50))
            .vcp(vcp::VOLUME, FakeVcp::Value(0, 100))
            .vcp(vcp::MUTE, FakeVcp::Value(vcp::MUTE_OFF, 2));
        let mut levels = fake.levels(None);
        let view = levels.read(&display()).unwrap();
        assert_eq!(view.brightness, slider(50, Source::Ddc));
        assert_eq!(view.muted, Some(false));

        levels.set(&display(), Control::Brightness, 80).unwrap();
        levels.set(&display(), Control::Volume, 0).unwrap();
        levels.set(&display(), Control::Mute, 1).unwrap();
        assert_eq!(
            fake.state().vcp_writes,
            [
                (vcp::BRIGHTNESS, 40),
                (vcp::VOLUME, 0),
                (vcp::MUTE, vcp::MUTE_ON)
            ]
        );
    }

    #[test]
    fn set_probes_paths_when_not_read_yet() {
        let fake = Fake::new().vcp(vcp::BRIGHTNESS, FakeVcp::Value(10, 100));
        fake.levels(None)
            .set(&display(), Control::Brightness, 30)
            .unwrap();
        assert_eq!(fake.state().vcp_writes, [(vcp::BRIGHTNESS, 30)]);
    }

    #[test]
    fn max_is_clamped_to_100_and_zero_means_100() {
        assert_eq!(effective_max(0), 100);
        assert_eq!(effective_max(255), 100);
        assert_eq!(effective_max(64), 64);
    }

    #[test]
    fn percent_round_trips_and_rounds() {
        assert_eq!(to_percent(62, 100), 62);
        assert_eq!(to_percent(1, 3), 33);
        assert_eq!(to_percent(2, 3), 67);
        assert_eq!(to_percent(500, 100), 100);
        for p in 0..=100 {
            assert_eq!(to_percent(to_raw(p, 100, false), 100), p);
        }
        assert_eq!(to_raw(50, 3, false), 2);
    }

    #[test]
    fn volume_above_zero_never_writes_zero() {
        assert_eq!(to_raw(1, 20, false), 0);
        assert_eq!(to_raw(1, 20, true), 1);
        assert_eq!(to_raw(0, 20, true), 0);
    }

    #[test]
    fn linear_curve_keeps_floor_at_zero_percent() {
        let base = identity(5);
        let dark = dim_table(&base, 0, DimCurve::Linear { floor: 0.15 });
        assert!((dark.red[4] - 0.15).abs() < 1e-6);
        let half = dim_table(&base, 50, DimCurve::Linear { floor: 0.15 });
        assert!((half.green[4] - 0.575).abs() < 1e-6);
        assert_eq!(
            dim_table(&base, 100, DimCurve::Linear { floor: 0.15 }),
            base
        );
    }

    #[test]
    fn peak_pinned_curve_keeps_peak_and_darkens_midtones() {
        let base = identity(257);
        let dark = dim_table(&base, 0, DimCurve::PeakPinned);
        assert!((dark.red[256] - 0.70).abs() < 1e-4);
        assert!((dark.red[128] - 0.10).abs() < 1e-3);
        let half = dim_table(&base, 50, DimCurve::PeakPinned);
        assert!((half.green[256] - 0.70).abs() < 1e-4);
        assert!((half.green[128] - 0.30).abs() < 1e-3);
        let bright = dim_table(&base, 75, DimCurve::PeakPinned);
        assert!((bright.blue[256] - 0.80).abs() < 1e-4);
        assert!((bright.blue[128] - 0.40).abs() < 1e-3);
        let full = dim_table(&base, 100, DimCurve::PeakPinned);
        assert!(tables_close(&full, &base));
    }

    #[test]
    fn gamma_fallback_dims_and_restores_base() {
        let fake = Fake::new();
        let mut levels = fake.levels(None);
        assert_eq!(
            levels.read(&display()).unwrap().brightness,
            slider(100, Source::Gamma)
        );

        levels.set(&display(), Control::Brightness, 40).unwrap();
        assert_eq!(levels.dimmed(), [DisplayId("A".into())]);
        assert_eq!(
            levels.read(&display()).unwrap().brightness,
            slider(40, Source::Gamma)
        );

        levels.set(&display(), Control::Brightness, 20).unwrap();
        let expected = dim_table(&identity(4), 20, DimCurve::Linear { floor: 0.15 });
        assert!(tables_close(
            fake.state().gamma.as_ref().unwrap(),
            &expected
        ));

        levels.set(&display(), Control::Brightness, 100).unwrap();
        assert_eq!(fake.state().gamma, Some(identity(4)));
        assert!(levels.dimmed().is_empty());
    }

    #[test]
    fn gamma_changed_by_others_reads_as_full_and_is_released() {
        let fake = Fake::new();
        let mut levels = fake.levels(None);
        levels.set(&display(), Control::Brightness, 30).unwrap();
        fake.state().gamma = Some(identity(4));

        assert_eq!(
            levels.read(&display()).unwrap().brightness,
            slider(100, Source::Gamma)
        );
        assert!(levels.dimmed().is_empty());
        let writes = fake.state().gamma_writes;
        levels.set(&display(), Control::Brightness, 100).unwrap();
        assert_eq!(fake.state().gamma_writes, writes);
    }

    #[test]
    fn redimming_after_reset_takes_current_table_as_base() {
        let fake = Fake::new();
        let mut levels = fake.levels(None);
        levels.set(&display(), Control::Brightness, 30).unwrap();
        let other = dim_table(&identity(4), 90, DimCurve::Linear { floor: 0.0 });
        fake.state().gamma = Some(other.clone());

        levels.set(&display(), Control::Brightness, 50).unwrap();
        levels.restore_all();
        assert_eq!(fake.state().gamma, Some(other));
    }

    #[test]
    fn rejected_gamma_keeps_previous_state() {
        let fake = Fake::new();
        let mut levels = fake.levels(None);
        levels.set(&display(), Control::Brightness, 30).unwrap();
        fake.state().reject_gamma = true;
        assert!(levels.set(&display(), Control::Brightness, 10).is_err());
        assert_eq!(
            levels.read(&display()).unwrap().brightness,
            slider(30, Source::Gamma)
        );
    }

    #[test]
    fn restore_all_returns_every_dimmed_display() {
        let fake = Fake::new();
        let mut levels = fake.levels(None);
        levels.set(&display(), Control::Brightness, 10).unwrap();
        levels.restore_all();
        assert_eq!(fake.state().gamma, Some(identity(4)));
        assert!(levels.dimmed().is_empty());
    }

    #[test]
    fn endpoint_must_match_exactly_one_name() {
        let names = [
            "Speakers (Realtek(R) Audio)",
            "HP D27k (NVIDIA High Definition Audio)",
            "Headphones (USB Audio)",
        ];
        assert_eq!(unique_match(&names, "hp d27k"), Some(1));
        assert_eq!(unique_match(&names, "Audio"), None);
        assert_eq!(unique_match(&names, "DELL U2723QE"), None);
        assert_eq!(unique_match::<&str>(&[], "HP"), None);
    }

    #[test]
    fn tables_compare_across_sample_counts() {
        assert!(tables_close(&identity(4), &identity(256)));
        let dimmed = dim_table(&identity(4), 0, DimCurve::Linear { floor: 0.15 });
        assert!(!tables_close(&identity(4), &dimmed));
        let empty = GammaTable {
            red: Vec::new(),
            green: Vec::new(),
            blue: Vec::new(),
        };
        assert!(!tables_close(&empty, &identity(4)));
    }
}
