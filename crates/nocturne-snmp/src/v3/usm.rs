//! User-based Security Model cryptography: password-to-key and key localisation (RFC 3414
//! appendix A, RFC 7860), HMAC authentication with the per-protocol truncation lengths, and
//! DES-CBC (RFC 3414 §8) / AES-CFB-128 (RFC 3826) privacy with the AES-192/256 key extensions
//! found in the field.

use std::fmt;

use cbc::cipher::block_padding::NoPadding;
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use hmac::{EagerHash, Hmac, KeyInit, Mac};
use md5::Md5;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};

use crate::SnmpError;

/// Authentication protocols (HMAC-MD5-96, HMAC-SHA-96 and the RFC 7860 SHA-2 family).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthProtocol {
    Md5,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
}

/// Privacy protocols.
///
/// AES-192 and AES-256 were never standardised for SNMP and exist in two incompatible
/// flavours that differ only in how a short localised key is stretched to the cipher's key
/// size — which matters with MD5 and SHA-1 (and SHA-224 for AES-256); with longer hashes the
/// two are identical.
///
/// * [`Aes192`](Self::Aes192) / [`Aes256`](Self::Aes256) use the draft-blumenthal-aes-usm-04
///   extension: net-snmp's `AES-192`/`AES-256`, and most other vendors.
/// * [`Aes192Cisco`](Self::Aes192Cisco) / [`Aes256Cisco`](Self::Aes256Cisco) use the
///   draft-reeder-snmpv3-usm-3desede-00 extension that Cisco IOS implements: net-snmp's
///   `AES-192-C`/`AES-256-C`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivProtocol {
    Des,
    Aes128,
    Aes192,
    Aes256,
    Aes192Cisco,
    Aes256Cisco,
}

/// One megabyte of repeated passphrase is hashed to derive a key (RFC 3414 A.2).
const PASSWORD_STREAM_LEN: usize = 1_048_576;

/// privParameters is always an 8-octet salt.
const SALT_LEN: usize = 8;

/// DES works on 8-octet blocks.
const DES_BLOCK: usize = 8;

fn digest<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
    let mut hasher = D::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().to_vec()
}

fn password_to_key<D: Digest>(password: &[u8]) -> Vec<u8> {
    // The RFC feeds the hash 64 octets at a time from the endlessly repeated password. Any
    // chunking yields the same stream, so use a chunk that holds many whole repetitions.
    let repetitions = (4096 / password.len().max(1)).max(1);
    let chunk = password.repeat(repetitions);
    let mut hasher = D::new();
    let mut remaining = PASSWORD_STREAM_LEN;
    while remaining > 0 {
        let take = remaining.min(chunk.len());
        hasher.update(&chunk[..take]);
        remaining -= take;
    }
    hasher.finalize().to_vec()
}

fn hmac<D: EagerHash>(key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut mac =
        <Hmac<D> as KeyInit>::new_from_slice(key).expect("HMAC accepts keys of any length");
    for part in parts {
        Mac::update(&mut mac, part);
    }
    mac.finalize().into_bytes().to_vec()
}

/// Runs a function that is generic over the hash with the hash of an [`AuthProtocol`].
macro_rules! with_hash {
    ($protocol:expr, $function:ident($($argument:expr),*)) => {
        match $protocol {
            AuthProtocol::Md5 => $function::<Md5>($($argument),*),
            AuthProtocol::Sha1 => $function::<Sha1>($($argument),*),
            AuthProtocol::Sha224 => $function::<Sha224>($($argument),*),
            AuthProtocol::Sha256 => $function::<Sha256>($($argument),*),
            AuthProtocol::Sha384 => $function::<Sha384>($($argument),*),
            AuthProtocol::Sha512 => $function::<Sha512>($($argument),*),
        }
    };
}

impl AuthProtocol {
    /// Octets of the HMAC kept in msgAuthenticationParameters.
    pub(crate) fn mac_len(self) -> usize {
        match self {
            AuthProtocol::Md5 | AuthProtocol::Sha1 => 12,
            AuthProtocol::Sha224 => 16,
            AuthProtocol::Sha256 => 24,
            AuthProtocol::Sha384 => 32,
            AuthProtocol::Sha512 => 48,
        }
    }

    /// Octets of the underlying hash, and therefore of a localised key.
    pub(crate) fn digest_len(self) -> usize {
        match self {
            AuthProtocol::Md5 => 16,
            AuthProtocol::Sha1 => 20,
            AuthProtocol::Sha224 => 28,
            AuthProtocol::Sha256 => 32,
            AuthProtocol::Sha384 => 48,
            AuthProtocol::Sha512 => 64,
        }
    }

    fn hash(self, parts: &[&[u8]]) -> Vec<u8> {
        with_hash!(self, digest(parts))
    }

    /// The user's master key Ku for a passphrase. This is the expensive step (1 MiB of
    /// hashing) and does not depend on the engine, so it is worth keeping.
    pub(crate) fn master_key(self, passphrase: &[u8]) -> Result<Vec<u8>, SnmpError> {
        if passphrase.is_empty() {
            return Err(SnmpError::Auth(
                "an SNMPv3 passphrase must not be empty".to_owned(),
            ));
        }
        Ok(with_hash!(self, password_to_key(passphrase)))
    }

    /// Localises a master key to an engine: Kul = H(Ku || engineID || Ku).
    pub(crate) fn localize(self, master_key: &[u8], engine_id: &[u8]) -> Vec<u8> {
        self.hash(&[master_key, engine_id, master_key])
    }

    /// The full, untruncated HMAC of `parts` concatenated.
    fn hmac(self, key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
        with_hash!(self, hmac(key, parts))
    }

    /// msgAuthenticationParameters for a message whose authentication field holds zeros.
    pub(crate) fn sign(self, key: &[u8], message: &[u8]) -> Vec<u8> {
        let mut mac = self.hmac(key, &[message]);
        mac.truncate(self.mac_len());
        mac
    }

    /// Checks the authentication parameters found at `auth_params` inside `message`.
    pub(crate) fn verify(
        self,
        key: &[u8],
        message: &[u8],
        auth_params: std::ops::Range<usize>,
    ) -> bool {
        let Some(received) = message.get(auth_params.clone()) else {
            return false;
        };
        if received.len() != self.mac_len() {
            return false;
        }
        // The digest is computed with the field zeroed, so feed zeros in its place.
        let zeros = [0u8; 48];
        let expected = self.hmac(
            key,
            &[
                &message[..auth_params.start],
                &zeros[..received.len()],
                &message[auth_params.end..],
            ],
        );
        constant_time_eq(&expected[..received.len()], received)
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// How a localised key that is too short for the cipher gets stretched.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyExtension {
    /// Kul' = Kul || H(Kul), repeated (draft-blumenthal-aes-usm-04 §3.1.2.1).
    Blumenthal,
    /// The password-to-key algorithm chained on its own output
    /// (draft-reeder-snmpv3-usm-3desede-00 §2.1), as Cisco does.
    Reeder,
}

impl PrivProtocol {
    /// Octets of localised key material the protocol consumes.
    fn key_material_len(self) -> usize {
        match self {
            // 8 octets of DES key followed by the 8-octet pre-IV.
            PrivProtocol::Des | PrivProtocol::Aes128 => 16,
            PrivProtocol::Aes192 | PrivProtocol::Aes192Cisco => 24,
            PrivProtocol::Aes256 | PrivProtocol::Aes256Cisco => 32,
        }
    }

    fn key_extension(self) -> KeyExtension {
        match self {
            PrivProtocol::Aes192Cisco | PrivProtocol::Aes256Cisco => KeyExtension::Reeder,
            _ => KeyExtension::Blumenthal,
        }
    }

    /// The same cipher with the other AES-192/256 key extension, when `auth`'s digest is
    /// short enough for the two to differ.
    pub(crate) fn alternative_key_extension(self, auth: AuthProtocol) -> Option<PrivProtocol> {
        let other = match self {
            PrivProtocol::Aes192 => PrivProtocol::Aes192Cisco,
            PrivProtocol::Aes192Cisco => PrivProtocol::Aes192,
            PrivProtocol::Aes256 => PrivProtocol::Aes256Cisco,
            PrivProtocol::Aes256Cisco => PrivProtocol::Aes256,
            PrivProtocol::Des | PrivProtocol::Aes128 => return None,
        };
        (auth.digest_len() < self.key_material_len()).then_some(other)
    }

    /// Derives the localised privacy key from the privacy passphrase's master key (computed
    /// with the *authentication* protocol's hash, as USM prescribes).
    pub(crate) fn localized_key(
        self,
        auth: AuthProtocol,
        master_key: &[u8],
        engine_id: &[u8],
    ) -> Vec<u8> {
        let needed = self.key_material_len();
        let mut key = auth.localize(master_key, engine_id);
        match self.key_extension() {
            KeyExtension::Blumenthal => {
                while key.len() < needed {
                    let more = auth.hash(&[&key]);
                    key.extend_from_slice(&more);
                }
            }
            KeyExtension::Reeder => {
                let mut previous = key.clone();
                while key.len() < needed {
                    // The previous block plays the passphrase; a digest is never empty.
                    let chained = with_hash!(auth, password_to_key(&previous));
                    previous = auth.localize(&chained, engine_id);
                    key.extend_from_slice(&previous);
                }
            }
        }
        key.truncate(needed);
        key
    }

    /// Encrypts a scoped PDU. `salt` must differ for every message sent with the same key.
    /// Returns the ciphertext and the privParameters to put in the message.
    pub(crate) fn encrypt(
        self,
        key: &[u8],
        engine_boots: u32,
        engine_time: u32,
        salt: u64,
        plaintext: &[u8],
    ) -> Result<(Vec<u8>, [u8; SALT_LEN]), SnmpError> {
        let mut data = plaintext.to_vec();
        let priv_params = match self {
            PrivProtocol::Des => {
                // RFC 3414 §8.1.1.1: the salt is engineBoots followed by a local counter.
                let mut priv_params = [0u8; SALT_LEN];
                priv_params[..4].copy_from_slice(&engine_boots.to_be_bytes());
                priv_params[4..].copy_from_slice(&(salt as u32).to_be_bytes());
                let (des_key, iv) = des_key_and_iv(key, &priv_params)?;
                // "The data to be encrypted is padded to a multiple of 8 octets."
                data.resize(data.len().next_multiple_of(DES_BLOCK), 0);
                let length = data.len();
                cbc::Encryptor::<des::Des>::new_from_slices(des_key, &iv)
                    .map_err(|_| bad_key())?
                    .encrypt_padded::<NoPadding>(&mut data, length)
                    .map_err(|_| bad_key())?;
                priv_params
            }
            _ => {
                let priv_params = salt.to_be_bytes();
                let iv = aes_iv(engine_boots, engine_time, &priv_params);
                self.aes_cfb(key, &iv, &mut data, Direction::Encrypt)?;
                priv_params
            }
        };
        Ok((data, priv_params))
    }

    /// Decrypts an encrypted scoped PDU. For DES the result may carry up to 7 octets of
    /// padding after the PDU.
    pub(crate) fn decrypt(
        self,
        key: &[u8],
        engine_boots: u32,
        engine_time: u32,
        priv_params: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, SnmpError> {
        let priv_params: &[u8; SALT_LEN] = priv_params
            .try_into()
            .map_err(|_| SnmpError::decode("privacy parameters must be 8 octets"))?;
        let mut data = ciphertext.to_vec();
        match self {
            PrivProtocol::Des => {
                if !data.len().is_multiple_of(DES_BLOCK) {
                    return Err(SnmpError::decode(
                        "DES ciphertext is not a multiple of 8 octets",
                    ));
                }
                let (des_key, iv) = des_key_and_iv(key, priv_params)?;
                cbc::Decryptor::<des::Des>::new_from_slices(des_key, &iv)
                    .map_err(|_| bad_key())?
                    .decrypt_padded::<NoPadding>(&mut data)
                    .map_err(|_| bad_key())?;
            }
            _ => {
                let iv = aes_iv(engine_boots, engine_time, priv_params);
                self.aes_cfb(key, &iv, &mut data, Direction::Decrypt)?;
            }
        }
        Ok(data)
    }

    fn aes_cfb(
        self,
        key: &[u8],
        iv: &[u8; 16],
        data: &mut [u8],
        direction: Direction,
    ) -> Result<(), SnmpError> {
        let key = key.get(..self.key_material_len()).ok_or_else(bad_key)?;
        macro_rules! run {
            ($cipher:ty) => {
                match direction {
                    Direction::Encrypt => cfb_mode::Encryptor::<$cipher>::new_from_slices(key, iv)
                        .map_err(|_| bad_key())?
                        .encrypt(data),
                    Direction::Decrypt => cfb_mode::Decryptor::<$cipher>::new_from_slices(key, iv)
                        .map_err(|_| bad_key())?
                        .decrypt(data),
                }
            };
        }
        match self {
            PrivProtocol::Aes128 => run!(aes::Aes128),
            PrivProtocol::Aes192 | PrivProtocol::Aes192Cisco => run!(aes::Aes192),
            PrivProtocol::Aes256 | PrivProtocol::Aes256Cisco => run!(aes::Aes256),
            PrivProtocol::Des => return Err(bad_key()),
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Encrypt,
    Decrypt,
}

fn bad_key() -> SnmpError {
    SnmpError::Auth("internal error: privacy key has the wrong size".to_owned())
}

/// Splits the 16 octets of DES key material into the key and the IV (pre-IV XOR salt).
fn des_key_and_iv<'k>(
    key: &'k [u8],
    salt: &[u8; SALT_LEN],
) -> Result<(&'k [u8], [u8; DES_BLOCK]), SnmpError> {
    let (des_key, pre_iv) = key.get(..16).ok_or_else(bad_key)?.split_at(DES_BLOCK);
    let mut iv = [0u8; DES_BLOCK];
    for (out, (a, b)) in iv.iter_mut().zip(pre_iv.iter().zip(salt)) {
        *out = a ^ b;
    }
    Ok((des_key, iv))
}

/// RFC 3826 §3.1.2.1: IV = engineBoots || engineTime || salt.
fn aes_iv(engine_boots: u32, engine_time: u32, salt: &[u8; SALT_LEN]) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[..4].copy_from_slice(&engine_boots.to_be_bytes());
    iv[4..8].copy_from_slice(&engine_time.to_be_bytes());
    iv[8..].copy_from_slice(salt);
    iv
}

/// The keys of one user, localised to one authoritative engine.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LocalizedKeys {
    pub auth_protocol: AuthProtocol,
    pub auth_key: Vec<u8>,
    pub privacy: Option<(PrivProtocol, Vec<u8>)>,
}

/// Key material never appears in logs.
impl fmt::Debug for LocalizedKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalizedKeys")
            .field("auth_protocol", &self.auth_protocol)
            .field(
                "priv_protocol",
                &self.privacy.as_ref().map(|(protocol, _)| protocol),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn hex(s: &str) -> Vec<u8> {
        let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    /// RFC 3414 appendix A.3: password "maplesyrup", engine ID 00 00 00 00 00 00 00 00 00 00 00 02.
    const RFC_ENGINE_ID: [u8; 12] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];

    #[test]
    fn rfc3414_a3_1_md5_password_to_key() {
        let ku = AuthProtocol::Md5.master_key(b"maplesyrup").unwrap();
        assert_eq!(ku, hex("9f af 32 83 88 4e 92 83 4e bc 98 47 d8 ed d9 63"));
        let kul = AuthProtocol::Md5.localize(&ku, &RFC_ENGINE_ID);
        assert_eq!(kul, hex("52 6f 5e ed 9f cc e2 6f 89 64 c2 93 07 87 d8 2b"));
    }

    #[test]
    fn rfc3414_a3_2_sha1_password_to_key() {
        let ku = AuthProtocol::Sha1.master_key(b"maplesyrup").unwrap();
        assert_eq!(
            ku,
            hex("9f b5 cc 03 81 49 7b 37 93 52 89 39 ff 78 8d 5d 79 14 52 11")
        );
        let kul = AuthProtocol::Sha1.localize(&ku, &RFC_ENGINE_ID);
        assert_eq!(
            kul,
            hex("66 95 fe bc 92 88 e3 62 82 23 5f c7 15 1f 12 84 97 b3 8f 3f")
        );
    }

    /// Localised keys as net-snmp 5.9.4 derived and stored them (`usmUser` lines of its
    /// persistent file) for `createUser` entries with these passphrases.
    const NETSNMP_ENGINE_ID: &str = "80001f8880f6679d6d1f60f66a00000000";

    fn netsnmp_auth_key(protocol: AuthProtocol, passphrase: &str) -> Vec<u8> {
        let ku = protocol.master_key(passphrase.as_bytes()).unwrap();
        protocol.localize(&ku, &hex(NETSNMP_ENGINE_ID))
    }

    #[test]
    fn auth_keys_match_netsnmp_for_every_hash() {
        let vectors = [
            (
                AuthProtocol::Md5,
                "authpass-md5-1234",
                "87aff15466e5d3afd74bec3e9c9d1b8e",
            ),
            (
                AuthProtocol::Sha1,
                "authpass-sha1-1234",
                "08609e374ef4b7b2c4fc73ee4f6cb49084e04ec5",
            ),
            (
                AuthProtocol::Sha224,
                "authpass-sha224-1234",
                "fdcb62fe14d2e2489c5d53b6ebef5f71bf29a93f5e90264e5ac8bb85",
            ),
            (
                AuthProtocol::Sha256,
                "authpass-sha256-1234",
                "2a44efcd8f20de39ba503153b4f169a39775280e31c9aab9220e80c4cab9393f",
            ),
            (
                AuthProtocol::Sha384,
                "authpass-sha384-1234",
                "822cb25e934bb6e1b7e794a5e8fcaa70e297e08f9bd76f20f04d867d9d9713b7\
                 e8e47e8368e47755234f396b0afd4e9c",
            ),
            (
                AuthProtocol::Sha512,
                "authpass-sha512-1234",
                "b4028c74039ace2489a90520865b189bb6fbe5444f113bebfbb0bc3ed070ea70\
                 3d4cecd1f27e4a20d1f14523713fd1ae506fd1a6f3448a79289758b9cbb695d6",
            ),
        ];
        for (protocol, passphrase, expected) in vectors {
            assert_eq!(
                netsnmp_auth_key(protocol, passphrase),
                hex(expected),
                "{protocol:?}"
            );
        }
    }

    fn netsnmp_priv_key(auth: AuthProtocol, protocol: PrivProtocol, passphrase: &str) -> Vec<u8> {
        let ku = auth.master_key(passphrase.as_bytes()).unwrap();
        protocol.localized_key(auth, &ku, &hex(NETSNMP_ENGINE_ID))
    }

    #[test]
    fn priv_keys_match_netsnmp_including_the_blumenthal_extension() {
        use AuthProtocol::{Md5, Sha1, Sha256, Sha512};
        use PrivProtocol::{Aes128, Aes192, Aes256, Des};
        let vectors = [
            (
                Md5,
                Des,
                "privpass-des-1234",
                "0e397c56643380405cc0b4040a2bc01c",
            ),
            (
                Md5,
                Aes128,
                "privpass-aes-1234",
                "b927968ee67ef261193e0d81d1127b80",
            ),
            // 16-octet MD5 keys stretched to 24 and 32 octets.
            (
                Md5,
                Aes192,
                "privpass-aes192-1234",
                "48c2c9e9fef0211a6488bd1f9fb14fd8956ced73152f112c",
            ),
            (
                Md5,
                Aes256,
                "privpass-aes256-1234",
                "e7bc12aff30c50347e3930e7df7312de693fa3278cd5adab5971b83540ada226",
            ),
            (
                Sha1,
                Des,
                "privpass-des-1234",
                "46f925c6b111653513b6f554d05efc43",
            ),
            // A 20-octet SHA-1 key truncated to 16...
            (
                Sha1,
                Aes128,
                "privpass-aes-1234",
                "c3191b655685736c83fa80f91b299738",
            ),
            // ...and stretched to 24 and 32.
            (
                Sha1,
                Aes192,
                "privpass-aes192-1234",
                "1447d78ce6d7291220de9f420a2416c6fd6c2ec2ac2a4b7f",
            ),
            (
                Sha1,
                Aes256,
                "privpass-aes256-1234",
                "d00940c4519032246c1a952226809ba0ff7cf83740fd0070c353a1a21a318187",
            ),
            // Long digests need no stretching, only truncation.
            (
                Sha256,
                Aes256,
                "privpass-aes256-1234",
                "370f30e12af3d729dcde6c0b3ce75907aecf02230681eabb0fd8c15597ff2358",
            ),
            (
                Sha512,
                Aes256,
                "privpass-aes256-1234",
                "4af2db56fd47f424feee981a9ee15d8db45f2c4dff0f194c3b514419cf1f529a",
            ),
            (
                Sha512,
                Des,
                "privpass-des-1234",
                "c49eae099719aacb14e6316061d443cf",
            ),
        ];
        for (auth, protocol, passphrase, expected) in vectors {
            assert_eq!(
                netsnmp_priv_key(auth, protocol, passphrase),
                hex(expected),
                "{auth:?}/{protocol:?}"
            );
        }
    }

    #[test]
    fn key_extensions_differ_only_when_the_digest_is_short() {
        use PrivProtocol::{Aes192, Aes192Cisco, Aes256, Aes256Cisco};
        let engine = hex(NETSNMP_ENGINE_ID);
        let key = |auth: AuthProtocol, protocol: PrivProtocol| {
            let ku = auth.master_key(b"privpass-aes256-1234").unwrap();
            protocol.localized_key(auth, &ku, &engine)
        };
        for auth in [AuthProtocol::Md5, AuthProtocol::Sha1] {
            let (blumenthal, cisco) = (key(auth, Aes256), key(auth, Aes256Cisco));
            assert_eq!((blumenthal.len(), cisco.len()), (32, 32));
            // Both start with the plain localised key and continue differently.
            let plain = auth.digest_len();
            assert_eq!(blumenthal[..plain], cisco[..plain]);
            assert_ne!(blumenthal, cisco);
            assert_ne!(key(auth, Aes192), key(auth, Aes192Cisco));
            assert_eq!(Aes256.alternative_key_extension(auth), Some(Aes256Cisco));
            assert_eq!(Aes192Cisco.alternative_key_extension(auth), Some(Aes192));
        }
        // SHA-224 yields 28 octets: enough for AES-192, short for AES-256.
        assert_eq!(
            key(AuthProtocol::Sha224, Aes192),
            key(AuthProtocol::Sha224, Aes192Cisco)
        );
        assert_ne!(
            key(AuthProtocol::Sha224, Aes256),
            key(AuthProtocol::Sha224, Aes256Cisco)
        );
        assert_eq!(Aes192.alternative_key_extension(AuthProtocol::Sha224), None);
        assert_eq!(
            Aes256Cisco.alternative_key_extension(AuthProtocol::Sha224),
            Some(Aes256)
        );
        for auth in [
            AuthProtocol::Sha256,
            AuthProtocol::Sha384,
            AuthProtocol::Sha512,
        ] {
            assert_eq!(key(auth, Aes256), key(auth, Aes256Cisco), "{auth:?}");
            assert_eq!(key(auth, Aes192), key(auth, Aes192Cisco), "{auth:?}");
            assert_eq!(Aes256.alternative_key_extension(auth), None);
        }
        assert_eq!(
            PrivProtocol::Des.alternative_key_extension(AuthProtocol::Md5),
            None
        );
        assert_eq!(
            PrivProtocol::Aes128.alternative_key_extension(AuthProtocol::Md5),
            None
        );
    }

    #[test]
    fn password_stream_is_independent_of_chunking() {
        // A reference implementation that follows the RFC's 64-octet loop literally.
        fn reference(password: &[u8]) -> Vec<u8> {
            let mut hasher = Sha1::new();
            let mut index = 0usize;
            for _ in 0..PASSWORD_STREAM_LEN / 64 {
                let mut block = [0u8; 64];
                for byte in &mut block {
                    *byte = password[index % password.len()];
                    index += 1;
                }
                hasher.update(block);
            }
            hasher.finalize().to_vec()
        }
        let long = vec![b'x'; 5000];
        for password in [&b"a"[..], b"maplesyrup", b"0123456789abcdefg", &long] {
            assert_eq!(
                AuthProtocol::Sha1.master_key(password).unwrap(),
                reference(password)
            );
        }
        assert!(matches!(
            AuthProtocol::Sha1.master_key(b""),
            Err(SnmpError::Auth(_))
        ));
    }

    #[test]
    fn mac_lengths_follow_the_rfcs() {
        let expected = [
            (AuthProtocol::Md5, 16, 12),
            (AuthProtocol::Sha1, 20, 12),
            (AuthProtocol::Sha224, 28, 16),
            (AuthProtocol::Sha256, 32, 24),
            (AuthProtocol::Sha384, 48, 32),
            (AuthProtocol::Sha512, 64, 48),
        ];
        for (protocol, digest_len, mac_len) in expected {
            assert_eq!(protocol.hash(&[b"x"]).len(), digest_len, "{protocol:?}");
            assert_eq!(protocol.digest_len(), digest_len, "{protocol:?}");
            assert_eq!(protocol.mac_len(), mac_len, "{protocol:?}");
            assert_eq!(protocol.sign(b"key", b"message").len(), mac_len);
        }
    }

    #[test]
    fn hmac_matches_rfc_2202_and_rfc_4231() {
        // RFC 2202 test case 2 (MD5 and SHA-1), RFC 4231 test case 2 (SHA-2).
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let vectors = [
            (AuthProtocol::Md5, "750c783e6ab0b503eaa86e310a5db738"),
            (
                AuthProtocol::Sha1,
                "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79",
            ),
            (
                AuthProtocol::Sha224,
                "a30e01098bc6dbbf45690f3a7e9e6d0f8bbea2a39e6148008fd05e44",
            ),
            (
                AuthProtocol::Sha256,
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            ),
            (
                AuthProtocol::Sha384,
                "af45d2e376484031617f78d2b58a6b1b9c7ef464f5a01b47e42ec3736322445e\
                 8e2240ca5e69e2c78b3239ecfab21649",
            ),
            (
                AuthProtocol::Sha512,
                "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea250554\
                 9758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737",
            ),
        ];
        for (protocol, expected) in vectors {
            let full = hex(expected);
            assert_eq!(protocol.hmac(key, &[data]), full, "{protocol:?}");
            // Split input gives the same MAC, and sign() is its prefix.
            assert_eq!(protocol.hmac(key, &[&data[..5], &data[5..]]), full);
            assert_eq!(protocol.sign(key, data), full[..protocol.mac_len()]);
        }
    }

    #[test]
    fn verify_accepts_only_the_right_digest_in_the_right_place() {
        for protocol in [
            AuthProtocol::Md5,
            AuthProtocol::Sha1,
            AuthProtocol::Sha256,
            AuthProtocol::Sha512,
        ] {
            let key = b"0123456789abcdef";
            let len = protocol.mac_len();
            let mut message = Vec::from(&b"header-"[..]);
            let start = message.len();
            message.extend(std::iter::repeat_n(0u8, len));
            message.extend_from_slice(b"-trailer");
            let mac = protocol.sign(key, &message);
            message[start..start + len].copy_from_slice(&mac);
            let range = start..start + len;

            assert!(protocol.verify(key, &message, range.clone()));
            assert!(!protocol.verify(b"another key", &message, range.clone()));
            // Any bit flipped anywhere invalidates it.
            for index in 0..message.len() {
                let mut tampered = message.clone();
                tampered[index] ^= 0x01;
                assert!(
                    !protocol.verify(key, &tampered, range.clone()),
                    "byte {index}"
                );
            }
            // A field of the wrong size or outside the message is never valid.
            assert!(!protocol.verify(key, &message, start..start + len - 1));
            assert!(!protocol.verify(key, &message, start..message.len() + 1));
            assert!(!protocol.verify(key, &message, 0..0));
        }
    }

    const ALL_PRIV: [PrivProtocol; 6] = [
        PrivProtocol::Des,
        PrivProtocol::Aes128,
        PrivProtocol::Aes192,
        PrivProtocol::Aes256,
        PrivProtocol::Aes192Cisco,
        PrivProtocol::Aes256Cisco,
    ];

    #[test]
    fn privacy_round_trips_for_every_protocol_and_length() {
        let key: Vec<u8> = (1..=32).collect();
        for protocol in ALL_PRIV {
            for len in [0usize, 1, 7, 8, 9, 15, 16, 17, 100, 1000] {
                let plaintext: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
                let (ciphertext, params) = protocol
                    .encrypt(&key, 5, 1234, 0x0102_0304_0506_0708, &plaintext)
                    .unwrap();
                if len > 0 {
                    assert_ne!(ciphertext[..len.min(ciphertext.len())], plaintext[..]);
                }
                let decrypted = protocol
                    .decrypt(&key, 5, 1234, &params, &ciphertext)
                    .unwrap();
                if protocol == PrivProtocol::Des {
                    // Padded up to the block size, padding after the data.
                    assert_eq!(ciphertext.len(), len.next_multiple_of(8), "{protocol:?}");
                    assert_eq!(decrypted[..len], plaintext[..]);
                } else {
                    assert_eq!(ciphertext.len(), len, "{protocol:?}");
                    assert_eq!(decrypted, plaintext, "{protocol:?} len {len}");
                }
                // A different key, time or salt does not decrypt (AES IV includes the time).
                if len >= 16 {
                    // (Not the lowest bit: DES ignores it as parity.)
                    let mut other_key = key.clone();
                    other_key[0] ^= 0x80;
                    let garbage = protocol
                        .decrypt(&other_key, 5, 1234, &params, &ciphertext)
                        .unwrap();
                    assert_ne!(garbage[..len], plaintext[..]);
                    let mut other_salt = params;
                    other_salt[7] ^= 1;
                    let garbage = protocol
                        .decrypt(&key, 5, 1234, &other_salt, &ciphertext)
                        .unwrap();
                    assert_ne!(garbage[..len], plaintext[..]);
                    if protocol != PrivProtocol::Des {
                        let garbage = protocol
                            .decrypt(&key, 5, 1235, &params, &ciphertext)
                            .unwrap();
                        assert_ne!(garbage[..len], plaintext[..]);
                    }
                }
            }
        }
    }

    #[test]
    fn des_salt_and_iv_follow_rfc_3414() {
        let key: Vec<u8> = (0x10..0x20).collect();
        let (ciphertext, params) = PrivProtocol::Des
            .encrypt(&key, 0x0000_0007, 99, 0xdead_beef_0102_0304, b"12345678")
            .unwrap();
        // privParameters = engineBoots || low 32 bits of the local salt counter.
        assert_eq!(params, [0, 0, 0, 7, 0x01, 0x02, 0x03, 0x04]);

        // Independent check with the block cipher alone: C1 = E(key[..8], P1 xor IV),
        // IV = key[8..16] xor salt.
        use des::cipher::{BlockCipherEncrypt, KeyInit as _};
        let mut block = *b"12345678";
        for (i, byte) in block.iter_mut().enumerate() {
            *byte ^= key[8 + i] ^ params[i];
        }
        let cipher = des::Des::new_from_slice(&key[..8]).unwrap();
        let mut block = block.into();
        cipher.encrypt_block(&mut block);
        assert_eq!(ciphertext, block.as_slice());
    }

    #[test]
    fn aes_iv_follows_rfc_3826() {
        let key: Vec<u8> = (0x20..0x40).collect();
        let (ciphertext, params) = PrivProtocol::Aes128
            .encrypt(
                &key,
                0x0102_0304,
                0x0a0b_0c0d,
                0x1122_3344_5566_7788,
                b"sixteen byte msg",
            )
            .unwrap();
        assert_eq!(params, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);

        // CFB-128, first block: C1 = P1 xor E(key, IV), IV = boots || time || salt.
        use aes::cipher::{BlockCipherEncrypt, KeyInit as _};
        let iv = [
            0x01, 0x02, 0x03, 0x04, 0x0a, 0x0b, 0x0c, 0x0d, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88,
        ];
        let cipher = aes::Aes128::new_from_slice(&key[..16]).unwrap();
        let mut keystream = iv.into();
        cipher.encrypt_block(&mut keystream);
        let expected: Vec<u8> = b"sixteen byte msg"
            .iter()
            .zip(keystream.iter())
            .map(|(p, k)| p ^ k)
            .collect();
        assert_eq!(ciphertext, expected);
    }

    #[test]
    fn decrypt_rejects_malformed_input_without_panicking() {
        let key: Vec<u8> = (1..=32).collect();
        for protocol in ALL_PRIV {
            // Salt of the wrong size.
            for bad_salt in [&[][..], &[0; 7], &[0; 9]] {
                assert!(matches!(
                    protocol.decrypt(&key, 1, 1, bad_salt, &[0; 16]),
                    Err(SnmpError::Decode(_))
                ));
            }
            // Key material that is too short is an error, not a panic.
            assert!(
                protocol
                    .decrypt(&key[..4], 1, 1, &[0; 8], &[0; 16])
                    .is_err()
            );
            assert!(protocol.encrypt(&key[..4], 1, 1, 0, &[0; 16]).is_err());
        }
        // DES needs whole blocks.
        for bad_len in [1usize, 7, 9, 15] {
            assert!(matches!(
                PrivProtocol::Des.decrypt(&key, 1, 1, &[0; 8], &vec![0; bad_len]),
                Err(SnmpError::Decode(_))
            ));
        }
        // AES-CFB is a stream mode: any length, including none, is fine.
        assert_eq!(
            PrivProtocol::Aes128
                .decrypt(&key, 1, 1, &[0; 8], &[])
                .unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn serde_names_are_snake_case() {
        let auth: Vec<String> = [
            AuthProtocol::Md5,
            AuthProtocol::Sha1,
            AuthProtocol::Sha224,
            AuthProtocol::Sha256,
            AuthProtocol::Sha384,
            AuthProtocol::Sha512,
        ]
        .iter()
        .map(|p| serde_json::to_string(p).unwrap())
        .collect();
        assert_eq!(
            auth,
            [
                "\"md5\"",
                "\"sha1\"",
                "\"sha224\"",
                "\"sha256\"",
                "\"sha384\"",
                "\"sha512\""
            ]
        );
        let privacy: Vec<String> = ALL_PRIV
            .iter()
            .map(|p| serde_json::to_string(p).unwrap())
            .collect();
        assert_eq!(
            privacy,
            [
                "\"des\"",
                "\"aes128\"",
                "\"aes192\"",
                "\"aes256\"",
                "\"aes192_cisco\"",
                "\"aes256_cisco\""
            ]
        );
        assert_eq!(
            serde_json::from_str::<PrivProtocol>("\"aes256\"").unwrap(),
            PrivProtocol::Aes256
        );
    }

    #[test]
    fn keys_are_not_printed() {
        let keys = LocalizedKeys {
            auth_protocol: AuthProtocol::Sha1,
            auth_key: vec![0xaa; 20],
            privacy: Some((PrivProtocol::Aes128, vec![0xbb; 16])),
        };
        assert_eq!(
            format!("{keys:?}"),
            "LocalizedKeys { auth_protocol: Sha1, priv_protocol: Some(Aes128), .. }"
        );
    }
}
