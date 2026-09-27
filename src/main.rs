mod claude;
mod commands;
mod fsx;
mod live;
mod ops;
mod paths;
mod project;
mod store;

use std::io::{self, BufRead};
use std::process::{Command, Stdio};

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::claude::Api;
use crate::live::Live;
use crate::project::REMUDA;
use crate::store::Store;

/// Keep several Claude Code logins and switch between them.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List stored credentials and which one Claude Code is using.
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Store the login Claude Code is signed into right now.
    Import {
        name: String,
        /// Replace a stored credential of the same name.
        #[arg(long)]
        force: bool,
    },
    /// Sign a new account in through the browser and store it, without touching Claude Code.
    Login {
        name: String,
        #[arg(long)]
        force: bool,
        /// Print the URL instead of opening it.
        #[arg(long)]
        no_browser: bool,
    },
    /// Make a stored credential the one Claude Code uses.
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
    /// Delete a stored credential.
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Cmd::Update { check } = cli.command {
        return update(check);
    }
    let store = Store::open(&paths::store_root());
    let live = Live::from_env();
    let api = Api::claude()?;
    let out = &mut io::stdout();
    if let Cmd::Login {
        name,
        force,
        no_browser,
    } = &cli.command
    {
        let open = |url: &str| {
            if !no_browser {
                let _ = Command::new("xdg-open")
                    .arg(url)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
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
        return commands::login(&store, &api, name, *force, prompt, out);
    }
    let _lock = store.lock()?;
    if let Cmd::Use { name, discard } = &cli.command {
        return ops::switch(&store, &live, &api, name, *discard, &|entry| {
            commands::refresh_entry(&api, entry)
        });
    }
    let state = ops::sync_live(&store, &live, &api)?;
    match cli.command {
        Cmd::List { json } => commands::list(&store, &state, json, out),
        Cmd::Import { name, force } => commands::import(&store, &live, &state, &name, force, out),
        Cmd::Refresh {
            name,
            force,
            within,
        } => {
            let scope = commands::RefreshScope {
                only: name.as_deref(),
                force,
                within_min: within,
            };
            commands::refresh(&store, &api, &state, scope, out, &mut io::stderr())
        }
        Cmd::Remove { name } => commands::remove(&store, &state, &name, out),
        Cmd::Use { .. } | Cmd::Login { .. } | Cmd::Update { .. } => unreachable!(),
    }
}
