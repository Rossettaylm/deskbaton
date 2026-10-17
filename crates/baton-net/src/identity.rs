//! 本机 TLS 身份：自签证书 + 私钥，首次启动生成后持久化，证书指纹即设备身份。
//!
//! TLS 层只验证对端持有所出示证书的私钥；对端是否可信由设备档案里固定的指纹、
//! 预共享令牌或用户确认的配对码决定，见 [`crate::pairing`]。

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::Arc;

use baton_core::{Error, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme,
};

use crate::auth::{self, Fingerprint, SERVER_NAME};

const CERT_FILE: &str = "cert.der";
const KEY_FILE: &str = "key.der";

pub struct TlsIdentity {
    pub client: Arc<ClientConfig>,
    pub server: Arc<ServerConfig>,
    pub fingerprint: Fingerprint,
}

impl TlsIdentity {
    /// 生成一次性身份（测试用；正式运行用 [`Self::load_or_create`]）。
    pub fn generate() -> Result<Self> {
        let (cert, key) = new_keypair()?;
        Self::from_der(cert, key)
    }

    /// 从 `dir` 读取身份；目录里没有时生成并保存。文件损坏时报错而不是重新生成，
    /// 否则所有已配对设备都会失效。
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let cert_path = dir.join(CERT_FILE);
        let key_path = dir.join(KEY_FILE);
        match (read(&cert_path)?, read(&key_path)?) {
            (Some(cert), Some(key)) => Self::from_der(cert, key),
            (None, None) => {
                fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
                let (cert, key) = new_keypair()?;
                write_new(&key_path, &key, true)?;
                write_new(&cert_path, &cert, false)?;
                log::info!("已生成本机身份：{}", dir.display());
                Self::from_der(cert, key)
            }
            _ => Err(Error::Config(format!(
                "本机身份文件不完整：{}。删除该目录会重新生成身份，需要与所有设备重新配对",
                dir.display()
            ))),
        }
    }

    fn from_der(cert: Vec<u8>, key: Vec<u8>) -> Result<Self> {
        let cert = CertificateDer::from(cert);
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key));
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
            fingerprint: auth::fingerprint(&cert),
        })
    }
}

fn new_keypair() -> Result<(Vec<u8>, Vec<u8>)> {
    let certified = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_owned()])
        .map_err(|e| Error::Net(format!("生成证书失败：{e}")))?;
    Ok((
        certified.cert.der().to_vec(),
        certified.signing_key.serialize_der(),
    ))
}

fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(path, e)),
    }
}

fn write_new(path: &Path, bytes: &[u8], secret: bool) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = secret;
    let mut file = options.open(path).map_err(|e| io_error(path, e))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| io_error(path, e))
}

fn io_error(path: &Path, e: std::io::Error) -> Error {
    Error::Config(format!("读写 {} 失败：{e}", path.display()))
}

/// 接受任意证书，但要求对端证明持有证书私钥。
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

    #[test]
    fn generated_identities_differ() {
        let a = TlsIdentity::generate().unwrap();
        let b = TlsIdentity::generate().unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn persisted_identity_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity");
        let first = TlsIdentity::load_or_create(&path).unwrap();
        let second = TlsIdentity::load_or_create(&path).unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path.join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn incomplete_identity_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        TlsIdentity::load_or_create(dir.path()).unwrap();
        fs::remove_file(dir.path().join(KEY_FILE)).unwrap();
        assert!(matches!(
            TlsIdentity::load_or_create(dir.path()),
            Err(Error::Config(_))
        ));
    }
}
