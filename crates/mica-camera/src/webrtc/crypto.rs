// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! The cryptography str0m needs, from pure-Rust crates: SRTP (AES-128 counter mode with
//! HMAC-SHA1, AES-GCM), HMAC-SHA1 for STUN, SHA-256 for certificate fingerprints and DTLS
//! (dimpl on RustCrypto). The DTLS certificate (a fresh self-signed ECDSA P-256 one per
//! connection) is made with rcgen on `ring`, which rustls already brings in. str0m's own
//! pure-Rust backend (`str0m-rust-crypto`) would pull in AWS-LC, a second, C crypto library,
//! only to make that certificate; the provider glue below follows its shape (str0m-rust-crypto
//! 0.6.0, MIT OR Apache-2.0, see THIRD_PARTY.md).

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes128Gcm, Aes256Gcm, Nonce};
use ctr::cipher::{KeyIvInit, StreamCipher};
use hmac::{Hmac, Mac};
use sha2::Digest;
use std::{sync::Arc, time::Instant};
use str0m::crypto::dtls::{
    DtlsCert, DtlsImplError, DtlsInstance, DtlsOutput, DtlsProvider, DtlsVersion, ProtocolVersion,
};
use str0m::crypto::{
    AeadAes128Gcm, AeadAes128GcmCipher, AeadAes256Gcm, AeadAes256GcmCipher, Aes128CmSha1_80Cipher,
    CryptoError, CryptoProvider, Sha1HmacProvider, Sha256Provider, SrtpProvider,
    SupportedAeadAes128Gcm, SupportedAeadAes256Gcm, SupportedAes128CmSha1_80,
};

/// The provider every Mica WebRTC connection uses.
pub fn provider() -> Arc<CryptoProvider> {
    static SRTP: Srtp = Srtp;
    static HMAC: HmacSha1 = HmacSha1;
    static SHA256: Sha256 = Sha256;
    static DTLS: Dtls = Dtls;
    Arc::new(CryptoProvider {
        srtp_provider: &SRTP,
        sha1_hmac_provider: &HMAC,
        sha256_provider: &SHA256,
        dtls_provider: &DTLS,
    })
}

#[derive(Debug)]
struct HmacSha1;
impl Sha1HmacProvider for HmacSha1 {
    fn sha1_hmac(&self, key: &[u8], payloads: &[&[u8]]) -> [u8; 20] {
        let mut mac =
            <Hmac<sha1::Sha1> as Mac>::new_from_slice(key).expect("HMAC takes any key length");
        for p in payloads {
            mac.update(p);
        }
        mac.finalize().into_bytes().into()
    }
}

#[derive(Debug)]
struct Sha256;
impl Sha256Provider for Sha256 {
    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        sha2::Sha256::digest(data).into()
    }
}

// ---- SRTP ----

type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

#[derive(Debug)]
struct CmCipher {
    key: [u8; 16],
}
impl Aes128CmSha1_80Cipher for CmCipher {
    fn encrypt(
        &mut self,
        iv: &[u8; 16],
        input: &[u8],
        output: &mut [u8],
    ) -> Result<(), CryptoError> {
        let out = output
            .get_mut(..input.len())
            .ok_or_else(|| CryptoError::Other("SRTP output buffer too small".into()))?;
        out.copy_from_slice(input);
        Aes128Ctr::new(&self.key.into(), iv.into()).apply_keystream(out);
        Ok(())
    }
    fn decrypt(
        &mut self,
        iv: &[u8; 16],
        input: &[u8],
        output: &mut [u8],
    ) -> Result<(), CryptoError> {
        // Counter mode is its own inverse.
        self.encrypt(iv, input, output)
    }
}

/// AES-GCM with the associated data given in pieces.
fn gcm_open<C: Aead>(
    cipher: &C,
    iv: &[u8],
    aads: &[&[u8]],
    input: &[u8],
    output: &mut [u8],
) -> Result<usize, CryptoError> {
    let aad = aads.concat();
    let plain = cipher
        .decrypt(
            Nonce::from_slice(iv),
            Payload {
                msg: input,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Other("AES-GCM authentication failed".into()))?;
    output
        .get_mut(..plain.len())
        .ok_or_else(|| CryptoError::Other("SRTP output buffer too small".into()))?
        .copy_from_slice(&plain);
    Ok(plain.len())
}

fn gcm_seal<C: Aead>(
    cipher: &C,
    iv: &[u8],
    aad: &[u8],
    input: &[u8],
    output: &mut [u8],
) -> Result<(), CryptoError> {
    let sealed = cipher
        .encrypt(Nonce::from_slice(iv), Payload { msg: input, aad })
        .map_err(|_| CryptoError::Other("AES-GCM encryption failed".into()))?;
    output
        .get_mut(..sealed.len())
        .ok_or_else(|| CryptoError::Other("SRTP output buffer too small".into()))?
        .copy_from_slice(&sealed);
    Ok(())
}

struct Gcm128(Aes128Gcm);
impl std::fmt::Debug for Gcm128 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Gcm128")
    }
}
impl AeadAes128GcmCipher for Gcm128 {
    fn encrypt(
        &mut self,
        iv: &[u8; AeadAes128Gcm::IV_LEN],
        aad: &[u8],
        input: &[u8],
        output: &mut [u8],
    ) -> Result<(), CryptoError> {
        gcm_seal(&self.0, iv, aad, input, output)
    }
    fn decrypt(
        &mut self,
        iv: &[u8; AeadAes128Gcm::IV_LEN],
        aads: &[&[u8]],
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CryptoError> {
        gcm_open(&self.0, iv, aads, input, output)
    }
}

struct Gcm256(Aes256Gcm);
impl std::fmt::Debug for Gcm256 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Gcm256")
    }
}
impl AeadAes256GcmCipher for Gcm256 {
    fn encrypt(
        &mut self,
        iv: &[u8; AeadAes256Gcm::IV_LEN],
        aad: &[u8],
        input: &[u8],
        output: &mut [u8],
    ) -> Result<(), CryptoError> {
        gcm_seal(&self.0, iv, aad, input, output)
    }
    fn decrypt(
        &mut self,
        iv: &[u8; AeadAes256Gcm::IV_LEN],
        aads: &[&[u8]],
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CryptoError> {
        gcm_open(&self.0, iv, aads, input, output)
    }
}

#[derive(Debug)]
struct Cm;
impl SupportedAes128CmSha1_80 for Cm {
    fn create_cipher(&self, key: [u8; 16], _encrypt: bool) -> Box<dyn Aes128CmSha1_80Cipher> {
        Box::new(CmCipher { key })
    }
}
#[derive(Debug)]
struct G128;
impl SupportedAeadAes128Gcm for G128 {
    fn create_cipher(&self, key: [u8; 16], _encrypt: bool) -> Box<dyn AeadAes128GcmCipher> {
        Box::new(Gcm128(Aes128Gcm::new(GenericArray::from_slice(&key))))
    }
}
#[derive(Debug)]
struct G256;
impl SupportedAeadAes256Gcm for G256 {
    fn create_cipher(&self, key: [u8; 32], _encrypt: bool) -> Box<dyn AeadAes256GcmCipher> {
        Box::new(Gcm256(Aes256Gcm::new(GenericArray::from_slice(&key))))
    }
}

/// One AES block of `input` and one of PKCS#7 padding (16 × 0x10), as SRTP key derivation
/// expects from an ECB "round".
fn ecb_round<C>(cipher: &C, input: &[u8], output: &mut [u8])
where
    C: aes::cipher::BlockEncrypt + aes::cipher::BlockSizeUser<BlockSize = aes::cipher::consts::U16>,
{
    assert!(input.len() == 16 && output.len() >= 32);
    let mut a = *GenericArray::from_slice(input);
    cipher.encrypt_block(&mut a);
    output[..16].copy_from_slice(&a);
    let mut b = GenericArray::from([0x10u8; 16]);
    cipher.encrypt_block(&mut b);
    output[16..32].copy_from_slice(&b);
}

#[derive(Debug)]
struct Srtp;
impl SrtpProvider for Srtp {
    fn aes_128_cm_sha1_80(&self) -> &'static dyn SupportedAes128CmSha1_80 {
        &Cm
    }
    fn aead_aes_128_gcm(&self) -> &'static dyn SupportedAeadAes128Gcm {
        &G128
    }
    fn aead_aes_256_gcm(&self) -> &'static dyn SupportedAeadAes256Gcm {
        &G256
    }
    fn srtp_aes_128_ecb_round(&self, key: &[u8], input: &[u8], output: &mut [u8]) {
        use aes::cipher::KeyInit;
        ecb_round(
            &aes::Aes128::new(GenericArray::from_slice(key)),
            input,
            output,
        );
    }
    fn srtp_aes_256_ecb_round(&self, key: &[u8], input: &[u8], output: &mut [u8]) {
        use aes::cipher::KeyInit;
        ecb_round(
            &aes::Aes256::new(GenericArray::from_slice(key)),
            input,
            output,
        );
    }
}

// ---- DTLS ----

#[derive(Debug)]
struct Dtls;
impl DtlsProvider for Dtls {
    fn generate_certificate(&self) -> Option<DtlsCert> {
        certificate().ok()
    }
    fn new_dtls(
        &self,
        cert: &DtlsCert,
        now: Instant,
        version: DtlsVersion,
        mtu: Option<usize>,
    ) -> Result<Box<dyn DtlsInstance>, CryptoError> {
        // ICE has already checked the peer's address, so no server cookies.
        let mut builder = dimpl::Config::builder().use_server_cookie(false);
        if let Some(mtu) = mtu {
            builder = builder.mtu(mtu);
        }
        let config = Arc::new(
            builder
                .build()
                .map_err(|e| CryptoError::Other(format!("DTLS configuration: {e}")))?,
        );
        let cert = dimpl::DtlsCertificate {
            certificate: cert.certificate.clone(),
            private_key: cert.private_key.clone(),
        };
        let dtls = match version {
            DtlsVersion::Dtls12 => dimpl::Dtls::new_12(config, cert, now),
            DtlsVersion::Dtls13 => dimpl::Dtls::new_13(config, cert, now),
            DtlsVersion::Auto => dimpl::Dtls::new_auto(config, cert, now),
            v => return Err(CryptoError::Other(format!("unsupported DTLS version {v}"))),
        };
        Ok(Box::new(Instance(dtls)))
    }
}

/// A self-signed ECDSA P-256 certificate (DER) and its PKCS#8 key, valid for a year, with a
/// random serial number (browsers and libdatachannel expect them to differ).
fn certificate() -> Result<DtlsCert, rcgen::Error> {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new())?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Mica Slicer");
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(365);
    // The serial: a hash of the fresh (randomly generated) public key, so unique per key.
    let mut serial: [u8; 16] = sha2::Sha256::digest(key.public_key_raw()).as_slice()[..16]
        .try_into()
        .expect("16 of 32 bytes");
    serial[0] &= 0x7F;
    params.serial_number = Some(rcgen::SerialNumber::from_slice(&serial));
    let cert = params.self_signed(&key)?;
    Ok(DtlsCert {
        certificate: cert.der().to_vec(),
        private_key: key.serialize_der(),
    })
}

struct Instance(dimpl::Dtls);
impl std::fmt::Debug for Instance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DtlsInstance")
    }
}
impl DtlsInstance for Instance {
    fn set_active(&mut self, active: bool) {
        self.0.set_active(active);
    }
    fn handle_packet(&mut self, packet: &[u8]) -> Result<(), DtlsImplError> {
        self.0.handle_packet(packet)
    }
    fn poll_output<'a>(&mut self, buf: &'a mut [u8]) -> DtlsOutput<'a> {
        self.0.poll_output(buf)
    }
    fn handle_timeout(&mut self, now: Instant) -> Result<(), DtlsImplError> {
        self.0.handle_timeout(now)
    }
    fn send_application_data(&mut self, data: &[u8]) -> Result<(), DtlsImplError> {
        self.0.send_application_data(data)
    }
    fn is_active(&self) -> bool {
        self.0.is_active()
    }
    fn protocol_version(&self) -> Option<ProtocolVersion> {
        self.0.protocol_version()
    }
    fn is_closing(&self) -> bool {
        self.0.is_closing()
    }
    fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
    fn close(&mut self) -> Result<(), DtlsImplError> {
        self.0.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificates_are_fresh_p256_with_distinct_serials() {
        let a = certificate().unwrap();
        let b = certificate().unwrap();
        assert_ne!(a.certificate, b.certificate);
        assert!(a.certificate.len() > 200 && a.private_key.len() > 100);
    }

    #[test]
    fn hmac_sha1_and_sha256_match_known_vectors() {
        // RFC 2202 test case 2.
        let mac = HmacSha1.sha1_hmac(b"Jefe", &[b"what do ya want ", b"for nothing?"]);
        assert_eq!(
            mac.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
        let h = Sha256.sha256(b"abc");
        assert_eq!(h[..4], [0xba, 0x78, 0x16, 0xbf]);
    }

    #[test]
    fn counter_mode_and_gcm_round_trip() {
        let mut c = Cm.create_cipher([7; 16], true);
        let mut enc = [0u8; 5];
        c.encrypt(&[1; 16], b"hello", &mut enc).unwrap();
        let mut dec = [0u8; 5];
        c.decrypt(&[1; 16], &enc, &mut dec).unwrap();
        assert_eq!(&dec, b"hello");
        let mut g = G128.create_cipher([3; 16], true);
        let mut sealed = [0u8; 5 + 16];
        g.encrypt(&[2; 12], &[9; 12], b"hello", &mut sealed)
            .unwrap();
        let mut open = [0u8; 21];
        let n = g
            .decrypt(&[2; 12], &[&[9; 6], &[9; 6]], &sealed, &mut open)
            .unwrap();
        assert_eq!(&open[..n], b"hello");
        sealed[0] ^= 1;
        assert!(
            g.decrypt(&[2; 12], &[&[9; 12]], &sealed, &mut open)
                .is_err()
        );
    }
}
