//! 配对：承诺式 6 位配对码与信任判定，流程见 `docs/superpowers/specs/2026-10-17-pairing-design.md` 第 4、5 节。

use baton_core::{Error, Result};
use sha2::{Digest, Sha256};

use crate::auth::Fingerprint;

pub type Nonce = [u8; 32];

pub fn new_nonce() -> Result<Nonce> {
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce).map_err(|e| Error::Net(format!("无法生成随机数：{e}")))?;
    Ok(nonce)
}

/// 拨号方对随机数的承诺。
pub fn commit(nonce: &[u8]) -> [u8; 32] {
    Sha256::digest(nonce).into()
}

pub fn verify_commit(commitment: &[u8], nonce: &[u8]) -> bool {
    commitment == commit(nonce).as_slice()
}

/// 两端用相同的输入算出相同的 6 位数字；任一指纹或随机数不同，结果几乎必然不同。
pub fn pairing_code(
    client_fp: &Fingerprint,
    server_fp: &Fingerprint,
    client_nonce: &[u8],
    server_nonce: &[u8],
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"deskbaton-pair-v1");
    hash.update(client_fp);
    hash.update(server_fp);
    hash.update(client_nonce);
    hash.update(server_nonce);
    let digest = hash.finalize();
    let n = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{n:06}")
}

/// 收到对端 hello 并交换完随机数后，本机对它的判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// 指纹已在设备档案里。
    Pinned,
    /// 预共享令牌校验通过，自动写入档案。
    Token,
    /// 需要用户核对配对码后确认。
    AskUser,
    Deny,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TrustInput {
    /// 拨号时指定了期望的指纹或名称，而对端不符。
    pub unexpected_peer: bool,
    pub pinned: bool,
    pub token_ok: bool,
    /// 本进程有配对界面。
    pub interactive: bool,
    /// 该指纹本次运行中被用户拒绝过。
    pub rejected_before: bool,
    /// 用户主动发起的这次配对。
    pub initiated_by_user: bool,
}

pub fn decide(input: TrustInput) -> Trust {
    if input.unexpected_peer {
        Trust::Deny
    } else if input.pinned {
        Trust::Pinned
    } else if input.token_ok {
        Trust::Token
    } else if input.interactive && (!input.rejected_before || input.initiated_by_user) {
        Trust::AskUser
    } else {
        Trust::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Fingerprint = [1; 32];
    const B: Fingerprint = [2; 32];

    #[test]
    fn both_ends_derive_the_same_code() {
        let (nc, ns) = ([3u8; 32], [4u8; 32]);
        let code = pairing_code(&A, &B, &nc, &ns);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(code, pairing_code(&A, &B, &nc, &ns));
    }

    #[test]
    fn code_depends_on_every_input() {
        let (nc, ns) = ([3u8; 32], [4u8; 32]);
        let code = pairing_code(&A, &B, &nc, &ns);
        assert_ne!(code, pairing_code(&B, &A, &nc, &ns));
        assert_ne!(code, pairing_code(&A, &[9; 32], &nc, &ns));
        assert_ne!(code, pairing_code(&A, &B, &[5; 32], &ns));
        assert_ne!(code, pairing_code(&A, &B, &nc, &[5; 32]));
    }

    #[test]
    fn commitment_binds_the_nonce() {
        let nonce = new_nonce().unwrap();
        let c = commit(&nonce);
        assert!(verify_commit(&c, &nonce));
        assert!(!verify_commit(&c, &[0; 32]));
        assert_ne!(nonce, new_nonce().unwrap());
    }

    #[test]
    fn trust_follows_the_decision_table() {
        let base = TrustInput::default();
        let with = |f: fn(&mut TrustInput)| {
            let mut input = base;
            f(&mut input);
            decide(input)
        };
        assert_eq!(decide(base), Trust::Deny);
        assert_eq!(
            with(|i| {
                i.pinned = true;
                i.unexpected_peer = true;
            }),
            Trust::Deny
        );
        assert_eq!(
            with(|i| {
                i.pinned = true;
                i.token_ok = true;
            }),
            Trust::Pinned
        );
        assert_eq!(with(|i| i.token_ok = true), Trust::Token);
        assert_eq!(with(|i| i.interactive = true), Trust::AskUser);
        assert_eq!(
            with(|i| {
                i.interactive = true;
                i.rejected_before = true;
            }),
            Trust::Deny
        );
        assert_eq!(
            with(|i| {
                i.interactive = true;
                i.rejected_before = true;
                i.initiated_by_user = true;
            }),
            Trust::AskUser
        );
        assert_eq!(with(|i| i.initiated_by_user = true), Trust::Deny);
    }
}
