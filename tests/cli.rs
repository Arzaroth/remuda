//! The shipped binary against a throwaway home, its providers pointed at a
//! local mock. The mock knows the profile behind each signed-in token and
//! answers anything else with an error.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

struct Home {
    dir: tempfile::TempDir,
    api: std::cell::RefCell<mockito::ServerGuard>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn login(access: &str, refresh: &str) -> Value {
    json!({
        "accessToken": access,
        "refreshToken": refresh,
        "expiresAt": now_ms() + 5 * 3_600_000,
        "scopes": ["user:inference"],
        "subscriptionType": "max",
        "rateLimitTier": "default_claude_max_20x",
    })
}

fn account(uuid: &str) -> Value {
    json!({"accountUuid": uuid, "emailAddress": format!("{uuid}@example.com")})
}

fn write(path: &Path, v: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

impl Home {
    fn new() -> Self {
        Home {
            dir: tempfile::tempdir().unwrap(),
            api: std::cell::RefCell::new(mockito::Server::new()),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn creds(&self) -> PathBuf {
        self.path(".claude/.credentials.json")
    }

    fn config(&self) -> PathBuf {
        self.path(".claude.json")
    }

    fn sign_in(&self, access: &str, refresh: &str, uuid: &str) {
        self.api
            .borrow_mut()
            .mock("GET", "/api/oauth/profile")
            .match_header("authorization", format!("Bearer {access}").as_str())
            .with_body(
                json!({
                    "account": {"uuid": uuid, "email": format!("{uuid}@example.com")},
                    "organization": {"uuid": "o-1", "organization_type": "claude_max"},
                })
                .to_string(),
            )
            .create();
        write(
            &self.creds(),
            &json!({"claudeAiOauth": login(access, refresh), "mcpOAuth": {"srv": {"accessToken": "mcp"}}}),
        );
        write(
            &self.config(),
            &json!({"numStartups": 3, "oauthAccount": account(uuid)}),
        );
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_remuda"));
        cmd.args(args)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("PATH", "/usr/bin:/bin")
            .env("REMUDA_TEST_CLAUDE_API", self.api.borrow().url())
            .env("REMUDA_TEST_OPENAI_API", self.api.borrow().url());
        // Coverage runs record the binary's own profile through this.
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            cmd.env("LLVM_PROFILE_FILE", profile);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "remuda {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(!out.status.success(), "remuda {args:?} succeeded");
        String::from_utf8(out.stderr).unwrap()
    }
}

#[test]
fn two_accounts_imported_switched_and_removed() {
    let home = Home::new();
    let store = home.path(".local/share/remuda/credentials/claude");

    assert!(home.ok(&["ls"]).starts_with("no stored credentials in "));

    home.sign_in("a-work", "r-work", "u-work");
    assert!(
        home.ok(&["ls"])
            .contains("u-work@example.com, which is not stored")
    );
    assert_eq!(
        home.ok(&["import", "work"]),
        "stored claude/work (u-work@example.com, max 20x)\n"
    );
    assert!(store.join("work.json").exists());
    assert!(
        home.fails(&["import", "again"])
            .contains("already stored as work")
    );

    home.sign_in("a-perso", "r-perso", "u-perso");
    home.ok(&["import", "perso"]);
    let ls = home.ok(&["ls"]);
    assert!(ls.contains("* perso"), "{ls}");
    assert!(ls.contains("  work "), "{ls}");
    assert!(ls.contains("max 20x"), "{ls}");

    assert_eq!(
        home.ok(&["use", "work"]),
        "switched Claude Code to work (u-work@example.com)\n"
    );
    let creds = read(&home.creds());
    assert_eq!(creds["claudeAiOauth"]["refreshToken"], "r-work");
    assert_eq!(creds["mcpOAuth"]["srv"]["accessToken"], "mcp");
    let config = read(&home.config());
    assert_eq!(config["oauthAccount"]["accountUuid"], "u-work");
    assert_eq!(config["numStartups"], 3);
    assert!(home.ok(&["use", "work"]).contains("already active"));

    let listed: Value = serde_json::from_str(&home.ok(&["ls", "--json"])).unwrap();
    let active: Vec<&str> = listed["credentials"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["active"] == true)
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(active, ["work"]);

    assert_eq!(home.ok(&["refresh"]), "");
    home.ok(&["label", "work", "Job"]);
    assert!(home.ok(&["ls"]).contains("* work (Job)"));
    home.ok(&["rename", "claude/perso", "home"]);
    home.ok(&["rename", "home", "perso"]);
    assert!(home.fails(&["rm", "work"]).contains("is active"));
    assert_eq!(home.ok(&["rm", "perso"]), "removed claude/perso\n");
    assert!(!store.join("perso.meta.json").exists());
    assert!(
        home.fails(&["use", "perso"])
            .contains("no credential named perso")
    );
}

#[test]
fn a_login_nobody_stored_is_not_switched_away_from_unless_discarded() {
    let home = Home::new();
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    home.sign_in("a-new", "r-new", "u-new");

    let err = home.fails(&["use", "work"]);
    assert!(
        err.contains("u-new@example.com) is not in the store"),
        "{err}"
    );
    assert_eq!(
        read(&home.creds())["claudeAiOauth"]["refreshToken"],
        "r-new"
    );

    home.ok(&["use", "work", "--discard"]);
    assert_eq!(
        read(&home.creds())["claudeAiOauth"]["refreshToken"],
        "r-work"
    );
}

#[test]
fn the_store_and_claude_code_follow_their_environment_variables() {
    let home = Home::new();
    let claude_dir = home.path("elsewhere");
    write(
        &claude_dir.join(".credentials.json"),
        &json!({"claudeAiOauth": login("a", "r")}),
    );
    write(
        &claude_dir.join(".claude.json"),
        &json!({"oauthAccount": account("u-cfg")}),
    );
    let xdg = home.path("xdg");
    let explicit = home.path("explicit");

    let run = |extra: &[(&str, &Path)], name: &str| {
        let mut cmd = home.command(&["import", name]);
        cmd.env("CLAUDE_CONFIG_DIR", &claude_dir);
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };

    run(&[("XDG_DATA_HOME", &xdg)], "a");
    assert!(xdg.join("remuda/credentials/claude/a.json").exists());

    run(&[("REMUDA_STORE", &explicit), ("XDG_DATA_HOME", &xdg)], "b");
    assert!(explicit.join("claude/b.json").exists());
    assert!(!home.path(".claude/.credentials.json").exists());
}

#[test]
fn a_bad_name_is_refused_before_anything_is_written() {
    let home = Home::new();
    home.sign_in("a", "r", "u");
    assert!(
        home.fails(&["import", "../evil"])
            .contains("not a usable name")
    );
    assert!(!home.path(".local/share/evil.json").exists());
}

#[test]
fn a_corrupt_config_is_reported_not_overwritten() {
    let home = Home::new();
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    home.sign_in("a-perso", "r-perso", "u-perso");
    home.ok(&["import", "perso"]);
    std::fs::write(home.config(), "{ not json").unwrap();

    let err = home.fails(&["use", "work"]);
    assert!(err.contains(".claude.json is not valid JSON"), "{err}");
    assert_eq!(
        std::fs::read_to_string(home.config()).unwrap(),
        "{ not json"
    );
}

fn jwt(claims: Value) -> String {
    use base64::Engine;
    let enc = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    format!("{}.{}.sig", enc(&json!({"alg": "none"})), enc(&claims))
}

fn codex_auth(account: &str, refresh: &str) -> Value {
    json!({
        "OPENAI_API_KEY": null,
        "auth_mode": "chatgpt",
        "tokens": {
            "id_token": jwt(json!({
                "email": format!("{account}@example.com"),
                "https://api.openai.com/auth": {"chatgpt_account_id": account, "chatgpt_plan_type": "plus"},
            })),
            "access_token": jwt(json!({
                "exp": now_ms() / 1000 + 86_400,
                "https://api.openai.com/auth": {"chatgpt_account_user_id": account},
            })),
            "refresh_token": refresh,
            "account_id": account,
        },
    })
}

#[test]
fn codex_logins_live_beside_claude_ones() {
    let home = Home::new();
    let auth = home.path(".codex/auth.json");
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);

    write(&auth, &codex_auth("acct-work", "cr-work"));
    assert_eq!(
        home.ok(&["import", "-p", "codex", "work"]),
        "stored codex/work (acct-work@example.com, plus)\n"
    );
    write(&auth, &codex_auth("acct-perso", "cr-perso"));
    home.ok(&["import", "--provider", "codex", "perso"]);

    let ls = home.ok(&["ls"]);
    assert!(ls.contains("Claude Code\n* work"), "{ls}");
    assert!(ls.contains("Codex\n* perso"), "{ls}");
    assert!(ls.contains("plus"), "{ls}");

    let err = home.fails(&["use", "work"]);
    assert!(err.contains("claude/work, codex/work"), "{err}");
    assert_eq!(
        home.ok(&["use", "codex/work"]),
        "switched Codex to work (acct-work@example.com)\n"
    );
    assert_eq!(read(&auth)["tokens"]["refresh_token"], "cr-work");
    assert_eq!(
        read(&home.creds())["claudeAiOauth"]["refreshToken"],
        "r-work"
    );
    assert!(
        home.fails(&["import", "-p", "gemini", "x"])
            .contains("gemini")
    );
}

#[test]
fn completions_cover_every_command() {
    let home = Home::new();
    let zsh = home.ok(&["completions", "zsh"]);
    for command in [
        "import", "login", "use", "label", "rename", "serve", "update",
    ] {
        assert!(zsh.contains(command), "zsh completions miss {command}");
    }
    assert!(home.ok(&["completions", "bash"]).contains("_remuda"));
    assert!(home.fails(&["completions", "tcsh"]).contains("tcsh"));
}

#[test]
fn the_scheduled_refresh_stops_where_the_shell_looked_elsewhere() {
    let home = Home::new();
    let elsewhere = home.path("work-claude");
    let run = |args: &[&str], envs: &[(&str, &Path)]| {
        let mut cmd = home.command(args);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    };
    let stderr = |out: &Output| String::from_utf8_lossy(&out.stderr).into_owned();

    let out = run(&["refresh", "--scheduled"], &[]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("there is no store"),
        "{}",
        stderr(&out)
    );
    assert!(!home.path(".local/share/remuda").exists());

    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    std::fs::remove_file(home.path(".local/share/remuda/credentials/.dirs.json")).unwrap();
    let out = run(&["refresh", "--scheduled"], &[]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("run `remuda ls` once"),
        "{}",
        stderr(&out)
    );

    assert!(
        run(&["ls"], &[("CLAUDE_CONFIG_DIR", &elsewhere)])
            .status
            .success()
    );
    let out = run(&["refresh", "--scheduled"], &[]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("claude: not refreshing"),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("work-claude"), "{}", stderr(&out));
    assert!(
        !stderr(&out).contains("codex: not refreshing"),
        "{}",
        stderr(&out)
    );

    let out = run(
        &["refresh", "--scheduled"],
        &[("CLAUDE_CONFIG_DIR", &elsewhere)],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let out = run(&["refresh", "--scheduled", "work"], &[]);
    assert!(
        stderr(&out).contains("not refreshing work: its CLI was skipped"),
        "{}",
        stderr(&out)
    );

    // A unit from 0.1.0 runs plain `refresh`: under systemd it is checked the
    // same way and records nothing.
    let out = run(&["refresh"], &[("INVOCATION_ID", Path::new("abc"))]);
    assert!(!out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("claude: not refreshing"),
        "{}",
        stderr(&out)
    );

    // A smoke test against a scratch store leaves the real store's record be.
    let scratch = home.path("scratch");
    assert!(run(&["ls"], &[("REMUDA_STORE", &scratch)]).status.success());
    let out = run(
        &["refresh", "--scheduled"],
        &[("CLAUDE_CONFIG_DIR", &elsewhere)],
    );
    assert!(out.status.success(), "{}", stderr(&out));

    let relative = run(&["ls"], &[("CLAUDE_CONFIG_DIR", Path::new("work-claude"))]);
    assert!(relative.status.success());
    let record = read(&home.path(".local/share/remuda/credentials/.dirs.json"));
    assert!(
        Path::new(record["homes"]["claude"].as_str().unwrap()).is_absolute(),
        "{record}"
    );
}
