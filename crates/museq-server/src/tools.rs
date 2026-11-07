//! Programs the server runs as separate processes (ffmpeg, yt-dlp). Mus-Eq never ships them.
//!
//! They are fetched on request, after an explicit typed confirmation, from a URL pinned in this release,
//! and are used only if the download matches the pinned SHA-256. A copy the owner already has (`MUSEQ_FFMPEG`,
//! or a program on `PATH`) is used as before and needs no download.

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

/// How a download is turned into the program.
#[derive(Clone, Debug, PartialEq)]
pub enum Archive {
    /// The download is the program.
    Raw,
    /// A `.tar.xz`; the program is the entry whose path ends with `member` (for example `/bin/ffmpeg`).
    TarXz { member: String },
}

/// One pinned download for one platform.
#[derive(Clone, Debug)]
pub struct Asset {
    pub os: String,
    pub arch: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub archive: Archive,
}

/// A second run after install: the program's output must contain every one of `contains`.
#[derive(Clone, Debug)]
pub struct ExtraCheck {
    pub args: Vec<String>,
    pub contains: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub purpose: String,
    pub version: String,
    pub licence: String,
    pub homepage: String,
    /// File name of the program once installed.
    pub binary: String,
    /// Run after install to prove it works, and what its first output line must start with ("" = anything).
    pub check_args: Vec<String>,
    pub check_prefix: String,
    pub extra_checks: Vec<ExtraCheck>,
    /// Downloads, and every redirect they follow, may only use these hosts (or their subdomains).
    pub hosts: Vec<String>,
    pub notes: String,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Progress {
    /// idle, downloading, verifying, unpacking, checking, done or failed
    pub phase: String,
    pub downloaded: u64,
    pub total: u64,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct ToolStatus {
    pub name: String,
    pub purpose: String,
    pub version: String,
    pub licence: String,
    pub homepage: String,
    pub notes: String,
    /// False if this release pins no build for the platform the server runs on.
    pub platform_supported: bool,
    pub source_url: Option<String>,
    pub download_bytes: Option<u64>,
    pub sha256: Option<String>,
    /// installed (downloaded by Mus-Eq), modified (the installed file no longer matches the install record and
    /// is not used), configured (MUSEQ_FFMPEG), system (found on PATH) or missing
    pub found: String,
    pub path: Option<String>,
    /// What a client shows before asking the owner to confirm.
    pub message: Option<String>,
    /// The exact text the install request must carry in `confirm`.
    pub confirm_phrase: String,
    pub progress: Progress,
}

pub struct Tools {
    dir: PathBuf,
    specs: Vec<ToolSpec>,
    explicit: Mutex<HashMap<String, String>>,
    state: Mutex<HashMap<String, Progress>>,
    downloads_allowed: bool,
    /// Tests only: allow plain http and loopback hosts.
    insecure: bool,
}

/// What a failed request says, with the HTTP status the API should answer with.
pub type Refusal = (u16, String);

impl Tools {
    pub fn new(dir: PathBuf, specs: Vec<ToolSpec>) -> Self {
        Self {
            dir,
            specs,
            explicit: Mutex::new(HashMap::new()),
            state: Mutex::new(HashMap::new()),
            downloads_allowed: true,
            insecure: false,
        }
    }

    /// The pinned tools for this release, stored under `dir`.
    pub fn builtin(dir: PathBuf) -> Self {
        Self::new(dir, builtin_specs())
    }

    pub fn downloads_allowed(mut self, yes: bool) -> Self {
        self.downloads_allowed = yes;
        self
    }

    /// Tests only.
    pub fn insecure_for_tests(mut self) -> Self {
        self.insecure = true;
        self
    }

    /// Use this program for `name` instead of any download (for example `MUSEQ_FFMPEG`).
    pub fn set_explicit(&self, name: &str, path: String) {
        self.explicit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), path);
    }

    fn spec(&self, name: &str) -> Option<&ToolSpec> {
        self.specs.iter().find(|s| s.name == name)
    }

    fn asset<'a>(&self, spec: &'a ToolSpec) -> Option<&'a Asset> {
        spec.assets
            .iter()
            .find(|a| a.os == std::env::consts::OS && a.arch == std::env::consts::ARCH)
    }

    fn installed_path(&self, spec: &ToolSpec) -> PathBuf {
        self.dir
            .join(&spec.name)
            .join(&spec.version)
            .join(&spec.binary)
    }

    /// The installed program, if its file is there and still has the size the install record names.
    pub fn installed_binary(&self, name: &str) -> Option<PathBuf> {
        let spec = self.spec(name)?;
        let p = self.installed_path(spec);
        // a symlink in the install position is never the program Mus-Eq installed
        let meta = p.symlink_metadata().ok()?;
        if !meta.file_type().is_file() {
            return None;
        }
        let len = meta.len();
        let text = std::fs::read_to_string(p.with_file_name("installed.json")).ok()?;
        let rec: serde_json::Value = serde_json::from_str(&text).ok()?;
        (rec["binary_bytes"].as_u64() == Some(len)).then_some(p)
    }

    /// The program to run for `name`: the configured one, else the one Mus-Eq installed, else `name` from PATH.
    pub fn resolve(&self, name: &str) -> String {
        if let Some(p) = self
            .explicit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
        {
            return p.clone();
        }
        if let Some(p) = self.installed_binary(name) {
            return p.to_string_lossy().into_owned();
        }
        name.to_string()
    }

    fn found(&self, spec: &ToolSpec) -> (String, Option<String>) {
        let explicit = self
            .explicit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&spec.name)
            .cloned();
        if let Some(p) = explicit {
            return ("configured".into(), Some(p));
        }
        let p = self.installed_path(spec);
        if self.installed_binary(&spec.name).is_some() {
            return ("installed".into(), Some(p.to_string_lossy().into_owned()));
        }
        if p.is_file() {
            // a file is there but it is not what the install record describes, so it is not used
            return ("modified".into(), Some(p.to_string_lossy().into_owned()));
        }
        match find_in_path(&spec.binary_name_on_path()) {
            Some(p) => ("system".into(), Some(p.to_string_lossy().into_owned())),
            None => ("missing".into(), None),
        }
    }

    pub fn confirm_phrase(spec: &ToolSpec) -> String {
        format!("install {} {}", spec.name, spec.version)
    }

    fn message(&self, spec: &ToolSpec, asset: &Asset) -> String {
        format!(
            "Mus-Eq needs {name} {purpose}. Installing it will download {name} {version} ({mb} MB) from {host} ({url}), \
             check it against a SHA-256 value pinned in this Mus-Eq release ({sha}), and store it in {dir}. \
             {name} is separate software under {licence} ({home}); Mus-Eq does not include it. {notes}\
             Nothing is downloaded until you confirm by sending: \"{phrase}\".",
            name = spec.name,
            purpose = spec.purpose,
            version = spec.version,
            mb = asset.bytes.div_ceil(1_000_000),
            host = host_of(&asset.url).unwrap_or_default(),
            url = asset.url,
            sha = asset.sha256,
            dir = self.dir.join(&spec.name).display(),
            licence = spec.licence,
            home = spec.homepage,
            notes = if spec.notes.is_empty() {
                String::new()
            } else {
                format!("{} ", spec.notes)
            },
            phrase = Self::confirm_phrase(spec),
        )
    }

    pub fn status(&self) -> Vec<ToolStatus> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        self.specs
            .iter()
            .map(|spec| {
                let asset = self.asset(spec);
                let (found, path) = self.found(spec);
                ToolStatus {
                    name: spec.name.clone(),
                    purpose: spec.purpose.clone(),
                    version: spec.version.clone(),
                    licence: spec.licence.clone(),
                    homepage: spec.homepage.clone(),
                    notes: spec.notes.clone(),
                    platform_supported: asset.is_some(),
                    source_url: asset.map(|a| a.url.clone()),
                    download_bytes: asset.map(|a| a.bytes),
                    sha256: asset.map(|a| a.sha256.clone()),
                    found,
                    path,
                    message: asset.map(|a| self.message(spec, a)),
                    confirm_phrase: Self::confirm_phrase(spec),
                    progress: state.get(&spec.name).cloned().unwrap_or(Progress {
                        phase: "idle".into(),
                        ..Default::default()
                    }),
                }
            })
            .collect()
    }

    fn set_progress(&self, name: &str, f: impl FnOnce(&mut Progress)) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(st.entry(name.to_string()).or_default());
    }

    /// Starts the install in the background. `confirm` must be the exact phrase from `status()`.
    pub fn install(self: &Arc<Self>, name: &str, confirm: &str) -> Result<(), Refusal> {
        let spec = self
            .spec(name)
            .ok_or((404, "no such tool".to_string()))?
            .clone();
        if !self.downloads_allowed {
            return Err((
                403,
                "downloads are switched off on this server (MUSEQ_TOOLS_DOWNLOADS=off); install the program yourself and point to it with its environment variable".into(),
            ));
        }
        let Some(asset) = self.asset(&spec).cloned() else {
            return Err((
                400,
                format!(
                    "this release pins no {name} build for {}/{}; install it yourself and point to it with its environment variable",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            ));
        };
        if confirm != Self::confirm_phrase(&spec) {
            return Err((
                400,
                format!(
                    "not confirmed. {} Send {{\"confirm\": \"{}\"}} to go ahead.",
                    self.message(&spec, &asset),
                    Self::confirm_phrase(&spec)
                ),
            ));
        }
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.get(name).is_some_and(|p| {
                matches!(
                    p.phase.as_str(),
                    "downloading" | "verifying" | "unpacking" | "checking"
                )
            }) {
                return Err((409, format!("{name} is already being installed")));
            }
            st.insert(
                name.to_string(),
                Progress {
                    phase: "downloading".into(),
                    total: asset.bytes,
                    ..Default::default()
                },
            );
        }
        let me = self.clone();
        tokio::spawn(async move {
            // The work runs in its own task, so even a panic in it ends as "failed" and frees the tool.
            let work = {
                let (me, spec, asset) = (me.clone(), spec.clone(), asset.clone());
                tokio::spawn(async move {
                    // a total deadline: a slow trickle must not hold the tool for ever
                    let limit = Duration::from_secs(15 * 60 + asset.bytes / (100 * 1024));
                    match tokio::time::timeout(limit, me.download_and_install(&spec, &asset)).await
                    {
                        Ok(r) => r,
                        Err(_) => Err("the install took too long and was stopped".to_string()),
                    }
                })
            };
            let result = work
                .await
                .unwrap_or_else(|_| Err("the install crashed".to_string()));
            match result {
                Ok(()) => me.set_progress(&spec.name, |p| {
                    p.phase = "done".into();
                    p.error = None;
                }),
                Err(e) => {
                    me.cleanup_partials(&spec);
                    me.set_progress(&spec.name, |p| {
                        p.phase = "failed".into();
                        p.error = Some(e);
                    })
                }
            }
        });
        Ok(())
    }

    /// Removes what Mus-Eq installed for `name` (a program the owner configured is never touched).
    pub fn uninstall(&self, name: &str) -> Result<(), Refusal> {
        let spec = self.spec(name).ok_or((404, "no such tool".to_string()))?;
        // the check and the removal happen under one lock, so an install cannot start in between
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.get(name).is_some_and(|p| {
            matches!(
                p.phase.as_str(),
                "downloading" | "verifying" | "unpacking" | "checking"
            )
        }) {
            return Err((409, format!("{name} is being installed")));
        }
        let _ = std::fs::remove_dir_all(self.dir.join(&spec.name));
        st.insert(
            name.to_string(),
            Progress {
                phase: "idle".into(),
                ..Default::default()
            },
        );
        Ok(())
    }

    fn new_path(&self, spec: &ToolSpec) -> PathBuf {
        self.installed_path(spec)
            .with_file_name(format!("{}.new", spec.binary))
    }

    fn cleanup_partials(&self, spec: &ToolSpec) {
        let _ = std::fs::remove_file(self.part_path(spec));
        let _ = std::fs::remove_file(self.new_path(spec));
        // empty folders from a failed first install (remove_dir only removes an empty folder)
        if let Some(version_dir) = self.installed_path(spec).parent() {
            let _ = std::fs::remove_dir(version_dir);
        }
        let _ = std::fs::remove_dir(self.dir.join(&spec.name));
    }

    fn part_path(&self, spec: &ToolSpec) -> PathBuf {
        self.dir.join(format!(".download-{}.part", spec.name))
    }

    fn client(&self, spec: &ToolSpec) -> Result<reqwest::Client, String> {
        let hosts = spec.hosts.clone();
        let insecure = self.insecure;
        reqwest::Client::builder()
            .user_agent("museq-server tool installer")
            .connect_timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.error("too many redirects");
                }
                let u = attempt.url();
                if !insecure && u.scheme() != "https" {
                    return attempt.error("redirect to a plain http address refused");
                }
                if host_allowed(u.host_str().unwrap_or(""), &hosts) {
                    attempt.follow()
                } else {
                    attempt.error("redirect to a host that is not allowed for this download")
                }
            }))
            .build()
            .map_err(|e| format!("cannot start the download: {e}"))
    }

    async fn download_and_install(&self, spec: &ToolSpec, asset: &Asset) -> Result<(), String> {
        let host = host_of(&asset.url).unwrap_or_default();
        if !self.insecure && !asset.url.starts_with("https://") {
            return Err("the pinned address is not https".into());
        }
        if !host_allowed(&host, &spec.hosts) {
            return Err("the pinned address is not on an allowed host".into());
        }
        let existed = self.dir.exists();
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("cannot create the tools folder: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            // a folder somebody else owns could be swapped under us: never run programs from one
            let meta = std::fs::metadata(&self.dir)
                .map_err(|e| format!("cannot read the tools folder: {e}"))?;
            // SAFETY: geteuid has no preconditions and cannot fail
            if meta.uid() != unsafe { libc::geteuid() } {
                return Err(
                    "the tools folder is not owned by the user the server runs as; refused".into(),
                );
            }
            // only a folder Mus-Eq created is locked down; one the owner pointed to keeps its permissions
            if !existed {
                let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        let part = self.part_path(spec);
        let client = self.client(spec)?;
        let resp = client
            .get(&asset.url)
            .send()
            .await
            .map_err(|e| format!("download failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "the server answered HTTP {}",
                resp.status().as_u16()
            ));
        }
        let cap = asset.bytes + 1024 * 1024;
        if resp.content_length().is_some_and(|n| n > cap) {
            return Err("the download is larger than the pinned size; refused".into());
        }
        // never write through something that is already there (a planted symlink, an old partial file)
        let _ = std::fs::remove_file(&part);
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .await
            .map_err(|e| format!("cannot write the download: {e}"))?;
        let mut done: u64 = 0;
        let mut stream = resp.bytes_stream();
        loop {
            let next = tokio::time::timeout(Duration::from_secs(60), stream.next())
                .await
                .map_err(|_| "the download stalled for 60 seconds".to_string())?;
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|e| format!("download failed: {e}"))?;
            done += chunk.len() as u64;
            if done > cap {
                return Err("the download is larger than the pinned size; refused".into());
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("cannot write the download: {e}"))?;
            self.set_progress(&spec.name, |p| p.downloaded = done);
        }
        file.flush()
            .await
            .map_err(|e| format!("cannot write the download: {e}"))?;
        drop(file);

        self.set_progress(&spec.name, |p| p.phase = "verifying".into());
        // hash what is on disk, which is exactly what gets unpacked
        let for_hash = part.clone();
        let got = tokio::task::spawn_blocking(move || sha256_file(&for_hash))
            .await
            .map_err(|_| "checking the download crashed".to_string())?
            .map_err(|e| format!("cannot read the download back: {e}"))?;
        if got != asset.sha256.to_lowercase() {
            return Err(format!(
                "the download does not match the pinned SHA-256 (got {got}, pinned {}); nothing was installed",
                asset.sha256
            ));
        }

        self.set_progress(&spec.name, |p| p.phase = "unpacking".into());
        let target = self.installed_path(spec);
        let new = self.new_path(spec);
        std::fs::create_dir_all(target.parent().unwrap_or(&self.dir))
            .map_err(|e| format!("cannot create the install folder: {e}"))?;
        let (part2, new2, archive) = (part.clone(), new.clone(), asset.archive.clone());
        tokio::task::spawn_blocking(move || unpack(&part2, &new2, &archive))
            .await
            .map_err(|_| "unpacking crashed".to_string())??;
        let _ = std::fs::remove_file(&part);

        self.set_progress(&spec.name, |p| p.phase = "checking".into());
        check_runs(&new, &spec.check_args, &spec.check_prefix).await?;
        for extra in &spec.extra_checks {
            check_output_has(&new, &extra.args, &extra.contains).await?;
        }
        // The record goes in first: a program counts as installed only when the record matches its file.
        let bytes = new
            .metadata()
            .map(|m| m.len())
            .map_err(|e| format!("cannot read the program: {e}"))?;
        let record = serde_json::json!({
            "tool": spec.name, "version": spec.version, "url": asset.url,
            "archive_sha256": asset.sha256, "binary_sha256": sha256_file(&new).ok(),
            "binary_bytes": bytes,
        });
        std::fs::write(target.with_file_name("installed.json"), record.to_string())
            .map_err(|e| format!("cannot write the install record: {e}"))?;
        std::fs::rename(&new, &target)
            .map_err(|e| format!("cannot put the program in place: {e}"))?;
        // an older pinned version, if any, is no longer needed
        if let Ok(rd) = std::fs::read_dir(self.dir.join(&spec.name)) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy() != spec.version {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        Ok(())
    }
}

impl ToolSpec {
    fn binary_name_on_path(&self) -> String {
        // yt-dlp is called yt-dlp on PATH, whatever the release asset was named
        self.name.clone()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_file(p: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(str::to_string)
}

/// A host passes if it is one of `allowed` or a subdomain of one.
pub fn host_allowed(host: &str, allowed: &[String]) -> bool {
    let host = host.to_ascii_lowercase();
    allowed.iter().any(|a| {
        let a = a.to_ascii_lowercase();
        host == a || host.ends_with(&format!(".{a}"))
    })
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.is_file()
            && p.metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Largest program accepted out of an archive.
const MAX_BINARY: u64 = 600 * 1024 * 1024;
/// Most that is read out of an archive while looking for the program.
const MAX_DECOMPRESSED: u64 = 2 * 1024 * 1024 * 1024;

/// Writes the program to `out`; the archive's own paths are never used as output paths.
fn unpack(download: &Path, out: &Path, archive: &Archive) -> Result<(), String> {
    use std::io::Read;
    // created exclusively: an existing file or symlink at this path is removed first, never written through
    let _ = std::fs::remove_file(out);
    let mut dest = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|e| format!("cannot write the program: {e}"))?;
    match archive {
        Archive::Raw => {
            let mut src = std::fs::File::open(download)
                .map_err(|e| format!("cannot read the download: {e}"))?;
            std::io::copy(&mut src, &mut dest)
                .map_err(|e| format!("cannot write the program: {e}"))?;
        }
        Archive::TarXz { member } => {
            let src = std::fs::File::open(download)
                .map_err(|e| format!("cannot read the download: {e}"))?;
            let mut tar = tar::Archive::new(xz2::read::XzDecoder::new(src).take(MAX_DECOMPRESSED));
            let mut found = false;
            for entry in tar
                .entries()
                .map_err(|e| format!("cannot read the archive: {e}"))?
            {
                let entry = entry.map_err(|e| format!("cannot read the archive: {e}"))?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let path = entry
                    .path()
                    .map_err(|e| format!("bad path in the archive: {e}"))?;
                if path.to_string_lossy().ends_with(member.as_str()) {
                    let mut limited = entry.take(MAX_BINARY + 1);
                    let n = std::io::copy(&mut limited, &mut dest)
                        .map_err(|e| format!("cannot write the program: {e}"))?;
                    if n > MAX_BINARY {
                        return Err("the program inside the archive is larger than allowed".into());
                    }
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(format!("the archive has no {member}"));
            }
        }
    }
    drop(dest);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("cannot make the program executable: {e}"))?;
    }
    let mut probe = [0u8; 1];
    if std::fs::File::open(out)
        .and_then(|mut f| f.read(&mut probe))
        .map_err(|e| e.to_string())?
        == 0
    {
        return Err("the program is empty".into());
    }
    Ok(())
}

/// Runs `program` once, without a shell, and returns what it printed. On Linux an exec can fail for a moment with
/// "text file busy" while another thread's fork still holds the just-written file open, so that one error is retried.
async fn run_output(program: &Path, args: &[String]) -> Result<std::process::Output, String> {
    const ETXTBSY: i32 = 26;
    let mut last = String::new();
    for _ in 0..20 {
        let attempt = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| "the new program did not answer in 30 seconds".to_string())?;
        match attempt {
            Ok(out) => return Ok(out),
            Err(e) if e.raw_os_error() == Some(ETXTBSY) => {
                last = e.to_string();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => return Err(format!("the new program cannot run on this system: {e}")),
        }
    }
    Err(format!("the new program cannot run on this system: {last}"))
}

/// Runs the new program once (no shell) and checks the first line it prints.
async fn check_runs(program: &Path, args: &[String], prefix: &str) -> Result<(), String> {
    let out = run_output(program, args).await?;
    if !out.status.success() {
        return Err(format!("the new program exited with {}", out.status));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next().unwrap_or("").trim();
    if first.is_empty() || !first.starts_with(prefix) {
        return Err("the new program did not identify itself as expected".into());
    }
    Ok(())
}

/// Runs the program with `args` and requires every string of `contains` in what it prints.
async fn check_output_has(
    program: &Path,
    args: &[String],
    contains: &[String],
) -> Result<(), String> {
    let out = run_output(program, args).await?;
    let text = String::from_utf8_lossy(&out.stdout);
    for want in contains {
        if !text.contains(want.as_str()) {
            return Err(format!(
                "the new program lacks what Mus-Eq needs ({})",
                want.trim()
            ));
        }
    }
    Ok(())
}

fn s(x: &str) -> String {
    x.to_string()
}

/// The downloads pinned in this release. Each address, size and SHA-256 was read from the project's own release
/// page when it was pinned (see docs/THIRD_PARTY.md); a new Mus-Eq release re-pins.
pub fn builtin_specs() -> Vec<ToolSpec> {
    let ffmpeg_url = |file: &str| {
        format!(
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-31-12-59/{file}"
        )
    };
    let ffmpeg_asset = |arch: &str, file: &str, sha: &str, bytes: u64| Asset {
        os: s("linux"),
        arch: s(arch),
        url: ffmpeg_url(file),
        sha256: s(sha),
        bytes,
        archive: Archive::TarXz {
            member: s("/bin/ffmpeg"),
        },
    };
    let ytdlp_asset = |arch: &str, file: &str, sha: &str, bytes: u64| Asset {
        os: s("linux"),
        arch: s(arch),
        url: format!("https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/{file}"),
        sha256: s(sha),
        bytes,
        archive: Archive::Raw,
    };
    vec![
        ToolSpec {
            name: s("ffmpeg"),
            purpose: s("to convert tracks to another format on the fly and to run library radios"),
            version: s("n8.1.3"),
            licence: s("LGPL-3.0-or-later (a build made without GPL parts)"),
            homepage: s("https://ffmpeg.org"),
            binary: s("ffmpeg"),
            check_args: vec![s("-version")],
            check_prefix: s("ffmpeg version"),
            // the encoders Mus-Eq uses must be in the build
            extra_checks: vec![ExtraCheck {
                args: vec![s("-hide_banner"), s("-encoders")],
                contains: vec![s("libopus"), s("libmp3lame"), s(" aac ")],
            }],
            hosts: vec![s("github.com"), s("githubusercontent.com")],
            notes: s("The build is made by the BtbN project, not by the FFmpeg project."),
            assets: vec![
                ffmpeg_asset(
                    "x86_64",
                    "ffmpeg-n8.1.3-27-g8dca1586f4-linux64-lgpl-8.1.tar.xz",
                    "a64f3171e6ad1f7d358d28890a907ee478c331d1177ec696a93418f79ac3f1f3",
                    135_188_284,
                ),
                ffmpeg_asset(
                    "aarch64",
                    "ffmpeg-n8.1.3-27-g8dca1586f4-linuxarm64-lgpl-8.1.tar.xz",
                    "2ea30086a637e3f0ad0f49a1b38db34121422b85e93cc82df2e01d9ee5200809",
                    115_803_692,
                ),
            ],
        },
        ToolSpec {
            name: s("yt-dlp"),
            purpose: s(
                "to fetch a track from a link you give it (the fetch feature, which is off by default)",
            ),
            version: s("2026.10.26"),
            licence: s(
                "Unlicense (the single-file build also bundles Python and other components under their own licences)",
            ),
            homepage: s("https://github.com/yt-dlp/yt-dlp"),
            binary: s("yt-dlp"),
            check_args: vec![s("--version")],
            check_prefix: s("2026."),
            extra_checks: vec![],
            hosts: vec![s("github.com"), s("githubusercontent.com")],
            notes: s(
                "Downloading from a streaming service may break that service's terms; that is your responsibility.",
            ),
            assets: vec![
                ytdlp_asset(
                    "x86_64",
                    "yt-dlp_linux",
                    "767ba9dce7052aa196f8b12652f433d76de128ba168966fa84b6394d5e6c6795",
                    41_166_968,
                ),
                ytdlp_asset(
                    "aarch64",
                    "yt-dlp_linux_aarch64",
                    "e87e9533e89fe5889abaa21f5114dfe7756d6aa6219b83551add04d48b18e483",
                    40_789_792,
                ),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_match_exactly_or_as_subdomains_only() {
        let allowed = vec![
            "github.com".to_string(),
            "githubusercontent.com".to_string(),
        ];
        assert!(host_allowed("github.com", &allowed));
        assert!(host_allowed(
            "release-assets.githubusercontent.com",
            &allowed
        ));
        assert!(host_allowed("GitHub.com", &allowed));
        assert!(!host_allowed("evilgithub.com", &allowed));
        assert!(!host_allowed("github.com.evil.example", &allowed));
        assert!(!host_allowed("", &allowed));
    }

    #[test]
    fn pinned_table_is_well_formed() {
        for spec in builtin_specs() {
            assert!(!spec.assets.is_empty(), "{} has no assets", spec.name);
            for a in &spec.assets {
                assert_eq!(a.sha256.len(), 64, "{} sha", spec.name);
                assert!(
                    a.sha256
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                );
                assert!(a.url.starts_with("https://"));
                assert!(host_allowed(&host_of(&a.url).unwrap(), &spec.hosts));
                assert!(a.bytes > 1_000_000);
            }
            let phrase = Tools::confirm_phrase(&spec);
            assert!(phrase.contains(&spec.name) && phrase.contains(&spec.version));
        }
    }
}
