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
        transcript.contains("waiting for input"),
        "the user needs to be told why the turn stalled: {transcript}"
    );
    assert!(
        transcript.contains("Continue? [y/N]"),
        "the command's own prompt must be surfaced: {transcript}"
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
