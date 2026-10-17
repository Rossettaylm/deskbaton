//! 预共享令牌认证：Hello 中的 `auth` 为令牌对「角色、发送方名称、双方证书指纹」的
//! HMAC-SHA256。中间人终止 TLS 时两端看到的指纹不同，证明无法通过。

use baton_core::ids::DeviceId;
use baton_core::{Error, Result};
use hmac::{Hmac, KeyInit, Mac};
use rustls::pki_types::CertificateDer;
use sha2::{Digest, Sha256};

pub type Fingerprint = [u8; 32];

/// TLS 握手时使用的服务器名；证书不按名称校验。
pub const SERVER_NAME: &str = "deskbaton";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// 主动拨号的一方。
    Client,
    /// 接受连接的一方。
    Server,
}

impl Role {
    fn label(self) -> &'static [u8] {
        match self {
            Role::Client => b"client",
            Role::Server => b"server",
        }
    }

    pub fn peer(self) -> Role {
        match self {
            Role::Client => Role::Server,
            Role::Server => Role::Client,
        }
    }
}

pub fn fingerprint(cert: &CertificateDer<'_>) -> Fingerprint {
    Sha256::digest(cert.as_ref()).into()
}

/// 计算 `sender` 以 `role` 身份发出的认证证明。
pub fn proof(
    token: &[u8],
    role: Role,
    sender: &DeviceId,
    client_fp: &Fingerprint,
    server_fp: &Fingerprint,
) -> Result<Vec<u8>> {
    Ok(mac(token, role, sender, client_fp, server_fp)?
        .finalize()
        .into_bytes()
        .to_vec())
}

/// 常数时间比较。
pub fn verify(
    token: &[u8],
    role: Role,
    sender: &DeviceId,
    client_fp: &Fingerprint,
    server_fp: &Fingerprint,
    claimed: &[u8],
) -> bool {
    mac(token, role, sender, client_fp, server_fp)
        .is_ok_and(|mac| mac.verify_slice(claimed).is_ok())
}

fn mac(
    token: &[u8],
    role: Role,
    sender: &DeviceId,
    client_fp: &Fingerprint,
    server_fp: &Fingerprint,
) -> Result<Hmac<Sha256>> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(token)
        .map_err(|e| Error::Net(format!("无效的令牌：{e}")))?;
    for part in [
        b"deskbaton-auth-v2".as_slice(),
        role.label(),
        sender.0.as_bytes(),
    ] {
        mac.update(&(part.len() as u32).to_be_bytes());
        mac.update(part);
    }
    mac.update(client_fp);
    mac.update(server_fp);
    Ok(mac)
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse_fingerprint(s: &str) -> Option<Fingerprint> {
    from_hex(s).and_then(|bytes| Fingerprint::try_from(bytes).ok())
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Fingerprint = [1; 32];
    const B: Fingerprint = [2; 32];

    #[test]
    fn proof_binds_token_role_sender_and_fingerprints() {
        let mac = DeviceId("mac".into());
        let p = proof(b"token", Role::Client, &mac, &A, &B).unwrap();
        assert!(verify(b"token", Role::Client, &mac, &A, &B, &p));
        assert!(!verify(b"other", Role::Client, &mac, &A, &B, &p));
        assert!(!verify(b"token", Role::Server, &mac, &A, &B, &p));
        assert!(!verify(
            b"token",
            Role::Client,
            &DeviceId("win".into()),
            &A,
            &B,
            &p
        ));
        assert!(!verify(b"token", Role::Client, &mac, &B, &A, &p));
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(
            from_hex(&to_hex(&[0, 0xAB, 0xFF])),
            Some(vec![0, 0xAB, 0xFF])
        );
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex("zz"), None);
    }
}
