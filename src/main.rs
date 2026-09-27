mod claude;
mod commands;
mod fsx;
mod ops;
mod paths;
mod pkce;
mod project;
mod provider;
mod store;

use std::io::{self, BufRead};
use std::process::{Command, Stdio};

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::claude::Claude;
use crate::project::REMUDA;
use crate::provider::Provider;
use crate::store::Store;

/// Keep several Claude Code logins and switch between them.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

const PROVIDERS: [&str; 1] = ["claude"];

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
        #[arg(short, long, default_value = "claude", value_parser = PROVIDERS)]
        provider: String,
        /// Replace a stored credential of the same name.
        #[arg(long)]
        force: bool,
    },
    /// Sign a new account in through the browser and store it, without touching the CLI.
    Login {
        name: String,
        #[arg(short, long, default_value = "claude", value_parser = PROVIDERS)]
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
    },
    /// Set the label shown beside a credential, or clear it. NAME or PROVIDER/NAME.
    Label { name: String, text: Option<String> },
    /// Rename a stored credential. NAME or PROVIDER/NAME.
    Rename { name: String, new_name: String },
    /// Delete a stored credential. NAME or PROVIDER/NAME.
    #[command(alias = "rm")]
    Remove { name: String },
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

fn open_in_browser(url: &str) {
    let _ = Command::new("xdg-open")
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Cmd::Update { check } = cli.command {
        return update(check);
    }
    let store = Store::open(&paths::store_root());
    let claude = Claude::from_env()?;
    let providers: [&dyn Provider; 1] = [&claude];
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
        } => {
            let only = name
                .as_deref()
                .map(|spec| commands::resolve(&store, &providers, spec))
                .transpose()?;
            let lives = commands::sync_all(&store, &providers)?;
            let scope = commands::RefreshScope {
                only: only.as_ref().map(|(p, name)| (*p, name.as_str())),
                force,
                within_min: within,
            };
            commands::refresh(&store, &lives, scope, out, &mut io::stderr())
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
        Cmd::Login { .. } | Cmd::Update { .. } => unreachable!(),
    }
}
