//! The `ask_user` tool: pause a turn to ask the human something, then resume
//! with their answer.
//!
//! This rides the existing stdin request pipe rather than adding a second one.
//! That pipe already suspends a tool on a `oneshot`, carries the prompt to the
//! client over `ServerEvent::StdinRequest`, and resolves the sender when
//! `Request::StdinResponse` comes back — which is exactly the round trip a
//! question needs. Reusing it means no protocol change and no second code path
//! to keep in sync.
//!
//! # Sessions with nobody watching
//!
//! The dangerous case is a session with no attached client: headless, ambient,
//! and swarm workers. There the request would go nowhere and the turn would
//! block forever. `ToolContext::stdin_request_tx` is `None` in exactly those
//! sessions, so the tool fails fast and tells the model to decide on its own
//! instead of waiting.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

/// Guards against a prompt so large it would crowd out the transcript.
const MAX_QUESTION_CHARS: usize = 2000;
const MAX_OPTIONS: usize = 10;

pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AskUserTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct AskUserInput {
    question: String,
    #[serde(default)]
    options: Option<Vec<String>>,
}

/// Render the question the user will read, numbering options so a short reply
/// like "2" is enough to answer.
fn format_prompt(question: &str, options: &[String]) -> String {
    let mut prompt = question.trim().to_string();
    if options.is_empty() {
        return prompt;
    }
    prompt.push_str("\n");
    for (index, option) in options.iter().enumerate() {
        prompt.push_str(&format!("\n  {}. {}", index + 1, option.trim()));
    }
    prompt.push_str("\n\nReply with a number or your own answer.");
    prompt
}

/// Resolve what the user typed against the offered options.
///
/// A bare index selects that option; anything else is passed through verbatim,
/// so the user is never forced into the choices the model imagined.
fn resolve_answer(raw: &str, options: &[String]) -> String {
    let trimmed = raw.trim();
    if options.is_empty() {
        return trimmed.to_string();
    }
    if let Ok(index) = trimmed.parse::<usize>()
        && index >= 1
        && index <= options.len()
    {
        return options[index - 1].trim().to_string();
    }
    trimmed.to_string()
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question and wait for their answer before continuing. Use this \
         when a decision is genuinely theirs — an ambiguous requirement, a choice between \
         approaches with real trade-offs, or a destructive action that needs confirmation. \
         Do not use it for things you can determine yourself from the code, and do not use \
         it to ask permission to keep working."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["question"],
            "properties": {
                "intent": super::intent_schema_property(),
                "question": {
                    "type": "string",
                    "description": "The question, written so it can be answered without \
                                    reading the rest of the conversation. State what you \
                                    need to know and why it changes what you will do."
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional concrete choices. The user may still answer \
                                    freely, so do not rely on getting one of these back."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: AskUserInput = serde_json::from_value(input)?;

        let question = params.question.trim();
        if question.is_empty() {
            return Err(anyhow::anyhow!("ask_user requires a non-empty question"));
        }
        if question.chars().count() > MAX_QUESTION_CHARS {
            return Err(anyhow::anyhow!(
                "ask_user question is too long ({} chars, max {}). Ask something shorter.",
                question.chars().count(),
                MAX_QUESTION_CHARS
            ));
        }

        let options: Vec<String> = params
            .options
            .unwrap_or_default()
            .into_iter()
            .filter(|option| !option.trim().is_empty())
            .take(MAX_OPTIONS)
            .collect();

        // No attached client means nobody can answer. Fail fast rather than
        // block the turn forever.
        let Some(stdin_tx) = ctx.stdin_request_tx.clone() else {
            return Err(anyhow::anyhow!(
                "No interactive user is attached to this session, so ask_user cannot be \
                 answered. Decide yourself using the information you have, state the \
                 assumption you made, and continue."
            ));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let request = jcode_tool_core::StdinInputRequest {
            request_id: format!("ask-{}", ctx.tool_call_id),
            prompt: format_prompt(question, &options),
            is_password: false,
            response_tx,
        };

        if stdin_tx.send(request).is_err() {
            return Err(anyhow::anyhow!(
                "The session's client disconnected before the question could be asked. \
                 Decide yourself and continue."
            ));
        }

        let answer = match response_rx.await {
            Ok(answer) => answer,
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "The question was dismissed without an answer. Decide yourself and continue."
                ));
            }
        };

        let resolved = resolve_answer(&answer, &options);
        if resolved.is_empty() {
            return Ok(ToolOutput::new(
                "The user answered with an empty line. Treat that as \"no preference\" and \
                 proceed with your own judgement.",
            ));
        }

        Ok(ToolOutput::new(format!("The user answered: {resolved}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_numbers_options_so_a_short_reply_is_enough() {
        let prompt = format_prompt("Which database?", &["Postgres".into(), "SQLite".into()]);
        assert!(prompt.starts_with("Which database?"));
        assert!(prompt.contains("1. Postgres"));
        assert!(prompt.contains("2. SQLite"));
        assert!(prompt.contains("Reply with a number"));
    }

    #[test]
    fn prompt_without_options_is_just_the_question() {
        let prompt = format_prompt("  Which database?  ", &[]);
        assert_eq!(prompt, "Which database?");
    }

    #[test]
    fn a_bare_index_selects_that_option() {
        let options = vec!["Postgres".to_string(), "SQLite".to_string()];
        assert_eq!(resolve_answer("2", &options), "SQLite");
        assert_eq!(resolve_answer("  1  ", &options), "Postgres");
    }

    /// The user must never be trapped in the choices the model imagined, so
    /// free text always wins over the option list.
    #[test]
    fn free_text_passes_through_even_when_options_exist() {
        let options = vec!["Postgres".to_string(), "SQLite".to_string()];
        assert_eq!(
            resolve_answer("actually use DuckDB", &options),
            "actually use DuckDB"
        );
    }

    /// An index outside the list is a typo, not a selection; passing it through
    /// lets the model see what was actually typed.
    #[test]
    fn out_of_range_index_is_not_treated_as_a_selection() {
        let options = vec!["Postgres".to_string(), "SQLite".to_string()];
        assert_eq!(resolve_answer("7", &options), "7");
        assert_eq!(resolve_answer("0", &options), "0");
    }
}
