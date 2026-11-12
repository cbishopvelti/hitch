//! Short labels for contexts, sized for a 320 px panel.

use std::path::Path;

use hitch_protocol::Context;

/// The section label: the folder name, `project / file`, the domain, or the
/// app's name.
pub fn context(context: &Context) -> String {
    match context {
        Context::File { project, file } => {
            format!("{} / {}", folder_name(project), file.display())
        }
        Context::Folder { path } => folder_name(path),
        Context::Url { url } => host(url).to_string(),
        Context::App { app_id } => app_name(app_id).to_string(),
    }
}

/// What kind of context this is, shown small next to the label.
pub fn kind(context: &Context) -> &'static str {
    match context {
        Context::File { .. } => "file",
        Context::Folder { .. } => "folder",
        Context::Url { .. } => "site",
        Context::App { .. } => "app",
    }
}

/// `~/code/hitch` style path for "Hitching to:".
pub fn tilde(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty())
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| tilde(path))
}

/// The host of a URL, without scheme, credentials, port or path.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host.starts_with('[') {
        // IPv6 literal: keep the brackets, drop the port.
        return host.split_inclusive(']').next().unwrap_or(host);
    }
    host.split(':').next().unwrap_or(host)
}

/// `dev.zed.Zed` -> `Zed`, `firefox` -> `firefox`.
fn app_name(app_id: &str) -> &str {
    app_id.rsplit('.').next().unwrap_or(app_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(
            context(&Context::File {
                project: "/home/u/code/hitch".into(),
                file: "src/main.rs".into()
            }),
            "hitch / src/main.rs"
        );
        assert_eq!(
            context(&Context::Folder {
                path: "/home/u/playground/boodle".into()
            }),
            "boodle"
        );
        assert_eq!(
            context(&Context::App {
                app_id: "dev.zed.Zed".into()
            }),
            "Zed"
        );
        assert_eq!(
            context(&Context::Url {
                url: "https://user:pw@docs.rs:443/iced?x#y".into()
            }),
            "docs.rs"
        );
    }

    #[test]
    fn hosts() {
        assert_eq!(host("example.com/a"), "example.com");
        assert_eq!(host("http://[::1]:4000/"), "[::1]");
        assert_eq!(host("http://localhost:4000"), "localhost");
    }
}
