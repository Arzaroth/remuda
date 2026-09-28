mod claude;
mod codex;
mod commands;
mod dirs;
mod fsx;
mod http;
mod oauth;
mod ops;
mod paths;
mod pkce;
mod project;
mod provider;
mod serve;
mod store;

use std::io::{self, BufRead};
use std::process::{Command, Stdio};

use anyhow::{Result, bail};
use clap::{CommandFactory, Parser, Subcommand};

use crate::claude::Claude;
use crate::codex::Codex;
use crate::project::REMUDA;
use crate::provider::Provider;
use crate::store::Store;

/// Keep several Claude Code and Codex logins and switch between them.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

const PROVIDERS: [&str; 2] = ["claude", "codex"];

#[derive(Subcommand)]
enum Cmd {
    /// List stored credentials and which one each CLI is using.
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Store the login a CLI is signed into right now.
    Import {
        name: String,
        #[arg(short, long, default_value = commands::DEFAULT_PROVIDER, value_parser = PROVIDERS)]
        provider: String,
        /// Replace a stored credential of the same name.
        #[arg(long)]
        force: bool,
    },
    /// Sign a new account in through the browser and store it, without touching the CLI.
    Login {
        name: String,
        #[arg(short, long, default_value = commands::DEFAULT_PROVIDER, value_parser = PROVIDERS)]
        provider: String,
        #[arg(long)]
        force: bool,
        /// Print the URL instead of opening it.
        #[arg(long)]
        no_browser: bool,
    },
    /// Make a stored credential the one its CLI uses. NAME or PROVIDER/NAME.
    Use {
        name: String,
        /// Switch even when the live login is not in the store, dropping it.
        #[arg(long)]
        discard: bool,
    },
    /// Refresh the tokens of the inactive credentials that are close to expiring.
    Refresh {
        name: Option<String>,
        /// Refresh even when the access token is still fresh.
        #[arg(long)]
        force: bool,
        /// Refresh anything expiring within this many minutes.
        #[arg(long, default_value_t = 60)]
        within: i64,
        /// Run as the refresh timer: stop where the directories differ from
        /// the ones the last interactive command used.
        #[arg(long)]
        scheduled: bool,
    },
    /// Set the label shown beside a credential, or clear it. NAME or PROVIDER/NAME.
    Label { name: String, text: Option<String> },
    /// Rename a stored credential. NAME or PROVIDER/NAME.
    Rename { name: String, new_name: String },
    /// Delete a stored credential. NAME or PROVIDER/NAME.
    #[command(alias = "rm")]
    Remove { name: String },
    /// Serve a local page to list, switch and sign in credentials from the browser.
    Serve {
        #[arg(long, default_value_t = 7429)]
        port: u16,
        /// Print the URL instead of opening it.
        #[arg(long)]
        no_browser: bool,
    },
    /// Print a completion script for a shell.
    Completions { shell: clap_complete::Shell },
    /// Replace this binary with the latest release.
    Update {
        /// Only report whether a newer release exists.
        #[arg(long)]
        check: bool,
    },
}

fn update(check_only: bool) -> Result<()> {
    let cache = selvedge::state::update_cache_file(&REMUDA);
    if check_only {
        let status = selvedge::update::check_cached(&REMUDA, &cache, true)?;
        match status.latest.as_deref() {
            Some(latest) if status.available => {
                println!("remuda {latest} is available (this is {})", status.current)
            }
            _ => println!("remuda {} is the latest release", status.current),
        }
        return Ok(());
    }
    let applied = selvedge::update::apply(&REMUDA, &cache)?;
    if applied.version == REMUDA.version {
        println!("remuda {} is the latest release", applied.version);
    } else {
        println!("updated remuda {} -> {}", REMUDA.version, applied.version);
    }
    Ok(())
}

/// A page that sends the browser on to `url`. The browser is handed this
/// file rather than the URL, because a command line is readable by every
/// local user and these URLs carry the page's token or a sign-in's state.
fn redirect_page(url: &str) -> String {
    let attr = url
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    let js = serde_json::to_string(url)
        .unwrap_or_default()
        .replace('<', "\\u003c");
    format!(
        "<!doctype html><meta charset=utf-8><meta name=referrer content=no-referrer>\
         <meta http-equiv=refresh content=\"0;url={attr}\">\
         <script>location.replace({js})</script><a href=\"{attr}\">Continue</a>\n"
    )
}

fn open_in_browser(url: &str) {
    let page = paths::runtime_dir().join("open.html");
    if fsx::write_private(&page, &redirect_page(url)).is_err() {
        return;
    }
    let _ = Command::new("xdg-open")
        .arg(&page)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Cmd::Update { check } = cli.command {
        return update(check);
    }
    if let Cmd::Completions { shell } = cli.command {
        clap_complete::generate(shell, &mut Cli::command(), "remuda", &mut io::stdout());
        return Ok(());
    }
    let store = Store::open(&paths::store_root());
    let claude = Claude::from_env()?;
    let codex = Codex::from_env()?;
    let providers: [&dyn Provider; 2] = [&claude, &codex];
    let seen = dirs::now(&providers);
    let scheduled = dirs::is_scheduled(matches!(
        cli.command,
        Cmd::Refresh {
            scheduled: true,
            ..
        }
    ));
    if !scheduled {
        dirs::record(&store, &seen);
    }
    if let Cmd::Serve { port, no_browser } = cli.command {
        let providers: Vec<Box<dyn Provider>> = vec![Box::new(claude), Box::new(codex)];
        let (listener, port) = http::bind(port)?;
        let token = pkce::random()?;
        let url = format!("http://127.0.0.1:{port}/#{token}");
        println!(
            "remuda is serving {url}\nThe link carries its access token; keep it to yourself. Ctrl-C stops it."
        );
        if !no_browser {
            open_in_browser(&url);
        }
        let app = serve::App::new(store, providers, token, port);
        http::serve(std::sync::Arc::new(app), listener, http::Limits::default());
        return Ok(());
    }
    let out = &mut io::stdout();

    if let Cmd::Login {
        name,
        provider,
        force,
        no_browser,
    } = &cli.command
    {
        let p = commands::find(&providers, provider)?;
        let open = |url: &str| {
            if !no_browser {
                open_in_browser(url);
            }
        };
        let mut read_code = || {
            let mut line = String::new();
            io::stdin().lock().read_line(&mut line)?;
            Ok(line)
        };
        let prompt = commands::Prompt {
            open: &open,
            read_code: &mut read_code,
        };
        return commands::login(&store, p, name, *force, prompt, out);
    }

    if scheduled && !store.root().exists() {
        bail!(
            "not refreshing: there is no store at {}; set REMUDA_STORE in ~/.config/environment.d/60-remuda.conf if yours is elsewhere",
            store.root().display()
        );
    }
    let _lock = store.lock()?;
    match cli.command {
        Cmd::Use { name, discard } => {
            let (p, name) = commands::resolve(&store, &providers, &name)?;
            println!("{}", ops::switch(&store, p, &name, discard)?);
            Ok(())
        }
        Cmd::List { json } => {
            let lives = commands::sync_all(&store, &providers)?;
            commands::list(&store, &lives, json, out)
        }
        Cmd::Import {
            name,
            provider,
            force,
        } => {
            let p = commands::find(&providers, &provider)?;
            let state = ops::sync_live(&store, p)?;
            commands::import(&store, p, &state, &name, force, out)
        }
        Cmd::Refresh {
            name,
            force,
            within,
            ..
        } => {
            let plan = scheduled.then(|| dirs::plan(dirs::recorded(&store).as_ref(), &seen));
            for line in plan.iter().flat_map(|p| &p.skipped) {
                eprintln!("{line}");
            }
            let usable: Vec<&dyn Provider> = providers
                .iter()
                .copied()
                .filter(|p| {
                    plan.as_ref()
                        .is_none_or(|plan| plan.usable.iter().any(|id| id == p.id()))
                })
                .collect();
            let only = name
                .as_deref()
                .map(|spec| commands::resolve(&store, &providers, spec))
                .transpose()?
                .filter(|(p, _)| usable.iter().any(|u| u.id() == p.id()));
            if name.is_some() && only.is_none() {
                bail!(
                    "not refreshing {}: its CLI was skipped",
                    name.unwrap_or_default()
                );
            }
            let lives = commands::sync_all(&store, &usable)?;
            let scope = commands::RefreshScope {
                only: only.as_ref().map(|(p, name)| (*p, name.as_str())),
                force,
                within_min: within,
            };
            commands::refresh(&store, &lives, scope, out, &mut io::stderr())?;
            let skipped = plan.map_or(0, |p| p.skipped.len());
            if skipped > 0 {
                bail!("{skipped} CLI(s) skipped");
            }
            Ok(())
        }
        Cmd::Remove { name } => {
            let (p, name) = commands::resolve(&store, &providers, &name)?;
            let state = ops::sync_live(&store, p)?;
            commands::remove(&store, p, &state, &name, out)
        }
        Cmd::Label { name, text } => {
            let (p, name) = commands::resolve(&store, &providers, &name)?;
            commands::label(&store, p, &name, text.as_deref(), out)
        }
        Cmd::Rename { name, new_name } => {
            let (p, name) = commands::resolve(&store, &providers, &name)?;
            commands::rename(&store, p, &name, &new_name, out)
        }
        Cmd::Login { .. } | Cmd::Serve { .. } | Cmd::Completions { .. } | Cmd::Update { .. } => {
            unreachable!()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redirect_page_quotes_the_url_for_both_of_its_uses() {
        let page = redirect_page("http://127.0.0.1:1/#a&b\"<c");
        assert!(page.contains("url=http://127.0.0.1:1/#a&amp;b&quot;&lt;c\""));
        assert!(page.contains(r#"location.replace("http://127.0.0.1:1/#a&b\"\u003cc")"#));
        assert!(!page.contains("\"<c"));
    }
}
