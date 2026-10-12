//! Stable desktop conversation identities and UTC labels, shared with legacy imports.
use pipkin_core::{ConversationId, ProjectId, stable_id};
use std::collections::HashSet;

/// The project of a session whose server does not report a working directory.
pub const SESSIONS_PROJECT: ProjectId = ProjectId(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    /// Unix seconds.
    pub created_at: i64,
}

/// Assign each session a stable `ConversationId` derived from its id. A collision (two live
/// sessions hashing alike) is resolved by probing in a deterministic order, so the same set of
/// sessions always gets the same ids.
pub fn assign_ids_with(
    sessions: &[Session],
    hash: impl Fn(&str) -> u64,
) -> Vec<(ConversationId, Session)> {
    let mut ordered: Vec<&Session> = sessions.iter().collect();
    ordered.sort_by(|a, b| (a.created_at, &a.session_id).cmp(&(b.created_at, &b.session_id)));
    let mut taken = HashSet::new();
    let mut out = Vec::new();
    for session in ordered {
        let mut id = hash(&session.session_id);
        while !taken.insert(id) {
            id = id.wrapping_add(1).max(1);
        }
        out.push((ConversationId(id), session.clone()));
    }
    out
}

pub fn assign_ids(sessions: &[Session]) -> Vec<(ConversationId, Session)> {
    assign_ids_with(sessions, stable_id)
}

/// `YYYY-MM-DD HH:MM` in UTC, without a date library.
pub fn format_time(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let secs = unix_seconds.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(id: &str, at: i64) -> Session {
        Session {
            session_id: id.into(),
            created_at: at,
        }
    }

    #[test]
    fn ids_are_stable_for_a_session_regardless_of_the_others() {
        let a = assign_ids(&[s("alpha", 1)]);
        let b = assign_ids(&[s("zeta", 0), s("alpha", 1), s("mid", 5)]);
        let id_of = |v: &[(ConversationId, Session)], name: &str| {
            v.iter().find(|(_, x)| x.session_id == name).unwrap().0
        };
        assert_eq!(id_of(&a, "alpha"), id_of(&b, "alpha"));
        assert!(b.iter().all(|(id, _)| id.0 > 0 && id.0 < (1 << 52)));
    }

    #[test]
    fn collisions_probe_deterministically() {
        let constant = |_: &str| 7u64;
        let first = assign_ids_with(&[s("b", 2), s("a", 1), s("c", 3)], constant);
        let ids: Vec<u64> = first.iter().map(|(i, _)| i.0).collect();
        assert_eq!(ids, [7, 8, 9]);
        // The oldest session keeps the base id however the input is ordered.
        let again = assign_ids_with(&[s("c", 3), s("a", 1), s("b", 2)], constant);
        assert_eq!(first, again);
        assert_eq!(first[0].1.session_id, "a");
    }

    #[test]
    fn formats_utc_times() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00"); // leap day
        assert_eq!(format_time(-1), "1969-12-31 23:59");
    }
}
