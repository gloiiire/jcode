//! Plan mode: investigate first, agree on the plan, then act.
//!
//! Two tools bracket it. `enter_plan_mode` closes the door on anything that
//! would change the world; `propose_plan` puts the plan to the user and only
//! reopens it if they approve. The refusal in between lives in
//! `Registry::execute`, so it covers every mutating tool rather than relying on
//! the model to hold back.
//!
//! Entering is a tool call rather than a client-side flag on purpose: it needs
//! no protocol change, and `/plan` already works by telling the model what to
//! do. The enforcement is real once entered, which is the part that matters —
//! a model that talks itself out of planning still cannot edit anything.

use super::confirm;
use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_PLAN_CHARS: usize = 8000;

pub struct EnterPlanModeTool;
pub struct ProposePlanTool;

impl EnterPlanModeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EnterPlanModeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ProposePlanTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProposePlanTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str {
        "enter_plan_mode"
    }

    fn description(&self) -> &str {
        "Enter plan mode: read-only tools keep working, but anything that edits files or \
         runs commands is refused until the user approves a plan. Call this when the user \
         asks for a plan, or when the work is large or risky enough that agreeing on the \
         approach first is worth a round trip."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "intent": super::intent_schema_property() }
        })
    }

    async fn execute(&self, _input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        confirm::set_plan_mode(&ctx.session_id, true);
        Ok(ToolOutput::new(
            "Plan mode is on. Read-only tools still work; editing and running commands will be \
             refused. Investigate as much as you need, then call `propose_plan` with what you \
             intend to do. The user has to approve it before anything runs.",
        ))
    }
}

#[derive(Deserialize)]
struct ProposePlanInput {
    plan: String,
}

#[async_trait]
impl Tool for ProposePlanTool {
    fn name(&self) -> &str {
        "propose_plan"
    }

    fn description(&self) -> &str {
        "Put a plan to the user and wait for their answer. On approval, plan mode is lifted \
         and you may act. On refusal, plan mode stays on and you get their reasoning, which \
         you should use to revise the plan and propose again."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["plan"],
            "properties": {
                "intent": super::intent_schema_property(),
                "plan": {
                    "type": "string",
                    "description": "What you intend to do, concretely: the files you will \
                                    change, the commands you will run, and anything \
                                    irreversible. Written so the user can spot a wrong \
                                    assumption without reading the code themselves."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: ProposePlanInput = serde_json::from_value(input)?;
        let plan = params.plan.trim();
        if plan.is_empty() {
            return Err(anyhow::anyhow!("propose_plan requires a non-empty plan"));
        }
        if plan.chars().count() > MAX_PLAN_CHARS {
            return Err(anyhow::anyhow!(
                "The plan is too long ({} chars, max {}). Summarise it.",
                plan.chars().count(),
                MAX_PLAN_CHARS
            ));
        }

        let prompt = format!(
            "{plan}\n\n  1. Approve and start\n  2. No\n\n\
             Enter alone approves. Anything else keeps plan mode on and is passed back to the \
             agent as your reasoning."
        );

        let answer =
            match confirm::request_user_input(&ctx, format!("plan-{}", ctx.tool_call_id), prompt)
                .await
            {
                Ok(answer) => answer,
                // Nobody to approve it. Leaving plan mode on would deadlock the
                // session, so lift it and make the model say it was unreviewed.
                Err(error) => {
                    confirm::set_plan_mode(&ctx.session_id, false);
                    return Ok(ToolOutput::new(format!(
                        "The plan could not be reviewed ({}). Plan mode has been lifted so you can \
                     proceed, but say clearly in your next message that the plan went \
                     unapproved.",
                        error.as_str()
                    )));
                }
            };

        match confirm::parse_decision(&answer) {
            confirm::Decision::Allow | confirm::Decision::AllowAlways => {
                confirm::set_plan_mode(&ctx.session_id, false);
                Ok(ToolOutput::new(
                    "The user approved the plan. Plan mode is lifted; carry it out. If you find \
                     you need to depart from it, say so rather than quietly doing something \
                     else.",
                ))
            }
            confirm::Decision::Deny(reason) => Ok(ToolOutput::new(format!(
                "The user did not approve the plan. {reason} Plan mode is still on. Revise the \
                 plan to address what they said and call `propose_plan` again."
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_tool_core::ToolExecutionMode;

    fn ctx(session_id: &str) -> ToolContext {
        ToolContext {
            session_id: session_id.to_string(),
            message_id: "m".to_string(),
            tool_call_id: "t".to_string(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::AgentTurn,
        }
    }

    #[tokio::test]
    async fn entering_plan_mode_sets_the_session_flag() {
        let session = "plan-tool-enter";
        confirm::clear_session_approvals(session);

        EnterPlanModeTool::new()
            .execute(json!({}), ctx(session))
            .await
            .expect("enter_plan_mode");

        assert!(confirm::plan_mode(session));
        confirm::clear_session_approvals(session);
    }

    /// With nobody to approve it, staying in plan mode would deadlock the
    /// session: every mutating tool refused, and no way to lift the refusal.
    #[tokio::test]
    async fn an_unreviewable_plan_lifts_plan_mode_instead_of_deadlocking() {
        let session = "plan-tool-headless";
        confirm::clear_session_approvals(session);
        confirm::set_plan_mode(session, true);

        let output = ProposePlanTool::new()
            .execute(json!({ "plan": "Rewrite the parser" }), ctx(session))
            .await
            .expect("propose_plan");

        assert!(!confirm::plan_mode(session), "plan mode must be lifted");
        assert!(
            output.output.contains("unapproved"),
            "the model must be told to disclose it: {}",
            output.output
        );
        confirm::clear_session_approvals(session);
    }

    #[tokio::test]
    async fn an_empty_plan_is_rejected() {
        let session = "plan-tool-empty";
        confirm::clear_session_approvals(session);
        let result = ProposePlanTool::new()
            .execute(json!({ "plan": "   " }), ctx(session))
            .await;
        assert!(result.is_err());
        confirm::clear_session_approvals(session);
    }
}
