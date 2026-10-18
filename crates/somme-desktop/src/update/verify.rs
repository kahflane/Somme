//! Checks an update before it is installed. Each release carries
//! `SHA256SUMS`, the SHA-256 of each file, and `SHA256SUMS.sig`, an Ed25519
//! signature over it made with Somme's release key. The public half of that
//! key is built into the app from `assets/update-key.pub`.
//!
//! Also the release tools: `somme-desktop --update-keygen PATH` makes a key
//! pair, and `somme-desktop --update-sign KEY FILE` signs a file. See
//! `release.sh`.

use std::io::Write as _;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use rand::Rng as _;
use zeroize::Zeroizing;

pub const SUMS: &str = "SHA256SUMS";
pub const SIGNATURE: &str = "SHA256SUMS.sig";
pub const KEYGEN_ARG: &str = "--update-keygen";
pub const SIGN_ARG: &str = "--update-sign";

/// The release key's public half. Lines starting with `#` are notes.
const KEY_FILE: &str = include_str!("../../assets/update-key.pub");

/// The key updates must be signed with. None when this build has none, and
/// then Somme installs no update itself.
pub fn public_key() -> Option<VerifyingKey> {
    parse_public_key(KEY_FILE)
}

fn parse_public_key(text: &str) -> Option<VerifyingKey> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let bytes: [u8; 32] = STANDARD.decode(line).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// Whether `signature`, as `SHA256SUMS.sig` holds it, is `key`'s over `sums`.
pub fn signed(key: &VerifyingKey, sums: &[u8], signature: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(signature) else { return false };
    let Ok(bytes) = STANDARD.decode(text.trim()) else { return false };
    let Ok(bytes) = <[u8; 64]>::try_from(bytes) else { return false };
    key.verify_strict(sums, &Signature::from_bytes(&bytes)).is_ok()
}

/// The hash `sums` lists for the file `name`. Lines read `HASH  NAME`, as
/// `shasum -a 256` writes them.
pub fn listed_hash(sums: &str, name: &str) -> Option<[u8; 32]> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        // `*` marks a file read as binary.
        let file = file.trim_start();
        let file = file.strip_prefix('*').unwrap_or(file);
        (file == name).then(|| from_hex(hash)).flatten()
    })
}

fn from_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (ix, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(ix * 2..ix * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ---- release tools ----

/// Runs `--update-keygen` or `--update-sign`. Returns the exit code.
pub fn run_tool(args: &[String]) -> i32 {
    let result = match args {
        [arg, path] if arg == KEYGEN_ARG => keygen(Path::new(path)),
        [arg, key, file] if arg == SIGN_ARG => sign_file(Path::new(key), Path::new(file)),
        _ => Err(format!("Usage: somme-desktop {KEYGEN_ARG} PRIVATE_KEY_PATH\n       somme-desktop {SIGN_ARG} PRIVATE_KEY_PATH FILE")),
    };
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

/// Makes a key pair. The private key goes to `path`, readable only by you,
/// and is never printed. Prints the public key, for `assets/update-key.pub`.
fn keygen(path: &Path) -> Result<(), String> {
    let mut seed = Zeroizing::new([0u8; 32]);
    rand::rng().fill_bytes(seed.as_mut_slice());
    let key = SigningKey::from_bytes(&seed);
    let text = Zeroizing::new(format!("{}\n", STANDARD.encode(seed.as_slice())));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .map_err(|err| format!("Can't make {}: {err}. Somme never writes over a key.", path.display()))?;
    file.write_all(text.as_bytes()).map_err(|err| format!("Can't write {}: {err}", path.display()))?;
    println!("{}", public_text(&key.verifying_key()));
    Ok(())
}

fn public_text(key: &VerifyingKey) -> String {
    STANDARD.encode(key.as_bytes())
}

fn read_private(path: &Path) -> Result<SigningKey, String> {
    let text = Zeroizing::new(std::fs::read_to_string(path).map_err(|err| format!("Can't read {}: {err}", path.display()))?);
    let bytes = Zeroizing::new(STANDARD.decode(text.trim()).map_err(|_| format!("{} is not a Somme update key", path.display()))?);
    let seed: &[u8; 32] = bytes.as_slice().try_into().map_err(|_| format!("{} is not a Somme update key", path.display()))?;
    Ok(SigningKey::from_bytes(seed))
}

/// `SHA256SUMS.sig` for `data`: the signature in base64, on one line.
fn signature_text(key: &SigningKey, data: &[u8]) -> String {
    format!("{}\n", STANDARD.encode(key.sign(data).to_bytes()))
}

/// Writes `FILE.sig`. Refuses a key the app would not accept, so a release
/// is never signed with the wrong one.
fn sign_file(key_path: &Path, file: &Path) -> Result<(), String> {
    sign_file_for(public_key(), key_path, file)
}

fn sign_file_for(public: Option<VerifyingKey>, key_path: &Path, file: &Path) -> Result<(), String> {
    let key = read_private(key_path)?;
    match public {
        None => return Err("This build has no public key. Put it in crates/somme-desktop/assets/update-key.pub and build again.".into()),
        Some(public) if public != key.verifying_key() => {
            return Err("This key doesn't match crates/somme-desktop/assets/update-key.pub. Somme would refuse the release.".into());
        }
        Some(_) => {}
    }
    let data = std::fs::read(file).map_err(|err| format!("Can't read {}: {err}", file.display()))?;
    let mut out = file.as_os_str().to_owned();
    out.push(".sig");
    std::fs::write(&out, signature_text(&key, &data)).map_err(|err| format!("Can't write the signature: {err}"))?;
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A throwaway key, for tests only.
    pub fn test_key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    pub fn sign(key: &SigningKey, data: &[u8]) -> String {
        signature_text(key, data)
    }

    #[test]
    fn reads_the_public_key_file() {
        let key = test_key().verifying_key();
        let file = format!("# Somme's update key\n\n{}\n", public_text(&key));
        assert_eq!(parse_public_key(&file), Some(key));
        assert_eq!(parse_public_key("# no key yet\n"), None);
        assert_eq!(parse_public_key("bm90IGEga2V5\n"), None);
    }

    #[test]
    fn checks_the_signature() {
        let key = test_key();
        let sums = b"abc  Somme-0.2.0-macos-aarch64.zip\n";
        let signature = sign(&key, sums);
        assert!(signed(&key.verifying_key(), sums, signature.as_bytes()));
        assert!(!signed(&key.verifying_key(), b"abd  Somme-0.2.0-macos-aarch64.zip\n", signature.as_bytes()));
        let other = SigningKey::from_bytes(&[8; 32]);
        assert!(!signed(&other.verifying_key(), sums, signature.as_bytes()));
        assert!(!signed(&key.verifying_key(), sums, b"not base64"));
        assert!(!signed(&key.verifying_key(), sums, b""));
    }

    #[test]
    fn finds_a_file_in_the_list() {
        let hash = "0123456789abcdef".repeat(4);
        let sums = format!("{hash}  Somme-0.2.0-macos-aarch64.zip\n{}  *Somme-0.2.0-macos-x86_64.zip\n", "ff".repeat(32));
        assert_eq!(listed_hash(&sums, "Somme-0.2.0-macos-aarch64.zip").map(|h| hex(&h)), Some(hash));
        assert_eq!(listed_hash(&sums, "Somme-0.2.0-macos-x86_64.zip"), Some([0xff; 32]));
        assert_eq!(listed_hash(&sums, "Somme-0.2.0-macos.zip"), None);
        assert_eq!(listed_hash("xyz  Somme.zip\n", "Somme.zip"), None);
    }

    #[test]
    fn keygen_and_sign_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.key");
        keygen(&path).unwrap();
        // It never writes over a key.
        assert!(keygen(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let key = read_private(&path).unwrap();
        let data = b"list";
        let signature = signature_text(&key, data);
        assert!(signed(&key.verifying_key(), data, signature.as_bytes()));
        // It signs only with the key the app takes.
        let file = dir.path().join("SHA256SUMS");
        std::fs::write(&file, data).unwrap();
        assert!(sign_file_for(None, &path, &file).unwrap_err().contains("no public key"));
        let other = test_key().verifying_key();
        assert!(sign_file_for(Some(other), &path, &file).unwrap_err().contains("doesn't match"));
        assert!(!dir.path().join("SHA256SUMS.sig").exists());
        sign_file_for(Some(key.verifying_key()), &path, &file).unwrap();
        let written = std::fs::read(dir.path().join("SHA256SUMS.sig")).unwrap();
        assert!(signed(&key.verifying_key(), data, &written));
        assert!(read_private(&file).is_err());
    }
}
