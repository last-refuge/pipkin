//! Explicit opt-in preflight. Only the CLI's public version/help/auth-status commands run.
//! CLI auth files/keychain credentials are never opened. This does NOT establish billing,
//! managed-policy isolation, permission safety, CLI-history freshness or process containment.
use super::{
    policy::{self, Environment, ModelAlias, TurnMode},
    process::{self, Spec},
};
use crate::store::CliBindingSpec;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
}
impl FileStamp {
    fn read(path: &Path) -> Result<Self, &'static str> {
        let meta = path
            .metadata()
            .map_err(|_| "cannot inspect executable metadata")?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            length: meta.len(),
            modified: meta
                .modified()
                .map_err(|_| "executable modification time unavailable")?,
            #[cfg(unix)]
            device: meta.dev(),
            #[cfg(unix)]
            inode: meta.ino(),
            #[cfg(unix)]
            mode: meta.mode(),
        })
    }
    fn marker(&self) -> String {
        let modified = match self.modified.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(age) => format!("+{}", age.as_nanos()),
            Err(age) => format!("-{}", age.duration().as_nanos()),
        };
        let mut parts = vec![self.length.to_string(), modified];
        #[cfg(unix)]
        parts.extend([
            self.device.to_string(),
            self.inode.to_string(),
            self.mode.to_string(),
        ]);
        policy::marker(parts)
    }
}

/// Filesystem-only candidate inspection. Show these paths BEFORE obtaining executable/project
/// consent. Approval must apply to this captured candidate, not a newly re-resolved path.
#[derive(Clone)]
pub struct Candidate {
    selected_executable: PathBuf,
    executable: PathBuf,
    selected_project: PathBuf,
    project: PathBuf,
    stamp: FileStamp,
    project_identity: String,
}
impl Candidate {
    pub fn inspect(
        executable: &Path,
        project: &Path,
        env: &Environment,
    ) -> Result<Self, &'static str> {
        if !project.is_absolute() || !project.is_dir() {
            return Err("select an absolute existing project directory");
        }
        let selected_executable = executable.to_path_buf();
        let executable = policy::resolve(Some(executable), env)?;
        let canonical_project = project
            .canonicalize()
            .map_err(|_| "cannot resolve project directory")?;
        if executable.to_str().is_none() || canonical_project.to_str().is_none() {
            return Err("CLI binding paths must be Unicode");
        }
        let stamp = FileStamp::read(&executable)?;
        let project_identity = directory_identity(&canonical_project)?;
        Ok(Self {
            selected_executable,
            executable,
            selected_project: project.into(),
            project: canonical_project,
            stamp,
            project_identity,
        })
    }
    pub fn executable(&self) -> &Path {
        &self.executable
    }
    pub fn project(&self) -> &Path {
        &self.project
    }
    fn revalidate(&self, env: &Environment) -> Result<(), &'static str> {
        let current = Self::inspect(&self.selected_executable, &self.selected_project, env)?;
        if self.executable != current.executable
            || self.project != current.project
            || self.stamp != current.stamp
            || self.project_identity != current.project_identity
        {
            return Err("approved executable or project changed; inspect and approve again");
        }
        Ok(())
    }
}

fn directory_identity(path: &Path) -> Result<String, &'static str> {
    let meta = path
        .metadata()
        .map_err(|_| "cannot inspect project identity")?;
    if !meta.is_dir() {
        return Err("project is no longer a directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!("{}:{}", meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        Err("local CLI supervision is unsupported on this platform")
    }
}

/// Observe directories, never credential files. Missing locations bind to their nearest
/// existing ancestor too, so a symlink swap cannot hide behind two NotFound observations.
fn configuration_locations(env: &Environment) -> Result<Vec<String>, &'static str> {
    let home = PathBuf::from(
        env.get(std::ffi::OsStr::new("HOME"))
            .ok_or("CLI HOME unavailable")?,
    );
    let config = env
        .get(std::ffi::OsStr::new("CLAUDE_CONFIG_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    [home, config]
        .into_iter()
        .map(|path| {
            let mut ancestor = path.as_path();
            let mut suffix = PathBuf::new();
            loop {
                match ancestor.canonicalize() {
                    Ok(canonical) => {
                        return Ok(policy::marker([
                            canonical.as_os_str().as_encoded_bytes(),
                            suffix.as_os_str().as_encoded_bytes(),
                            directory_identity(&canonical)?.as_bytes(),
                        ]));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        let name = ancestor
                            .file_name()
                            .ok_or("cannot resolve CLI configuration location")?;
                        suffix = Path::new(name).join(suffix);
                        ancestor = ancestor
                            .parent()
                            .ok_or("cannot resolve CLI configuration parent")?;
                    }
                    Err(_) => return Err("cannot inspect CLI configuration location"),
                }
            }
        })
        .collect()
}

/// No Debug/Serialize: an environment snapshot can contain proxy/keyring configuration.
pub struct Request {
    pub candidate: Candidate,
    pub trust_executable: bool,
    pub trust_project: bool,
    pub environment: Environment,
    pub model: ModelAlias,
}

/// Ephemeral verified metadata, NOT execution authority. The runtime must still durably admit
/// a run and prove fresh-vs-resume eligibility. Never cache this across submissions/restarts.
pub struct Prepared {
    candidate: Candidate,
    env: Environment,
    model: ModelAlias,
    config_ref: String,
    account_scope: String,
    locations: Vec<String>,
    checked_at: Instant,
}
impl Prepared {
    pub fn binding(&self, session_uuid: &str) -> Result<CliBindingSpec, &'static str> {
        if !policy::valid_uuid(session_uuid) {
            return Err("invalid explicit CLI session identifier");
        }
        Ok(CliBindingSpec {
            cli_session_id: session_uuid.into(),
            project_path: self.candidate.project.to_str().unwrap().into(),
            executable_path: self.candidate.executable.to_str().unwrap().into(),
            cli_version: policy::QUALIFIED_VERSION.into(),
            config_ref: self.config_ref.clone(),
            account_scope: self.account_scope.clone(),
            launch_policy: policy::POLICY_ID.into(),
        })
    }
    /// Fixed no-tools policy only. Binding equality does not prove CLI-history integrity:
    /// the caller owns durable launch intent, run exclusivity and fresh/resume selection.
    pub fn launch_spec(
        &self,
        binding: &CliBindingSpec,
        mode: TurnMode,
        prompt: &str,
    ) -> Result<Spec, &'static str> {
        if self.checked_at.elapsed() > Duration::from_secs(30) {
            return Err("CLI preflight expired; refresh before admission");
        }
        self.candidate.revalidate(&self.env)?;
        if configuration_locations(&self.env)? != self.locations {
            return Err("CLI configuration location changed; refresh preflight");
        }
        if &self.binding(&binding.cli_session_id)? != binding {
            return Err("CLI binding no longer matches preflight; use a fresh conversation");
        }
        policy::turn_spec(
            &self.candidate.executable,
            &self.candidate.project,
            &self.env,
            &binding.cli_session_id,
            mode,
            self.model,
            prompt,
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatus {
    logged_in: bool,
    auth_method: String,
    api_provider: Option<String>,
    email: Option<String>,
    org_id: Option<String>,
}
fn account_scope(bytes: &[u8]) -> Result<String, &'static str> {
    let auth: AuthStatus = serde_json::from_slice(bytes).map_err(|_| "invalid CLI auth status")?;
    if !auth.logged_in {
        return Err("sign in using the CLI's own terminal login, then refresh");
    }
    if auth.auth_method != "claude.ai" || auth.api_provider.as_deref() != Some("firstParty") {
        return Err("CLI does not report a recognized Claude-account route; no fallback attempted");
    }
    let email = auth
        .email
        .as_deref()
        .filter(|s| !s.is_empty() && s.len() <= 512)
        .ok_or("CLI auth status lacks a bounded account identity")?;
    let org = auth
        .org_id
        .as_deref()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or("CLI auth status lacks a bounded organization identity")?;
    Ok(policy::marker(["cli-account-v1", email, org]))
}

pub async fn check(request: Request, cancel: CancellationToken) -> Result<Prepared, &'static str> {
    if !request.trust_executable {
        return Err("explicit executable trust is required before CLI probes");
    }
    if !request.trust_project {
        return Err("explicit project trust is required before CLI preflight");
    }
    let env = policy::launch_environment(&request.environment)?;
    let candidate = request.candidate;
    candidate.revalidate(&env)?;
    let scratch = tempfile::tempdir().map_err(|_| "cannot create private CLI probe directory")?;
    let version = process::run(
        policy::metadata_spec(
            &candidate.executable,
            scratch.path(),
            &env,
            vec!["--version".into()],
        ),
        cancel.clone(),
    )
    .await?;
    if !version.clean_exit() {
        return Err("CLI version probe did not settle cleanly");
    }
    policy::validate_version(&version.captured)?;
    candidate.revalidate(&env)?;
    let help = process::run(
        policy::metadata_spec(
            &candidate.executable,
            scratch.path(),
            &env,
            vec!["--help".into()],
        ),
        cancel.clone(),
    )
    .await?;
    if !help.clean_exit() {
        return Err("CLI help probe did not settle cleanly");
    }
    policy::validate_help(&help.captured)?;
    candidate.revalidate(&env)?;
    let locations = configuration_locations(&env)?;
    let mut args = policy::safety_args();
    args.extend(["auth", "status", "--json"].into_iter().map(Into::into));
    let mut auth = process::run(
        policy::metadata_spec(&candidate.executable, &candidate.project, &env, args),
        cancel.clone(),
    )
    .await?;
    if !auth.clean_exit() {
        return Err("CLI auth probe did not settle cleanly; use CLI terminal login if signed out");
    }
    let account = account_scope(&auth.captured);
    auth.captured.fill(0);
    let account_scope = account?;
    if locations != configuration_locations(&env)? {
        return Err("CLI configuration location changed during auth probe; refresh preflight");
    }
    if cancel.is_cancelled() {
        return Err("CLI preflight cancelled");
    }
    candidate.revalidate(&env)?;
    let mut parts = vec![
        policy::POLICY_ID.as_bytes().to_vec(),
        request.model.as_str().as_bytes().to_vec(),
        candidate.stamp.marker().into_bytes(),
        candidate.project_identity.as_bytes().to_vec(),
    ];
    // Do not fingerprint transient desktop sockets, locale or proxy credentials. Relevant
    // home/config selection and the fixed policy/model are immutable context markers.
    for name in [
        "HOME",
        "CLAUDE_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
    ] {
        parts.push(name.as_bytes().to_vec());
        parts.push(
            env.get(std::ffi::OsStr::new(name))
                .map(|s| s.as_encoded_bytes().to_vec())
                .unwrap_or_default(),
        );
    }
    parts.extend(locations.iter().map(|s| s.as_bytes().to_vec()));
    Ok(Prepared {
        candidate,
        env,
        model: request.model,
        config_ref: policy::marker(parts),
        account_scope,
        locations,
        checked_at: Instant::now(),
    })
}

#[cfg(test)]
mod tests;
