//! Asking the user something from inside a tool call, and the session-scoped
//! state that follows from their answers.
//!
//! Everything here rides the stdin request pipe: it already suspends a tool on
//! a `oneshot`, carries a prompt to the client, and resolves the sender when
//! the reply arrives. `ask_user`, the approval gate and plan mode all need that
//! same round trip, so they share it rather than growing three variants.
//!
//! # Sessions with nobody watching
//!
//! A session with no attached client — headless, ambient, swarm worker — has no
//! `stdin_request_tx`. Asking there would block the turn forever, so
//! [`request_user_input`] reports [`AskError::NoInteractiveUser`] and each
//! caller decides what that means for it. They do not all agree: a question
//! should fail so the model decides for itself, while an approval gate must
//! fall back to its non-interactive policy rather than silently allowing.

use super::ToolContext;
use std::collections::HashSet;
use std::sync::{LazyLock, RwLock};

/// Why a question could not be put to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AskError {
    /// Nobody is attached to this session.
    NoInteractiveUser,
    /// A client was attached but went away before answering.
    Disconnected,
}

impl AskError {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::NoInteractiveUser => "no interactive user is attached to this session",
            Self::Disconnected => "the client disconnected before answering",
        }
    }
}

/// Put `prompt` to the user and wait for a line back.
///
/// The returned string is exactly what they typed, trimmed. Callers interpret
/// it; this only owns the round trip.
pub(crate) async fn request_user_input(
    ctx: &ToolContext,
    request_id: String,
    prompt: String,
) -> Result<String, AskError> {
    let Some(stdin_tx) = ctx.stdin_request_tx.clone() else {
        return Err(AskError::NoInteractiveUser);
    };

    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    let request = jcode_tool_core::StdinInputRequest {
        request_id,
        prompt,
        is_password: false,
        response_tx,
    };

    if stdin_tx.send(request).is_err() {
        return Err(AskError::Disconnected);
    }

    match response_rx.await {
        Ok(answer) => Ok(answer.trim().to_string()),
        Err(_) => Err(AskError::Disconnected),
    }
}

/// What the user decided about a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Allow,
    /// Allow, and stop asking about this tool for the rest of the session.
    AllowAlways,
    /// Refused, with whatever the user said about it.
    Deny(String),
}

/// Read a decision out of a free-text answer.
///
/// Anything that is not recognisably an approval is a refusal, and the text is
/// carried through to the model as the reason. Defaulting the ambiguous case to
/// "deny" is the whole point of an approval gate: a typo must not run a
/// destructive command.
pub(crate) fn parse_decision(answer: &str) -> Decision {
    let normalized = answer.trim().to_lowercase();
    match normalized.as_str() {
        "1" | "y" | "yes" | "ok" | "allow" | "approve" | "" => Decision::Allow,
        "2" | "a" | "always" | "allow always" | "yes always" => Decision::AllowAlways,
        "3" | "n" | "no" | "deny" | "reject" | "cancel" => {
            Decision::Deny("The user declined.".to_string())
        }
        _ => Decision::Deny(format!("The user declined and said: {}", answer.trim())),
    }
}

/// The approval prompt. Enter alone approves, which keeps the common case one
/// keystroke; refusing takes a deliberate answer.
pub(crate) fn approval_prompt(tool: &str, detail: &str) -> String {
    let detail = detail.trim();
    let subject = if detail.is_empty() {
        format!("Allow `{tool}` to run?")
    } else {
        format!("Allow `{tool}` to run?\n\n  {detail}")
    };
    format!(
        "{subject}\n\n  1. Yes\n  2. Yes, and stop asking for `{tool}` this session\n  3. No\n\n\
         Enter alone approves. Anything else is a refusal and is passed to the agent as your reason."
    )
}

/// Per-session state built from the user's answers.
///
/// Keyed by session id in a global map, mirroring `SESSION_TOOL_POLICIES`,
/// because the `Registry` is cloned per subagent and this must not be cloned
/// away: an approval the user granted applies to the session, not to whichever
/// registry clone happened to ask.
#[derive(Debug, Default, Clone)]
struct SessionApprovalState {
    always_allowed: HashSet<String>,
    plan_mode: bool,
}

static SESSION_APPROVALS: LazyLock<
    RwLock<std::collections::HashMap<String, SessionApprovalState>>,
> = LazyLock::new(|| RwLock::new(std::collections::HashMap::new()));

fn with_state<R>(session_id: &str, f: impl FnOnce(&mut SessionApprovalState) -> R) -> R {
    let mut map = SESSION_APPROVALS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(map.entry(session_id.to_string()).or_default())
}

fn read_state<R>(session_id: &str, f: impl FnOnce(&SessionApprovalState) -> R) -> R {
    let map = SESSION_APPROVALS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match map.get(session_id) {
        Some(state) => f(state),
        None => f(&SessionApprovalState::default()),
    }
}

pub(crate) fn is_always_allowed(session_id: &str, tool: &str) -> bool {
    read_state(session_id, |state| state.always_allowed.contains(tool))
}

pub(crate) fn remember_always_allowed(session_id: &str, tool: &str) {
    with_state(session_id, |state| {
        state.always_allowed.insert(tool.to_string());
    });
}

pub(crate) fn plan_mode(session_id: &str) -> bool {
    read_state(session_id, |state| state.plan_mode)
}

pub(crate) fn set_plan_mode(session_id: &str, enabled: bool) {
    with_state(session_id, |state| state.plan_mode = enabled);
}

/// Drop a session's state. Called alongside the tool-policy cleanup so a long
/// lived server does not accumulate entries for sessions that are gone.
pub fn clear_session_approvals(session_id: &str) {
    SESSION_APPROVALS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_enter_approves_so_the_common_case_is_one_keystroke() {
        assert_eq!(parse_decision(""), Decision::Allow);
        assert_eq!(parse_decision("  "), Decision::Allow);
        assert_eq!(parse_decision("1"), Decision::Allow);
        assert_eq!(parse_decision("Yes"), Decision::Allow);
    }

    #[test]
    fn always_is_its_own_decision() {
        assert_eq!(parse_decision("2"), Decision::AllowAlways);
        assert_eq!(parse_decision("always"), Decision::AllowAlways);
    }

    /// The safety property: anything unrecognised must refuse, never run. A
    /// typo at a destructive-command prompt has to be harmless.
    #[test]
    fn anything_unrecognised_refuses_and_carries_the_reason() {
        match parse_decision("no, use the staging bucket instead") {
            Decision::Deny(reason) => {
                assert!(
                    reason.contains("staging bucket"),
                    "the user's reason must reach the model: {reason}"
                );
            }
            other => panic!("unrecognised answer must deny, got {other:?}"),
        }
        assert!(matches!(parse_decision("ys"), Decision::Deny(_)));
        assert!(matches!(parse_decision("3"), Decision::Deny(_)));
    }

    #[test]
    fn approval_prompt_shows_the_detail_and_the_choices() {
        let prompt = approval_prompt("bash", "rm -rf build/");
        assert!(prompt.contains("rm -rf build/"));
        assert!(prompt.contains("1. Yes"));
        assert!(prompt.contains("stop asking for `bash`"));
        assert!(prompt.contains("3. No"));
    }

    #[test]
    fn always_allowed_is_scoped_to_one_session() {
        let a = "session-approval-a";
        let b = "session-approval-b";
        clear_session_approvals(a);
        clear_session_approvals(b);

        remember_always_allowed(a, "bash");
        assert!(is_always_allowed(a, "bash"));
        assert!(!is_always_allowed(b, "bash"));
        assert!(!is_always_allowed(a, "write"));

        clear_session_approvals(a);
        assert!(!is_always_allowed(a, "bash"));
    }

    #[test]
    fn plan_mode_is_scoped_to_one_session() {
        let a = "session-plan-a";
        let b = "session-plan-b";
        clear_session_approvals(a);
        clear_session_approvals(b);

        assert!(!plan_mode(a));
        set_plan_mode(a, true);
        assert!(plan_mode(a));
        assert!(!plan_mode(b));

        set_plan_mode(a, false);
        assert!(!plan_mode(a));
    }
}
