//! Language servers Somme installs by itself, for people who don't have them.
//! Many are written for Node.js, so Somme gets Node first, then the servers
//! from npm, all in `servers` under its data folder. Node is checked against
//! its known hash, and each npm package is pinned to one version.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::{App, AppContext as _, Context, Entity, Global, Task};

use crate::download::{self, Pinned};
use crate::runtime;

const REFRESH: Duration = Duration::from_millis(250);

/// A Node.js build for one kind of computer.
struct NodeBuild {
    os: &'static str,
    arch: &'static str,
    /// The folder the archive holds.
    folder: &'static str,
    file: Pinned,
}

const NODE: &[NodeBuild] = &[
    NodeBuild {
        os: "macos",
        arch: "aarch64",
        folder: "node-v24.22.0-darwin-arm64",
        file: Pinned {
            url: "https://nodejs.org/dist/v24.22.0/node-v24.22.0-darwin-arm64.tar.gz",
            size: 53_049_576,
            sha256: "6a9724d9b50a5f22b4816cded441b5a8d9ddae546191a5e5b7efa5a5e2ee26d5",
        },
    },
    NodeBuild {
        os: "macos",
        arch: "x86_64",
        folder: "node-v24.22.0-darwin-x64",
        file: Pinned {
            url: "https://nodejs.org/dist/v24.22.0/node-v24.22.0-darwin-x64.tar.gz",
            size: 54_356_645,
            sha256: "495e3f273c645f3c4677d438304cdfc701729ea5dec5728f5c8e76a2dece4b91",
        },
    },
    NodeBuild {
        os: "linux",
        arch: "x86_64",
        folder: "node-v24.22.0-linux-x64",
        file: Pinned {
            url: "https://nodejs.org/dist/v24.22.0/node-v24.22.0-linux-x64.tar.gz",
            size: 58_256_361,
            sha256: "28ea0a438de9a13bd7891fd758eee6c7819ee8af5442dfd8d45bd65e56d4228c",
        },
    },
    NodeBuild {
        os: "linux",
        arch: "aarch64",
        folder: "node-v24.22.0-linux-arm64",
        file: Pinned {
            url: "https://nodejs.org/dist/v24.22.0/node-v24.22.0-linux-arm64.tar.gz",
            size: 57_997_377,
            sha256: "b7e7991d5407815adfa74a2a9cd4db683552c8ce32ff4a295c22e972849c3f5d",
        },
    },
    NodeBuild {
        os: "windows",
        arch: "x86_64",
        folder: "node-v24.22.0-win-x64",
        file: Pinned {
            url: "https://nodejs.org/dist/v24.22.0/node-v24.22.0-win-x64.zip",
            size: 37_733_703,
            sha256: "f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3",
        },
    },
    NodeBuild {
        os: "windows",
        arch: "aarch64",
        folder: "node-v24.22.0-win-arm64",
        file: Pinned {
            url: "https://nodejs.org/dist/v24.22.0/node-v24.22.0-win-arm64.zip",
            size: 33_770_411,
            sha256: "3a5c209a5bc59e65b3bc0fa13ed5e166a26bd3d55332847f498b76b037408d61",
        },
    },
];

fn node_build() -> Option<&'static NodeBuild> {
    NODE.iter().find(|build| build.os == std::env::consts::OS && build.arch == std::env::consts::ARCH)
}

/// Servers installed together from npm.
#[derive(Debug, PartialEq, Eq)]
pub struct Package {
    pub id: &'static str,
    /// What it helps with, like "JSON, HTML and CSS".
    pub label: &'static str,
    /// What npm installs, each at a fixed version.
    npm: &'static [&'static str],
    /// The command that installs it by hand.
    pub hint: &'static str,
}

pub const LANGSERVERS: Package = Package {
    id: "vscode-langservers",
    label: "JSON, HTML and CSS",
    npm: &["vscode-langservers-extracted@4.10.0"],
    hint: "npm i -g vscode-langservers-extracted",
};
const TYPESCRIPT: Package = Package {
    id: "typescript",
    label: "JavaScript and TypeScript",
    npm: &["typescript-language-server@6.0.2", "typescript@6.0.3"],
    hint: "npm i -g typescript typescript-language-server",
};
const YAML: Package = Package {
    id: "yaml",
    label: "YAML",
    npm: &["yaml-language-server@2.0.0"],
    hint: "npm i -g yaml-language-server",
};
const PYRIGHT: Package = Package {
    id: "pyright",
    label: "Python",
    npm: &["pyright@1.1.414"],
    hint: "npm i -g pyright",
};
const BASH: Package = Package {
    id: "bash",
    label: "shell scripts",
    npm: &["bash-language-server@5.8.1"],
    hint: "npm i -g bash-language-server",
};

/// The package with `program`, and the script in its `node_modules` that starts it.
fn script_for(program: &str) -> Option<(&'static Package, &'static str)> {
    Some(match program {
        "vscode-json-language-server" => (&LANGSERVERS, "vscode-langservers-extracted/bin/vscode-json-language-server"),
        "vscode-html-language-server" => (&LANGSERVERS, "vscode-langservers-extracted/bin/vscode-html-language-server"),
        "vscode-css-language-server" => (&LANGSERVERS, "vscode-langservers-extracted/bin/vscode-css-language-server"),
        "typescript-language-server" => (&TYPESCRIPT, "typescript-language-server/lib/cli.mjs"),
        "yaml-language-server" => (&YAML, "yaml-language-server/bin/yaml-language-server"),
        "pyright-langserver" => (&PYRIGHT, "pyright/langserver.index.js"),
        "bash-language-server" => (&BASH, "bash-language-server/out/cli.js"),
        _ => return None,
    })
}

/// The package Somme can install with `program` in it, on this computer.
pub fn package_for(program: &str) -> Option<&'static Package> {
    node_build()?;
    script_for(program).map(|(package, _)| package)
}

/// How to install `program` by hand.
pub fn hint(program: &str) -> Option<&'static str> {
    if let Some((package, _)) = script_for(program) {
        return Some(package.hint);
    }
    let mac = cfg!(target_os = "macos");
    Some(match program {
        "rust-analyzer" => "rustup component add rust-analyzer",
        "gopls" => "go install golang.org/x/tools/gopls@latest",
        "taplo" => "cargo install taplo-cli --locked",
        "basedpyright-langserver" => "pip install basedpyright",
        "ruby-lsp" => "gem install ruby-lsp",
        "intelephense" => "npm i -g intelephense",
        "svelteserver" => "npm i -g svelte-language-server",
        "clangd" if mac => "xcode-select --install",
        "clangd" if cfg!(windows) => "winget install LLVM.LLVM",
        "clangd" => "sudo apt install clangd",
        "sourcekit-lsp" if mac => "xcode-select --install",
        "marksman" if mac => "brew install marksman",
        "lua-language-server" if mac => "brew install lua-language-server",
        "zls" if mac => "brew install zls",
        _ => return None,
    })
}

/// An installed server: Node, and what to pass it.
pub struct Installed {
    pub node: PathBuf,
    pub script: PathBuf,
    /// Node's own folder, so servers that run `node` find this one.
    pub bin: PathBuf,
}

fn node_dir(dir: &Path) -> Option<PathBuf> {
    node_build().map(|build| dir.join(build.folder))
}

fn node_program(node_dir: &Path) -> PathBuf {
    if cfg!(windows) { node_dir.join("node.exe") } else { node_dir.join("bin").join("node") }
}

fn node_bin(node_dir: &Path) -> PathBuf {
    if cfg!(windows) { node_dir.to_path_buf() } else { node_dir.join("bin") }
}

fn npm_cli(node_dir: &Path) -> PathBuf {
    let modules = if cfg!(windows) { node_dir.join("node_modules") } else { node_dir.join("lib").join("node_modules") };
    modules.join("npm").join("bin").join("npm-cli.js")
}

fn package_dir(dir: &Path, package: &Package) -> PathBuf {
    dir.join("npm").join(package.id)
}

/// `program`, when Somme installed it in `dir`.
pub fn installed(dir: &Path, program: &str) -> Option<Installed> {
    let (package, script) = script_for(program)?;
    let node_dir = node_dir(dir)?;
    let node = node_program(&node_dir);
    let script = package_dir(dir, package).join("node_modules").join(script);
    (node.is_file() && script.is_file()).then(|| Installed {
        node,
        script,
        bin: node_bin(&node_dir),
    })
}

/// How far an install has got.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    /// Node is in place, and npm is fetching the servers.
    pub npm: AtomicBool,
    cancel: AtomicBool,
}

/// Installs `package` in `dir`, and Node first if it isn't there.
pub async fn install(dir: PathBuf, package: &'static Package, progress: Arc<Progress>) -> Result<(), String> {
    let build = node_build().ok_or("Somme can't get Node.js for this kind of computer.")?;
    tokio::fs::create_dir_all(&dir).await.map_err(|err| format!("Could not make the servers folder: {err}"))?;
    let node_dir = dir.join(build.folder);
    if !node_program(&node_dir).is_file() {
        progress.total.store(build.file.size, Ordering::Relaxed);
        let name = build.file.url.rsplit('/').next().unwrap_or("node.tar.gz");
        let archive = dir.join(name);
        let client = download::client()?;
        download::fetch(&client, &build.file, &archive, "Node.js", &progress.done, &progress.cancel).await?;
        let (into, unpack_from, folder) = (dir.clone(), archive.clone(), build.folder);
        let unpacked = tokio::task::spawn_blocking(move || unpack(&unpack_from, &into, folder)).await;
        let _ = tokio::fs::remove_file(&archive).await;
        unpacked.map_err(|err| err.to_string())??;
    }
    progress.npm.store(true, Ordering::Relaxed);
    let target = package_dir(&dir, package);
    tokio::task::spawn_blocking(move || npm_install(&dir, &node_dir, package, &target))
        .await
        .map_err(|err| err.to_string())?
}

/// Unpacks Node's archive into `dir/folder`, through a folder of its own so
/// a half-unpacked one is never used. The `tar` every system has can do it:
/// Windows has one that reads zip files.
fn unpack(archive: &Path, dir: &Path, folder: &str) -> Result<(), String> {
    let work = dir.join(format!(".unpack-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|err| format!("Could not unpack Node.js: {err}"))?;
    let tar = if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        root.join("System32").join("tar.exe")
    } else {
        ["/usr/bin/tar", "/bin/tar"].into_iter().map(PathBuf::from).find(|tar| tar.is_file()).unwrap_or("tar".into())
    };
    let output = Command::new(tar)
        .arg(if cfg!(windows) { "-xf" } else { "-xzf" })
        .arg(archive)
        .arg("-C")
        .arg(&work)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("Could not unpack Node.js: {err}"));
    let result = output.and_then(|output| {
        if !output.status.success() {
            return Err(format!("Could not unpack Node.js: {}", last_line(&output.stderr)));
        }
        let target = dir.join(folder);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::rename(work.join(folder), &target).map_err(|err| format!("Could not unpack Node.js: {err}"))
    });
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Runs npm from Somme's Node to put `package` in `target`. Install scripts
/// are turned off: none of these servers needs one, and they would run code
/// from every package down the tree.
fn npm_install(dir: &Path, node_dir: &Path, package: &Package, target: &Path) -> Result<(), String> {
    let partial = target.with_extension("part");
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|err| format!("Could not make the servers folder: {err}"))?;
    let path = std::env::join_paths(
        std::iter::once(node_bin(node_dir)).chain(std::env::var_os("PATH").iter().flat_map(std::env::split_paths)),
    )
    .map_err(|err| err.to_string())?;
    let output = Command::new(node_program(node_dir))
        .arg(npm_cli(node_dir))
        .arg("install")
        .arg("--prefix")
        .arg(&partial)
        .args(["--no-audit", "--no-fund", "--ignore-scripts", "--omit=dev", "--no-package-lock", "--loglevel=error"])
        .args(package.npm)
        .env("PATH", path)
        .env("npm_config_cache", dir.join("npm-cache"))
        .env("npm_config_update_notifier", "false")
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("Could not run npm: {err}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(format!("npm couldn't install the servers: {}", last_line(&output.stderr)));
    }
    let _ = std::fs::remove_dir_all(target);
    std::fs::rename(&partial, target).map_err(|err| format!("Could not save the servers: {err}"))
}

fn last_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let line = text.lines().rev().map(str::trim).find(|line| !line.is_empty()).unwrap_or("it stopped with an error");
    line.trim_start_matches("npm error").trim().to_owned()
}

/// What an install is doing, for the status bar.
pub enum State<'a> {
    Idle,
    /// Getting Node, `done` of `total` bytes.
    Node { done: u64, total: u64 },
    /// npm is fetching the servers.
    Npm,
    Failed(&'a str),
}

struct Job {
    progress: Arc<Progress>,
    _task: Task<()>,
}

/// Installs going on, shared by every editor.
pub struct Installer {
    dir: PathBuf,
    jobs: HashMap<&'static str, Job>,
    errors: HashMap<&'static str, String>,
    /// Goes up each time an install finishes, so editors can look again.
    pub finished: u64,
}

struct GlobalInstaller(Entity<Installer>);

impl Global for GlobalInstaller {}

impl Installer {
    pub fn init(data_dir: &Path, cx: &mut App) {
        // Servers start in the project's folder, so the path must not be relative.
        let dir = data_dir.join("servers");
        let dir = std::path::absolute(&dir).unwrap_or(dir);
        let installer = cx.new(|_| Self {
            dir,
            jobs: HashMap::new(),
            errors: HashMap::new(),
            finished: 0,
        });
        cx.set_global(GlobalInstaller(installer));
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalInstaller>().map(|global| global.0.clone())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn state(&self, package: &Package) -> State<'_> {
        if let Some(job) = self.jobs.get(package.id) {
            let progress = &job.progress;
            return if progress.npm.load(Ordering::Relaxed) {
                State::Npm
            } else {
                State::Node {
                    done: progress.done.load(Ordering::Relaxed),
                    total: progress.total.load(Ordering::Relaxed),
                }
            };
        }
        self.errors.get(package.id).map_or(State::Idle, |error| State::Failed(error))
    }

    /// The download size, when Node isn't here yet.
    pub fn download_size(&self) -> Option<u64> {
        let build = node_build()?;
        (!node_program(&self.dir.join(build.folder)).is_file()).then_some(build.file.size)
    }

    pub fn start(&mut self, package: &'static Package, cx: &mut Context<Self>) {
        if self.jobs.contains_key(package.id) {
            return;
        }
        self.errors.remove(package.id);
        let progress = Arc::new(Progress::default());
        let work = runtime::io(install(self.dir.clone(), package, progress.clone()));
        let task = cx.spawn(async move |installer, cx| {
            let mut work = std::pin::pin!(work);
            let result = loop {
                let tick = cx.background_executor().timer(REFRESH);
                match futures::future::select(work.as_mut(), tick).await {
                    futures::future::Either::Left((result, _)) => break result,
                    futures::future::Either::Right(_) => {
                        let _ = installer.update(cx, |_, cx| cx.notify());
                    }
                }
            };
            let _ = installer.update(cx, |installer, cx| {
                installer.jobs.remove(package.id);
                match result {
                    Ok(()) => installer.finished += 1,
                    Err(err) => {
                        installer.errors.insert(package.id, err);
                    }
                }
                cx.notify();
            });
        });
        self.jobs.insert(package.id, Job { progress, _task: task });
        cx.notify();
    }

    pub fn cancel(&mut self, package: &Package) {
        if let Some(job) = self.jobs.get(package.id) {
            job.progress.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_common_computer_has_a_node_build() {
        for (os, arch) in
            [("macos", "aarch64"), ("macos", "x86_64"), ("linux", "x86_64"), ("linux", "aarch64"), ("windows", "x86_64")]
        {
            let build = NODE.iter().find(|build| build.os == os && build.arch == arch);
            let build = build.unwrap_or_else(|| panic!("no Node for {os} {arch}"));
            assert!(build.file.url.ends_with(if os == "windows" { ".zip" } else { ".tar.gz" }));
            assert!(build.file.url.contains(build.folder), "{}", build.folder);
            assert_eq!(build.file.sha256.len(), 64);
        }
    }

    #[test]
    fn servers_map_to_packages_and_hints() {
        assert_eq!(script_for("vscode-css-language-server").map(|(p, _)| p.id), Some("vscode-langservers"));
        assert_eq!(hint("vscode-json-language-server"), Some("npm i -g vscode-langservers-extracted"));
        assert_eq!(hint("gopls"), Some("go install golang.org/x/tools/gopls@latest"));
        assert_eq!(hint("jdtls"), None);
        // Every package is pinned to a version.
        for program in ["vscode-json-language-server", "typescript-language-server", "yaml-language-server", "pyright-langserver", "bash-language-server"] {
            let (package, _) = script_for(program).unwrap();
            assert!(package.npm.iter().all(|name| name[1..].contains('@')), "{program}");
        }
    }

    #[test]
    fn nothing_counts_as_installed_in_an_empty_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert!(installed(dir.path(), "vscode-json-language-server").is_none());
        assert!(installed(dir.path(), "rust-analyzer").is_none());
    }

    /// Downloads Node and the JSON server for real, then talks to it. Needs
    /// the network: `cargo test -p somme-desktop installs_and_starts -- --ignored`.
    #[test]
    #[ignore]
    fn installs_and_starts_the_json_server() {
        use std::io::{Read as _, Write as _};

        let dir = tempfile::tempdir().unwrap();
        let progress = Arc::new(Progress::default());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(install(dir.path().to_owned(), &LANGSERVERS, progress)).unwrap();
        // Installed twice is fine: Node is kept, npm runs again.
        runtime.block_on(install(dir.path().to_owned(), &LANGSERVERS, Arc::default())).unwrap();
        let server = installed(dir.path(), "vscode-json-language-server").expect("installed");
        let mut child = Command::new(&server.node)
            .arg(&server.script)
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":null,"capabilities":{}}}"#;
        let mut stdin = child.stdin.take().unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
        let mut reply = Vec::new();
        let mut stdout = child.stdout.take().unwrap();
        let mut buf = [0u8; 4096];
        while !String::from_utf8_lossy(&reply).contains("capabilities") {
            let n = stdout.read(&mut buf).unwrap();
            assert!(n > 0, "the server closed: {}", String::from_utf8_lossy(&reply));
            reply.extend_from_slice(&buf[..n]);
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(String::from_utf8_lossy(&reply).contains("\"id\":1"));
    }
}
