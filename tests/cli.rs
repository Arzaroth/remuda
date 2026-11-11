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
            .env("REMUDA_TEST_OPENAI_API", self.api.borrow().url())
            .env("REMUDA_TEST_XAI_API", self.api.borrow().url())
            .env("REMUDA_TEST_KIMI_API", self.api.borrow().url())
            .env("REMUDA_TEST_CURSOR_API", self.api.borrow().url());
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

    home.sign_in("a-work", "r-work", "u-work");
    assert_eq!(
        home.ok(&["import", "work"]),
        "stored claude/work (u-work@example.com, ultra)\n"
    );
    assert!(store.join("work.json").exists());
    assert!(
        home.fails(&["import", "again"])
            .contains("already stored as work")
    );

    home.sign_in("a-perso", "r-perso", "u-perso");
    assert!(
        home.ok(&["ls"])
            .contains("u-perso@example.com, which is not stored")
    );
    home.ok(&["import", "perso"]);
    let ls = home.ok(&["ls"]);
    assert!(ls.contains("* perso"), "{ls}");
    assert!(ls.contains("  work "), "{ls}");
    assert!(ls.contains("ultra"), "{ls}");

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

    let log = home.ok(&["log"]);
    let kinds: Vec<&str> = log
        .lines()
        .map(|l| l.split_whitespace().nth(2).unwrap_or_default())
        .collect();
    assert_eq!(
        kinds,
        ["store", "store", "switch", "store", "store", "store"],
        "{log}"
    );
    assert!(
        log.contains("switched Claude Code to work (u-work@example.com)  [cli]"),
        "{log}"
    );
    assert_eq!(home.ok(&["log", "work", "-n", "1"]).lines().count(), 1);
    assert!(home.fails(&["log", "-n", "0"]).contains("0"));
    let listed: Value = serde_json::from_str(&home.ok(&["log", "--json", "perso"])).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 3);
    assert!(
        !std::fs::read_to_string(home.path(".local/share/remuda/credentials/.log.jsonl"))
            .unwrap()
            .contains("r-work")
    );
}

#[test]
fn signout_leaves_the_cli_with_no_login_until_one_is_used() {
    let home = Home::new();
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);

    assert_eq!(
        home.ok(&["signout", "claude"]),
        "signed Claude Code out of work\n"
    );
    let creds = read(&home.creds());
    assert!(creds.get("claudeAiOauth").is_none());
    assert_eq!(creds["mcpOAuth"]["srv"]["accessToken"], "mcp");
    assert!(read(&home.config()).get("oauthAccount").is_none());
    assert!(home.ok(&["ls"]).contains("signed out"));
    assert!(
        home.ok(&["signout", "claude"])
            .contains("already signed out")
    );

    home.ok(&["use", "work"]);
    assert_eq!(
        read(&home.creds())["claudeAiOauth"]["refreshToken"],
        "r-work"
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
fn only_import_and_login_start_a_missing_store() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let store = home.path(".local/share/remuda/credentials");
    home.sign_in("a-work", "r-work", "u-work");

    for (args, doing) in [
        (&["ls"][..], "not listing"),
        (&["use", "work"], "not switching"),
        (&["refresh"], "not refreshing"),
        (&["label", "work", "x"], "not labelling"),
        (&["rename", "work", "job"], "not renaming"),
        (&["rm", "work"], "not removing"),
        (&["log"], "not reading the log"),
        (&["serve", "--no-browser", "--port", "0"], "not serving"),
    ] {
        let err = home.fails(args);
        assert!(
            err.contains(&format!("{doing}: there is no store")),
            "{err}"
        );
        assert!(err.contains("`remuda import`"), "{err}");
    }
    assert!(!home.path(".local/share/remuda").exists());

    let fresh = home
        .command(&["import", "work"])
        .env("INVOCATION_ID", "abc")
        .output()
        .unwrap();
    assert!(
        fresh.status.success(),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    assert!(store.join(".dirs.json").exists());
    std::fs::remove_dir_all(home.path(".local/share/remuda")).unwrap();

    let mut login = home.command(&["login", "perso", "--no-browser", "--paste"]);
    login.stdin(std::process::Stdio::null());
    let _ = login.output().unwrap();
    let mode = std::fs::metadata(&store).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    std::fs::remove_dir_all(home.path(".local/share/remuda")).unwrap();

    home.ok(&["import", "work"]);
    let mode = std::fs::metadata(&store).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    assert!(store.join("claude/work.json").exists());
    assert!(store.join(".dirs.json").exists());
}

#[test]
fn claude_signs_in_by_pasting_over_ssh_unless_told_otherwise() {
    let home = Home::new();
    let out = home
        .command(&["login", "perso", "--no-browser"])
        .env("SSH_CONNECTION", "10.0.0.2 50000 10.0.0.1 22")
        .env("DISPLAY", ":0")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let said = String::from_utf8(out.stdout).unwrap();
    assert!(said.contains("in any browser:"), "{said}");
    assert!(said.contains("Paste the code"), "{said}");

    let both = home.fails(&["login", "perso", "--paste", "--no-paste"]);
    assert!(both.contains("cannot be used with"), "{both}");
}

#[test]
fn codex_signs_in_by_device_code_over_ssh() {
    let home = Home::new();
    let usercode = home
        .api
        .borrow_mut()
        .mock("POST", "/api/accounts/deviceauth/usercode")
        .with_body(
            json!({"device_auth_id": "da-1", "user_code": "WXYZ-9876", "interval": 1}).to_string(),
        )
        .create();
    home.api
        .borrow_mut()
        .mock("POST", "/api/accounts/deviceauth/token")
        .with_status(400)
        .with_body("denied")
        .create();
    let out = home
        .command(&["login", "perso", "-p", "codex", "--no-browser"])
        .env("SSH_CONNECTION", "10.0.0.2 50000 10.0.0.1 22")
        .env("DISPLAY", ":0")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    usercode.assert();
    let said = String::from_utf8(out.stdout).unwrap();
    assert!(said.contains("in any browser:"), "{said}");
    assert!(said.contains("/codex/device"), "{said}");
    assert!(said.contains("Enter the code WXYZ-9876 there."), "{said}");
}

#[test]
fn auto_says_where_it_would_move_a_cli_and_moves_nothing() {
    let home = Home::new();
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    home.sign_in("a-perso", "r-perso", "u-perso");
    home.ok(&["import", "perso"]);
    assert!(
        home.ok(&["auto"])
            .starts_with("auto mode is set up for no CLI")
    );

    std::fs::create_dir_all(home.path(".config/remuda")).unwrap();
    std::fs::write(
        home.path(".config/remuda/config.toml"),
        "[auto.claude]\nlogins = [\"work\", \"perso\"]\n",
    )
    .unwrap();
    let window = |used: u64| json!({"primary": {"usedPercent": used, "windowMinutes": 300}});
    write(
        &home.path(".local/state/tokengauge/tokengauge-usage.json"),
        &json!({
            "meta": {"schemaVersion": 2, "updatedAtMs": now_ms()},
            "payloads": [
                {"provider": "claude", "credential": "work", "usage": window(20)},
                {"provider": "claude", "credential": "perso", "usage": window(100)},
            ],
        }),
    );

    let out = home.ok(&["auto"]);

    assert!(
        out.starts_with("Claude Code: auto mode off, no rule in force\n"),
        "{out}"
    );
    assert!(out.contains("perso"), "{out}");
    assert!(
        out.ends_with("  would switch to work: perso's 5 hours limit maxed\n"),
        "{out}"
    );
    assert_eq!(
        read(&home.creds())["claudeAiOauth"]["refreshToken"],
        "r-perso"
    );

    std::fs::write(
        home.path(".config/remuda/config.toml"),
        "[warm]\nclaude = [\"work\"]\n",
    )
    .unwrap();
    assert_eq!(
        home.ok(&["auto"]),
        "keep warm claude/work: its 5-hour window is closed; remuda serve opens it\n"
    );
}

#[test]
fn doctor_checks_the_setup_and_fails_on_what_is_broken() {
    let home = Home::new();
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    let nothing = home.path("empty-bin");
    std::fs::create_dir_all(&nothing).unwrap();
    let doctor = |args: &[&str]| {
        home.command(args)
            .env("PATH", &nothing)
            .current_dir(home.path(""))
            .output()
            .unwrap()
    };

    let out = doctor(&["doctor"]);
    assert!(out.status.success());
    let out = String::from_utf8(out.stdout).unwrap();
    assert!(out.contains("info  remuda serve "), "{out}");
    assert!(out.contains("1 stored, all verified"), "{out}");
    assert!(!out.contains("Claude Code version"), "{out}");

    std::fs::create_dir_all(home.path(".config/remuda")).unwrap();
    std::fs::write(
        home.path(".config/remuda/config.toml"),
        "[auto.claude]\nenabled = true\nlogins = [\"work\"]\n",
    )
    .unwrap();
    let out = doctor(&["doctor"]);
    assert!(!out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("not running, so auto mode does nothing"),
        "{stdout}"
    );
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("1 vital(s) failing")
    );

    let out = doctor(&["doctor", "--json"]);
    assert!(!out.status.success());
    let vitals: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(vitals[0]["id"], "serve");
    assert_eq!(vitals[0]["level"], "bad");
}

#[test]
fn completions_cover_every_command() {
    let home = Home::new();
    let zsh = home.ok(&["completions", "zsh"]);
    for command in [
        "import",
        "login",
        "use",
        "label",
        "rename",
        "auto",
        "doctor",
        "log",
        "run",
        "pin",
        "shell-init",
        "serve",
        "open",
        "--update",
        "--check-update",
    ] {
        assert!(zsh.contains(command), "zsh completions miss {command}");
    }
    assert!(home.ok(&["completions", "bash"]).contains("_remuda"));
    assert!(home.fails(&["completions", "tcsh"]).contains("tcsh"));
}

#[test]
fn plain_remuda_shows_the_logins_and_which_one_is_in_use() {
    let home = Home::new();
    assert_eq!(
        home.ok(&[]),
        "no logins yet: `remuda login <name> [-p <cli>]` signs one in, `remuda import <name>` keeps the one a CLI uses\n"
    );

    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    home.sign_in("a-perso", "r-perso", "u-perso");
    home.ok(&["import", "perso"]);

    let table = home.ok(&[]);
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines[0], "cli     login  in use  kept fresh", "{table}");
    assert!(lines[1].starts_with("claude  perso  ●"), "{table}");
    assert!(lines[1].ends_with("just now"), "{table}");
    assert!(lines[2].starts_with("claude  work         "), "{table}");
    assert!(
        !table.contains('\x1b'),
        "piped output has no colour: {table:?}"
    );
}

#[test]
fn updating_is_a_flag_as_in_tokengauge_and_tailgauge() {
    let home = Home::new();
    assert!(home.fails(&["update"]).contains("unrecognized subcommand"));
    assert!(home.fails(&["--update", "ls"]).contains("cannot be used"));
    assert!(
        home.fails(&["--update", "--check-update"])
            .contains("cannot be used")
    );
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
    let last_run = home.path(".local/share/remuda/credentials/.last-refresh.json");
    let run_record = read(&last_run);
    assert_eq!(run_record["refreshed"], json!([]), "{run_record}");
    assert!(
        run_record["problems"][0]
            .as_str()
            .unwrap()
            .contains("run `remuda ls` once"),
        "{run_record}"
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
    assert!(
        read(&last_run)["problems"]
            .to_string()
            .contains("not refreshing work: its CLI was skipped"),
        "{}",
        read(&last_run)
    );
    std::fs::remove_file(&last_run).unwrap();
    let out = run(&["refresh"], &[("CLAUDE_CONFIG_DIR", &elsewhere)]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!last_run.exists());

    for marker in ["1", ""] {
        let out = run(&["refresh"], &[("REMUDA_SERVICE", Path::new(marker))]);
        assert!(!out.status.success(), "{}", stderr(&out));
        assert!(
            stderr(&out).contains("claude: not refreshing"),
            "{}",
            stderr(&out)
        );
    }

    // A desktop that runs its terminal as a unit hands INVOCATION_ID to every
    // shell, and a shell's command still records.
    let out = run(&["ls"], &[("INVOCATION_ID", Path::new("abc"))]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = run(&["refresh", "--scheduled"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        run(&["ls"], &[("CLAUDE_CONFIG_DIR", &elsewhere)])
            .status
            .success()
    );

    // A smoke test against a scratch store leaves the real store's record be.
    let scratch = home.path("scratch");
    assert!(
        run(&["import", "scratch"], &[("REMUDA_STORE", &scratch)])
            .status
            .success()
    );
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

struct Serving(std::process::Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn first_line(child: &mut std::process::Child) -> String {
    use std::io::BufRead;
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    line
}

fn get_state(url: &str) -> String {
    get(url, "/api/state")
}

fn get(url: &str, path: &str) -> String {
    use std::io::{Read, Write};
    let (base, token) = url.trim().split_once("/#").unwrap();
    let host = base.strip_prefix("http://").unwrap();
    let mut s = std::net::TcpStream::connect(host).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nX-Remuda-Token: {token}\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    s.read_to_string(&mut answer).unwrap();
    answer
}

#[test]
fn open_finds_the_page_a_running_serve_shows() {
    let home = Home::new();
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    assert!(
        home.fails(&["open", "--no-browser"])
            .contains("not serving")
    );

    let mut serve = home.command(&["serve", "--no-browser", "--port", "0"]);
    serve.stdout(std::process::Stdio::piped());
    let mut serving = Serving(serve.spawn().unwrap());
    let printed = first_line(&mut serving.0);
    assert!(printed.contains("/#"), "{printed}");

    let url = home.ok(&["open", "--no-browser"]);
    assert!(printed.contains(url.trim()), "{printed} / {url}");
    assert!(get_state(&url).starts_with("HTTP/1.1 200"));
    let adding = home.ok(&["open", "--no-browser", "--add"]);
    assert_eq!(adding.trim(), url.trim().replacen("/#", "/?add#", 1));
    let page = get(&url, "/?add");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(
        page.to_ascii_lowercase()
            .contains("content-type: text/html"),
        "{page}"
    );
    let status = read(&home.path(".local/share/remuda/credentials/serve.json"));
    assert!(
        !status
            .to_string()
            .contains(url.trim().split_once('#').unwrap().1)
    );
    assert_eq!(status["pid"], json!(serving.0.id()));

    drop(serving);
    assert!(
        home.fails(&["open", "--no-browser"])
            .contains("not serving")
    );
}

#[test]
fn a_serve_run_by_systemd_checks_the_directories_and_prints_no_token() {
    let home = Home::new();
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    let unit = |envs: &[(&str, &Path)]| {
        let mut cmd = home.command(&["serve", "--port", "0"]);
        cmd.env("REMUDA_SERVICE", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd
    };

    let out = unit(&[("CLAUDE_CONFIG_DIR", &home.path("elsewhere"))])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("claude: not serving"), "{stderr}");

    let mut serving = Serving(unit(&[]).spawn().unwrap());
    let printed = first_line(&mut serving.0);
    assert!(printed.contains("remuda open"), "{printed}");
    assert!(!printed.contains('#'), "{printed}");
    let url = home.ok(&["open", "--no-browser"]);
    assert!(get_state(&url).starts_with("HTTP/1.1 200"));
}

#[test]
fn sync_units_rewrites_the_installed_units_and_reloads_systemd() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let bin = home.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = home.path("systemctl.log");
    std::fs::write(
        bin.join("systemctl"),
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in *is-active*) [ -e '{}' ] || exit 3;; esac\n",
            log.display(),
            home.path("serve-active").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        bin.join("systemctl"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let units = home.path(".config/systemd/user");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::write(units.join("remuda-refresh.service"), "old").unwrap();
    let run = || {
        home.command(&["sync-units"])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap()
    };

    let out = run();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("remuda-refresh.service"));
    assert_eq!(
        std::fs::read_to_string(units.join("remuda-refresh.service")).unwrap(),
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/systemd/remuda-refresh.service"
        ))
        .unwrap()
    );
    assert!(!units.join("remuda-serve.service").exists());
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(calls.contains("--user daemon-reload"), "{calls}");
    assert!(!calls.contains("restart"), "{calls}");

    std::fs::remove_file(&log).unwrap();
    assert!(run().status.success());
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(!calls.contains("daemon-reload"), "{calls}");

    std::fs::remove_file(&log).unwrap();
    std::fs::write(home.path("serve-active"), "").unwrap();
    assert!(run().status.success());
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains("--user restart remuda-serve.service"),
        "{calls}"
    );
}

/// A TokenGauge daemon where the binary looks for one under this home, which
/// acks every command and hands over each one it read.
fn gauge_daemon(home: &Home) -> std::sync::mpsc::Receiver<String> {
    use std::io::{BufRead, BufReader, Write};
    let dir = home.path(".local/state/tokengauge");
    std::fs::create_dir_all(&dir).unwrap();
    let daemon = std::os::unix::net::UnixListener::bind(dir.join("tokengauge.sock")).unwrap();
    let (heard, commands) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for s in daemon.incoming() {
            let s = s.unwrap();
            let mut line = String::new();
            BufReader::new(&s).read_line(&mut line).unwrap();
            if heard.send(line).is_err() {
                return;
            }
            (&s).write_all(b"{\"type\":\"ack\"}\n").unwrap();
        }
    });
    commands
}

#[test]
fn use_and_a_refresh_that_refreshed_ask_tokengauge_to_fetch_now() {
    let home = Home::new();
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    home.sign_in("a-perso", "r-perso", "u-perso");
    home.ok(&["import", "perso"]);
    let commands = gauge_daemon(&home);
    let none = std::time::Duration::ZERO;

    home.ok(&["use", "work"]);
    assert_eq!(commands.try_recv().unwrap(), "{\"cmd\":\"refresh\"}\n");

    assert_eq!(home.ok(&["refresh"]), "");
    assert!(commands.recv_timeout(none).is_err());

    {
        let mut api = home.api.borrow_mut();
        api.mock("POST", "/v1/oauth/token")
            .with_body(
                json!({"access_token": "a-perso2", "refresh_token": "r-perso2",
                       "expires_in": 28800, "account": {"uuid": "u-perso"}})
                .to_string(),
            )
            .create();
        api.mock("GET", "/api/oauth/profile")
            .match_header("authorization", "Bearer a-perso2")
            .with_body(
                json!({
                    "account": {"uuid": "u-perso", "email": "u-perso@example.com"},
                    "organization": {"uuid": "o-1", "organization_type": "claude_max"},
                })
                .to_string(),
            )
            .create();
    }
    home.ok(&["refresh", "perso", "--force"]);
    assert_eq!(commands.try_recv().unwrap(), "{\"cmd\":\"refresh\"}\n");
}

/// A `claude` that notes what it was started with, then rotates its tokens
/// as a refresh would.
fn fake_claude(home: &Home) -> String {
    let bin = home.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("claude");
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         printf '%s\\n' \"$CLAUDE_CONFIG_DIR\" > \"$HOME/seen-dir\"\n\
         printf '%s\\n' \"$*\" > \"$HOME/seen-args\"\n\
         [ -n \"$CLAUDE_CONFIG_DIR\" ] || exit 0\n\
         for _ in 1 2 3 4 5 6 7 8 9 10; do ls \"$CLAUDE_CONFIG_DIR/..\" | grep -q '^cli-' && break; sleep 0.2; done\n\
         ls \"$CLAUDE_CONFIG_DIR/..\" > \"$HOME/seen-place\"\n\
         cp \"$CLAUDE_CONFIG_DIR/.credentials.json\" \"$HOME/seen-creds\"\n\
         sed -i 's/\"r-work\"/\"r-rotated\"/; s/\"a-work\"/\"a-rotated\"/' \"$CLAUDE_CONFIG_DIR/.credentials.json\"\n\
         exit 7\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!("{}:/usr/bin:/bin", bin.display())
}

#[test]
fn run_starts_the_cli_on_a_login_of_its_own_and_keeps_what_it_rotated() {
    let home = Home::new();
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    home.sign_in("a-perso", "r-perso", "u-perso");
    home.ok(&["import", "perso"]);
    home.api
        .borrow_mut()
        .mock("GET", "/api/oauth/profile")
        .match_header("authorization", "Bearer a-rotated")
        .with_body(
            json!({"account": {"uuid": "u-work", "email": "u-work@example.com"},
                   "organization": {"uuid": "o-1", "organization_type": "claude_max"}})
            .to_string(),
        )
        .create();
    let path = fake_claude(&home);
    let store = home.path(".local/share/remuda/credentials");

    let out = home
        .command(&["run", "work", "--", "--resume", "x"])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dir = std::fs::read_to_string(home.path("seen-dir")).unwrap();
    assert!(dir.trim_end().ends_with(".runs/claude/work/home"), "{dir}");
    assert_eq!(
        std::fs::read_to_string(home.path("seen-args")).unwrap(),
        "--resume x\n"
    );
    let place = std::fs::read_to_string(home.path("seen-place")).unwrap();
    assert!(
        place.lines().any(|l| l.starts_with("cli-")),
        "the session notes its CLI's process: {place}"
    );
    let seen = read(&home.path("seen-creds"));
    assert_eq!(seen["claudeAiOauth"]["refreshToken"], "r-work");
    assert_eq!(seen["mcpOAuth"]["srv"]["accessToken"], "mcp");
    assert_eq!(
        read(&home.creds())["claudeAiOauth"]["refreshToken"],
        "r-perso",
        "the CLI's own login is left alone"
    );
    assert_eq!(
        read(&store.join("claude/work.json"))["claudeAiOauth"]["refreshToken"],
        "r-rotated"
    );
    assert!(!store.join(".runs/claude/work").exists());
    let log = home.ok(&["log", "work"]);
    assert!(
        log.contains("started a session") && log.contains("the session ended"),
        "{log}"
    );

    for target in ["perso", "claude"] {
        std::fs::remove_file(home.path("seen-dir")).unwrap();
        let out = home
            .command(&["run", target])
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(home.path("seen-dir")).unwrap(),
            "\n",
            "{target} runs the CLI as it is"
        );
    }
    assert!(home.fails(&["run", "glm"]).contains("cannot start"));

    let inside = home
        .command(&["ls", "--json"])
        .env("CLAUDE_CONFIG_DIR", home.path("elsewhere"))
        .env("REMUDA_OUTER_CLAUDE_CONFIG_DIR", "")
        .output()
        .unwrap();
    let listed: Value = serde_json::from_slice(&inside.stdout).unwrap();
    let active: Vec<&str> = listed["credentials"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["active"] == true)
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        active,
        ["perso"],
        "a remuda inside a session sees the CLI's own files"
    );
}

#[test]
fn a_pinned_directory_starts_its_cli_on_the_pinned_login() {
    let home = Home::new();
    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    home.sign_in("a-perso", "r-perso", "u-perso");
    home.ok(&["import", "perso"]);
    home.api
        .borrow_mut()
        .mock("GET", "/api/oauth/profile")
        .match_header("authorization", "Bearer a-rotated")
        .with_body(
            json!({"account": {"uuid": "u-work", "email": "u-work@example.com"},
                   "organization": {"uuid": "o-1", "organization_type": "claude_max"}})
            .to_string(),
        )
        .create();
    let path = fake_claude(&home);
    let repo = home.path("repo");
    let sub = repo.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    let at = |dir: &Path, args: &[&str]| {
        home.command(args)
            .current_dir(dir)
            .env("PATH", &path)
            .output()
            .unwrap()
    };

    let out = at(&repo, &["pin", "work"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".remuda.toml")).unwrap(),
        "claude = \"work\"\n"
    );
    let shown = String::from_utf8(at(&sub, &["pin"]).stdout).unwrap();
    assert!(shown.starts_with("claude: work ("), "{shown}");

    for args in [
        &["run", "claude"][..],
        &["claude", "--", "-p", "hi"],
        &["claude", "-p", "hi"],
    ] {
        let out = at(&sub, args);
        assert_eq!(
            out.status.code(),
            Some(7),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let dir = std::fs::read_to_string(home.path("seen-dir")).unwrap();
        assert!(dir.contains(".runs/claude/work/home"), "{args:?}: {dir}");
    }
    assert_eq!(
        std::fs::read_to_string(home.path("seen-args")).unwrap(),
        "-p hi\n"
    );
    assert_eq!(
        read(&home.path(".local/share/remuda/credentials/claude/work.json"))["claudeAiOauth"]["refreshToken"],
        "r-rotated"
    );
    let out = at(&home.path(""), &["claude", "perso"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(home.path("seen-dir")).unwrap(),
        "\n"
    );

    std::fs::write(repo.join(".remuda.toml"), "claude = \"perso\"\n").unwrap();
    let out = at(&sub, &["run", "claude"]);
    assert!(out.status.success());
    assert_eq!(
        std::fs::read_to_string(home.path("seen-dir")).unwrap(),
        "\n",
        "a file remuda pin did not write is not read"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("--allow"));
    assert!(at(&sub, &["pin", "--allow"]).status.success());
    let shown = String::from_utf8(at(&sub, &["pin"]).stdout).unwrap();
    assert!(shown.starts_with("claude: perso ("), "{shown}");
    assert!(at(&repo, &["pin", "--remove", "claude"]).status.success());
    let out = at(&sub, &["run", "claude"]);
    assert!(out.status.success());
    assert_eq!(
        std::fs::read_to_string(home.path("seen-dir")).unwrap(),
        "\n"
    );

    let init = home.ok(&["shell-init", "bash"]);
    assert!(
        init.contains("claude() { command remuda run claude -- \"$@\"; }"),
        "{init}"
    );
    assert!(init.contains("codex()"), "{init}");
    assert!(home.fails(&["nope"]).contains("unrecognized subcommand"));
}

impl Home {
    fn hook(&self, event: &str, input: Value, response: Value) -> String {
        use std::io::Write;
        let mut child = self
            .command(&["hook", "artifact"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let body = json!({
            "hook_event_name": event,
            "session_id": "s-1",
            "tool_name": "Artifact",
            "tool_input": input,
            "tool_response": response,
        });
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

#[test]
fn the_artifact_hook_switches_to_the_publishing_login_and_back() {
    let home = Home::new();
    assert_eq!(home.hook("PreToolUse", json!({}), Value::Null), "");

    home.sign_in("a-work", "r-work", "u-work");
    home.ok(&["import", "work"]);
    home.sign_in("a-perso", "r-perso", "u-perso");
    home.ok(&["import", "perso"]);
    let page = json!({"file_path": "/s/page.html"});
    let published = json!({"url": "https://claude.ai/artifact/Abc", "path": "/s/page.html"});
    assert_eq!(
        home.hook("PostToolUse", page.clone(), published.clone()),
        ""
    );
    home.ok(&["use", "work"]);

    let said: Value =
        serde_json::from_str(&home.hook("PreToolUse", page.clone(), Value::Null)).unwrap();
    let text = said["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(text.contains("from work to perso"), "{text}");
    assert_eq!(
        read(&home.creds())["claudeAiOauth"]["refreshToken"],
        "r-perso"
    );

    home.hook("PostToolUse", page, published);
    assert_eq!(
        read(&home.creds())["claudeAiOauth"]["refreshToken"],
        "r-work"
    );
    let log = home.ok(&["log", "-n", "2"]);
    assert_eq!(log.matches("[artifact hook]").count(), 2, "{log}");
}

#[test]
fn help_prints_the_skill_and_each_command() {
    let home = Home::new();
    let skill = home.ok(&["help", "skill"]);
    assert!(skill.starts_with("---\nname: remuda\n"), "{skill}");
    assert!(skill.contains("remuda hook artifact"));
    assert!(
        home.ok(&["help", "use"])
            .contains("Make a stored credential")
    );
    assert!(home.ok(&["help"]).contains("Commands:"));
    assert!(home.fails(&["help", "nope"]).contains("topics: skill"));
}

#[test]
fn export_seals_the_logins_and_restore_sets_another_machine_up() {
    let old = Home::new();
    old.sign_in("a-perso", "r-perso", "u-perso");
    old.ok(&["import", "perso"]);
    old.sign_in("a-work", "r-work", "u-work");
    old.ok(&["import", "work"]);
    let pass = old.path("pass");
    std::fs::write(&pass, "a passphrase\n").unwrap();
    let sealed = old.path("out.age");
    let (sealed_arg, pass_arg) = (sealed.to_str().unwrap(), pass.to_str().unwrap());

    let said = old.ok(&[
        "export",
        sealed_arg,
        "--passphrase-file",
        pass_arg,
        "--move",
        "perso",
    ]);
    assert!(
        said.contains("It carries 1 login(s) as they are, 1 to sign in again."),
        "{said}"
    );
    assert!(said.contains("removed claude/perso"), "{said}");
    let kept = said
        .split_once("its tokens are set aside in ")
        .unwrap()
        .1
        .split_whitespace()
        .next()
        .unwrap();
    assert_eq!(
        read(Path::new(kept))["claudeAiOauth"]["refreshToken"],
        "r-perso"
    );
    assert!(!old.ok(&["ls"]).contains("perso"));
    assert!(
        !std::fs::read(&sealed)
            .unwrap()
            .windows(7)
            .any(|w| w == b"r-perso")
    );
    let err = old.fails(&["export", "--move", "work", sealed_arg]);
    assert!(err.contains("is the login Claude Code uses"), "{err}");
    let out = old
        .command(&["export"])
        .env("PATH", old.path("nowhere"))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("neither wyrmyon (wyrm) nor magic-wormhole")
    );

    let new = Home::new();
    let said = new.ok(&["restore", sealed_arg, "--passphrase-file", pass_arg]);
    assert!(said.contains("stored claude/perso"), "{said}");
    assert!(
        said.contains("remuda login work -p claude --paste    (u-work@example.com)"),
        "{said}"
    );
    let stored = read(&new.path(".local/share/remuda/credentials/claude/perso.json"));
    assert_eq!(stored["claudeAiOauth"]["refreshToken"], "r-perso");
    assert!(new.ok(&["ls"]).contains("perso"));

    std::fs::write(&pass, "another\n").unwrap();
    let err = new.fails(&["restore", sealed_arg, "--passphrase-file", pass_arg]);
    assert!(err.contains("wrong passphrase"), "{err}");
}

#[test]
fn a_received_bundle_restore_cannot_read_is_kept_to_try_again() {
    use std::os::unix::fs::PermissionsExt;
    let new = Home::new();
    let sent = new.path("sent.json");
    write(
        &sent,
        &json!({"remuda_export": 2, "carried": [], "sign_in": []}),
    );
    let bin = new.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let client = bin.join("wyrm");
    std::fs::write(
        &client,
        format!("#!/bin/sh\ncp {} \"$5\"\n", sent.display()),
    )
    .unwrap();
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = new
        .command(&["restore"])
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .unwrap();

    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("format 2"), "{err} {said}");
    let kept = said
        .split_once("what was received is kept in ")
        .unwrap()
        .1
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(
        kept.starts_with(
            new.path(".local/share/remuda/credentials/.received-")
                .to_str()
                .unwrap()
        )
    );
    let mut bundle = read(Path::new(&kept));
    bundle["remuda_export"] = json!(1);
    write(Path::new(&kept), &bundle);

    new.ok(&["restore", &kept]);
    assert!(!Path::new(&kept).exists());
}

#[test]
fn a_kept_bundle_that_cannot_be_removed_leaves_the_sign_ins_listed() {
    use std::os::unix::fs::PermissionsExt;
    let new = Home::new();
    let root = new.path(".local/share/remuda/credentials");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".lock"), "").unwrap();
    let kept = root.join(".received-1.json");
    write(
        &kept,
        &json!({"remuda_export": 1, "carried": [], "sign_in": [{"provider": "claude", "name": "work", "account_id": "u-1", "email": "me@example.com"}]}),
    );
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
    let enforced = std::fs::write(root.join("probe"), "").is_err();

    let out = enforced.then(|| new.run(&["restore", kept.to_str().unwrap(), "--no-sign-in"]));
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();

    if let Some(out) = out {
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{said} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(said.contains("warning: could not remove"), "{said}");
        assert!(
            said.contains("remuda login work -p claude --paste"),
            "{said}"
        );
    }
}

#[test]
fn carried_tokens_that_cannot_be_kept_leave_the_bundle_and_the_rest_restored() {
    use std::os::unix::fs::PermissionsExt;
    let new = Home::new();
    let sent = new.path("sent.json");
    write(
        &sent,
        &json!({
            "remuda_export": 1,
            "carried": [
                {
                    "provider": "claude",
                    "name": "perso",
                    "creds": json!({"claudeAiOauth": login("a-perso", "r-perso")}).to_string(),
                    "meta": {"accountId": "u-perso", "email": "u-perso@example.com", "capturedAt": 1},
                },
                {
                    "provider": "glm",
                    "name": "k",
                    "creds": json!({"key": "sk-1"}).to_string(),
                    "meta": {"accountId": "key-0000000000000000", "email": "key ending sk-1", "capturedAt": 1},
                },
            ],
            "sign_in": [{"provider": "claude", "name": "work", "account_id": "u-work", "email": "u-work@example.com"}],
        }),
    );
    let bin = new.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let client = bin.join("wyrm");
    std::fs::write(
        &client,
        format!("#!/bin/sh\ncp {} \"$5\"\n", sent.display()),
    )
    .unwrap();
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).unwrap();
    let root = new.path(".local/share/remuda/credentials");
    let claude = root.join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o500)).unwrap();
    let enforced = std::fs::write(claude.join("probe"), "").is_err();

    let received = || {
        new.command(&["restore", "--no-sign-in"])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap()
    };
    let out = enforced.then(received);
    let Some(out) = out else {
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o700)).unwrap();
        return;
    };
    let said = String::from_utf8_lossy(&out.stdout).into_owned();
    let kept = said
        .split_once("what was received is kept in ")
        .map(|(_, rest)| rest.split(';').next().unwrap().to_owned())
        .unwrap_or_default();
    let again = new.run(&["restore", &kept, "--no-sign-in"]);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
    let unkept = received();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("could not store claude/perso"), "{said}");
    assert!(said.contains("stored glm/k"), "{said}");
    assert!(
        said.contains("remuda login work -p claude --paste"),
        "{said}"
    );
    assert!(kept.starts_with(root.join(".received-").to_str().unwrap()));
    assert!(
        err.contains(&format!(
            "the tokens carried for claude/perso could be neither stored nor set aside here; what was received is kept in {kept}"
        )),
        "{err}"
    );

    assert!(!again.status.success());
    let err = String::from_utf8_lossy(&again.stderr);
    assert!(err.contains(&format!("; {kept} still holds them")), "{err}");
    assert!(Path::new(&kept).exists());

    assert!(!unkept.status.success());
    let said = String::from_utf8_lossy(&unkept.stdout);
    let err = String::from_utf8_lossy(&unkept.stderr);
    assert!(
        said.contains("warning: what was received is lost"),
        "{said}"
    );
    assert!(
        said.contains("remuda login work -p claude --paste"),
        "{said}"
    );
    assert!(
        err.contains("what was received could not be kept, so they are only in the sending machine's store or the set-aside file export printed there"),
        "{err}"
    );
    assert!(!err.contains("kept in"), "{err}");

    let said = new.ok(&["restore", &kept, "--no-sign-in"]);
    assert!(said.contains("stored claude/perso"), "{said}");
    assert_eq!(
        read(&claude.join("perso.json"))["claudeAiOauth"]["refreshToken"],
        "r-perso"
    );
    assert!(!Path::new(&kept).exists());
}
