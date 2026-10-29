//! `git torrent seed --install`: runs a seed command as a per-user service
//! that starts at login and restarts on failure: a launchd agent on macOS, a
//! systemd user service on Linux. There is one such service; installing
//! again replaces it.

use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::cli::SeedArgs;
use crate::{key, seed};

/// The launchd label, and the plist's file name without `.plist`.
const LAUNCHD_LABEL: &str = "dev.git-torrent.seed";
/// The systemd unit name, without `.service`.
const SYSTEMD_UNIT: &str = "git-torrent-seed";
/// Variables the service keeps from the installing shell, when set: what git
/// and `git torrent` need to find their config and key.
const KEPT_ENV: &[&str] = &[
    "HOME",
    "PATH",
    "XDG_CONFIG_HOME",
    "GIT_TORRENT_HOME",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_NOSYSTEM",
];

/// What the service runs.
#[derive(Debug, PartialEq, Eq)]
struct Service {
    /// The program and its arguments.
    command: Vec<String>,
    /// The directory it runs in: the repository `--follow` names remotes of,
    /// and the one seeded when no `--repo` is given.
    dir: PathBuf,
    /// Environment variables, in order.
    env: Vec<(String, String)>,
    /// Where the output goes, for launchd; systemd keeps it in the journal.
    log: PathBuf,
}

pub(crate) fn run(args: &SeedArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let platform = Platform::current()?;
    if args.service.uninstall {
        return platform.uninstall(out);
    }
    let service = service(args, cwd)?;
    let file = platform.render(&service);
    if args.service.print_service {
        write!(out, "{file}")?;
        return Ok(());
    }
    platform.install(&file, out)
}

/// The service for `args`, run from `cwd`, after checking that it can start
/// unattended: its repositories and remotes exist, and the signing key, if
/// anything is signed, has no passphrase.
fn service(args: &SeedArgs, cwd: &Path) -> anyhow::Result<Service> {
    let repos = seed::repositories(args, cwd)?;
    seed::follow_targets(args, cwd)?;
    if !repos.is_empty() {
        key::require_unattended_key()?;
    }
    let dir = cwd
        .canonicalize()
        .with_context(|| format!("resolving {}", cwd.display()))?;
    let program = std::env::current_exe().context("finding the git-torrent binary")?;
    let mut command = vec![program.to_string_lossy().into_owned(), "seed".to_owned()];
    command.extend(seed_arguments(args, &dir)?);
    let env = KEPT_ENV
        .iter()
        .filter_map(|&name| Some((name.to_owned(), std::env::var(name).ok()?)))
        .collect();
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let log = Path::new(&home)
        .join("Library")
        .join("Logs")
        .join("git-torrent-seed.log");
    Ok(Service {
        command,
        dir,
        env,
        log,
    })
}

/// `args` as `git torrent seed` arguments, without the install flags, with
/// `--repo` paths made absolute against `dir`.
fn seed_arguments(args: &SeedArgs, dir: &Path) -> anyhow::Result<Vec<String>> {
    let sign = &args.sign;
    if sign.builds_on_previous() {
        bail!("--update and --delete sign once; they can't be part of a service");
    }
    let mut out = vec!["--listen".to_owned(), args.listen.to_string()];
    let mut push = |flag: &str, value: String| {
        out.push(flag.to_owned());
        out.push(value);
    };
    for repo in &args.repos {
        let path = dir.join(repo);
        let path = path
            .canonicalize()
            .with_context(|| format!("resolving {}", path.display()))?;
        push("--repo", path.to_string_lossy().into_owned());
    }
    for remote in &args.follow {
        push("--follow", remote.clone());
    }
    for address in &args.republish {
        push("--republish", address.to_string());
    }
    for node in &args.dht_bootstrap {
        push("--dht-bootstrap", node.clone());
    }
    if !sign.name.is_empty() {
        push("--name", sign.name.clone());
    }
    if let Some(upstream) = &sign.upstream {
        push("--upstream", upstream.clone());
    }
    for mirror in &sign.mirrors {
        push("--mirror", mirror.clone());
    }
    let flags = [
        (args.no_peer_dht, "--no-peer-dht"),
        (args.no_local_discovery, "--no-local-discovery"),
        (args.upnp, "--upnp"),
        (sign.all, "--all"),
    ];
    out.extend(
        flags
            .into_iter()
            .filter(|&(set, _)| set)
            .map(|(_, flag)| flag.to_owned()),
    );
    Ok(out)
}

/// The service manager of the platform this runs on.
enum Platform {
    Launchd,
    Systemd,
}

impl Platform {
    fn current() -> anyhow::Result<Self> {
        match std::env::consts::OS {
            "macos" => Ok(Platform::Launchd),
            "linux" => Ok(Platform::Systemd),
            other => bail!(
                "--install supports macOS (launchd) and Linux (systemd), not {other}; run \
                 `git torrent seed` with your platform's service manager instead"
            ),
        }
    }

    fn render(&self, service: &Service) -> String {
        match self {
            Platform::Launchd => launchd_plist(service),
            Platform::Systemd => systemd_unit(service),
        }
    }

    fn path(&self) -> anyhow::Result<PathBuf> {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
        Ok(match self {
            Platform::Launchd => home
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{LAUNCHD_LABEL}.plist")),
            Platform::Systemd => std::env::var_os("XDG_CONFIG_HOME")
                .map_or_else(|| home.join(".config"), PathBuf::from)
                .join("systemd")
                .join("user")
                .join(format!("{SYSTEMD_UNIT}.service")),
        })
    }

    fn install(&self, file: &str, out: &mut dyn Write) -> anyhow::Result<()> {
        let path = self.path()?;
        let replacing = path.exists();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        match self {
            Platform::Launchd => {
                let domain = launchd_domain()?;
                // Stop the running agent, if any, so the new file takes effect.
                let _ = Command::new("launchctl")
                    .args(["bootout", &format!("{domain}/{LAUNCHD_LABEL}")])
                    .output();
                fs::write(&path, file)?;
                run_tool(
                    Command::new("launchctl")
                        .args(["bootstrap", &domain])
                        .arg(&path),
                )?;
            }
            Platform::Systemd => {
                fs::write(&path, file)?;
                run_tool(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
                run_tool(Command::new("systemctl").args(["--user", "enable", SYSTEMD_UNIT]))?;
                run_tool(Command::new("systemctl").args(["--user", "restart", SYSTEMD_UNIT]))?;
            }
        }
        writeln!(
            out,
            "{} the seeder service: {}",
            if replacing { "Replaced" } else { "Installed" },
            path.display()
        )?;
        match self {
            Platform::Launchd => writeln!(
                out,
                "It starts at login and restarts on failure. Status: `launchctl print {}/{LAUNCHD_LABEL}`; \
                 log: ~/Library/Logs/git-torrent-seed.log",
                launchd_domain()?
            )?,
            Platform::Systemd => writeln!(
                out,
                "It starts at login and restarts on failure. Log: `journalctl --user -u \
                 {SYSTEMD_UNIT} -f`. To keep it running while you are logged out: \
                 `loginctl enable-linger $USER`"
            )?,
        }
        writeln!(out, "`git torrent seed --uninstall` removes it.")?;
        Ok(())
    }

    fn uninstall(&self, out: &mut dyn Write) -> anyhow::Result<()> {
        let path = self.path()?;
        if !path.exists() {
            writeln!(out, "No seeder service is installed ({}).", path.display())?;
            return Ok(());
        }
        match self {
            Platform::Launchd => {
                let _ = Command::new("launchctl")
                    .args(["bootout", &format!("{}/{LAUNCHD_LABEL}", launchd_domain()?)])
                    .output();
                fs::remove_file(&path)?;
            }
            Platform::Systemd => {
                let _ = Command::new("systemctl")
                    .args(["--user", "disable", "--now", SYSTEMD_UNIT])
                    .output();
                fs::remove_file(&path)?;
                run_tool(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
            }
        }
        writeln!(out, "Removed the seeder service ({}).", path.display())?;
        Ok(())
    }
}

/// `gui/<uid>`: the launchd domain of the logged-in user's agents.
fn launchd_domain() -> anyhow::Result<String> {
    let output = Command::new("id").arg("-u").output()?;
    let uid = String::from_utf8(output.stdout)?;
    Ok(format!("gui/{}", uid.trim()))
}

fn run_tool(command: &mut Command) -> anyhow::Result<()> {
    let output = command
        .output()
        .with_context(|| format!("running {command:?}"))?;
    if !output.status.success() {
        bail!(
            "{command:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn launchd_plist(service: &Service) -> String {
    let string = |value: &str| format!("<string>{}</string>", xml_escape(value));
    let mut arguments = String::new();
    for argument in &service.command {
        let _ = writeln!(arguments, "    {}", string(argument));
    }
    let mut env = String::new();
    for (name, value) in &service.env {
        let _ = writeln!(
            env,
            "    <key>{}</key>\n    {}",
            xml_escape(name),
            string(value)
        );
    }
    let log = string(&service.log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- Written by `git torrent seed --install`; `git torrent seed --uninstall` removes it. -->
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{arguments}  </array>
  <key>WorkingDirectory</key>
  {dir}
  <key>EnvironmentVariables</key>
  <dict>
{env}  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>30</integer>
  <key>StandardOutPath</key>
  {log}
  <key>StandardErrorPath</key>
  {log}
</dict>
</plist>
"#,
        dir = string(&service.dir.to_string_lossy()),
    )
}

fn systemd_unit(service: &Service) -> String {
    let command = service
        .command
        .iter()
        .map(|argument| systemd_quote(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let mut env = String::new();
    for (name, value) in &service.env {
        let _ = writeln!(
            env,
            "Environment={}",
            systemd_quote(&format!("{name}={value}"))
        );
    }
    format!(
        "# Written by `git torrent seed --install`; `git torrent seed --uninstall` removes it.\n\
         [Unit]\n\
         Description=git-remote-torrent seeder\n\
         Wants=network-online.target\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={command}\n\
         WorkingDirectory={dir}\n\
         {env}\
         # `seed` removes its pid files and stops cleanly on SIGINT.\n\
         KillSignal=SIGINT\n\
         Restart=on-failure\n\
         RestartSec=30\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        dir = systemd_quote(&service.dir.to_string_lossy()),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// One word for a systemd directive: double-quoted, with `\` and `"`
/// escaped, and `%` and `$` doubled so systemd expands neither.
fn systemd_quote(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Service {
        Service {
            command: vec![
                "/opt/my tools/git-torrent".to_owned(),
                "seed".to_owned(),
                "--follow".to_owned(),
                "origin".to_owned(),
            ],
            dir: PathBuf::from("/home/a <b>/src/project"),
            env: vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())],
            log: PathBuf::from("/home/a/Library/Logs/git-torrent-seed.log"),
        }
    }

    #[test]
    fn plists_escape_and_list_every_argument() {
        let plist = launchd_plist(&sample());
        assert!(plist.contains("<string>/opt/my tools/git-torrent</string>"));
        assert!(plist.contains("<string>--follow</string>\n    <string>origin</string>"));
        assert!(plist.contains("<string>/home/a &lt;b&gt;/src/project</string>"));
        assert!(plist.contains("<key>PATH</key>\n    <string>/usr/bin:/bin</string>"));
        assert!(plist.contains(&format!("<string>{LAUNCHD_LABEL}</string>")));
    }

    #[test]
    fn units_quote_every_word() {
        let unit = systemd_unit(&sample());
        assert!(
            unit.contains(
                "ExecStart=\"/opt/my tools/git-torrent\" \"seed\" \"--follow\" \"origin\"\n"
            ),
            "{unit}"
        );
        assert!(unit.contains("WorkingDirectory=\"/home/a <b>/src/project\"\n"));
        assert!(unit.contains("Environment=\"PATH=/usr/bin:/bin\"\n"));
        assert_eq!(systemd_quote(r#"100% "$x" \"#), r#""100%% \"$$x\" \\""#);
    }
}
