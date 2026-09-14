//! Icon URLs and the icon mappings.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Cache-busting `?v=` suffix; bump it when an existing icon changes.
pub const ICONS_VERSION: &str = "26";
pub const ICONS_URL: &str =
    "https://raw.githubusercontent.com/NikSneMC/cord-icons/master/icons/";
/// A Discord application for an editor or editor distribution.
pub struct Client {
    pub name: &'static str,
    pub id: &'static str,
    /// Icon name, resolved with the theme and flavor.
    pub icon: &'static str,
    pub tooltip: &'static str,
}

pub const CLIENTS: &[Client] = &[
    Client {
        name: "helix",
        id: "1549114006269075457",
        icon: "helix",
        tooltip: "Helix",
    },
    Client {
        name: "nova",
        id: "1549114532323000512",
        icon: "nova",
        tooltip: "Yazelix Nova",
    },
];

pub fn client(name: &str) -> Option<&'static Client> {
    CLIENTS.iter().find(|c| c.name == name)
}

/// Yazelix Nova if `YAZELIX_HELIX_BRIDGE` or `YAZELIX_STATE_DIR` is set, Helix
/// otherwise.
pub fn detect_client() -> &'static Client {
    let nova = client("nova").filter(|_| {
        super::util::env("YAZELIX_HELIX_BRIDGE").is_some()
            || super::util::env("YAZELIX_STATE_DIR").is_some()
    });
    nova.or_else(|| client("helix"))
        .expect("the helix client exists")
}
pub const DEFAULT_IDLE_ICON: &str = "idle";

pub const THEMES: &[&str] = &[
    "default",
    "atom",
    "catppuccin",
    "minecraft",
    "void",
    "classic",
];
pub const FLAVORS: &[&str] = &["dark", "light", "accent"];

/// Theme and flavor of the current configuration, for `cord-icon`.
pub static THEME: std::sync::RwLock<(String, String)> =
    std::sync::RwLock::new((String::new(), String::new()));

pub fn icon_url(name: &str, theme: &str, flavor: &str) -> String {
    format!("{ICONS_URL}{theme}/{flavor}/{name}.png?v={ICONS_VERSION}")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub ty: String,
    pub icon: String,
    pub tooltip: String,
}

type Entry = (&'static str, &'static str, &'static str);

/// Mappings by kind (`filetype`, `filename`, `extension`, `cord`, `cli`,
/// `default`) and key.
static MAPPINGS: LazyLock<HashMap<(&str, &str), Entry>> = LazyLock::new(|| {
    include_str!("mappings.tsv")
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut f = line.split('\t');
            let kind = f.next()?;
            let key = f.next()?;
            Some(((kind, key), (f.next()?, f.next()?, f.next().unwrap_or(""))))
        })
        .collect()
});

fn get(kind: &str, key: &str) -> Option<Found> {
    MAPPINGS
        .get(&(kind, key))
        .map(|&(ty, icon, tooltip)| Found {
            ty: ty.to_string(),
            icon: icon.to_string(),
            tooltip: tooltip.to_string(),
        })
}

/// Mapping of a command focused next to Helix.
pub fn lookup_cli(command: &str) -> Option<Found> {
    get("cli", command)
}

pub fn default_icon(ty: &str) -> &'static str {
    MAPPINGS
        .get(&("default", ty))
        .or_else(|| MAPPINGS.get(&("default", "language")))
        .map_or("text", |(_, icon, _)| icon)
}

pub struct Lookup<'a> {
    pub filetype: &'a str,
    pub filename: &'a str,
    pub force_fileext: Option<&'a str>,
    pub force_filetype: Option<&'a str>,
    /// A command focused next to Helix (see the multiplexer extensions).
    pub force_cli: Option<&'a str>,
    pub match_in_mappings: bool,
}

/// Mapping for a buffer, or `None` if nothing matched.
pub fn lookup(q: Lookup<'_>) -> Option<Found> {
    if let Some(cli) = q.force_cli {
        return get("cli", cli);
    }
    if let Some(ext) = q.force_fileext {
        return get("extension", ext);
    }
    if let Some(ft) = q.force_filetype {
        return get("filetype", ft);
    }
    get("filename", &q.filename.to_lowercase())
        .or_else(|| {
            q.match_in_mappings
                .then(|| super::util::lua_extension(q.filename))
                .flatten()
                .and_then(|ext| get("extension", ext))
        })
        .or_else(|| get("filetype", q.filetype))
        .or_else(|| get("cord", q.filetype))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(filetype: &str, filename: &str) -> Option<Found> {
        lookup(Lookup {
            filetype,
            filename,
            force_fileext: None,
            force_filetype: None,
            force_cli: None,
            match_in_mappings: true,
        })
    }

    #[test]
    fn lookups() {
        assert_eq!(get("rust", "main.rs").unwrap().tooltip, "Rust");
        assert_eq!(get("toml", "Cargo.toml").unwrap().icon, "cargo");
        assert_eq!(get("c-sharp", "x").unwrap().tooltip, "C#");
        assert_eq!(get("a", "x.component.ts").unwrap().tooltip, "Angular");
        assert_eq!(get("Cord.new", "").unwrap().tooltip, "New file");
        assert_eq!(get("nope", "nope"), None);
        assert_eq!(default_icon("nope"), "text");
        assert_eq!(default_icon("vcs"), "git");
        let cli = lookup(Lookup {
            filetype: "rust",
            filename: "main.rs",
            force_fileext: None,
            force_filetype: None,
            force_cli: Some("lazygit"),
            match_in_mappings: true,
        });
        assert_eq!(cli.unwrap().ty, "vcs");
    }
}
