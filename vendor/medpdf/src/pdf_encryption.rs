//! PDF document encryption (AES-256, AES-128, and RC4-128) with permission controls.
//!
//! AES-256 (V5) generates a random file encryption key; passwords wrap it for authentication.
//! AES-128 (V4) derives the key from the password and file ID.
//! RC4-128 (V2/R3) derives the key from the password and file ID. Cryptographically weak but
//! included as a workaround for lopdf AES encryption bugs.

use std::collections::BTreeMap;
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes128CryptFilter, Aes256CryptFilter, CryptFilter};
use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
use lopdf::Document;
use rand::Rng;

use crate::Result;

/// Encryption algorithm to use when saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncryptionAlgorithm {
    /// AES-256 (PDF 2.0) — strongest, but lopdf corrupts content streams (renders blank)
    Aes256,
    /// AES-128 (PDF 1.6+) — default until lopdf AES-256 bug is fixed
    #[default]
    Aes128,
    /// RC4-128 (PDF 1.4, V2/R3) — cryptographically weak but works around lopdf AES bugs
    Rc4_128,
}

/// Parameters for encrypting a PDF document.
pub struct EncryptionParams {
    pub user_password: String,
    pub owner_password: String,
    pub algorithm: EncryptionAlgorithm,
    pub permissions: Permissions,
}

impl EncryptionParams {
    /// Create new encryption params with the given passwords.
    /// Defaults to AES-256 with all permissions granted.
    pub fn new(user_password: impl Into<String>, owner_password: impl Into<String>) -> Self {
        Self {
            user_password: user_password.into(),
            owner_password: owner_password.into(),
            algorithm: EncryptionAlgorithm::default(),
            permissions: Permissions::default(),
        }
    }

    pub fn algorithm(mut self, algorithm: EncryptionAlgorithm) -> Self {
        self.algorithm = algorithm;
        self
    }

    pub fn permissions(mut self, permissions: Permissions) -> Self {
        self.permissions = permissions;
        self
    }
}

/// Encrypt a PDF document in place.
pub fn encrypt_document(doc: &mut Document, params: &EncryptionParams) -> Result<()> {
    let state = match params.algorithm {
        EncryptionAlgorithm::Aes256 => {
            let crypt_filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
            let crypt_filters = BTreeMap::from([(b"StdCF".to_vec(), crypt_filter)]);
            let mut file_encryption_key = [0u8; 32];
            rand::rng().fill(&mut file_encryption_key);

            let version = EncryptionVersion::V5 {
                encrypt_metadata: true,
                crypt_filters,
                file_encryption_key: &file_encryption_key,
                stream_filter: b"StdCF".to_vec(),
                string_filter: b"StdCF".to_vec(),
                owner_password: &params.owner_password,
                user_password: &params.user_password,
                permissions: params.permissions,
            };
            EncryptionState::try_from(version)?
        }
        EncryptionAlgorithm::Aes128 => {
            let crypt_filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
            let crypt_filters = BTreeMap::from([(b"StdCF".to_vec(), crypt_filter)]);
            let version = EncryptionVersion::V4 {
                document: doc,
                encrypt_metadata: true,
                crypt_filters,
                stream_filter: b"StdCF".to_vec(),
                string_filter: b"StdCF".to_vec(),
                owner_password: &params.owner_password,
                user_password: &params.user_password,
                permissions: params.permissions,
            };
            EncryptionState::try_from(version)?
        }
        EncryptionAlgorithm::Rc4_128 => {
            let version = EncryptionVersion::V2 {
                document: doc,
                owner_password: &params.owner_password,
                user_password: &params.user_password,
                key_length: 128,
                permissions: params.permissions,
            };
            EncryptionState::try_from(version)?
        }
    };

    doc.encrypt(&state)?;
    Ok(())
}

/// Parse a single permission name into a `Permissions` flag.
///
/// Names are case-insensitive. Both short and long forms are accepted:
/// `print`/`printable`, `modify`/`modifiable`, etc.
pub fn parse_permission_name(name: &str) -> std::result::Result<Permissions, String> {
    match name.to_ascii_lowercase().as_str() {
        "print" | "printable" => Ok(Permissions::PRINTABLE),
        "modify" | "modifiable" => Ok(Permissions::MODIFIABLE),
        "copy" | "copyable" => Ok(Permissions::COPYABLE),
        "annotate" | "annotable" => Ok(Permissions::ANNOTABLE),
        "fill" | "fillable" => Ok(Permissions::FILLABLE),
        "accessibility" | "copyable_for_accessibility" => {
            Ok(Permissions::COPYABLE_FOR_ACCESSIBILITY)
        }
        "assemble" | "assemblable" => Ok(Permissions::ASSEMBLABLE),
        "print_hq" | "printable_in_high_quality" => Ok(Permissions::PRINTABLE_IN_HIGH_QUALITY),
        "all" => Ok(Permissions::all()),
        "none" => Ok(Permissions::empty()),
        _ => Err(format!("Unknown permission: '{name}'. Valid names: print, modify, copy, annotate, fill, accessibility, assemble, print_hq, all, none")),
    }
}

/// Parse a slice of permission names into a combined `Permissions` value.
///
/// If the slice is empty, returns all permissions (the default).
pub fn parse_permissions(names: &[impl AsRef<str>]) -> std::result::Result<Permissions, String> {
    if names.is_empty() {
        return Ok(Permissions::all());
    }
    let mut perms = Permissions::empty();
    for name in names {
        perms |= parse_permission_name(name.as_ref())?;
    }
    Ok(perms)
}
