//! Which system each server runs, so hosts show its logo. Learned from the
//! server's `/etc/os-release` after each first login of a run, and kept in a
//! file so the logos show before connecting.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui_kit::component::{ActiveTheme as _, Icon};
use gpui_kit::{App, AssetSource, Div, Global, Hsla, SharedString, Styled as _, rgb};
use somme_application::dto::HostView;
use somme_domain::Id;

use crate::terminal::disk::Disk;
use crate::ui::common::{Lucide, parse_color, tinted_badge};

const FILE_NAME: &str = "host-os.json";
/// More than any release file needs.
const READ_LIMIT: u64 = 64 * 1024;

pub struct Logo {
    pub id: &'static str,
    /// The brand colour, or None when it is black and would vanish on a dark theme.
    color: Option<u32>,
    svg: &'static [u8],
}

macro_rules! logo {
    ($id:literal, $color:expr) => {
        Logo {
            id: $id,
            color: $color,
            svg: include_bytes!(concat!("../assets/os/", $id, ".svg")),
        }
    };
}

/// Logos from Simple Icons (CC0), with their brand colours, a lighter one
/// where the brand's is too dark to see on a dark theme.
const LOGOS: &[Logo] = &[
    logo!("almalinux", None),
    logo!("alpinelinux", Some(0x2A85C4)),
    logo!("apple", None),
    logo!("archlinux", Some(0x1793D1)),
    logo!("centos", Some(0x9CCD2A)),
    logo!("debian", Some(0xD70A53)),
    logo!("fedora", Some(0x51A2DA)),
    logo!("freebsd", Some(0xAB2B28)),
    logo!("gentoo", Some(0x9A8BCB)),
    logo!("kalilinux", Some(0x557C94)),
    logo!("linux", Some(0xFCC624)),
    logo!("linuxmint", Some(0x86BE43)),
    logo!("manjaro", Some(0x35BF5C)),
    logo!("nixos", Some(0x5277C3)),
    logo!("opensuse", Some(0x73BA25)),
    logo!("raspberrypi", Some(0xA22846)),
    logo!("redhat", Some(0xEE0000)),
    logo!("rockylinux", Some(0x10B981)),
    logo!("suse", Some(0x30BA78)),
    logo!("ubuntu", Some(0xE95420)),
];

impl Logo {
    fn path(&self) -> SharedString {
        format!("os/{}.svg", self.id).into()
    }
}

fn logo(id: &str) -> Option<&'static Logo> {
    LOGOS.iter().find(|logo| logo.id == id)
}

/// The kit's icons, plus the system logos at `os/<id>.svg`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let own = path.strip_prefix("os/").and_then(|name| name.strip_suffix(".svg")).and_then(logo);
        match own {
            Some(logo) => Ok(Some(Cow::Borrowed(logo.svg))),
            None => gpui_kit::assets::AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut list = gpui_kit::assets::AllAssets.list(path)?;
        list.extend(LOGOS.iter().map(Logo::path).filter(|own| own.starts_with(path)));
        Ok(list)
    }
}

/// Host id to logo id.
struct HostOs {
    file: PathBuf,
    known: HashMap<String, String>,
    /// Hosts already looked at this run.
    checked: HashSet<String>,
}

impl Global for HostOs {}

pub fn init(data_dir: &Path, cx: &mut App) {
    let file = data_dir.join(FILE_NAME);
    let known = std::fs::read_to_string(&file)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    cx.set_global(HostOs {
        file,
        known,
        checked: HashSet::new(),
    });
}

fn host_logo(host: Id, cx: &App) -> Option<&'static Logo> {
    let id = cx.try_global::<HostOs>()?.known.get(&host.to_string())?;
    logo(id)
}

/// The host's own colour, else its system's, else the theme's.
fn tint(host: &HostView, logo: Option<&Logo>, cx: &App) -> Hsla {
    parse_color(host.color.as_deref())
        .or_else(|| logo.and_then(|logo| logo.color).map(|color| rgb(color).into()))
        .unwrap_or(cx.theme().primary)
}

/// The system's logo, or a plain server until it is known.
pub fn icon(host: &HostView, cx: &App) -> Icon {
    let logo = host_logo(host.id, cx);
    let icon = match logo {
        Some(logo) => Icon::empty().path(logo.path()),
        None => Icon::new(Lucide::Server),
    };
    icon.text_color(tint(host, logo, cx))
}

/// The tinted square with the host's logo, for host lists.
pub fn badge(host: &HostView, cx: &App) -> Div {
    let logo = host_logo(host.id, cx);
    let icon = match logo {
        Some(logo) => Icon::empty().path(logo.path()),
        None => Icon::new(Lucide::Server),
    };
    tinted_badge(tint(host, logo, cx), icon)
}

/// Looks up the system of a server just logged into, once a run.
pub fn check(host: Id, disk: impl Future<Output = Option<Disk>> + 'static, cx: &mut App) {
    let key = host.to_string();
    let Some(state) = cx.try_global::<HostOs>() else { return };
    if state.checked.contains(&key) {
        return;
    }
    cx.global_mut::<HostOs>().checked.insert(key.clone());
    cx.spawn(async move |cx| {
        let Some(disk) = disk.await else { return };
        let Some(found) = disk.run(|disk| detect(disk)).await else { return };
        cx.update(|cx| remember(key, found, cx));
    })
    .detach();
}

fn remember(host: String, found: &'static str, cx: &mut App) {
    let state = cx.global_mut::<HostOs>();
    if state.known.get(&host).map(String::as_str) == Some(found) {
        return;
    }
    state.known.insert(host, found.to_owned());
    match serde_json::to_string_pretty(&state.known) {
        Ok(raw) => {
            if let Err(err) = std::fs::write(&state.file, raw) {
                tracing::warn!(%err, "could not save host systems");
            }
        }
        Err(err) => tracing::warn!(%err, "could not encode host systems"),
    }
    cx.refresh_windows();
}

/// Reads the server's release file. None when the server shows none.
fn detect(disk: &Disk) -> Option<&'static str> {
    for file in ["/etc/os-release", "/usr/lib/os-release"] {
        if let Ok(bytes) = disk.read(Path::new(file), READ_LIMIT) {
            return Some(from_os_release(&String::from_utf8_lossy(&bytes)));
        }
    }
    disk.read(Path::new("/System/Library/CoreServices/SystemVersion.plist"), READ_LIMIT)
        .ok()
        .map(|_| "apple")
}

/// The logo for an os-release file: its `ID`, else the first of `ID_LIKE`
/// with a logo, else Tux.
fn from_os_release(text: &str) -> &'static str {
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let value = line.trim().strip_prefix(name)?.strip_prefix('=')?;
            Some(value.trim().trim_matches(|c| c == '"' || c == '\'').to_lowercase())
        })
    };
    let id = field("ID").unwrap_or_default();
    let like = field("ID_LIKE").unwrap_or_default();
    std::iter::once(id.as_str())
        .chain(like.split_whitespace())
        .find_map(logo_for)
        .unwrap_or("linux")
}

fn logo_for(id: &str) -> Option<&'static str> {
    Some(match id {
        "ubuntu" => "ubuntu",
        "debian" => "debian",
        "raspbian" => "raspberrypi",
        "centos" => "centos",
        "rhel" => "redhat",
        "fedora" => "fedora",
        "rocky" => "rockylinux",
        "almalinux" => "almalinux",
        "alpine" => "alpinelinux",
        "arch" => "archlinux",
        "manjaro" => "manjaro",
        "linuxmint" => "linuxmint",
        "kali" => "kalilinux",
        "gentoo" => "gentoo",
        "nixos" => "nixos",
        "freebsd" => "freebsd",
        "sles" | "sled" | "suse" => "suse",
        id if id.starts_with("opensuse") => "opensuse",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_release_files() {
        let ubuntu = "PRETTY_NAME=\"Ubuntu 24.04 LTS\"\nNAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n";
        assert_eq!(from_os_release(ubuntu), "ubuntu");
        assert_eq!(from_os_release("ID=\"rhel\"\nID_LIKE=\"fedora\"\n"), "redhat");
        assert_eq!(from_os_release("ID='opensuse-tumbleweed'\nID_LIKE=\"opensuse suse\"\n"), "opensuse");
        // Pop!_OS has no logo here, so it gets Ubuntu's, which it is built on.
        assert_eq!(from_os_release("ID=pop\nID_LIKE=\"ubuntu debian\"\n"), "ubuntu");
        assert_eq!(from_os_release("ID=amzn\nID_LIKE=\"centos rhel fedora\"\n"), "centos");
        assert_eq!(from_os_release("ID=something\n"), "linux");
        assert_eq!(from_os_release(""), "linux");
        // `ID_LIKE` is not `ID`.
        assert_eq!(from_os_release("ID_LIKE=arch\nID=endeavouros\n"), "archlinux");
    }

    #[test]
    fn every_logo_is_bundled() {
        for logo in LOGOS {
            assert!(logo.svg.starts_with(b"<svg"), "{}", logo.id);
            assert_eq!(Assets.load(&logo.path()).unwrap().as_deref(), Some(logo.svg));
        }
        for id in ["ubuntu", "debian", "raspbian", "rhel", "rocky", "alpine", "arch", "kali", "sles", "opensuse-leap"] {
            assert!(logo(logo_for(id).unwrap()).is_some(), "{id}");
        }
        assert!(Assets.load(&Lucide::Server.path()).unwrap().is_some(), "the kit's icons still load");
    }
}
