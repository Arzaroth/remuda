//! The shipped binary against a throwaway home. Every case here stays off the
//! network: live logins match a stored one by token, and nothing is due for a
//! refresh.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

struct Home {
    dir: tempfile::TempDir,
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
            .env("PATH", "/usr/bin:/bin");
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
        "stored claude/work (u-work@example.com)\n"
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
