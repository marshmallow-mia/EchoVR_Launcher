//! Ed25519 signatures of what the launcher gets from release.echovr.de: the plugins
//! catalogue (`plugins.v2.json.sig`) and a plugin's own files (`plugin.sig`). A `.sig` file
//! holds signatures by the key's id (`scripts/sign.py` makes them); the launcher knows the
//! public keys. The format is the CDN index's (`index.json.sig`), so one check serves all.
use std::collections::BTreeMap;

use serde::Deserialize;

/// The Ed25519 public keys first-party plugins and the plugins catalogue are signed with
/// (the plugins key, not the mods index's). A new key is added here a release before
/// anything is signed with it.
pub const PLUGIN_KEYS: [[u8; 32]; 1] = [[
    0x6a, 0x67, 0x4e, 0xa2, 0xd8, 0x05, 0x1c, 0xff, 0x0a, 0xb6, 0xf6, 0x30, 0x9e, 0xbd, 0x2b, 0x63,
    0xd2, 0x7b, 0x84, 0xf9, 0x7f, 0x76, 0x0b, 0xce, 0xd8, 0x6d, 0xc8, 0x02, 0xd1, 0x5f, 0xb2, 0x68,
]];

/// What a plugin's signature says, on its first line.
const PLUGIN_STATEMENT: &str = "echovr-plugin-v1";

/// A `.sig` file: the signatures of a text now and of the one before it (a cache may have
/// either).
#[derive(Debug, Deserialize)]
struct Signatures {
    #[serde(default)]
    signatures: Vec<Signature>,
}

#[derive(Debug, Deserialize)]
struct Signature {
    /// The first 16 hex digits of the public key's SHA-256.
    key: String,
    /// Of the text it signs.
    sha256: String,
    /// Base64, 64 bytes.
    sig: String,
}

/// Pure: whether `sigs` (a `.sig` file) holds a valid signature of exactly `text` by one
/// of `keys`.
pub fn signed(text: &str, sigs: &str, keys: &[[u8; 32]]) -> bool {
    use base64::Engine;
    use ed25519_dalek::{Signature as Sig, Verifier, VerifyingKey};
    let Ok(sigs) = serde_json::from_str::<Signatures>(sigs) else {
        return false;
    };
    let sha = sha256_hex(text.as_bytes());
    sigs.signatures
        .iter()
        .filter(|s| s.sha256.eq_ignore_ascii_case(&sha))
        .any(|s| {
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&s.sig) else {
                return false;
            };
            let Ok(sig) = Sig::from_slice(&bytes) else {
                return false;
            };
            keys.iter().any(|k| {
                sha256_hex(k)[..16].eq_ignore_ascii_case(&s.key)
                    && VerifyingKey::from_bytes(k)
                        .is_ok_and(|vk| vk.verify(text.as_bytes(), &sig).is_ok())
            })
        })
}

/// What a plugin's `plugin.sig` vouches for: the plugin, its version and its files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginStatement {
    pub id: String,
    pub version: String,
    /// Path in the plugin's folder (`/` between folders) → lowercase SHA-256.
    pub files: BTreeMap<String, String>,
}

impl PluginStatement {
    /// Pure: the text that is signed. Its lines: [`PLUGIN_STATEMENT`], `id <id>`,
    /// `version <version>`, then `<sha256>  <path>` per file by path (as `sha256sum`
    /// writes them).
    pub fn text(&self) -> String {
        let mut out = format!(
            "{PLUGIN_STATEMENT}\nid {}\nversion {}\n",
            self.id, self.version
        );
        for (path, sha) in &self.files {
            out.push_str(&format!("{sha}  {path}\n"));
        }
        out
    }

    /// Pure: the statement in `text`; none unless it is exactly as [`Self::text`] writes it.
    pub fn parse(text: &str) -> Option<PluginStatement> {
        let mut lines = text.strip_suffix('\n')?.split('\n');
        if lines.next()? != PLUGIN_STATEMENT {
            return None;
        }
        let id = lines.next()?.strip_prefix("id ")?.to_string();
        let version = lines.next()?.strip_prefix("version ")?.to_string();
        let mut files = BTreeMap::new();
        for l in lines {
            let (sha, path) = l.split_once("  ")?;
            if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) || path.is_empty() {
                return None;
            }
            files.insert(path.to_string(), sha.to_ascii_lowercase());
        }
        let s = PluginStatement { id, version, files };
        (s.text() == text).then_some(s)
    }
}

/// Pure: the statement a plugin's `plugin.sig` (`text`) holds, if one of `keys` signed it.
pub fn plugin_statement(text: &str, keys: &[[u8; 32]]) -> Option<PluginStatement> {
    #[derive(Deserialize)]
    struct File {
        statement: String,
    }
    let f: File = serde_json::from_str(text).ok()?;
    if !signed(&f.statement, text, keys) {
        return None;
    }
    PluginStatement::parse(&f.statement)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A key for tests (`openssl` takes it as the PKCS#8 DER
    /// `302e020100300506032b657004220420` + 32 × `07`).
    pub(crate) const TEST_SEED: [u8; 32] = [7; 32];

    pub(crate) fn test_key() -> [u8; 32] {
        ed25519_dalek::SigningKey::from_bytes(&TEST_SEED)
            .verifying_key()
            .to_bytes()
    }

    /// A `.sig` file signing `text` with the test key, as `scripts/sign.py` writes one.
    pub(crate) fn sig_file(text: &str, statement: bool) -> String {
        use base64::Engine;
        use ed25519_dalek::Signer;
        let sig = ed25519_dalek::SigningKey::from_bytes(&TEST_SEED).sign(text.as_bytes());
        let mut f = serde_json::json!({
            "schema": 1,
            "signatures": [{
                "key": &sha256_hex(&test_key())[..16],
                "sha256": sha256_hex(text.as_bytes()),
                "sig": base64::engine::general_purpose::STANDARD.encode(sig.to_bytes()),
            }],
        });
        if statement {
            f["statement"] = serde_json::Value::String(text.to_string());
        }
        serde_json::to_string(&f).unwrap()
    }

    #[test]
    fn a_text_counts_only_with_its_signature() {
        let text = "{\"schema\": 1}\n";
        let good = sig_file(text, false);
        assert!(signed(text, &good, &[test_key()]));
        // Another text, another key, a broken file: no.
        assert!(!signed("{\"schema\": 2}\n", &good, &[test_key()]));
        assert!(!signed(text, &good, &PLUGIN_KEYS));
        assert!(!signed(text, "not json", &[test_key()]));
        let tampered = good.replacen("\"sig\":\"", "\"sig\":\"A", 1);
        assert!(!signed(text, &tampered, &[test_key()]));
    }

    #[test]
    fn a_plugin_statement_round_trips_and_is_signed() {
        let s = PluginStatement {
            id: "controllers".into(),
            version: "0.1.3".into(),
            files: [
                ("plugin.json".to_string(), "a".repeat(64)),
                ("icons/hand.png".to_string(), "b".repeat(64)),
            ]
            .into(),
        };
        let text = s.text();
        assert_eq!(
            text,
            format!(
                "echovr-plugin-v1\nid controllers\nversion 0.1.3\n{}  icons/hand.png\n{}  plugin.json\n",
                "b".repeat(64),
                "a".repeat(64)
            )
        );
        assert_eq!(PluginStatement::parse(&text), Some(s.clone()));
        // Written any other way: not a statement.
        assert_eq!(PluginStatement::parse(&text.replace("  ", " ")), None);
        assert_eq!(PluginStatement::parse(text.trim_end()), None);
        assert_eq!(PluginStatement::parse(&text.replace("id ", "name ")), None);
        let file = sig_file(&text, true);
        assert_eq!(plugin_statement(&file, &[test_key()]), Some(s));
        assert_eq!(plugin_statement(&file, &PLUGIN_KEYS), None);
        // The statement changed after signing.
        let changed = file.replace("0.1.3", "0.1.4");
        assert_eq!(plugin_statement(&changed, &[test_key()]), None);
    }

    /// `scripts/sign.py plugin` (OpenSSL) signs as the launcher checks: its output for the
    /// test key over a statement (the key file made from [`TEST_SEED`]).
    #[test]
    fn the_signing_script_signs_as_the_launcher_checks() {
        let file = include_str!("testdata/plugin.sig");
        let s = plugin_statement(file, &[test_key()]).expect("signed by the test key");
        assert_eq!(s.id, "example");
        assert_eq!(s.files.len(), 1);
    }
}
