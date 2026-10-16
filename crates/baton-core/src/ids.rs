use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandoffId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DisplayId(pub String);

/// MCCS VCP `0x60`（Input Source）的取值，各显示器厂商可能不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InputCode(pub u16);

impl FromStr for InputCode {
    type Err = Error;

    /// 接受十进制（`27`）或十六进制（`0x1B`）。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let parsed = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            Some(hex) => u16::from_str_radix(hex, 16),
            None => s.parse(),
        };
        parsed
            .map(InputCode)
            .map_err(|_| Error::Config(format!("无效的输入源编码：{s}")))
    }
}

impl fmt::Display for InputCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:02X}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacAddr(pub [u8; 6]);

impl FromStr for MacAddr {
    type Err = Error;

    /// 接受 `aa:bb:cc:dd:ee:ff` 或 `aa-bb-cc-dd-ee-ff`。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || Error::Config(format!("无效的 MAC 地址：{s}"));
        let parts: Vec<&str> = s.split([':', '-']).collect();
        if parts.len() != 6 {
            return Err(invalid());
        }
        let mut bytes = [0u8; 6];
        for (byte, part) in bytes.iter_mut().zip(parts) {
            *byte = u8::from_str_radix(part, 16).map_err(|_| invalid())?;
        }
        Ok(MacAddr(bytes))
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{g:02x}")
    }
}

impl Serialize for MacAddr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MacAddr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_code_parses_hex_and_decimal() {
        assert_eq!("0x1B".parse::<InputCode>().unwrap(), InputCode(0x1B));
        assert_eq!("27".parse::<InputCode>().unwrap(), InputCode(27));
        assert!("0xZZ".parse::<InputCode>().is_err());
    }

    #[test]
    fn mac_addr_roundtrip() {
        let mac: MacAddr = "AA-bb-01-02-03-04".parse().unwrap();
        assert_eq!(mac.to_string(), "aa:bb:01:02:03:04");
        assert!("aa:bb:cc".parse::<MacAddr>().is_err());
    }
}
