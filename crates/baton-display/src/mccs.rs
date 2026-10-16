//! DDC/CI（MCCS）报文的拼装与解析。
//!
//! 报文不含主机源地址 `0x51`：传输层把它作为 I2C 寄存器偏移单独发送，但它参与校验和。

use std::time::Duration;

use baton_core::{Error, Result};

pub const VCP_INPUT_SOURCE: u8 = 0x60;
/// DDC/CI 要求主机发完请求后至少等 40 毫秒再读回包。
pub const REPLY_DELAY: Duration = Duration::from_millis(50);
pub const VCP_REPLY_LEN: usize = 11;

const DISPLAY_ADDRESS: u8 = 0x6E;
const HOST_ADDRESS: u8 = 0x51;
const REPLY_CHECKSUM_SEED: u8 = 0x50;
const LENGTH_FLAG: u8 = 0x80;
const GET_VCP: u8 = 0x01;
const GET_VCP_REPLY: u8 = 0x02;
const SET_VCP: u8 = 0x03;

pub fn get_vcp_request(vcp: u8) -> [u8; 4] {
    with_checksum([LENGTH_FLAG | 2, GET_VCP, vcp, 0])
}

pub fn set_vcp_request(vcp: u8, value: u16) -> [u8; 6] {
    let [high, low] = value.to_be_bytes();
    with_checksum([LENGTH_FLAG | 4, SET_VCP, vcp, high, low, 0])
}

/// 解析 Get VCP Feature 回包：`6E 88 02 结果 VCP 类型 最大值(2) 当前值(2) 校验和`，返回当前值。
pub fn parse_vcp_reply(vcp: u8, reply: &[u8; VCP_REPLY_LEN]) -> Result<u16> {
    let bad = |what: &str| {
        Err(Error::Ddc(format!(
            "VCP 0x{vcp:02X} 回包{what}：{reply:02X?}"
        )))
    };
    if reply[0] != DISPLAY_ADDRESS || reply[1] != LENGTH_FLAG | 8 || reply[2] != GET_VCP_REPLY {
        return bad("格式不对");
    }
    if xor(REPLY_CHECKSUM_SEED, &reply[..10]) != reply[10] {
        return bad("校验和不对");
    }
    if reply[4] != vcp {
        return bad("的 VCP 编码不匹配");
    }
    if reply[3] != 0 {
        return Err(Error::Ddc(format!("显示器不支持 VCP 0x{vcp:02X}")));
    }
    Ok(u16::from_be_bytes([reply[8], reply[9]]))
}

fn with_checksum<const N: usize>(mut packet: [u8; N]) -> [u8; N] {
    packet[N - 1] = xor(DISPLAY_ADDRESS ^ HOST_ADDRESS, &packet[..N - 1]);
    packet
}

fn xor(seed: u8, bytes: &[u8]) -> u8 {
    bytes.iter().fold(seed, |acc, b| acc ^ b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(current: u16) -> [u8; VCP_REPLY_LEN] {
        let [high, low] = current.to_be_bytes();
        let mut r = [0x6E, 0x88, 0x02, 0x00, 0x60, 0x00, 0x00, 0x1B, high, low, 0];
        r[10] = xor(0x50, &r[..10]);
        r
    }

    #[test]
    fn requests_match_known_bytes() {
        assert_eq!(get_vcp_request(VCP_INPUT_SOURCE), [0x82, 0x01, 0x60, 0xDC]);
        assert_eq!(
            set_vcp_request(VCP_INPUT_SOURCE, 0x0F),
            [0x84, 0x03, 0x60, 0x00, 0x0F, 0xD7]
        );
    }

    #[test]
    fn parses_current_value() {
        assert_eq!(
            parse_vcp_reply(VCP_INPUT_SOURCE, &reply(0x0F)).unwrap(),
            0x0F
        );
    }

    #[test]
    fn rejects_empty_or_null_reply() {
        let empty = [0u8; VCP_REPLY_LEN];
        assert!(parse_vcp_reply(VCP_INPUT_SOURCE, &empty).is_err());
        let mut null = [0u8; VCP_REPLY_LEN];
        null[..3].copy_from_slice(&[0x6E, 0x80, 0xBE]);
        assert!(parse_vcp_reply(VCP_INPUT_SOURCE, &null).is_err());
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut r = reply(0x0F);
        r[10] ^= 0xFF;
        assert!(parse_vcp_reply(VCP_INPUT_SOURCE, &r).is_err());
    }

    #[test]
    fn rejects_other_vcp_or_unsupported() {
        assert!(parse_vcp_reply(0x10, &reply(0x0F)).is_err());
        let mut r = reply(0x0F);
        r[3] = 0x01;
        r[10] = xor(0x50, &r[..10]);
        assert!(parse_vcp_reply(VCP_INPUT_SOURCE, &r).is_err());
    }
}
