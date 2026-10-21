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
        assert_eq!(body["data"][0]["owned_by"], "claude");
        let (_, _, body) = get("anthropic-version: 2023-06-01\r\n");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["data"][0]["type"], "model");
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
}
