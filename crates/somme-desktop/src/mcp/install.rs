//! Sets `Somme --service` to start at login, so agents keep working while the
//! app is closed: a launchd agent on macOS, a systemd user service on Linux,
//! and a Run entry on Windows. Test data folders get the files written next
//! to them instead, and nothing is run. The steps are also used to show
//! Somme in the system's "Open With" lists (see `open_with`).

use std::path::{Path, PathBuf};
use std::process::Command;

use super::service::SERVICE_ARG;

const LABEL: &str = "app.somme.agents";
const UNIT: &str = "somme-agents.service";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Somme Agents";
/// What was last set up, so it is done again only when it changes.
const MARKER: &str = "service-installed.txt";

/// Where and what to install.
pub struct Target {
    pub exe: PathBuf,
    pub home: PathBuf,
    /// The agents folder, which also keeps the service's log.
    pub agents_dir: PathBuf,
    /// Passed on to the service, for test data folders only.
    pub data_dir: Option<PathBuf>,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub uid: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Write(PathBuf, String),
    /// Writes text as UTF-16, as Windows tools like `reg import` want it.
    #[cfg_attr(not(windows), allow(dead_code))]
    WriteUtf16(PathBuf, String),
    Remove(PathBuf),
    /// Runs a program and waits. `may_fail` is for undoing what may not be there.
    Run {
        program: String,
        args: Vec<String>,
        may_fail: bool,
    },
    /// Starts a program and leaves it running.
    Start { program: String, args: Vec<String> },
}

pub fn run(program: &str, args: &[&str], may_fail: bool) -> Step {
    Step::Run {
        program: program.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        may_fail,
    }
}

pub fn describe(steps: &[Step]) -> String {
    steps
        .iter()
        .map(|step| match step {
            Step::Write(path, text) | Step::WriteUtf16(path, text) => format!("write {}\n{text}\n", path.display()),
            Step::Remove(path) => format!("remove {}\n", path.display()),
            Step::Run { program, args, .. } => format!("run {program} {}\n", args.join(" ")),
            Step::Start { program, args } => format!("start {program} {}\n", args.join(" ")),
        })
        .collect()
}

pub fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn plist_path(target: &Target) -> PathBuf {
    target.home.join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

pub fn launchd(target: &Target, on: bool) -> Vec<Step> {
    let domain = format!("gui/{}", target.uid);
    let job = format!("{domain}/{LABEL}");
    let path = plist_path(target);
    if !on {
        return vec![run("launchctl", &["bootout", &job], true), Step::Remove(path)];
    }
    let env = target
        .data_dir
        .as_ref()
        .map(|dir| {
            format!(
                "\t<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>SOMME_DATA_DIR</key>\n\t\t<string>{}</string>\n\t</dict>\n",
                xml(&dir.display().to_string())
            )
        })
        .unwrap_or_default();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{exe}</string>
		<string>{SERVICE_ARG}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ProcessType</key>
	<string>Background</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
{env}</dict>
</plist>
"#,
        exe = xml(&target.exe.display().to_string()),
        log = xml(&target.agents_dir.join("service.log").display().to_string()),
    );
    let plist_arg = path.display().to_string();
    vec![
        Step::Write(path, plist),
        // Loading it again picks up a moved app.
        run("launchctl", &["bootout", &job], true),
        run("launchctl", &["bootstrap", &domain, &plist_arg], false),
    ]
}

/// `text` as one word in a systemd line.
fn systemd_word(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', r"\\").replace('"', "\\\"").replace('%', "%%"))
}

pub fn systemd(target: &Target, on: bool) -> Vec<Step> {
    let path = target.home.join(".config/systemd/user").join(UNIT);
    if !on {
        return vec![
            run("systemctl", &["--user", "disable", "--now", UNIT], true),
            Step::Remove(path),
            run("systemctl", &["--user", "daemon-reload"], true),
        ];
    }
    let env = target
        .data_dir
        .as_ref()
        .map(|dir| format!("Environment={}\n", systemd_word(&format!("SOMME_DATA_DIR={}", dir.display()))))
        .unwrap_or_default();
    let unit = format!(
        "[Unit]\nDescription=Somme agents (MCP server)\n\n[Service]\nExecStart={} {SERVICE_ARG}\n{env}Restart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        systemd_word(&target.exe.display().to_string())
    );
    vec![
        Step::Write(path, unit),
        run("systemctl", &["--user", "daemon-reload"], false),
        run("systemctl", &["--user", "enable", UNIT], false),
        // Starts it, or starts it again with a moved app.
        run("systemctl", &["--user", "restart", UNIT], false),
    ]
}

/// The service stops by itself once the setting is off, so turning it off
/// only removes it from login.
pub fn windows(target: &Target, on: bool) -> Vec<Step> {
    if !on {
        return vec![run("reg", &["delete", RUN_KEY, "/v", RUN_VALUE, "/f"], true)];
    }
    let exe = target.exe.display().to_string();
    let command = format!("\"{exe}\" {SERVICE_ARG}");
    vec![
        run("reg", &["add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &command, "/f"], false),
        Step::Start {
            program: exe,
            args: vec![SERVICE_ARG.to_owned()],
        },
    ]
}

pub fn steps(target: &Target, on: bool) -> Vec<Step> {
    if cfg!(target_os = "macos") {
        launchd(target, on)
    } else if cfg!(windows) {
        windows(target, on)
    } else {
        systemd(target, on)
    }
}

pub fn target(agents_dir: &Path) -> anyhow::Result<Target> {
    #[cfg(unix)]
    let uid = {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::metadata(agents_dir)?.uid()
    };
    #[cfg(not(unix))]
    let uid = 0;
    Ok(Target {
        exe: std::env::current_exe()?,
        home: dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot find the home folder"))?,
        agents_dir: agents_dir.to_owned(),
        data_dir: std::env::var_os("SOMME_DATA_DIR").map(PathBuf::from),
        uid,
    })
}

/// Makes the login service match `on`. Blocks while the system's tools run.
pub fn apply(agents_dir: &Path, on: bool) -> anyhow::Result<()> {
    let target = target(agents_dir)?;
    let steps = steps(&target, on);
    let marker = agents_dir.join(MARKER);
    let wanted = on.then(|| describe(&steps));
    if std::fs::read_to_string(&marker).ok() == wanted {
        return Ok(());
    }
    if target.data_dir.is_some() {
        dry_run(&agents_dir.join("service-files"), &steps)?;
    } else {
        for step in &steps {
            perform(step)?;
        }
    }
    match wanted {
        Some(text) => std::fs::write(&marker, text)?,
        None => drop(std::fs::remove_file(&marker)),
    }
    Ok(())
}

/// Shows what would be done, in the folder `out`, for test data folders.
pub fn dry_run(out: &Path, steps: &[Step]) -> anyhow::Result<()> {
    std::fs::create_dir_all(out)?;
    for step in steps {
        if let Step::Write(path, text) | Step::WriteUtf16(path, text) = step
            && let Some(name) = path.file_name()
        {
            std::fs::write(out.join(name), text)?;
        }
    }
    std::fs::write(out.join("steps.txt"), describe(steps))?;
    Ok(())
}

pub fn perform(step: &Step) -> anyhow::Result<()> {
    match step {
        Step::Write(path, text) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, text)?;
        }
        Step::WriteUtf16(path, text) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let bytes: Vec<u8> = [0xfeff].into_iter().chain(text.encode_utf16()).flat_map(u16::to_le_bytes).collect();
            std::fs::write(path, bytes)?;
        }
        Step::Remove(path) => match std::fs::remove_file(path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
            _ => {}
        },
        Step::Run { program, args, may_fail } => {
            let out = Command::new(program).args(args).output()?;
            if !out.status.success() && !may_fail {
                let said = String::from_utf8_lossy(&out.stderr);
                anyhow::bail!("{program} {} failed: {}", args.join(" "), said.trim());
            }
        }
        Step::Start { program, args } => {
            let mut command = Command::new(program);
            command
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt as _;
                const DETACHED_PROCESS: u32 = 0x0000_0008;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
            }
            command.spawn()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target {
            exe: PathBuf::from("/Apps/Somme & Co.app/Contents/MacOS/Somme"),
            home: PathBuf::from("/home/me"),
            agents_dir: PathBuf::from("/data/agents"),
            data_dir: None,
            uid: 501,
        }
    }

    fn text(steps: &[Step]) -> &str {
        steps
            .iter()
            .find_map(|step| match step {
                Step::Write(_, text) => Some(text.as_str()),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn launchd_starts_at_login_and_again_after_a_crash() {
        let steps = launchd(&target(), true);
        let plist = text(&steps);
        assert!(plist.contains("<string>/Apps/Somme &amp; Co.app/Contents/MacOS/Somme</string>"));
        assert!(plist.contains("<string>--service</string>"));
        assert!(plist.contains("<key>SuccessfulExit</key>\n\t\t<false/>"));
        assert!(plist.contains("<string>/data/agents/service.log</string>"));
        assert!(!plist.contains("SOMME_DATA_DIR"));
        assert_eq!(
            steps[2],
            run("launchctl", &["bootstrap", "gui/501", "/home/me/Library/LaunchAgents/app.somme.agents.plist"], false)
        );
        let off = launchd(&target(), false);
        assert_eq!(off[0], run("launchctl", &["bootout", "gui/501/app.somme.agents"], true));
        assert_eq!(off[1], Step::Remove("/home/me/Library/LaunchAgents/app.somme.agents.plist".into()));
    }

    #[test]
    fn systemd_quotes_the_program() {
        let mut target = target();
        target.exe = PathBuf::from("/opt/my \"somme\"/100%/somme");
        target.data_dir = Some("/tmp/data".into());
        let unit = text(&systemd(&target, true)).to_owned();
        assert!(unit.contains(r#"ExecStart="/opt/my \"somme\"/100%%/somme" --service"#), "{unit}");
        assert!(unit.contains(r#"Environment="SOMME_DATA_DIR=/tmp/data""#), "{unit}");
        assert!(unit.contains("Restart=on-failure"));
    }

    #[test]
    fn windows_runs_at_login() {
        let mut target = target();
        target.exe = PathBuf::from(r"C:\Program Files\Somme\Somme.exe");
        let steps = windows(&target, true);
        let Step::Run { args, .. } = &steps[0] else { panic!() };
        assert_eq!(args[7], r#""C:\Program Files\Somme\Somme.exe" --service"#);
        assert!(matches!(&steps[1], Step::Start { args, .. } if args == &["--service"]));
    }

    #[test]
    fn test_folders_only_get_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let steps = steps(&target(), true);
        dry_run(&dir.path().join("service-files"), &steps).unwrap();
        assert!(std::fs::read_to_string(dir.path().join("service-files/steps.txt")).unwrap().contains("run "));
    }
}
