mod forward;
mod listen;
pub mod route;
pub mod settings;

use std::sync::{Mutex, Weak};

use anyhow::Result;

use crate::provider::Provider;

/// What the listener needs from the process that runs it.
pub trait Backend: Send + Sync {
    fn settings(&self) -> Result<settings::Settings>;
    fn providers(&self) -> Vec<&dyn Provider>;
}

/// Whether the listener runs, and on which port; started, moved and stopped
/// to follow the settings.
#[derive(Default)]
pub struct Control {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    running: Option<listen::Running>,
    /// The port asked for, which differs from the one bound when it is 0.
    asked: Option<u16>,
    error: Option<String>,
}

impl Control {
    pub fn apply(&self, backend: Weak<dyn Backend>, settings: &settings::Settings) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let want = settings.enabled.then_some(settings.port);
        if state.asked == want && (state.running.is_some() || want.is_none()) {
            return;
        }
        if let Some(running) = state.running.take() {
            running.stop();
        }
        state.asked = want;
        state.error = None;
        if let Some(port) = want {
            match listen::start(backend, port) {
                Ok(running) => state.running = Some(running),
                Err(e) => state.error = Some(format!("{e:#}")),
            }
        }
    }

    /// The bound port, or why there is none while it is enabled.
    pub fn status(&self) -> (Option<u16>, Option<String>) {
        match self.state.lock() {
            Ok(s) => (s.running.as_ref().map(|r| r.port), s.error.clone()),
            Err(_) => (None, Some("the proxy's state is poisoned".to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::Arc;

    use mockito::Matcher;
    use serde_json::{Value, json};

    use super::*;
    use crate::claude::{self, Claude};
    use crate::codex::{self, Codex};
    use crate::oauth::fake_jwt;
    use crate::ops::testing::*;

    const KEY: &str = "rmd-test";

    struct Test {
        providers: Vec<Box<dyn Provider>>,
        settings: settings::Settings,
    }

    impl Backend for Test {
        fn settings(&self) -> Result<settings::Settings> {
            Ok(self.settings.clone())
        }
        fn providers(&self) -> Vec<&dyn Provider> {
            self.providers.iter().map(|p| p.as_ref()).collect()
        }
    }

    fn on() -> settings::Settings {
        settings::Settings {
            enabled: true,
            port: 0,
            keys: vec![settings::Key {
                id: "k".into(),
                name: "test".into(),
                key: KEY.into(),
                created: 0,
            }],
        }
    }

    struct Running {
        _backend: Arc<Test>,
        control: Control,
        port: u16,
    }

    fn run(providers: Vec<Box<dyn Provider>>) -> Running {
        let backend = Arc::new(Test {
            providers,
            settings: on(),
        });
        let control = Control::default();
        let weak: Weak<dyn Backend> = Arc::downgrade(&backend) as Weak<dyn Backend>;
        control.apply(weak, &backend.settings);
        let port = control.status().0.expect("the proxy did not start");
        Running {
            _backend: backend,
            control,
            port,
        }
    }

    fn send(port: u16, raw: &str) -> (u16, String, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).unwrap();
        let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, head.to_owned(), body.to_owned())
    }

    fn post(port: u16, path: &str, headers: &str, body: &Value) -> (u16, String, String) {
        let body = body.to_string();
        send(
            port,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: x\r\n{headers}Content-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    fn signed_in_claude(server: &mockito::Server, expires_in: i64) -> (tempfile::TempDir, Claude) {
        let e = env(&server.url());
        e.sign_in(oauth("a-live", "r-live", expires_in), account("u-1"));
        let claude = Claude::at(e.tmp.path(), claude::Api::local(&server.url()));
        (e.tmp, claude)
    }

    #[test]
    fn a_request_without_a_key_is_refused_in_the_clients_shape() {
        let p = run(Vec::new());
        let (status, _, body) = post(p.port, "/v1/messages", "", &json!({}));
        assert_eq!(status, 401);
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["error"]["type"], "authentication_error");
        let (status, _, body) = post(
            p.port,
            "/v1/chat/completions",
            "Authorization: Bearer rmd-wrong\r\n",
            &json!({}),
        );
        assert_eq!(status, 401);
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["error"]["code"], 401);
    }

    #[test]
    fn a_refused_client_reads_the_refusal_rather_than_a_reset() {
        let p = run(Vec::new());
        let body = "x".repeat(2 * 1024 * 1024);
        let mut s = TcpStream::connect(("127.0.0.1", p.port)).unwrap();
        write!(
            s,
            "POST /v1/messages HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .unwrap();
        let _ = s.write_all(body.as_bytes());
        let mut answer = Vec::new();
        let _ = s.read_to_end(&mut answer);
        let answer = String::from_utf8_lossy(&answer);
        assert!(answer.starts_with("HTTP/1.1 401"), "{answer}");
        assert!(answer.contains("nosniff"), "{answer}");
    }

    #[test]
    fn a_client_waiting_for_continue_is_told_to_go_on() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/v1/messages")
            .match_query(Matcher::Any)
            .with_body("{}")
            .create();
        let (_tmp, claude) = signed_in_claude(&server, HOUR);
        let p = run(vec![Box::new(claude)]);
        let body = json!({"model": "claude-x", "messages": []}).to_string();
        let mut s = TcpStream::connect(("127.0.0.1", p.port)).unwrap();
        write!(
            s,
            "POST /v1/messages HTTP/1.1\r\nx-api-key: {KEY}\r\nExpect: 100-continue\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .unwrap();
        let mut first = [0u8; 25];
        s.read_exact(&mut first).unwrap();
        assert_eq!(&first, b"HTTP/1.1 100 Continue\r\n\r\n");
        s.write_all(body.as_bytes()).unwrap();
        let mut rest = String::new();
        s.read_to_string(&mut rest).unwrap();
        assert!(rest.starts_with("HTTP/1.1 200"), "{rest}");
    }

    #[test]
    fn claude_requests_go_out_on_the_active_login_as_claude_code() {
        let mut server = mockito::Server::new();
        let upstream = server
            .mock("POST", "/v1/messages")
            .match_query(Matcher::UrlEncoded("beta".into(), "true".into()))
            .match_header("authorization", "Bearer a-live")
            .match_header("x-app", "cli")
            .match_header(
                "anthropic-beta",
                Matcher::Regex("oauth-2025-04-20.*,extra-beta$".into()),
            )
            .match_body(Matcher::PartialJson(json!({
                "model": "claude-sonnet-4-5",
                "system": [
                    {"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude."},
                    {"type": "text", "text": "be brief"},
                ],
            })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_header("request-id", "req-1")
            .with_body("event: message_start\ndata: {}\n\n")
            .create();
        let (_tmp, claude) = signed_in_claude(&server, HOUR);
        let p = run(vec![Box::new(claude)]);
        let (status, head, body) = post(
            p.port,
            "/v1/messages",
            &format!("x-api-key: {KEY}\r\nanthropic-beta: extra-beta\r\nUser-Agent: some-sdk\r\n"),
            &json!({"model": "claude-sonnet-4-5", "system": "be brief", "stream": true, "messages": []}),
        );
        upstream.assert();
        assert_eq!(status, 200);
        assert!(head.contains("text/event-stream"), "{head}");
        assert!(head.contains("request-id: req-1"), "{head}");
        assert_eq!(body, "event: message_start\ndata: {}\n\n");
    }

    #[test]
    fn claude_codes_own_requests_pass_untouched() {
        let mut server = mockito::Server::new();
        let upstream = server
            .mock("POST", "/v1/messages")
            .match_query(Matcher::Any)
            .match_body(Matcher::PartialJson(json!({"system": "mine"})))
            .with_body("{}")
            .create();
        let (_tmp, claude) = signed_in_claude(&server, HOUR);
        let p = run(vec![Box::new(claude)]);
        let (status, _, _) = post(
            p.port,
            "/v1/messages",
            &format!("x-api-key: {KEY}\r\nUser-Agent: claude-cli/2.1.304 (external, cli)\r\n"),
            &json!({"model": "claude-haiku-4-5", "system": "mine", "messages": []}),
        );
        assert_eq!(status, 200);
        upstream.assert();
    }

    #[test]
    fn upstream_errors_come_back_as_they_are() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/v1/messages")
            .match_query(Matcher::Any)
            .with_status(429)
            .with_header("retry-after", "30")
            .with_body(r#"{"type":"error"}"#)
            .create();
        let (_tmp, claude) = signed_in_claude(&server, HOUR);
        let p = run(vec![Box::new(claude)]);
        let (status, head, body) = post(
            p.port,
            "/v1/messages",
            &format!("x-api-key: {KEY}\r\n"),
            &json!({"model": "claude-x", "messages": []}),
        );
        assert_eq!(status, 429);
        assert!(head.contains("retry-after: 30"), "{head}");
        assert_eq!(body, r#"{"type":"error"}"#);
    }

    #[test]
    fn the_active_login_is_spent_but_never_refreshed() {
        let mut server = mockito::Server::new();
        let refresh = server.mock("POST", "/v1/oauth/token").expect(0).create();
        let (tmp, claude) = signed_in_claude(&server, -HOUR);
        let p = run(vec![Box::new(claude)]);
        let auth = format!("Authorization: Bearer {KEY}\r\n");
        let (status, _, body) = post(
            p.port,
            "/v1/messages",
            &auth,
            &json!({"model": "claude-x", "messages": []}),
        );
        assert_eq!(status, 401);
        assert!(body.contains("expired"), "{body}");
        refresh.assert();

        std::fs::remove_file(tmp.path().join(".credentials.json")).unwrap();
        let (status, _, body) = post(
            p.port,
            "/v1/messages",
            &auth,
            &json!({"model": "claude-x", "messages": []}),
        );
        assert_eq!(status, 503);
        assert!(body.contains("signed out"), "{body}");
    }

    #[test]
    fn a_format_the_provider_does_not_take_is_named() {
        let server = mockito::Server::new();
        let (_tmp, claude) = signed_in_claude(&server, HOUR);
        let p = run(vec![Box::new(claude)]);
        let auth = format!("Authorization: Bearer {KEY}\r\n");
        let (status, _, body) = post(
            p.port,
            "/v1/chat/completions",
            &auth,
            &json!({"model": "claude-x", "messages": []}),
        );
        assert_eq!(status, 400);
        assert!(body.contains("takes Anthropic messages"), "{body}");
        let (status, _, body) = post(
            p.port,
            "/v1/chat/completions",
            &auth,
            &json!({"model": "llama"}),
        );
        assert_eq!(status, 404);
        assert!(body.contains("provider/model"), "{body}");
        let (status, _, _) = post(p.port, "/v1/chat/completions", &auth, &json!({}));
        assert_eq!(status, 400);
        let (status, _, _) = post(p.port, "/v1/nothing", &auth, &json!({}));
        assert_eq!(status, 404);
    }

    #[test]
    fn a_chunked_request_is_read() {
        let mut server = mockito::Server::new();
        let upstream = server
            .mock("POST", "/v1/messages")
            .match_query(Matcher::Any)
            .with_body("{}")
            .create();
        let (_tmp, claude) = signed_in_claude(&server, HOUR);
        let p = run(vec![Box::new(claude)]);
        let body = json!({"model": "claude-x", "messages": []}).to_string();
        let (half, rest) = body.split_at(10);
        let (status, _, _) = send(
            p.port,
            &format!(
                "POST /v1/messages HTTP/1.1\r\nx-api-key: {KEY}\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{half}\r\n{:x}\r\n{rest}\r\n0\r\n\r\n",
                half.len(),
                rest.len()
            ),
        );
        assert_eq!(status, 200);
        upstream.assert();
    }

    fn signed_in_codex(server: &mockito::Server) -> (tempfile::TempDir, Codex) {
        let tmp = tempfile::tempdir().unwrap();
        let exp = crate::fsx::now_ms() / 1000 + 3600;
        let claims = json!({"exp": exp, "https://api.openai.com/auth": {"chatgpt_account_user_id": "user-1__ws-1"}});
        crate::fsx::write_json(
            &tmp.path().join("auth.json"),
            &json!({"tokens": {
                "access_token": fake_jwt(claims.clone()),
                "id_token": fake_jwt(claims),
                "refresh_token": "r",
                "account_id": "ws-1",
            }}),
        )
        .unwrap();
        let codex = Codex::at(tmp.path(), codex::Api::local(&server.url()), 0);
        (tmp, codex)
    }

    #[test]
    fn codex_gets_a_streamed_unstored_request_for_its_workspace() {
        let mut server = mockito::Server::new();
        let upstream = server
            .mock("POST", "/responses")
            .match_header("chatgpt-account-id", "ws-1")
            .match_header("authorization", Matcher::Regex("^Bearer ey".into()))
            .match_body(Matcher::PartialJson(
                json!({"store": false, "stream": true, "instructions": ""}),
            ))
            .with_header("content-type", "text/event-stream")
            .with_body("data: {}\n\n")
            .create();
        let (_tmp, codex) = signed_in_codex(&server);
        let p = run(vec![Box::new(codex)]);
        let auth = format!("Authorization: Bearer {KEY}\r\n");
        let (status, _, body) = post(
            p.port,
            "/v1/responses",
            &auth,
            &json!({"model": "gpt-5", "input": "hi", "stream": true, "temperature": 1}),
        );
        assert_eq!(status, 200, "{body}");
        upstream.assert();
        let (status, _, body) = post(
            p.port,
            "/v1/responses",
            &auth,
            &json!({"model": "gpt-5", "input": "hi"}),
        );
        assert_eq!(status, 400);
        assert!(body.contains("stream"), "{body}");
    }

    #[test]
    fn models_lists_the_providers_with_an_active_login() {
        let server = mockito::Server::new();
        let (_tmp, claude) = signed_in_claude(&server, HOUR);
        let p = run(vec![Box::new(claude)]);
        let get = |extra: &str| {
            send(
                p.port,
                &format!("GET /v1/models HTTP/1.1\r\nAuthorization: Bearer {KEY}\r\n{extra}\r\n"),
            )
        };
        let (status, _, body) = get("");
        assert_eq!(status, 200);
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["object"], "list");
        let (_, _, body) = get("anthropic-version: 2023-06-01\r\n");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["data"][0]["type"], "model");
        assert_eq!(body["data"][0]["id"], "claude-opus-4-5");
        assert_eq!(body["has_more"], false);
    }

    #[test]
    fn turning_it_off_frees_the_port() {
        let p = run(Vec::new());
        let backend: Arc<dyn Backend> = Arc::new(Test {
            providers: Vec::new(),
            settings: on(),
        });
        let off = settings::Settings {
            enabled: false,
            ..on()
        };
        p.control.apply(Arc::downgrade(&backend), &off);
        assert_eq!(p.control.status(), (None, None));
        assert!(std::net::TcpListener::bind(("127.0.0.1", p.port)).is_ok());

        let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy = settings::Settings {
            port: taken.local_addr().unwrap().port(),
            ..on()
        };
        p.control.apply(Arc::downgrade(&backend), &busy);
        let (running, error) = p.control.status();
        assert_eq!(running, None);
        assert!(error.unwrap().contains("cannot listen"));
    }

    #[test]
    fn an_api_key_goes_to_the_path_its_plan_draws_on() {
        let mut server = mockito::Server::new();
        let chat = server
            .mock("POST", "/coding/paas/v4/chat/completions")
            .match_header("authorization", "Bearer zk-1")
            .with_body("{}")
            .create();
        let messages = server
            .mock("POST", "/anthropic/v1/messages")
            .match_header("x-api-key", "zk-1")
            .match_header("anthropic-version", "2023-06-01")
            .with_body("{}")
            .create();
        let glm = crate::apikey::ApiKey::glm()
            .with(None, |name| {
                (name == "Z_AI_API_KEY").then(|| "zk-1".to_owned())
            })
            .inferring_at(&server.url());
        let p = run(vec![Box::new(glm)]);
        let auth = format!("Authorization: Bearer {KEY}\r\n");
        let model = json!({"model": "glm-4.6", "messages": []});
        assert_eq!(post(p.port, "/v1/chat/completions", &auth, &model).0, 200);
        assert_eq!(post(p.port, "/v1/messages", &auth, &model).0, 200);
        let (status, _, body) = post(p.port, "/v1/responses", &auth, &model);
        assert_eq!(status, 400);
        assert!(
            body.contains("GLM takes Anthropic messages or OpenAI chat completions"),
            "{body}"
        );
        chat.assert();
        messages.assert();
    }

    #[test]
    fn opencode_is_reached_by_name_in_each_models_own_format() {
        let opencode = crate::apikey::ApiKey::opencode().with(None, |_| None);
        assert_eq!(
            opencode.formats("claude-sonnet-4-5"),
            vec![crate::provider::Format::Messages]
        );
        assert_eq!(
            opencode.formats("gpt-5"),
            vec![crate::provider::Format::Responses]
        );
        assert_eq!(
            opencode.formats("glm-4.6"),
            vec![crate::provider::Format::Chat]
        );
        assert_eq!(
            route::route("opencode/claude-sonnet-4-5", &["claude", "opencode"]),
            Some(("opencode".to_owned(), "claude-sonnet-4-5"))
        );
    }

    #[test]
    fn grok_goes_out_as_the_grok_cli() {
        let mut server = mockito::Server::new();
        let upstream = server
            .mock("POST", "/responses")
            .match_header("authorization", Matcher::Regex("^Bearer ey".into()))
            .match_header("x-xai-token-auth", "xai-grok-cli")
            .match_header("x-grok-client-identifier", "grok-shell")
            .match_header(
                "x-grok-client-version",
                Matcher::Regex(r"^\d+\.\d+\.\d+$".into()),
            )
            .match_header("user-agent", Matcher::Regex("^xai-grok-workspace/".into()))
            .with_body("{}")
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let exp = crate::fsx::now_ms() / 1000 + 3600;
        crate::fsx::write_json(
            &tmp.path().join("auth.json"),
            &json!({"https://auth.x.ai::client": {"key": fake_jwt(json!({"exp": exp, "sub": "u"})), "refresh_token": "r"}}),
        )
        .unwrap();
        let grok = crate::grok::Grok::at(tmp.path(), crate::grok::Api::local(&server.url()));
        let p = run(vec![Box::new(grok)]);
        let (status, _, body) = post(
            p.port,
            "/v1/responses",
            &format!("Authorization: Bearer {KEY}\r\n"),
            &json!({"model": "grok-4", "input": "hi"}),
        );
        assert_eq!(status, 200, "{body}");
        upstream.assert();
    }

    #[test]
    fn kimi_takes_every_format_on_its_own_path() {
        let mut server = mockito::Server::new();
        let mocks: Vec<_> = ["/chat/completions", "/responses", "/messages"]
            .iter()
            .map(|path| {
                server
                    .mock("POST", *path)
                    .match_header("authorization", "Bearer k-live")
                    .match_header("x-msh-platform", "kimi_code_cli")
                    .with_body("{}")
                    .create()
            })
            .collect();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("credentials")).unwrap();
        crate::fsx::write_json(
            &tmp.path().join("credentials/kimi-code.json"),
            &json!({"access_token": "k-live", "refresh_token": "r", "expires_at": crate::fsx::now_ms() / 1000 + 3600}),
        )
        .unwrap();
        let kimi = crate::kimi::Kimi::at(tmp.path(), crate::kimi::Api::local(&server.url()));
        let p = run(vec![Box::new(kimi)]);
        let auth = format!("Authorization: Bearer {KEY}\r\n");
        for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
            let (status, _, body) = post(p.port, path, &auth, &json!({"model": "kimi-for-coding"}));
            assert_eq!(status, 200, "{path}: {body}");
        }
        for m in mocks {
            m.assert();
        }
    }

    #[test]
    fn opencode_sends_each_family_to_its_makers_path() {
        let mut server = mockito::Server::new();
        let mocks = [
            server
                .mock("POST", "/anthropic/v1/messages")
                .match_header("x-api-key", "oc-1")
                .with_body("{}")
                .create(),
            server
                .mock("POST", "/openai/v1/responses")
                .match_header("authorization", "Bearer oc-1")
                .with_body("{}")
                .create(),
            server
                .mock("POST", "/openai/v1/chat/completions")
                .match_header("authorization", "Bearer oc-1")
                .with_body("{}")
                .create(),
        ];
        let opencode = crate::apikey::ApiKey::opencode()
            .with(None, |name| {
                (name == "OPENCODE_API_KEY").then(|| "oc-1".to_owned())
            })
            .inferring_at(&server.url());
        let p = run(vec![Box::new(opencode)]);
        let auth = format!("Authorization: Bearer {KEY}\r\n");
        let send = |path: &str, model: &str| post(p.port, path, &auth, &json!({"model": model})).0;
        assert_eq!(send("/v1/messages", "opencode/claude-sonnet-4-5"), 200);
        assert_eq!(send("/v1/responses", "opencode/gpt-5"), 200);
        assert_eq!(send("/v1/chat/completions", "opencode/glm-4.6"), 200);
        assert_eq!(send("/v1/chat/completions", "opencode/gemini-3-pro"), 400);
        for m in mocks {
            m.assert();
        }
    }

    #[test]
    fn codex_is_sent_none_of_the_fields_it_refuses() {
        let mut server = mockito::Server::new();
        let upstream = server
            .mock("POST", "/responses")
            .match_request(|req| {
                let body: Value = serde_json::from_slice(req.body().unwrap()).unwrap();
                [
                    "temperature",
                    "top_p",
                    "max_output_tokens",
                    "previous_response_id",
                    "stream_options",
                ]
                .iter()
                .all(|f| body.get(f).is_none())
                    && req.header("chatgpt-account-id").is_empty()
            })
            .with_body("data: {}\n\n")
            .create();
        let tmp = tempfile::tempdir().unwrap();
        let exp = crate::fsx::now_ms() / 1000 + 3600;
        let claims = json!({"exp": exp, "https://api.openai.com/auth": {"chatgpt_account_user_id": "user-1"}});
        crate::fsx::write_json(
            &tmp.path().join("auth.json"),
            &json!({"tokens": {"access_token": fake_jwt(claims.clone()), "id_token": fake_jwt(claims), "refresh_token": "r"}}),
        )
        .unwrap();
        let codex = Codex::at(tmp.path(), codex::Api::local(&server.url()), 0);
        let p = run(vec![Box::new(codex)]);
        let (status, _, body) = post(
            p.port,
            "/v1/responses",
            &format!("Authorization: Bearer {KEY}\r\nsession_id: s-1\r\n"),
            &json!({"model": "gpt-5", "input": "hi", "stream": true, "temperature": 1, "top_p": 1,
                    "max_output_tokens": 9, "previous_response_id": "r0", "stream_options": {}}),
        );
        assert_eq!(status, 200, "{body}");
        upstream.assert();
    }

    #[test]
    fn claude_counts_tokens_on_its_own_endpoint() {
        let mut server = mockito::Server::new();
        let upstream = server
            .mock("POST", "/v1/messages/count_tokens")
            .match_query(Matcher::UrlEncoded("beta".into(), "true".into()))
            .with_body(r#"{"input_tokens":3}"#)
            .create();
        let (_tmp, claude) = signed_in_claude(&server, HOUR);
        let p = run(vec![Box::new(claude)]);
        let (status, _, body) = post(
            p.port,
            "/v1/messages/count_tokens",
            &format!("x-api-key: {KEY}\r\n"),
            &json!({"model": "claude-x", "messages": []}),
        );
        assert_eq!(status, 200);
        assert_eq!(body, r#"{"input_tokens":3}"#);
        upstream.assert();
    }

    #[test]
    fn an_anthropic_client_is_offered_only_models_it_can_reach() {
        let server = mockito::Server::new();
        let (_tmp, claude) = signed_in_claude(&server, HOUR);
        let (_ctmp, codex) = signed_in_codex(&server);
        let p = run(vec![Box::new(claude), Box::new(codex)]);
        let get = |extra: &str| {
            let (_, _, body) = send(
                p.port,
                &format!("GET /v1/models HTTP/1.1\r\nAuthorization: Bearer {KEY}\r\n{extra}\r\n"),
            );
            let v: Value = serde_json::from_str(&body).unwrap();
            v["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        let anthropic = get("anthropic-version: 2023-06-01\r\n");
        assert!(
            anthropic.iter().all(|m| m.starts_with("claude-")),
            "{anthropic:?}"
        );
        let openai = get("");
        assert!(openai.iter().all(|m| m.starts_with("gpt-")), "{openai:?}");
    }
}
