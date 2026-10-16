//! EDID 1.x 解析：只取厂商、型号和序列号。

const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
const DESCRIPTOR_OFFSETS: [usize; 4] = [54, 72, 90, 108];
const TAG_SERIAL: u8 = 0xFF;
const TAG_NAME: u8 = 0xFC;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edid {
    /// PNP 厂商代码，如 `GSM`。
    pub vendor: String,
    pub model: Option<String>,
    pub serial: Option<String>,
}

pub fn parse(edid: &[u8]) -> Option<Edid> {
    if edid.len() < 128 || edid[..8] != HEADER {
        return None;
    }
    let id = u16::from_be_bytes([edid[8], edid[9]]);
    let vendor = [10, 5, 0]
        .iter()
        .map(|shift| char::from(b'A' - 1 + ((id >> shift) & 0x1F) as u8))
        .collect();
    let descriptor_text = |tag: u8| {
        DESCRIPTOR_OFFSETS.iter().find_map(|&offset| {
            let d = &edid[offset..offset + 18];
            (d[..3] == [0, 0, 0] && d[3] == tag).then(|| descriptor_string(&d[5..]))
        })
    };
    let numeric_serial = u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]);
    Some(Edid {
        vendor,
        model: descriptor_text(TAG_NAME).filter(|s| !s.is_empty()),
        serial: descriptor_text(TAG_SERIAL)
            .filter(|s| !s.is_empty())
            .or_else(|| (numeric_serial != 0).then(|| numeric_serial.to_string())),
    })
}

/// 描述符文本以 `0x0A` 结束，其余用空格补齐。
fn descriptor_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0x0A).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

/// 由 `EnumDisplayDevices` 返回的设备接口路径（`\\?\DISPLAY#GSM5B7F#5&2f1b&0&UID4352#{...}`）
/// 得到注册表中保存 EDID 的键（相对 `HKEY_LOCAL_MACHINE`）。
pub fn registry_key(device_interface: &str) -> Option<String> {
    let mut parts = device_interface.split('#');
    let _prefix = parts.next()?;
    let model = parts.next().filter(|s| !s.is_empty())?;
    let instance = parts.next().filter(|s| !s.is_empty())?;
    Some(format!(
        r"SYSTEM\CurrentControlSet\Enum\DISPLAY\{model}\{instance}\Device Parameters"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(tag: u8, text: &str) -> [u8; 18] {
        let mut d = [0u8; 18];
        d[3] = tag;
        let mut body = [0x20u8; 13];
        body[..text.len()].copy_from_slice(text.as_bytes());
        if text.len() < 13 {
            body[text.len()] = 0x0A;
        }
        d[5..].copy_from_slice(&body);
        d
    }

    fn sample(numeric_serial: u32, descriptors: &[[u8; 18]]) -> Vec<u8> {
        let mut edid = vec![0u8; 128];
        edid[..8].copy_from_slice(&HEADER);
        // "GSM"：G=7, S=19, M=13
        let id: u16 = (7 << 10) | (19 << 5) | 13;
        edid[8..10].copy_from_slice(&id.to_be_bytes());
        edid[12..16].copy_from_slice(&numeric_serial.to_le_bytes());
        for (d, offset) in descriptors.iter().zip(DESCRIPTOR_OFFSETS) {
            edid[offset..offset + 18].copy_from_slice(d);
        }
        edid
    }

    #[test]
    fn parses_vendor_name_and_serial() {
        let edid = sample(
            1,
            &[
                descriptor(TAG_NAME, "P275MV"),
                descriptor(TAG_SERIAL, "ABC123"),
            ],
        );
        assert_eq!(
            parse(&edid),
            Some(Edid {
                vendor: "GSM".into(),
                model: Some("P275MV".into()),
                serial: Some("ABC123".into()),
            })
        );
    }

    #[test]
    fn falls_back_to_numeric_serial() {
        let edid = sample(50236, &[descriptor(TAG_NAME, "HP D27k")]);
        let parsed = parse(&edid).unwrap();
        assert_eq!(parsed.model.as_deref(), Some("HP D27k"));
        assert_eq!(parsed.serial.as_deref(), Some("50236"));
    }

    #[test]
    fn rejects_bad_header_or_short_input() {
        assert_eq!(parse(&[0u8; 128]), None);
        assert_eq!(parse(&HEADER), None);
    }

    #[test]
    fn registry_key_from_device_interface() {
        assert_eq!(
            registry_key(
                r"\\?\DISPLAY#GSM5B7F#5&2f1b1a0&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            )
            .as_deref(),
            Some(
                r"SYSTEM\CurrentControlSet\Enum\DISPLAY\GSM5B7F\5&2f1b1a0&0&UID4352\Device Parameters"
            )
        );
        assert_eq!(registry_key("MONITOR\\Default_Monitor"), None);
    }
}
