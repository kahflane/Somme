//! Installs a checked update on macOS. The new app is downloaded, checked
//! and unpacked inside the data folder. Then a small script waits for
//! Somme to quit, puts the new app where the old one was and opens it. On
//! Linux and Windows Somme only points at the release page.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context as _;
use ed25519_dalek::VerifyingKey;
use reqwest::Url;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

use super::release::{Asset, Release, Version};
use super::verify::{self, SIGNATURE, SUMS};

/// Bigger downloads are stopped. Somme's own is far smaller.
pub const MAX_ZIP: u64 = 300 * 1024 * 1024;
const MAX_SUMS: u64 = 64 * 1024;
const MAX_SIGNATURE: u64 = 4 * 1024;
const REDIRECTS: usize = 5;
const AGENT: &str = concat!("somme/", env!("CARGO_PKG_VERSION"));
/// Downloads and the script live here, inside the data folder.
pub const FOLDER: &str = "updates";
/// Starts an app on macOS.
const OPEN: &str = "/usr/bin/open";

pub const NO_DOWNLOAD: &str = "Couldn't download the update. Check your internet connection and try again.";

/// What installing needs, worked out when the update is found.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The running app, as `…/Somme.app`.
    pub bundle: PathBuf,
    pub zip: Asset,
    pub sums: Asset,
    pub signature: Asset,
    pub key: VerifyingKey,
}

/// How this copy of Somme can get the update.
#[derive(Clone, Debug)]
pub enum Way {
    Install(Box<Plan>),
    /// Only from the release page. Says why, in plain words.
    Page(&'static str),
}

/// Works out the way for this computer. Looks at the disk, so call it off
/// the UI thread.
pub fn way(release: &Release) -> Way {
    let exe = std::env::current_exe().ok().map(|exe| std::fs::canonicalize(&exe).unwrap_or(exe));
    way_for(release, cfg!(target_os = "macos"), verify::public_key(), exe.as_deref())
}

fn way_for(release: &Release, mac: bool, key: Option<VerifyingKey>, exe: Option<&Path>) -> Way {
    if !mac {
        return Way::Page("Download it from the release page.");
    }
    let Some(key) = key else {
        return Way::Page("This copy of Somme can't check downloads, so it won't install them itself.");
    };
    let Some(bundle) = exe.and_then(bundle_of) else {
        return Way::Page("Somme isn't running from its app, so it can't replace itself.");
    };
    if !writable(&bundle) {
        return Way::Page("Somme can't change the folder its app is in.");
    }
    let (Some(zip), Some(sums), Some(signature)) =
        (release.asset(&release.mac_zip()), release.asset(SUMS), release.asset(SIGNATURE))
    else {
        return Way::Page("This release has no signed download for this Mac.");
    };
    if zip.size > MAX_ZIP {
        return Way::Page("This release's download is too big to be Somme.");
    }
    Way::Install(Box::new(Plan {
        bundle,
        zip: zip.clone(),
        sums: sums.clone(),
        signature: signature.clone(),
        key,
    }))
}

/// The app bundle `exe` runs from, when it is `…/X.app/Contents/MacOS/X`.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let fits = macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && bundle.extension()? == "app";
    (fits && bundle.parent().is_some()).then(|| bundle.to_path_buf())
}

/// Whether Somme may move `bundle`: its folder takes new files, and the
/// bundle is the user's own.
fn writable(bundle: &Path) -> bool {
    let Some(folder) = bundle.parent() else { return false };
    let Ok(probe) = tempfile::Builder::new().prefix(".somme-update-").tempfile_in(folder) else {
        return false;
    };
    same_owner(probe.path(), bundle)
}

#[cfg(unix)]
fn same_owner(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.uid() == b.uid(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_owner(_: &Path, _: &Path) -> bool {
    true
}

/// Only secure links to GitHub's release pages and the servers they send
/// downloads to.
pub fn allowed(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && matches!(
            url.host_str(),
            Some("github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com")
        )
}

/// Fetches release files, following redirects only to allowed places.
pub struct Fetch {
    client: reqwest::Client,
    allowed: fn(&Url) -> bool,
}

impl Fetch {
    pub fn new() -> reqwest::Result<Self> {
        Self::with(allowed)
    }

    fn with(allowed: fn(&Url) -> bool) -> reqwest::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(AGENT)
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= REDIRECTS {
                    attempt.error("too many redirects")
                } else if !allowed(attempt.url()) {
                    attempt.error("redirected away from GitHub")
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        Ok(Self { client, allowed })
    }

    /// Streams `asset`, at most `limit` bytes, to `sink`. Returns its SHA-256.
    async fn fetch(&self, asset: &Asset, limit: u64, sink: &mut Sink, mut progress: impl FnMut(u64) + Send) -> anyhow::Result<[u8; 32]> {
        let url = Url::parse(&asset.browser_download_url)?;
        anyhow::ensure!((self.allowed)(&url), "{url} is not on GitHub");
        let mut response = self.client.get(url).send().await?.error_for_status()?;
        if response.content_length().is_some_and(|length| length > limit) {
            anyhow::bail!("{} is too big", asset.name);
        }
        let mut hash = Sha256::new();
        let mut done = 0u64;
        while let Some(chunk) = response.chunk().await? {
            done += chunk.len() as u64;
            anyhow::ensure!(done <= limit, "{} is too big", asset.name);
            hash.update(&chunk);
            match sink {
                Sink::Memory(bytes) => bytes.extend_from_slice(&chunk),
                Sink::File(file) => file.write_all(&chunk).await?,
            }
            progress(done);
        }
        if let Sink::File(file) = sink {
            file.flush().await?;
        }
        Ok(hash.finalize().into())
    }

    async fn bytes(&self, asset: &Asset, limit: u64) -> anyhow::Result<Vec<u8>> {
        let mut sink = Sink::Memory(Vec::new());
        self.fetch(asset, limit, &mut sink, |_| {}).await?;
        match sink {
            Sink::Memory(bytes) => Ok(bytes),
            Sink::File(_) => unreachable!("fetched into memory"),
        }
    }
}

enum Sink {
    Memory(Vec<u8>),
    File(tokio::fs::File),
}

/// Where an install is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Downloading { done: u64, total: u64 },
    Checking,
}

/// Downloads the update into `work`, checks it, and unpacks the new app.
/// Returns where the new app is. Errors read as plain words.
pub async fn prepare(
    plan: &Plan,
    version: Version,
    work: &Path,
    fetch: &Fetch,
    step: impl Fn(Step) + Send + Sync,
) -> anyhow::Result<PathBuf> {
    // The small signed list comes first, so a bad release stops before the big download.
    let sums = fetch.bytes(&plan.sums, MAX_SUMS).await.context(NO_DOWNLOAD)?;
    let signature = fetch.bytes(&plan.signature, MAX_SIGNATURE).await.context(NO_DOWNLOAD)?;
    if !verify::signed(&plan.key, &sums, &signature) {
        anyhow::bail!("The update isn't signed by Somme's makers, so it wasn't installed.");
    }
    let Some(expected) = verify::listed_hash(&String::from_utf8_lossy(&sums), &plan.zip.name) else {
        anyhow::bail!("The update's signed file list doesn't name this download, so it wasn't installed.");
    };

    tokio::fs::create_dir_all(work).await.context("Couldn't make a folder for the update.")?;
    let zip = work.join("download.zip");
    let file = tokio::fs::File::create(&zip).await.context("Couldn't save the update.")?;
    let total = plan.zip.size;
    step(Step::Downloading { done: 0, total });
    // Each tenth of a percent at most, so the popover isn't redrawn for every piece.
    let mut shown = 0;
    let got = fetch
        .fetch(&plan.zip, MAX_ZIP, &mut Sink::File(file), |done| {
            if done - shown >= (total / 1000).max(64 * 1024) || done == total {
                shown = done;
                step(Step::Downloading { done, total });
            }
        })
        .await
        .context(NO_DOWNLOAD)?;
    step(Step::Checking);
    if got != expected {
        anyhow::bail!("The download was damaged on the way. Try again.");
    }

    let unpacked = work.join("new");
    unzip(&zip, &unpacked).await.context("Couldn't unpack the update.")?;
    let _ = tokio::fs::remove_file(&zip).await;
    let app = unpacked.join("Somme.app");
    check_app(&app, version)?;
    Ok(app)
}

async fn unzip(zip: &Path, into: &Path) -> anyhow::Result<()> {
    let status = tokio::process::Command::new("/usr/bin/ditto")
        .arg("-x")
        .arg("-k")
        .arg(zip)
        .arg(into)
        .stdin(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "ditto stopped with {status}");
    Ok(())
}

/// Checks the unpacked app is Somme `version`.
fn check_app(app: &Path, version: Version) -> anyhow::Result<()> {
    let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).context("The download holds no Somme app.")?;
    if !app.join("Contents/MacOS/Somme").is_file() {
        anyhow::bail!("The download holds no Somme app.");
    }
    match plist_string(&plist, "CFBundleShortVersionString") {
        Some(found) if found == version.to_string() => Ok(()),
        Some(found) => anyhow::bail!("The download holds Somme {found}, not {version}, so it wasn't installed."),
        None => anyhow::bail!("The download's app has no version, so it wasn't installed."),
    }
}

/// The string under `key` at the top of an XML property list.
fn plist_string(xml: &str, key: &str) -> Option<String> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(xml, options).ok()?;
    let dict = doc.root_element().children().find(|node| node.has_tag_name("dict"))?;
    let mut items = dict.children().filter(roxmltree::Node::is_element);
    while let Some(item) = items.next() {
        if item.has_tag_name("key") && item.text() == Some(key) {
            let value = items.next()?;
            return value.has_tag_name("string").then(|| value.text().unwrap_or_default().trim().to_owned());
        }
    }
    None
}

/// The folders the script works on.
#[derive(Clone, Debug)]
pub struct Swap {
    /// Somme's process. The script waits for it to end.
    pub pid: u32,
    /// The running app, replaced.
    pub app: PathBuf,
    /// The new app, moved in.
    pub new: PathBuf,
    /// Where the old app waits until the new one starts.
    pub aside: PathBuf,
    /// The download folder, removed at the end.
    pub work: PathBuf,
    pub log: PathBuf,
}

impl Swap {
    pub fn new(app: &Path, new: PathBuf, work: PathBuf, log: PathBuf) -> Self {
        let pid = std::process::id();
        let name = app.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "Somme.app".into());
        let mut aside = app.with_file_name(format!("{name}.old-{pid}"));
        let mut n = 1;
        while aside.exists() {
            n += 1;
            aside = app.with_file_name(format!("{name}.old-{pid}-{n}"));
        }
        Self {
            pid,
            app: app.to_path_buf(),
            new,
            aside,
            work,
            log,
        }
    }
}

/// `path` in single quotes, for `/bin/sh`. None when it isn't UTF-8.
fn quote(path: &Path) -> Option<String> {
    Some(format!("'{}'", path.to_str()?.replace('\'', r"'\''")))
}

/// The script that swaps the apps once Somme quits. `open` starts an app.
fn script(swap: &Swap, open: &Path) -> Option<String> {
    Some(format!(
        r#"#!/bin/sh
# Written by Somme to finish an update. Waits for Somme to quit, puts the
# new app where the old one was and opens it. Puts the old app back if
# anything goes wrong.
set -u
pid={pid}
app={app}
new={new}
old={old}
work={work}
open={open}
exec >>{log} 2>&1
echo "$(date): updating $app"

# Wait for Somme to quit, two minutes at most.
tries=0
while kill -0 "$pid" 2>/dev/null; do
    tries=$((tries + 1))
    if [ "$tries" -gt 600 ]; then
        echo "Somme didn't quit, so nothing was changed."
        rm -rf "$work"
        exit 1
    fi
    sleep 0.2
done

if ! mv "$app" "$old"; then
    echo "Couldn't move the old app aside, so nothing was changed."
    rm -rf "$work"
    "$open" "$app"
    exit 1
fi
if mv "$new" "$app" && "$open" "$app"; then
    rm -rf "$old" "$work"
    echo "Updated."
    exit 0
fi
echo "The new app didn't start, so the old one is back."
rm -rf "$app"
mv "$old" "$app"
rm -rf "$work"
"$open" "$app"
exit 1
"#,
        pid = swap.pid,
        app = quote(&swap.app)?,
        new = quote(&swap.new)?,
        old = quote(&swap.aside)?,
        work = quote(&swap.work)?,
        open = quote(open)?,
        log = quote(&swap.log)?,
    ))
}

/// Starts the script on its own, so it outlives Somme. Somme must quit next.
pub fn relaunch(swap: &Swap, script_path: &Path) -> anyhow::Result<()> {
    start_script(swap, script_path, Path::new(OPEN)).map(drop)
}

fn start_script(swap: &Swap, script_path: &Path, open: &Path) -> anyhow::Result<std::process::Child> {
    let text = script(swap, open).context("Somme's folders have names the installer can't use.")?;
    std::fs::write(script_path, text).context("Couldn't write the installer.")?;
    let mut command = std::process::Command::new("/bin/sh");
    command.arg(script_path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // Its own process group, so nothing sent to Somme's reaches it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn().context("Couldn't start the installer.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::verify::tests::{sign, test_key};

    fn release(assets: &[&str]) -> Release {
        Release {
            version: Version(0, 2, 0),
            tag: "v0.2.0".into(),
            notes: String::new(),
            assets: assets
                .iter()
                .map(|name| Asset {
                    name: (*name).to_owned(),
                    size: 10,
                    browser_download_url: format!("https://github.com/kahflane/Somme/releases/download/v0.2.0/{name}"),
                })
                .collect(),
        }
    }

    fn page(way: Way) -> &'static str {
        match way {
            Way::Page(why) => why,
            Way::Install(_) => panic!("expected the release page"),
        }
    }

    /// A fake `Somme.app` holding `marker` and the given version.
    fn fake_app(at: &Path, version: &str, marker: &str) -> PathBuf {
        let app = at.join("Somme.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/Somme"), marker).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Somme</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
</dict>
</plist>
"#
            ),
        )
        .unwrap();
        app
    }

    #[test]
    fn finds_the_app_bundle() {
        assert_eq!(
            bundle_of(Path::new("/Applications/Somme.app/Contents/MacOS/Somme")),
            Some(PathBuf::from("/Applications/Somme.app"))
        );
        assert_eq!(bundle_of(Path::new("/Users/ann/somme/target/debug/somme-desktop")), None);
        assert_eq!(bundle_of(Path::new("/Applications/Somme/Contents/MacOS/Somme")), None);
        assert_eq!(bundle_of(Path::new("Somme")), None);
    }

    #[test]
    fn picks_install_only_when_it_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let app = fake_app(dir.path(), "0.1.0", "old");
        let exe = app.join("Contents/MacOS/Somme");
        let key = Some(test_key().verifying_key());
        let full = release(&["Somme-0.2.0-macos-aarch64.zip", "Somme-0.2.0-macos-x86_64.zip", SUMS, SIGNATURE]);

        assert!(page(way_for(&full, false, key, Some(&exe))).contains("release page"));
        assert!(page(way_for(&full, true, None, Some(&exe))).contains("can't check"));
        assert!(page(way_for(&full, true, key, Some(Path::new("/tmp/somme-desktop")))).contains("isn't running from its app"));
        let unsigned = release(&["Somme-0.2.0-macos-aarch64.zip", "Somme-0.2.0-macos-x86_64.zip", SUMS]);
        assert!(page(way_for(&unsigned, true, key, Some(&exe))).contains("no signed download"));
        let other_arch = release(&["Somme-0.2.0-macos-arm.zip", SUMS, SIGNATURE]);
        assert!(page(way_for(&other_arch, true, key, Some(&exe))).contains("no signed download"));
        let mut huge = full.clone();
        huge.assets.iter_mut().for_each(|asset| asset.size = MAX_ZIP + 1);
        assert!(page(way_for(&huge, true, key, Some(&exe))).contains("too big"));

        let Way::Install(plan) = way_for(&full, true, key, Some(&exe)) else { panic!("expected install") };
        assert_eq!(plan.bundle, app);
        assert_eq!(plan.zip.name, full.mac_zip());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_somme_cant_write_means_the_release_page() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("Locked");
        std::fs::create_dir(&locked).unwrap();
        let app = fake_app(&locked, "0.1.0", "old");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let full = release(&["Somme-0.2.0-macos-aarch64.zip", "Somme-0.2.0-macos-x86_64.zip", SUMS, SIGNATURE]);
        let why = page(way_for(&full, true, Some(test_key().verifying_key()), Some(&app.join("Contents/MacOS/Somme"))));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(why.contains("can't change"), "{why}");
    }

    #[test]
    fn only_github_downloads() {
        let ok = |url: &str| allowed(&Url::parse(url).unwrap());
        assert!(ok("https://github.com/kahflane/Somme/releases/download/v0.2.0/SHA256SUMS"));
        assert!(ok("https://objects.githubusercontent.com/github-production-release-asset/1"));
        assert!(ok("https://release-assets.githubusercontent.com/github-production-release-asset/1"));
        assert!(!ok("http://github.com/kahflane/Somme/releases/download/v0.2.0/SHA256SUMS"));
        assert!(!ok("https://github.com.example.test/x"));
        assert!(!ok("https://evil.example.test/github.com"));
        assert!(!ok("https://user@github.com/x"));
        assert!(!ok("https://github.com:8443/x"));
        assert!(!ok("https://127.0.0.1/x"));
    }

    #[test]
    fn reads_the_app_version() {
        let dir = tempfile::tempdir().unwrap();
        let app = fake_app(dir.path(), "0.2.0", "new");
        assert!(check_app(&app, Version(0, 2, 0)).is_ok());
        assert!(check_app(&app, Version(0, 3, 0)).unwrap_err().to_string().contains("0.2.0, not 0.3.0"));
        assert!(check_app(&dir.path().join("Gone.app"), Version(0, 2, 0)).is_err());
        assert_eq!(plist_string("<plist><dict><key>A</key><integer>1</integer></dict></plist>", "A"), None);
    }

    /// Serves `files` over plain HTTP on this computer, for as long as the test runs.
    async fn serve(files: Vec<(&'static str, Vec<u8>)>) -> String {
        use tokio::io::AsyncReadExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let files = files.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let request = String::from_utf8_lossy(&request);
                    let path = request.split_whitespace().nth(1).unwrap_or_default().trim_start_matches('/').to_owned();
                    let reply = match files.iter().find(|(name, _)| *name == path) {
                        Some((_, body)) => {
                            let mut reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                            reply.extend_from_slice(body);
                            reply
                        }
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = socket.write_all(&reply).await;
                });
            }
        });
        base
    }

    fn local(url: &Url) -> bool {
        url.scheme() == "http" && url.host_str() == Some("127.0.0.1")
    }

    fn plan_for(base: &str, bundle: PathBuf, zip_name: &str, zip_size: u64) -> Plan {
        let asset = |name: &str, size| Asset {
            name: name.to_owned(),
            size,
            browser_download_url: format!("{base}/{name}"),
        };
        Plan {
            bundle,
            zip: asset(zip_name, zip_size),
            sums: asset(SUMS, 100),
            signature: asset(SIGNATURE, 100),
            key: test_key().verifying_key(),
        }
    }

    /// Zips a fake new app the way `release.sh` does.
    #[cfg(target_os = "macos")]
    fn fake_zip(dir: &Path, version: &str) -> Vec<u8> {
        let staging = dir.join("staging");
        let app = fake_app(&staging, version, "new");
        let zip = dir.join("release.zip");
        let status = std::process::Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&app)
            .arg(&zip)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::read(zip).unwrap()
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn downloads_checks_and_unpacks_the_update() {
        let dir = tempfile::tempdir().unwrap();
        let name = "Somme-0.2.0-macos-aarch64.zip";
        let zip = fake_zip(dir.path(), "0.2.0");
        let sums = format!("{}  {name}\n", verify::hex(&Sha256::digest(&zip)));
        let signature = sign(&test_key(), sums.as_bytes());
        let size = zip.len() as u64;
        let base = serve(vec![(name, zip), (SUMS, sums.into_bytes()), (SIGNATURE, signature.into_bytes())]).await;
        let fetch = Fetch::with(local).unwrap();
        let plan = plan_for(&base, dir.path().join("Somme.app"), name, size);
        let steps = std::sync::Mutex::new(Vec::new());
        let work = dir.path().join("updates/1");
        let app = prepare(&plan, Version(0, 2, 0), &work, &fetch, |step| steps.lock().unwrap().push(step)).await.unwrap();
        assert_eq!(app, work.join("new/Somme.app"));
        assert_eq!(std::fs::read_to_string(app.join("Contents/MacOS/Somme")).unwrap(), "new");
        assert!(!work.join("download.zip").exists());
        let steps = steps.into_inner().unwrap();
        assert_eq!(steps.first(), Some(&Step::Downloading { done: 0, total: size }));
        assert!(steps.contains(&Step::Downloading { done: size, total: size }));
        assert_eq!(steps.last(), Some(&Step::Checking));

        // A version other than the release's is refused.
        let err = prepare(&plan, Version(0, 3, 0), &dir.path().join("updates/2"), &fetch, |_| {}).await.unwrap_err();
        assert!(err.to_string().contains("not 0.3.0"), "{err}");
    }

    #[tokio::test]
    async fn refuses_bad_signatures_damage_and_other_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let name = "Somme-0.2.0-macos-aarch64.zip";
        let zip = b"zip bytes".to_vec();
        let good = format!("{}  {name}\n", verify::hex(&Sha256::digest(&zip)));
        let wrong = format!("{}  {name}\n", "00".repeat(32));
        let base = serve(vec![
            (name, zip.clone()),
            ("good/SHA256SUMS", good.clone().into_bytes()),
            ("good/SHA256SUMS.sig", sign(&test_key(), good.as_bytes()).into_bytes()),
            ("forged/SHA256SUMS", good.clone().into_bytes()),
            ("forged/SHA256SUMS.sig", sign(&ed25519_dalek::SigningKey::from_bytes(&[9; 32]), good.as_bytes()).into_bytes()),
            ("wrong/SHA256SUMS", wrong.clone().into_bytes()),
            ("wrong/SHA256SUMS.sig", sign(&test_key(), wrong.as_bytes()).into_bytes()),
        ])
        .await;
        let fetch = Fetch::with(local).unwrap();
        let attempt = |folder: &str, limit_size: u64| {
            let mut plan = plan_for(&base, dir.path().join("Somme.app"), name, limit_size);
            plan.sums.browser_download_url = format!("{base}/{folder}/{SUMS}");
            plan.signature.browser_download_url = format!("{base}/{folder}/{SIGNATURE}");
            plan
        };
        let run = async |plan: Plan| {
            prepare(&plan, Version(0, 2, 0), &dir.path().join("work"), &fetch, |_| {}).await.unwrap_err().to_string()
        };
        assert!(run(attempt("forged", 9)).await.contains("isn't signed"));
        assert!(run(attempt("wrong", 9)).await.contains("damaged"));
        // Everything checks out, but it isn't a zip.
        assert!(run(attempt("good", 9)).await.contains("unpack"));
        // Downloads that aren't allowed are never asked for.
        let strict = Fetch::new().unwrap();
        let err = prepare(&attempt("good", 9), Version(0, 2, 0), &dir.path().join("work2"), &strict, |_| {}).await.unwrap_err();
        assert!(format!("{err:#}").contains("not on GitHub"), "{err:#}");
    }

    #[test]
    fn script_quotes_every_path() {
        let swap = Swap {
            pid: 42,
            app: PathBuf::from("/Applications/Ann's Somme.app"),
            new: PathBuf::from("/data/updates/1/new/Somme.app"),
            aside: PathBuf::from("/Applications/Ann's Somme.app.old-42"),
            work: PathBuf::from("/data/updates/1"),
            log: PathBuf::from("/data/updates/update $(log).txt"),
        };
        let text = script(&swap, Path::new(OPEN)).unwrap();
        assert!(text.starts_with("#!/bin/sh\n"));
        assert!(text.contains("pid=42\n"));
        assert!(text.contains(r"app='/Applications/Ann'\''s Somme.app'"));
        assert!(text.contains(r"old='/Applications/Ann'\''s Somme.app.old-42'"));
        assert!(text.contains("exec >>'/data/updates/update $(log).txt' 2>&1"));
        assert!(text.contains("open='/usr/bin/open'"));
        // The old app is removed only after the new one opened.
        let opened = text.find(r#"if mv "$new" "$app" && "$open" "$app"; then"#).unwrap();
        let removed = text.find(r#"rm -rf "$old" "$work""#).unwrap();
        assert!(opened < removed);
    }

    #[test]
    fn aside_name_is_free() {
        let dir = tempfile::tempdir().unwrap();
        let app = fake_app(dir.path(), "0.1.0", "old");
        let first = Swap::new(&app, PathBuf::new(), PathBuf::new(), PathBuf::new());
        std::fs::create_dir(&first.aside).unwrap();
        let second = Swap::new(&app, PathBuf::new(), PathBuf::new(), PathBuf::new());
        assert_ne!(first.aside, second.aside);
        assert_eq!(second.aside.parent(), app.parent());
    }

    /// Runs the real script on temp folders, with a fake `open` that only
    /// writes down what it was asked to open, and a Somme that already quit.
    #[cfg(unix)]
    fn run_swap(dir: &Path, with_new: bool, open_works: bool) -> (std::process::ExitStatus, String) {
        let apps = dir.join("Applications");
        let app = fake_app(&apps, "0.1.0", "old");
        let work = dir.join("updates/1");
        let new = if with_new { fake_app(&work.join("new"), "0.2.0", "new") } else { work.join("new/Somme.app") };
        std::fs::create_dir_all(&work).unwrap();
        let opened = dir.join("opened.txt");
        let open = dir.join("fake-open.sh");
        std::fs::write(
            &open,
            format!("#!/bin/sh\necho \"$1\" >> {}\nexit {}\n", quote(&opened).unwrap(), if open_works { 0 } else { 1 }),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut quit = std::process::Command::new("/bin/sh").arg("-c").arg("exit 0").spawn().unwrap();
        quit.wait().unwrap();
        let mut swap = Swap::new(&app, new, work, dir.join("updates/update.log"));
        swap.pid = quit.id();
        let status = start_script(&swap, &dir.join("updates/relaunch.sh"), &open).unwrap().wait().unwrap();
        (status, std::fs::read_to_string(opened).unwrap_or_default())
    }

    #[cfg(unix)]
    #[test]
    fn script_swaps_the_apps_and_opens_the_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let (status, opened) = run_swap(dir.path(), true, true);
        assert!(status.success());
        let app = dir.path().join("Applications/Somme.app");
        assert_eq!(std::fs::read_to_string(app.join("Contents/MacOS/Somme")).unwrap(), "new");
        assert_eq!(opened.trim(), app.to_str().unwrap());
        // The old app and the download are gone; only the app and the log stay.
        let left: Vec<_> = std::fs::read_dir(dir.path().join("Applications")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["Somme.app"]);
        assert!(!dir.path().join("updates/1").exists());
        assert!(std::fs::read_to_string(dir.path().join("updates/update.log")).unwrap().contains("Updated."));
    }

    #[cfg(unix)]
    #[test]
    fn script_puts_the_old_app_back_when_the_new_one_fails() {
        for (with_new, open_works) in [(false, true), (true, false)] {
            let dir = tempfile::tempdir().unwrap();
            let (status, opened) = run_swap(dir.path(), with_new, open_works);
            assert!(!status.success());
            let app = dir.path().join("Applications/Somme.app");
            assert_eq!(std::fs::read_to_string(app.join("Contents/MacOS/Somme")).unwrap(), "old");
            assert!(opened.trim().ends_with("Somme.app"), "{opened}");
            let left: Vec<_> = std::fs::read_dir(dir.path().join("Applications")).unwrap().map(|e| e.unwrap().file_name()).collect();
            assert_eq!(left, ["Somme.app"]);
            assert!(!dir.path().join("updates/1").exists());
        }
    }
}
