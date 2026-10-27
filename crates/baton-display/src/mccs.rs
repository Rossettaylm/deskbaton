//! DDC/CI（MCCS）报文的拼装与解析。
//!
//! 报文不含主机源地址 `0x51`：传输层把它作为 I2C 寄存器偏移单独发送，但它参与校验和。

use baton_core::{Error, Result};

use crate::vcp::VcpValue;

pub const VCP_REPLY_LEN: usize = 11;

const DISPLAY_ADDRESS: u8 = 0x6E;
const HOST_ADDRESS: u8 = 0x51;
const REPLY_CHECKSUM_SEED: u8 = 0x50;
const LENGTH_FLAG: u8 = 0x80;
const GET_VCP: u8 = 0x01;
const GET_VCP_REPLY: u8 = 0x02;
const SET_VCP: u8 = 0x03;
const NULL_MESSAGE: u8 = 0xBE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcpReply {
    Ready(VcpValue),
    /// 回包结果码非 0：显示器不支持这个 VCP。
    Unsupported,
    NotReady,
}

pub fn get_vcp_request(vcp: u8) -> [u8; 4] {
    with_checksum([LENGTH_FLAG | 2, GET_VCP, vcp, 0])
}

pub fn set_vcp_request(vcp: u8, value: u16) -> [u8; 6] {
    let [high, low] = value.to_be_bytes();
    with_checksum([LENGTH_FLAG | 4, SET_VCP, vcp, high, low, 0])
}

/// Null Message（`6E 80 BE`）只有 3 字节，缓冲区其余部分是总线残留，只看头三字节。
fn is_null_message(reply: &[u8; VCP_REPLY_LEN]) -> bool {
    reply[0] == DISPLAY_ADDRESS && reply[1] == LENGTH_FLAG && reply[2] == NULL_MESSAGE
}

pub fn classify_vcp_reply(vcp: u8, reply: &[u8; VCP_REPLY_LEN]) -> Result<VcpReply> {
    if is_null_message(reply) {
        return Ok(VcpReply::NotReady);
    }
    Ok(match parse_vcp_reply(vcp, reply)? {
        Some(value) => VcpReply::Ready(value),
        None => VcpReply::Unsupported,
    })
}

/// 解析 Get VCP Feature 回包：`6E 88 02 结果 VCP 类型 最大值(2) 当前值(2) 校验和`。结果码非 0 时为 `None`。
fn parse_vcp_reply(vcp: u8, reply: &[u8; VCP_REPLY_LEN]) -> Result<Option<VcpValue>> {
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
        return Ok(None);
    }
    Ok(Some(VcpValue {
        current: u16::from_be_bytes([reply[8], reply[9]]),
        max: u16::from_be_bytes([reply[6], reply[7]]),
    }))
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
    use crate::vcp::{BRIGHTNESS, INPUT_SOURCE};

    fn reply(current: u16) -> [u8; VCP_REPLY_LEN] {
        let [high, low] = current.to_be_bytes();
        let mut r = [0x6E, 0x88, 0x02, 0x00, 0x60, 0x00, 0x00, 0x1B, high, low, 0];
        r[10] = xor(0x50, &r[..10]);
        r
    }

    #[test]
    fn requests_match_known_bytes() {
        assert_eq!(get_vcp_request(INPUT_SOURCE), [0x82, 0x01, 0x60, 0xDC]);
        assert_eq!(
            set_vcp_request(INPUT_SOURCE, 0x0F),
            [0x84, 0x03, 0x60, 0x00, 0x0F, 0xD7]
        );
    }

    #[test]
    fn parses_current_and_max() {
        assert_eq!(
            parse_vcp_reply(INPUT_SOURCE, &reply(0x0F)).unwrap(),
            Some(VcpValue {
                current: 0x0F,
                max: 0x1B
            })
        );
    }

    #[test]
    fn rejects_empty_or_null_reply() {
        let empty = [0u8; VCP_REPLY_LEN];
        assert!(parse_vcp_reply(INPUT_SOURCE, &empty).is_err());
        let mut null = [0u8; VCP_REPLY_LEN];
        null[..3].copy_from_slice(&[0x6E, 0x80, 0xBE]);
        assert!(parse_vcp_reply(INPUT_SOURCE, &null).is_err());
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut r = reply(0x0F);
        r[10] ^= 0xFF;
        assert!(parse_vcp_reply(INPUT_SOURCE, &r).is_err());
    }

    #[test]
    fn rejects_other_vcp() {
        assert!(parse_vcp_reply(BRIGHTNESS, &reply(0x0F)).is_err());
    }

    #[test]
    fn classifies_nonzero_result_code_as_unsupported() {
        let mut r = reply(0x0F);
        r[3] = 0x01;
        r[10] = xor(0x50, &r[..10]);
        assert_eq!(
            classify_vcp_reply(INPUT_SOURCE, &r).unwrap(),
            VcpReply::Unsupported
        );
    }

    #[test]
    fn classifies_null_message_as_not_ready_regardless_of_tail() {
        let p275mv_null = [
            0x6E, 0x80, 0xBE, 0x0D, 0xD1, 0x08, 0x06, 0xB2, 0x67, 0xBE, 0xA2,
        ];
        assert_eq!(
            classify_vcp_reply(INPUT_SOURCE, &p275mv_null).unwrap(),
            VcpReply::NotReady
        );
    }

    #[test]
    fn classifies_valid_reply_as_ready() {
        assert_eq!(
            classify_vcp_reply(INPUT_SOURCE, &reply(0x0F)).unwrap(),
            VcpReply::Ready(VcpValue {
                current: 0x0F,
                max: 0x1B
            })
        );
    }

    #[test]
    fn classifies_garbage_as_error() {
        let garbage = [
            0xA8, 0xFC, 0x9B, 0x0D, 0xD1, 0x08, 0x06, 0xB2, 0x67, 0xBE, 0xA2,
        ];
        assert!(classify_vcp_reply(INPUT_SOURCE, &garbage).is_err());
    }
}
