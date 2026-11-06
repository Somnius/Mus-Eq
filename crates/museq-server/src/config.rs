use std::path::PathBuf;

/// Runtime configuration, read from the environment.
///
/// - `MUSEQ_BIND`     listen address (default `127.0.0.1:8080`)
/// - `MUSEQ_LIBRARY`  folder with the music files (default `./library`)
/// - `MUSEQ_DB`       SQLite file (default `./museq.db`)
/// - `MUSEQ_SKINS_DIR` where uploaded `.wsz` skins are stored (default `./skins`)
/// - `MUSEQ_STATIONS_SEED` a `Name|Category|URL` list imported at startup (new stations only)
/// - `MUSEQ_WEB_DIR`  built web client to serve at `/` (optional)
/// - `MUSEQ_TOKEN` or `MUSEQ_TOKEN_FILE`  the owner bearer token (required, at least 16 characters)
#[derive(Clone, Debug)]
pub struct Config {
    pub bind: String,
    pub library: PathBuf,
    pub db: PathBuf,
    pub token: String,
    pub skins_dir: PathBuf,
    pub web_dir: Option<PathBuf>,
    pub stations_seed: Option<PathBuf>,
}

pub const MIN_TOKEN_LEN: usize = 16;

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let token = match (
            std::env::var("MUSEQ_TOKEN"),
            std::env::var("MUSEQ_TOKEN_FILE"),
        ) {
            (Ok(t), _) => t,
            (Err(_), Ok(path)) => std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read MUSEQ_TOKEN_FILE {path}: {e}"))?,
            _ => return Err("set MUSEQ_TOKEN or MUSEQ_TOKEN_FILE".into()),
        };
        let token = token.trim().to_string();
        if token.len() < MIN_TOKEN_LEN {
            return Err(format!(
                "the token must be at least {MIN_TOKEN_LEN} characters"
            ));
        }
        Ok(Self {
            bind: std::env::var("MUSEQ_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into()),
            library: std::env::var("MUSEQ_LIBRARY")
                .unwrap_or_else(|_| "./library".into())
                .into(),
            db: std::env::var("MUSEQ_DB")
                .unwrap_or_else(|_| "./museq.db".into())
                .into(),
            token,
            skins_dir: std::env::var("MUSEQ_SKINS_DIR")
                .unwrap_or_else(|_| "./skins".into())
                .into(),
            web_dir: std::env::var("MUSEQ_WEB_DIR").ok().map(Into::into),
            stations_seed: std::env::var("MUSEQ_STATIONS_SEED").ok().map(Into::into),
        })
    }
}
