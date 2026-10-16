//! TLS 身份与对端认证。
//!
//! 每次启动生成自签证书，TLS 层只验证对端持有所出示证书的私钥；对端身份由 Hello 中的
//! `auth` 证明：预共享令牌对「角色、发送方名称、双方证书指纹」的 HMAC-SHA256。
//! 中间人终止 TLS 时两端看到的指纹不同，证明无法通过。

use std::sync::Arc;

use baton_core::ids::DeviceId;
use baton_core::{Error, Result};
use hmac::{Hmac, KeyInit, Mac};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme,
};
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

pub struct TlsIdentity {
    pub client: Arc<ClientConfig>,
    pub server: Arc<ServerConfig>,
    pub fingerprint: Fingerprint,
}

impl TlsIdentity {
    pub fn generate() -> Result<Self> {
        let certified = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_owned()])
            .map_err(|e| Error::Net(format!("生成证书失败：{e}")))?;
        let cert = certified.cert.der().clone();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(PossessionOnly(provider.signature_verification_algorithms));
        let tls = |e: rustls::Error| Error::Net(format!("TLS 配置失败：{e}"));

        let client = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(tls)?
            .dangerous()
            .with_custom_certificate_verifier(verifier.clone())
            .with_client_auth_cert(vec![cert.clone()], key.clone_key())
            .map_err(tls)?;
        let server = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(tls)?
            .with_client_cert_verifier(verifier)
            .with_single_cert(vec![cert.clone()], key)
            .map_err(tls)?;
        Ok(Self {
            client: Arc::new(client),
            server: Arc::new(server),
            fingerprint: fingerprint(&cert),
        })
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

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// 接受任意证书，但要求对端证明持有证书私钥；身份由 Hello 的 `auth` 认证。
#[derive(Debug)]
struct PossessionOnly(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for PossessionOnly {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("只支持 TLS 1.3".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

impl ClientCertVerifier for PossessionOnly {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("只支持 TLS 1.3".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
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

    #[test]
    fn generates_identity() {
        let a = TlsIdentity::generate().unwrap();
        let b = TlsIdentity::generate().unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
    }
}
