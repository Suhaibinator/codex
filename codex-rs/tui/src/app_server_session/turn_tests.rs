use super::*;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::TurnStatus;
use futures::SinkExt;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn turn_start_preserves_model_and_surfaces_daybreak_rejection() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let websocket_url = format!("ws://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut websocket = accept_async(stream).await?;
        let mut requests = Vec::new();
        while let Some(frame) = websocket.next().await {
            let Message::Text(text) = frame? else {
                continue;
            };
            let JSONRPCMessage::Request(request) = serde_json::from_str(&text)? else {
                continue;
            };
            let response = match request.method.as_str() {
                "initialize" => json!({
                    "id": request.id,
                    "result": { "userAgent": "codex-tui-test" }
                }),
                "turn/start" => {
                    requests.push(request.params.expect("turn/start params"));
                    if requests.len() == 1 {
                        json!({
                            "id": request.id,
                            "result": {
                                "turn": { "id": "turn-1", "items": [], "status": "inProgress" }
                            }
                        })
                    } else {
                        json!({
                            "id": request.id,
                            "error": { "code": -32602, "message": "cyber program not authorized" }
                        })
                    }
                }
                method => panic!("unexpected app-server request: {method}"),
            };
            websocket
                .send(Message::Text(response.to_string().into()))
                .await?;
        }
        Ok::<_, color_eyre::Report>(requests)
    });
    let client = crate::connect_remote_app_server(
        codex_app_server_client::RemoteAppServerEndpoint::WebSocket {
            websocket_url,
            auth_token: None,
        },
    )
    .await?;
    let mut session = AppServerSession::new(client, ThreadParamsMode::Remote);
    let thread_id = ThreadId::new();
    let cwd = tempfile::tempdir()?;
    let models = ["gpt-5.6-sol", "another-selected-model"];
    let mut expected_requests = Vec::new();
    for (index, model) in models.into_iter().enumerate() {
        let input = vec![UserInput::Text {
            text: "hello".to_string(),
            text_elements: Vec::new(),
        }];
        expected_requests.push(serde_json::to_value(TurnStartParams {
            thread_id: thread_id.to_string(),
            input: input.clone(),
            cwd: Some(cwd.path().to_path_buf()),
            runtime_workspace_roots: Some(Vec::new()),
            approval_policy: Some(AskForApproval::OnRequest),
            approvals_reviewer: Some(codex_app_server_protocol::ApprovalsReviewer::User),
            model: Some(model.to_string()),
            cyber_access_program: Some(CyberAccessProgram::DaybreakBlue),
            ..Default::default()
        })?);
        let result = session
            .turn_start(
                thread_id,
                input,
                cwd.path().to_path_buf(),
                AskForApproval::OnRequest,
                codex_protocol::config_types::ApprovalsReviewer::User,
                TurnPermissionsOverride::Preserve,
                &[],
                model.to_string(),
                /*effort*/ None,
                /*summary*/ None,
                /*service_tier*/ None,
                /*collaboration_mode*/ None,
                /*personality*/ None,
                /*output_schema*/ None,
            )
            .await;
        if index == 0 {
            assert_eq!(result?.turn.status, TurnStatus::InProgress);
        } else {
            let error = result.expect_err("server rejection must reach the caller");
            assert!(format!("{error:#}").contains("cyber program not authorized"));
        }
    }
    session.shutdown().await?;
    // Exact wire requests also prove rejection did not retry with a different
    // model or silently drop the requested program.
    assert_eq!(server.await??, expected_requests);
    Ok(())
}
