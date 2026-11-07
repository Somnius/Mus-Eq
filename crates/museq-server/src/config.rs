use std::path::PathBuf;

/// Runtime configuration, read from the environment.
///
/// - `MUSEQ_BIND`     listen address (default `127.0.0.1:8080`)
/// - `MUSEQ_LIBRARY`  folder with the music files (default `./library`)
/// - `MUSEQ_DB`       SQLite file (default `./museq.db`)
/// - `MUSEQ_SKINS_DIR` where uploaded `.wsz` skins are stored (default `./skins`)
/// - `MUSEQ_STATIONS_SEED` comma-separated paths of `Name|Category|URL` list files imported at startup (new stations only)
/// - `MUSEQ_ACOUSTID_KEY` your AcoustID application key (identify tracks by audio; without it that stays off)
/// - `MUSEQ_CONTACT` contact for the User-Agent sent to MusicBrainz (default: the project page)
/// - `MUSEQ_MUSICBRAINZ_URL`, `MUSEQ_ACOUSTID_URL` override the service addresses (for testing; keep https in
///   production, the AcoustID key travels in the query string)
/// - `MUSEQ_ALLOW_PRIVATE_STATIONS` set to `1` to allow stations on loopback/private addresses (for radio on your own network)
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
    pub stations_seeds: Vec<PathBuf>,
    pub allow_private_stations: bool,
    pub services: crate::metadata::Services,
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
        let mut services = crate::metadata::Services::default();
        if let Ok(u) = std::env::var("MUSEQ_MUSICBRAINZ_URL") {
            services.musicbrainz_url = u;
        }
        if let Ok(u) = std::env::var("MUSEQ_ACOUSTID_URL") {
            services.acoustid_url = u;
        }
        services.acoustid_key = std::env::var("MUSEQ_ACOUSTID_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty());
        if let Ok(contact) = std::env::var("MUSEQ_CONTACT") {
            services.user_agent = format!("Mus-Eq/{} ({})", env!("CARGO_PKG_VERSION"), contact);
        }
        Ok(Self {
            services,
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
            allow_private_stations: matches!(
                std::env::var("MUSEQ_ALLOW_PRIVATE_STATIONS").as_deref(),
                Ok("1" | "true")
            ),
            stations_seeds: std::env::var("MUSEQ_STATIONS_SEED")
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}
