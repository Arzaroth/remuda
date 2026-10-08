mod browser;
mod claude;
mod codex;
mod commands;
mod dirs;
mod fsx;
mod gauge;
mod http;
mod oauth;
mod ops;
mod paths;
mod pkce;
mod project;
mod provider;
mod runs;
mod serve;
mod served;
mod store;
mod units;

use std::io::{self, BufRead};

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
    /// Open the page a running `remuda serve` shows, starting remuda-serve.service when it is installed.
    Open {
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
    /// Run by `update` as the new binary: bring the installed units up to date.
    #[command(hide = true)]
    SyncUnits,
}

impl Cmd {
    /// What the command was doing, for one that will not start a store.
    fn needs_store(&self) -> Option<&'static str> {
        match self {
            Cmd::Use { .. } => Some("switching"),
            Cmd::List { .. } => Some("listing"),
            Cmd::Refresh { .. } => Some("refreshing"),
            Cmd::Label { .. } => Some("labelling"),
            Cmd::Rename { .. } => Some("renaming"),
            Cmd::Remove { .. } => Some("removing"),
            Cmd::Serve { .. } => Some("serving"),
            Cmd::Import { .. }
            | Cmd::Login { .. }
            | Cmd::Open { .. }
            | Cmd::Completions { .. }
            | Cmd::Update { .. }
            | Cmd::SyncUnits => None,
        }
    }
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
    let exe = std::env::current_exe();
    let applied = selvedge::update::apply(&REMUDA, &cache)?;
    if applied.version == REMUDA.version {
        println!("remuda {} is the latest release", applied.version);
    } else {
        println!("updated remuda {} -> {}", REMUDA.version, applied.version);
        let synced = exe.and_then(|exe| {
            std::process::Command::new(exe)
                .arg("sync-units")
                .stdin(std::process::Stdio::null())
                .status()
        });
        if !synced.is_ok_and(|s| s.success()) {
            eprintln!(
                "warning: the systemd units were not brought up to date; run `remuda sync-units`"
            );
        }
    }
    Ok(())
}

fn systemctl(args: &[&str]) -> bool {
    std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn sync_units() -> Result<()> {
    let done = units::refresh(&paths::installed_units());
    if !done.changed.is_empty() {
        println!("updated {}", done.changed.join(", "));
        if !systemctl(&["daemon-reload"]) {
            eprintln!("warning: run `systemctl --user daemon-reload`");
        }
    }
    if systemctl(&["is-active", "--quiet", SERVE_UNIT]) && !systemctl(&["restart", SERVE_UNIT]) {
        eprintln!(
            "warning: {SERVE_UNIT} still runs the old binary; run `systemctl --user restart {SERVE_UNIT}`"
        );
    }
    if !done.failed.is_empty() {
        bail!("{}", done.failed.join("\n"));
    }
    Ok(())
}

const SERVE_UNIT: &str = "remuda-serve.service";

fn start_service() -> bool {
    let installed = paths::systemd_user_units()
        .iter()
        .any(|dir| dir.join(SERVE_UNIT).exists());
    installed
        && std::process::Command::new("systemctl")
            .args(["--user", "start", SERVE_UNIT])
            .stdin(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

fn open(no_browser: bool) -> Result<()> {
    let url = served::find(
        &paths::runtime_dir(),
        start_service,
        std::time::Duration::from_secs(10),
    )?;
    if no_browser {
        println!("{url}");
    } else {
        browser::open(&url);
        let shown = url.split('#').next().unwrap_or_default();
        println!("opened {shown}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Cmd::Open { no_browser } = cli.command {
        return open(no_browser);
    }
    if let Cmd::Update { check } = cli.command {
        return update(check);
    }
    if let Cmd::SyncUnits = cli.command {
        return sync_units();
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
    if let Some(doing) = cli.command.needs_store()
        && !store.root().exists()
    {
        bail!(
            "not {doing}: there is no store at {}; `remuda import` or `remuda login` starts one, or set REMUDA_STORE in ~/.config/environment.d/60-remuda.conf if yours is elsewhere",
            store.root().display()
        );
    }
    let service = dirs::is_service(matches!(
        cli.command,
        Cmd::Refresh {
            scheduled: true,
            ..
        }
    ));
    if !service {
        dirs::record(&store, &seen);
    }
    if let Cmd::Serve { port, no_browser } = cli.command {
        if service {
            let plan = dirs::plan(dirs::recorded(&store).as_ref(), &seen, "serving");
            for line in &plan.skipped {
                eprintln!("{line}");
            }
            if !plan.skipped.is_empty() {
                bail!("not serving: the service would act on other logins than the shell's");
            }
        }
        let providers: Vec<Box<dyn Provider>> = vec![Box::new(claude), Box::new(codex)];
        let (listener, port) = http::bind(port)?;
        let token = pkce::random()?;
        let url = format!("http://127.0.0.1:{port}/#{token}");
        if let Err(e) = served::announce(&paths::runtime_dir(), port, &url) {
            eprintln!("warning: `remuda open` will not find this page: {e:#}");
        }
        if service {
            println!("remuda is serving http://127.0.0.1:{port}/; `remuda open` opens it");
        } else {
            println!(
                "remuda is serving {url}\nThe link carries its access token; keep it to yourself. Ctrl-C stops it."
            );
        }
        if !no_browser && !service {
            browser::open(&url);
        }
        let open: serve::Opener = if no_browser {
            Box::new(|_| false)
        } else {
            Box::new(browser::open_private)
        };
        let app = serve::App::new(
            store,
            serve::Places::from_env(),
            providers,
            token,
            port,
            open,
        );
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
            if *no_browser {
                return false;
            }
            browser::open_private(url) || {
                browser::open(url);
                false
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
            ..
        } => {
            let plan =
                service.then(|| dirs::plan(dirs::recorded(&store).as_ref(), &seen, "refreshing"));
            for line in plan.iter().flat_map(|p| &p.skipped) {
                eprintln!("{line}");
            }
            let mut report = commands::Report::default();
            let result = (|| {
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
                        name.as_deref().unwrap_or_default()
                    );
                }
                let lives = commands::sync_all(&store, &usable)?;
                let scope = commands::RefreshScope {
                    only: only.as_ref().map(|(p, name)| (*p, name.as_str())),
                    force,
                    within_min: within,
                };
                commands::refresh(&store, &lives, scope, out, &mut io::stderr(), &mut report)
            })();
            if let Some(plan) = &plan {
                let run = runs::Run::of(fsx::now_ms(), &plan.skipped, report, &result);
                runs::record(&store, &run);
            }
            result?;
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
        Cmd::Login { .. }
        | Cmd::Serve { .. }
        | Cmd::Open { .. }
        | Cmd::Completions { .. }
        | Cmd::Update { .. }
        | Cmd::SyncUnits => {
            unreachable!()
        }
    }
}
