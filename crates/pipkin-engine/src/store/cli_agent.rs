//! Immutable storage identity for CLI orchestration. No filesystem/login checks,
//! approval minting, prompt admission, resume authority or provider fallback happens here.
//! Public recognition never grants execution: only the connected owner admits work.
use super::*;
use crate::execution::claude_cli::policy;
use sha2::{Digest, Sha256};

#[derive(Clone, PartialEq, Eq)]
pub struct Create {
    pub session: String,
    pub request_key: String,
    pub identity: CliBindingSpec,
    pub model: String,
    pub thinking: Option<String>,
    pub parent: Option<String>,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Binding {
    pub create: Create,
    pub root: Option<String>,
    pub digest: [u8; 32],
}
impl Binding {
    /// Detect edits to a detached metadata value before comparing it with captured authority.
    pub fn verified_digest(&self) -> Option<[u8; 32]> {
        (validate(&self.create).is_ok() && digest(&self.create, &self.root) == self.digest)
            .then_some(self.digest)
    }
}
impl std::fmt::Debug for Create {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CliAgentCreate([redacted])")
    }
}
impl std::fmt::Debug for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CliAgentBinding([redacted])")
    }
}
fn validate(create: &Create) -> Result<()> {
    external::validate_spec(&create.identity)?;
    if !external::token(&create.session)
        || !external::token(&create.request_key)
        || create
            .parent
            .as_deref()
            .is_some_and(|s| !external::token(s))
        || crate::execution::claude_cli::orchestration_input::thinking(create.thinking.as_deref())
            .is_err()
        || !matches!(create.model.as_str(), "haiku" | "sonnet" | "opus")
        || create.identity.launch_policy != cli_bridge::ORCHESTRATION_POLICY
        || policy::supported_version(
            format!("{} (Claude Code)", create.identity.cli_version).as_bytes(),
        )
        .ok()
        .as_deref()
            != Some(create.identity.cli_version.as_str())
    {
        return Err(StoreError::ExecutionConflict);
    }
    Ok(())
}
fn digest(create: &Create, root: &Option<String>) -> [u8; 32] {
    let i = &create.identity;
    Sha256::digest(
        serde_json::to_vec(&serde_json::json!([
            "pipkin-cli-agent-binding-v1",
            create.session,
            create.request_key,
            i.cli_session_id,
            i.project_path,
            i.executable_path,
            i.cli_version,
            i.config_ref,
            i.account_scope,
            i.launch_policy,
            create.model,
            create.thinking,
            create.parent,
            root,
        ]))
        .expect("JSON identity"),
    )
    .into()
}
impl Store {
    pub(super) fn cli_agent_binding_in(c: &Connection, session: &str) -> Result<Option<Binding>> {
        let value=c.query_row("SELECT b.*,s.create_key,s.model,s.thinking,s.parent,s.root FROM cli_agent_bindings b JOIN sessions s ON s.id=b.session
            WHERE b.session=?1 AND s.execution_backend='claudeCliAgent'",[session],|r|Ok(Binding {
                create:Create {session:r.get("session")?,request_key:r.get("create_key")?,model:r.get("model")?,thinking:r.get("thinking")?,parent:r.get("parent")?,
                    identity:CliBindingSpec {cli_session_id:r.get("cli_session_id")?,project_path:r.get("project_path")?,executable_path:r.get("executable_path")?,
                        cli_version:r.get("cli_version")?,config_ref:r.get("config_ref")?,account_scope:r.get("account_scope")?,launch_policy:r.get("launch_policy")?}},
                root:r.get("root")?,digest:r.get("digest")?,
            })).optional()?;
        if let Some(b) = &value {
            validate(&b.create)?;
            if digest(&b.create, &b.root) != b.digest {
                return Err(StoreError::ExecutionConflict);
            }
        }
        Ok(value)
    }
    /// Metadata lookup only. A known binding/UUID is never a prompt or connection grant.
    pub fn cli_agent_binding(&self, session: &str) -> Result<Option<Binding>> {
        self.read(|c| Self::cli_agent_binding_in(c, session))
    }
    /// Storage-only create, called only AFTER the connected owner independently qualifies and
    /// approves this identity. Exact repeats return the same receipt without new authority.
    /// Desktop setup delegates authority to the owner. Children must inherit the parent's CLI account,
    /// executable/project/version/policy; profiles may select another CLI alias, never an API.
    pub fn create_cli_agent_session(&self, create: &Create, now: i64) -> Result<(Binding, bool)> {
        self.tx(|tx| Self::create_cli_agent_session_in(tx, create, now))
    }
    /// Transaction seam for atomic native child task/binding creation. Not an owner grant.
    pub(super) fn create_cli_agent_session_in(
        tx: &Connection,
        create: &Create,
        now: i64,
    ) -> Result<(Binding, bool)> {
        validate(create)?;
        Self::assert_session_live(tx, &create.session)?;
        if let Some(existing) = Self::cli_agent_binding_in(tx, &create.session)? {
            if existing.create != *create {
                return Err(StoreError::ExecutionConflict);
            }
            return Ok((existing, false));
        }
        let collision:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1 OR create_key=?2)
                OR EXISTS(SELECT 1 FROM session_removals WHERE root=?1 OR create_key=?2)
                OR EXISTS(SELECT 1 FROM cli_uuid_owners WHERE session=?1 OR create_key=?2 OR cli_session_id=?3)",
                params![create.session,create.request_key,create.identity.cli_session_id],|r|r.get(0))?;
        if collision {
            return Err(StoreError::ExecutionConflict);
        }
        let root = if let Some(parent) = &create.parent {
            Self::assert_session_live(tx, parent)?;
            let p = Self::cli_agent_binding_in(tx, parent)?.ok_or(StoreError::ExecutionConflict)?;
            let allowed: bool = tx.query_row(
                "SELECT subagents_enabled=1 AND blocked IS NULL FROM sessions WHERE id=?1",
                [parent],
                |r| r.get(0),
            )?;
            let mut inherited = p.create.identity.clone();
            inherited.cli_session_id = create.identity.cli_session_id.clone();
            if !allowed || inherited != create.identity {
                return Err(StoreError::ExecutionConflict);
            }
            Some(p.root.unwrap_or_else(|| parent.clone()))
        } else {
            None
        };
        let binding = Binding {
            create: create.clone(),
            digest: digest(create, &root),
            root,
        };
        let i = &create.identity;
        tx.execute("INSERT INTO sessions(id,cwd,created_at,updated_at,origin,subagents_enabled,create_key,execution_backend,model,thinking,parent,root)
                VALUES(?1,?2,?3,?3,?4,1,?5,'claudeCliAgent',?6,?7,?8,?9)",params![create.session,i.project_path,now,json(&Origin::default()),create.request_key,create.model,create.thinking,create.parent,binding.root])?;
        tx.execute("INSERT INTO cli_agent_bindings(session,cli_session_id,project_path,executable_path,cli_version,config_ref,account_scope,launch_policy,digest)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![create.session,i.cli_session_id,i.project_path,i.executable_path,i.cli_version,i.config_ref,i.account_scope,i.launch_policy,binding.digest])?;
        Ok((binding, true))
    }
}
/// Historical-schema fixtures must remove the additive v27 objects before downgrade.
#[cfg(test)]
pub(crate) fn remove_schema_for_test(c: &Connection) -> Result<()> {
    cli_agent_turns::remove_schema_for_test(c)?;
    c.execute_batch("DROP TRIGGER cli_agent_session_immutable; DROP TRIGGER cli_uuid_session_reserved;
        DROP TRIGGER cli_legacy_global_owner; DROP TABLE cli_agent_bindings; DROP TABLE cli_uuid_owners;")?;
    Ok(())
}
#[cfg(test)]
pub(crate) mod tests;
