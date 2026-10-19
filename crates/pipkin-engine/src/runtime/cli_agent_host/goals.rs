//! Goal reporting/evidence uses the existing domain, with a scoped atomic compare/write.
//! Scheduling and independent audit must have their own CLI-only owners; never borrow API.
use super::*;
use pipkin_engine_protocol::{ErrorCode, GoalView, ProtoError};
pub(in crate::runtime) struct GoalAuthority(pub(super) Host);
impl GoalAuthority {
    pub(in crate::runtime) fn call(&self) -> &str {
        &self.0.call
    }
    pub(in crate::runtime) fn run(&self) -> u64 {
        self.0.authority.scope().run
    }
    pub(in crate::runtime) fn check(&self) -> Result<(), String> {
        self.0.check()
    }
    pub(in crate::runtime) fn commit(
        &self,
        session: &str,
        expected: &GoalView,
        f: impl FnOnce(&rusqlite::Transaction) -> crate::store::Result<()>,
    ) -> Result<Option<GoalView>, ProtoError> {
        if session != self.0.authority.scope().session {
            return Err(ProtoError::new(
                ErrorCode::Invalid,
                "CLI goal scope mismatch",
            ));
        }
        self.0
            .transaction(|tx| {
                if Store::goal_current(tx, session)?.as_ref() != Some(expected) {
                    return Err(StoreError::ExecutionConflict);
                }
                f(tx)?;
                let goal = Store::goal_current(tx, session)?;
                let revision = Store::bump(tx, session, now_ms())?;
                Ok((
                    goal.clone(),
                    Some(Update {
                        scope: self.0.authority.scope().clone(),
                        revision,
                        changes: vec![SessionChange::GoalChanged {
                            goal: goal.map(Box::new),
                        }],
                    }),
                ))
            })
            .map_err(|e| ProtoError::new(ErrorCode::Invalid, e))
    }
}

impl Host {
    pub(super) async fn audit_goal(&self, goal: GoalView) -> Result<GoalView, String> {
        let session = &self.authority.scope().session;
        let round = goal.audit.rounds;
        let fingerprint = goal
            .audit
            .result
            .as_ref()
            .and_then(|r| r.fingerprint.as_deref())
            .ok_or("Missing audit fingerprint")?;
        let (tokens, cost, model) = tokio::task::block_in_place(|| {
            let model = self
                .shared
                .settings_for(session)
                .other
                .get("goals")
                .and_then(|g| g.get("auditorModel"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            (
                self.shared.session_tokens(session),
                self.shared.session_cost(session),
                model,
            )
        });
        let launch = SubagentLaunch {
            child: None,
            task: crate::goals::audit_prompt(
                &goal,
                round,
                goal.note.as_deref().unwrap_or_default(),
            ),
            profile: Some("auditor".into()),
            context: Some(crate::profiles::ContextMode::Fresh),
            wait: true,
            wake: false,
            model,
        };
        let outcome = self
            .start_child(launch, self.cancel.clone())
            .await
            .unwrap_or_else(|_| Err("Auditor reply lost; do not replay".into()));
        tokio::task::block_in_place(|| {
            self.shared.settle_goal_audit(
                session,
                &goal,
                round,
                fingerprint,
                outcome,
                tokens,
                cost,
                Some(&GoalAuthority(self.clone())),
            )
        })?
        .ok_or_else(|| "Goal changed while audit was running".into())
    }
}
