//! Small platform boundaries. Clipboard and dialogs belong to GPUI; only filesystem
//! locations and local server identity live here.

use std::io::Read;
use std::path::{Path, PathBuf};

pub const DATA_DIR_ENV: &str = "PIPKIN_DATA";
pub const DB_FILE: &str = "pipkin.sqlite3";

/// Resolve the data directory: explicit CLI value, then `PIPKIN_DATA`, then
/// `$XDG_DATA_HOME/pipkin`, then `~/.local/share/pipkin`.
pub fn data_dir(cli: Option<&Path>) -> PathBuf {
    resolve_data_dir(
        cli,
        std::env::var_os(DATA_DIR_ENV).map(PathBuf::from),
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

pub fn resolve_data_dir(
    cli: Option<&Path>,
    env_override: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> PathBuf {
    let non_empty = |p: &PathBuf| !p.as_os_str().is_empty();
    if let Some(p) = cli {
        return p.to_path_buf();
    }
    if let Some(p) = env_override.filter(non_empty) {
        return p;
    }
    if let Some(p) = xdg_data_home.filter(non_empty) {
        return p.join("pipkin");
    }
    home.filter(non_empty)
        .map(|h| h.join(".local/share/pipkin"))
        .unwrap_or_else(|| PathBuf::from(".pipkin"))
}

/// A canonical UUIDv4 using OS entropy on our current Unix targets, without Linux /proc.
/// Do not substitute timestamps if entropy is unavailable: a unique server identity is required.
pub fn fresh_server_id() -> std::io::Result<String> {
    let mut bytes = [0_u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

pub fn db_path(dir: &Path) -> PathBuf {
    dir.join(DB_FILE)
}

/// The built-in engine's data directory (database, artifacts) and configuration directory
/// (settings, models, credentials).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineDirs {
    pub data: PathBuf,
    pub config: PathBuf,
}

/// Where the built-in engine keeps its data and configuration. An explicit desktop data
/// directory (`--data-dir`, or `PIPKIN_DATA`) isolates everything under it: `<dir>/engine` and
/// `<dir>/engine-config`. Otherwise data lives in `<desktop data dir>/engine` and configuration
/// in the platform's configuration directory, under `pipkin/engine`.
pub fn engine_dirs(cli: Option<&Path>) -> EngineDirs {
    resolve_engine_dirs(
        cli,
        std::env::var_os(DATA_DIR_ENV).map(PathBuf::from),
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        cfg!(target_os = "macos"),
    )
}

pub fn resolve_engine_dirs(
    cli: Option<&Path>,
    env_override: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
    macos: bool,
) -> EngineDirs {
    let non_empty = |p: &PathBuf| !p.as_os_str().is_empty();
    let explicit = cli
        .map(Path::to_path_buf)
        .or_else(|| env_override.clone().filter(non_empty));
    if let Some(dir) = explicit {
        return EngineDirs {
            data: dir.join("engine"),
            config: dir.join("engine-config"),
        };
    }
    let data = resolve_data_dir(None, None, xdg_data_home, home.clone()).join("engine");
    let config_root = if macos {
        home.filter(non_empty)
            .map(|h| h.join("Library/Application Support"))
    } else {
        xdg_config_home
            .filter(non_empty)
            .or_else(|| home.filter(non_empty).map(|h| h.join(".config")))
    };
    let config = config_root
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("pipkin")
        .join("engine");
    EngineDirs { data, config }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_ids_are_distinct_canonical_protocol_uuid_v4() {
        let first = fresh_server_id().unwrap();
        let second = fresh_server_id().unwrap();
        assert!(pi_client::protocol::is_server_id(&first));
        assert!(pi_client::protocol::is_server_id(&second));
        assert_ne!(first, second);
    }

    #[test]
    fn engine_directories_follow_the_desktop_data_dir_and_the_platform_config_dir() {
        let p = |s: &str| Some(PathBuf::from(s));
        let isolated = resolve_engine_dirs(
            Some(Path::new("/x")),
            p("/env"),
            p("/xdg"),
            p("/cfg"),
            p("/home/u"),
            false,
        );
        assert_eq!(isolated.data, PathBuf::from("/x/engine"));
        assert_eq!(isolated.config, PathBuf::from("/x/engine-config"));
        let env = resolve_engine_dirs(None, p("/env"), None, None, p("/home/u"), false);
        assert_eq!(env.config, PathBuf::from("/env/engine-config"));
        let linux = resolve_engine_dirs(None, None, p("/xdg"), p("/cfg"), p("/home/u"), false);
        assert_eq!(linux.data, PathBuf::from("/xdg/pipkin/engine"));
        assert_eq!(linux.config, PathBuf::from("/cfg/pipkin/engine"));
        let home_only = resolve_engine_dirs(None, p(""), None, None, p("/home/u"), false);
        assert_eq!(
            home_only.data,
            PathBuf::from("/home/u/.local/share/pipkin/engine")
        );
        assert_eq!(
            home_only.config,
            PathBuf::from("/home/u/.config/pipkin/engine")
        );
        let mac = resolve_engine_dirs(None, None, None, p("/cfg"), p("/Users/u"), true);
        assert_eq!(
            mac.config,
            PathBuf::from("/Users/u/Library/Application Support/pipkin/engine")
        );
    }

    #[test]
    fn precedence_is_cli_env_xdg_home() {
        let p = |s: &str| Some(PathBuf::from(s));
        assert_eq!(
            resolve_data_dir(Some(Path::new("/cli")), p("/env"), p("/xdg"), p("/home/u")),
            PathBuf::from("/cli")
        );
        assert_eq!(
            resolve_data_dir(None, p("/env"), p("/xdg"), p("/home/u")),
            PathBuf::from("/env")
        );
        assert_eq!(
            resolve_data_dir(None, None, p("/xdg"), p("/home/u")),
            PathBuf::from("/xdg/pipkin")
        );
        assert_eq!(
            resolve_data_dir(None, p(""), None, p("/home/u")),
            PathBuf::from("/home/u/.local/share/pipkin")
        );
    }
}
