// Tests for answering a command that blocks reading stdin.
//
// The server suspends the tool on a `oneshot` and emits `StdinRequest`; the turn
// stays blocked until a `StdinResponse` comes back. The TUI used to only print a
// notice and let the command time out, so these pin the state transition that
// makes the prompt answerable.

#[test]
fn stdin_request_arms_the_composer_to_answer_the_command() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::StdinRequest {
            request_id: "req-1".to_string(),
            prompt: "Continue? [y/N]".to_string(),
            is_password: false,
            tool_call_id: "call-1".to_string(),
        },
        &mut remote,
    );

    let pending = app
        .pending_stdin_request
        .as_ref()
        .expect("stdin request must arm the composer");
    assert_eq!(pending.request_id, "req-1");
    assert_eq!(pending.prompt, "Continue? [y/N]");
    assert!(!pending.is_password);

    let transcript = app
        .display_messages()
        .iter()
        .map(|msg| msg.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        transcript.contains("Continue? [y/N]"),
        "the prompt must be surfaced: {transcript}"
    );
    assert!(
        transcript.contains("press Enter"),
        "the user needs to be told how to answer: {transcript}"
    );
}

/// A supplied prompt means the agent is asking something (`ask_user`); an empty
/// one means a child process is reading stdin. Same wire message, so the
/// wording has to be driven by the prompt or one of the two reads as nonsense.
#[test]
fn stdin_request_without_a_prompt_reads_as_a_command_not_a_question() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::StdinRequest {
            request_id: "req-raw".to_string(),
            prompt: String::new(),
            is_password: false,
            tool_call_id: "call-3".to_string(),
        },
        &mut remote,
    );

    assert!(app.pending_stdin_request.is_some());
    let transcript = app
        .display_messages()
        .iter()
        .map(|msg| msg.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        transcript.contains("running command is waiting for input"),
        "raw stdin must not be worded as a question: {transcript}"
    );
}

/// The composer echoes what it is given and has no masking, so a password
/// prompt must be declined rather than collected in the clear.
#[test]
fn stdin_request_for_a_password_is_declined_rather_than_echoed() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::StdinRequest {
            request_id: "req-secret".to_string(),
            prompt: "Password:".to_string(),
            is_password: true,
            tool_call_id: "call-2".to_string(),
        },
        &mut remote,
    );

    assert!(
        app.pending_stdin_request.is_none(),
        "a password prompt must not arm the composer"
    );
    let transcript = app
        .display_messages()
        .iter()
        .map(|msg| msg.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        transcript.contains("password"),
        "the refusal must say why: {transcript}"
    );
}
