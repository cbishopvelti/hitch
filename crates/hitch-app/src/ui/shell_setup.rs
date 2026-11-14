//! Setting up the Hitch GNOME Shell extension from the panel.
//!
//! On GNOME Wayland only the extension (`integrations/gnome-shell`) can tell
//! Hitch which window has focus and keep the pinned panel on top. When it
//! isn't on the session bus, the panel works out why ([`Status::check`]) and
//! lists what's left to do ([`Status::steps`]): install it, turn it on, turn
//! on extensions for the account, log out and back in. Each step has the
//! command that does it, to copy, and most have a button that does it from
//! Hitch.
//!
//! The extension's files are built into Hitch, so the Install button works
//! from any build, not only from a checkout of the repo. It writes them where
//! `make -C integrations/gnome-shell install` does.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The extension's UUID, which is also its folder name.
pub const UUID: &str = "hitch@cbishopvelti.github.io";

const METADATA: &str =
    include_str!("../../../../integrations/gnome-shell/hitch@cbishopvelti.github.io/metadata.json");
const EXTENSION: &str =
    include_str!("../../../../integrations/gnome-shell/hitch@cbishopvelti.github.io/extension.js");

/// What's in the way of the extension running.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// The desktop is GNOME (`XDG_CURRENT_DESKTOP`).
    pub gnome: bool,
    /// Where the extension is installed, if it is.
    pub installed: Option<PathBuf>,
    /// The installed copy differs from the one built into this Hitch.
    pub outdated: bool,
    /// It is in `org.gnome.shell enabled-extensions`. `None` if `gsettings`
    /// couldn't say.
    pub enabled: Option<bool>,
    /// `org.gnome.shell disable-user-extensions` is on.
    pub user_extensions_off: bool,
    /// The shell's state for it (`gnome-extensions info`), e.g. `ACTIVE`,
    /// `ERROR`, `OUT OF DATE`. `None` when the running shell doesn't know
    /// it, which on Wayland means it was installed since you logged in.
    pub state: Option<String>,
    /// The running GNOME Shell's version, e.g. `51.0`.
    pub shell_version: Option<String>,
}

/// Something the panel can do for you.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// Write the extension's files and turn it on.
    Install,
    /// Turn it on.
    Enable,
    /// Turn off `disable-user-extensions`.
    AllowUserExtensions,
}

impl Fix {
    pub fn label(self) -> &'static str {
        match self {
            Fix::Install => "Install",
            Fix::Enable => "Turn on",
            Fix::AllowUserExtensions => "Turn on",
        }
    }
}

/// One thing to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub text: String,
    /// The command that does it, to copy into a terminal.
    pub command: Option<String>,
    /// The button that does it.
    pub fix: Option<Fix>,
}

impl Status {
    /// Looks at the desktop, the extension's folders and GNOME's settings.
    pub fn check() -> Self {
        let gnome = std::env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("gnome")));
        let user_dir = user_dir();
        let installed = user_dir
            .iter()
            .cloned()
            .chain([PathBuf::from("/usr/share/gnome-shell/extensions").join(UUID)])
            .find(|dir| dir.join("metadata.json").is_file());
        let outdated = match (&installed, &user_dir) {
            (Some(dir), Some(user)) if dir == user => !matches_built_in(dir),
            _ => false,
        };
        let enabled = gsettings_get("enabled-extensions").map(|v| parse_strv(&v).contains(&UUID));
        let user_extensions_off =
            gsettings_get("disable-user-extensions").is_some_and(|v| v.trim() == "true");
        let state = run("gnome-extensions", &["info", UUID]).and_then(|out| parse_state(&out));
        let shell_version = run("gnome-shell", &["--version"])
            .and_then(|out| out.trim().rsplit(' ').next().map(str::to_string));
        Status {
            gnome,
            installed,
            outdated,
            enabled,
            user_extensions_off,
            state,
            shell_version,
        }
    }

    /// The first line of the notice.
    pub fn headline(&self) -> &'static str {
        if self.gnome {
            "Hitch can't see which window has focus or keep the panel on top: \
             its GNOME Shell extension isn't running."
        } else {
            "Hitch can't see which window has focus. On Wayland only GNOME Shell, \
             with the Hitch extension, can tell it. Editors and terminals with a \
             Hitch plugin still work."
        }
    }

    /// What's left to do, in order. Empty off GNOME, where the extension
    /// can't help.
    pub fn steps(&self, repo: Option<&Path>) -> Vec<Step> {
        if !self.gnome {
            return Vec::new();
        }
        let enable = format!("gnome-extensions enable {UUID}");
        let make = repo.map(|repo| {
            format!(
                "make -C {} install",
                shell_quote(&repo.join("integrations/gnome-shell"))
            )
        });
        let mut steps = Vec::new();
        if self.installed.is_none() {
            steps.push(Step {
                text: "Install the extension and turn it on.".into(),
                command: make.map(|make| format!("{make} && {enable}")),
                fix: Some(Fix::Install),
            });
        } else {
            if self.outdated {
                steps.push(Step {
                    text: "The installed extension is older than this Hitch. Update it.".into(),
                    command: make,
                    fix: Some(Fix::Install),
                });
            }
            if self.enabled != Some(true) {
                steps.push(Step {
                    text: "Turn the extension on.".into(),
                    command: Some(enable),
                    fix: Some(Fix::Enable),
                });
            }
        }
        if self.user_extensions_off {
            steps.push(Step {
                text: "Extensions are switched off for your account. Turn them on.".into(),
                command: Some("gsettings set org.gnome.shell disable-user-extensions false".into()),
                fix: Some(Fix::AllowUserExtensions),
            });
        }
        match self.state.as_deref() {
            Some("ERROR") => steps.push(Step {
                text: "GNOME Shell couldn't start the extension. Its log says why:".into(),
                command: Some("journalctl -b _COMM=gnome-shell | grep -i hitch".into()),
                fix: None,
            }),
            Some("OUT OF DATE") => steps.push(Step {
                text: format!(
                    "This GNOME Shell{} isn't a version the extension supports ({}).",
                    self.shell_version
                        .as_ref()
                        .map(|v| format!(" ({v})"))
                        .unwrap_or_default(),
                    supported_versions().join(", ")
                ),
                command: None,
                fix: None,
            }),
            // The shell knows it: turning it on starts it with no log out.
            Some(_) if !steps.is_empty() && !self.outdated => {}
            // The shell doesn't know it yet, or loaded an old copy.
            _ => steps.push(Step {
                text: "Log out and back in. GNOME on Wayland only loads a new or \
                       updated extension when you log in."
                    .into(),
                command: None,
                fix: None,
            }),
        }
        steps
    }
}

/// Does `fix`.
pub fn apply(fix: Fix) -> io::Result<()> {
    match fix {
        Fix::Install => {
            let dir = user_dir().ok_or_else(|| io::Error::other("HOME is not set"))?;
            install_into(&dir)?;
            enable()
        }
        Fix::Enable => enable(),
        Fix::AllowUserExtensions => gsettings_set("disable-user-extensions", "false"),
    }
}

/// The repo checkout this Hitch was built from, if it is still there, for
/// the `make` command.
pub fn repo() -> Option<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repo = repo.canonicalize().ok()?;
    repo.join("integrations/gnome-shell/Makefile")
        .is_file()
        .then_some(repo)
}

/// `~/.local/share/gnome-shell/extensions/<uuid>`.
fn user_dir() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("gnome-shell/extensions").join(UUID))
}

fn install_into(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, contents) in [("metadata.json", METADATA), ("extension.js", EXTENSION)] {
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, contents)?;
        std::fs::rename(&tmp, dir.join(name))?;
    }
    Ok(())
}

fn matches_built_in(dir: &Path) -> bool {
    [("metadata.json", METADATA), ("extension.js", EXTENSION)]
        .iter()
        .all(|(name, contents)| {
            std::fs::read_to_string(dir.join(name)).is_ok_and(|c| c == *contents)
        })
}

/// Adds the extension to `enabled-extensions` and takes it out of
/// `disabled-extensions`, as `gnome-extensions enable` does. This works
/// before the shell has loaded the extension, so it's on after you log in.
fn enable() -> io::Result<()> {
    let enabled = gsettings_get("enabled-extensions")
        .ok_or_else(|| io::Error::other("can't read GNOME's settings (gsettings)"))?;
    let mut list = parse_strv(&enabled);
    if !list.contains(&UUID) {
        list.push(UUID);
        gsettings_set("enabled-extensions", &format_strv(&list))?;
    }
    if let Some(disabled) = gsettings_get("disabled-extensions") {
        let list = parse_strv(&disabled);
        if list.contains(&UUID) {
            let rest: Vec<&str> = list.into_iter().filter(|u| *u != UUID).collect();
            gsettings_set("disabled-extensions", &format_strv(&rest))?;
        }
    }
    Ok(())
}

fn gsettings_get(key: &str) -> Option<String> {
    run("gsettings", &["get", "org.gnome.shell", key])
}

fn gsettings_set(key: &str, value: &str) -> io::Result<()> {
    let out = Command::new("gsettings")
        .args(["set", "org.gnome.shell", key, value])
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "gsettings set {key} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Runs a command and returns what it printed, if it succeeded.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The strings of a GVariant string array as `gsettings get` prints it:
/// `['a@b', 'c@d']` or `@as []`. Extension UUIDs have no quotes in them.
fn parse_strv(value: &str) -> Vec<&str> {
    value.split('\'').skip(1).step_by(2).collect()
}

fn format_strv(list: &[&str]) -> String {
    let items: Vec<String> = list.iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", items.join(", "))
}

/// The `State:` line of `gnome-extensions info`.
fn parse_state(info: &str) -> Option<String> {
    info.lines()
        .find_map(|line| line.trim().strip_prefix("State:"))
        .map(|state| state.trim().to_string())
}

/// The GNOME Shell versions the extension's metadata lists.
fn supported_versions() -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(METADATA)
        .ok()
        .and_then(|m| {
            m["shell-version"].as_array().map(|versions| {
                versions
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn shell_quote(path: &Path) -> String {
    let path = path.to_string_lossy();
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+@~".contains(c))
    {
        path.into_owned()
    } else {
        format!("'{}'", path.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gnome() -> Status {
        Status {
            gnome: true,
            ..Status::default()
        }
    }

    fn texts(steps: &[Step]) -> Vec<&str> {
        steps.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn parses_gsettings_arrays() {
        assert_eq!(parse_strv("@as []\n"), Vec::<&str>::new());
        assert_eq!(
            parse_strv("['ding@rastersoft.com', 'hitch@cbishopvelti.github.io']\n"),
            ["ding@rastersoft.com", UUID]
        );
        assert_eq!(
            format_strv(&["a@b", UUID]),
            "['a@b', 'hitch@cbishopvelti.github.io']"
        );
        assert_eq!(format_strv(&[]), "[]");
    }

    #[test]
    fn parses_info_state() {
        let info =
            "hitch@cbishopvelti.github.io\n  Name: Hitch\n  Enabled: Yes\n  State: OUT OF DATE\n";
        assert_eq!(parse_state(info).as_deref(), Some("OUT OF DATE"));
        assert_eq!(parse_state("nothing"), None);
    }

    #[test]
    fn reads_supported_versions_from_the_metadata() {
        assert!(!supported_versions().is_empty());
    }

    #[test]
    fn not_installed_installs_then_logs_out() {
        let repo = Path::new("/home/chris/playground/hitch");
        let steps = gnome().steps(Some(repo));
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].fix, Some(Fix::Install));
        assert_eq!(
            steps[0].command.as_deref(),
            Some(
                "make -C /home/chris/playground/hitch/integrations/gnome-shell install \
                 && gnome-extensions enable hitch@cbishopvelti.github.io"
            )
        );
        assert!(steps[1].text.starts_with("Log out"));
        // Without the checkout there's only the button.
        assert_eq!(gnome().steps(None)[0].command, None);
    }

    #[test]
    fn installed_since_login_and_enabled_only_needs_a_log_out() {
        let status = Status {
            installed: Some(PathBuf::from("/x")),
            enabled: Some(true),
            ..gnome()
        };
        assert_eq!(
            texts(&status.steps(None)),
            [
                "Log out and back in. GNOME on Wayland only loads a new or updated \
              extension when you log in."
            ]
        );
    }

    #[test]
    fn known_but_off_just_needs_turning_on() {
        let status = Status {
            installed: Some(PathBuf::from("/x")),
            enabled: Some(false),
            state: Some("INACTIVE".into()),
            ..gnome()
        };
        let steps = status.steps(None);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].fix, Some(Fix::Enable));
        assert_eq!(
            steps[0].command.as_deref(),
            Some("gnome-extensions enable hitch@cbishopvelti.github.io")
        );
    }

    #[test]
    fn user_extensions_off_and_errors() {
        let status = Status {
            installed: Some(PathBuf::from("/x")),
            enabled: Some(true),
            user_extensions_off: true,
            state: Some("ERROR".into()),
            ..gnome()
        };
        let steps = status.steps(None);
        assert_eq!(steps[0].fix, Some(Fix::AllowUserExtensions));
        assert!(steps[1].text.contains("couldn't start"));
        assert_eq!(steps.len(), 2);

        let status = Status {
            installed: Some(PathBuf::from("/x")),
            enabled: Some(true),
            state: Some("OUT OF DATE".into()),
            shell_version: Some("52.1".into()),
            ..gnome()
        };
        assert!(status.steps(None)[0].text.contains("(52.1)"));
    }

    #[test]
    fn outdated_copy_is_updated_then_reloaded() {
        let status = Status {
            installed: Some(PathBuf::from("/x")),
            outdated: true,
            enabled: Some(true),
            state: Some("ACTIVE".into()),
            ..gnome()
        };
        let steps = status.steps(None);
        assert_eq!(steps[0].fix, Some(Fix::Install));
        assert!(steps[1].text.starts_with("Log out"));
    }

    #[test]
    fn off_gnome_has_nothing_to_do() {
        assert!(Status::default().steps(None).is_empty());
        assert!(Status::default().headline().contains("GNOME Shell"));
    }

    #[test]
    fn install_writes_the_built_in_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(UUID);
        install_into(&dir).unwrap();
        assert!(matches_built_in(&dir));
        std::fs::write(dir.join("extension.js"), "old").unwrap();
        assert!(!matches_built_in(&dir));
    }

    #[test]
    fn quotes_paths_with_spaces() {
        assert_eq!(shell_quote(Path::new("/a b/c")), "'/a b/c'");
        assert_eq!(shell_quote(Path::new("/a/b")), "/a/b");
    }
}
