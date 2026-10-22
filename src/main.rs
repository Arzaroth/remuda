mod apikey;
mod auto;
mod browser;
mod claude;
mod codex;
mod commands;
mod config;
mod cursor;
mod device;
mod dirs;
mod fsx;
mod gauge;
mod grok;
mod http;
mod kimi;
mod logbook;
mod oauth;
mod ops;
mod paths;
mod pins;
mod pkce;
mod project;
mod provider;
mod proxy;
mod runs;
mod serve;
mod served;
mod session;
mod shown;
mod store;
mod units;
mod warm;

use std::io::{self, BufRead};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};

use crate::apikey::ApiKey;
use crate::claude::Claude;
use crate::codex::Codex;
use crate::cursor::Cursor;
use crate::grok::Grok;
use crate::kimi::Kimi;
use crate::logbook::By;
use crate::project::REMUDA;
use crate::provider::Provider;
use crate::store::Store;

/// Keep several Claude Code and Codex logins and switch between them.
#[derive(Parser)]
#[command(
    version,
    args_conflicts_with_subcommands = true,
    arg_required_else_help = true
)]
struct Cli {
    /// Replace this binary with the latest release.
    #[arg(long, conflicts_with = "check_update")]
    update: bool,
    /// Only report whether a newer release exists.
    #[arg(long)]
    check_update: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

const PROVIDERS: [&str; 7] = [
    "claude", "codex", "cursor", "glm", "grok", "kimi", "opencode",
];

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
    /// Print the shell line that makes a CLI reading its key from the
    /// environment use a stored one: `eval "$(remuda env glm/work)"`.
    Env { name: String },
    /// Rename a stored credential. NAME or PROVIDER/NAME.
    Rename { name: String, new_name: String },
    /// Delete a stored credential. NAME or PROVIDER/NAME.
    #[command(alias = "rm")]
    Remove { name: String },
    /// Say which login auto mode puts each CLI on now, and why, without switching.
    Auto,
    /// Start a CLI on a stored login without switching the one every other
    /// session uses: NAME, PROVIDER/NAME, or a provider alone for the login
    /// `.remuda.toml` pins here, else the one it uses now. Anything after `--`
    /// goes to the CLI.
    Run {
        name: String,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Pin a login for this directory and below in ./.remuda.toml, so
    /// `remuda run <cli>` starts it there; with no NAME, show the pins in force.
    Pin {
        name: Option<String>,
        /// Unpin this CLI from ./.remuda.toml instead.
        #[arg(long, value_name = "CLI", conflicts_with = "name")]
        remove: Option<String>,
    },
    /// Print shell functions that start `claude` and `codex` through
    /// `remuda run`, for the login pinned where they start:
    /// `eval "$(remuda shell-init zsh)"`.
    ShellInit { shell: String },
    /// `remuda claude [NAME] [ARGS...]`, read before clap so the CLI's own
    /// arguments reach it untouched.
    #[command(skip)]
    Cli(Vec<String>),
    /// Show what remuda did: switches and who made them, refreshes, auto
    /// mode, keep-warm, the proxy. PROVIDER, NAME or PROVIDER/NAME narrows it.
    Log {
        /// A provider (claude), a login (claude/work) or a bare name.
        name: Option<String>,
        /// How many of the latest events to show.
        #[arg(short = 'n', long, default_value_t = 50, value_parser = clap::value_parser!(u64).range(1..))]
        lines: u64,
        /// Print the events as JSON.
        #[arg(long)]
        json: bool,
    },
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
    /// Run by `--update` as the new binary: bring the installed units up to date.
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
            Cmd::Env { .. } => Some("printing a key"),
            Cmd::Rename { .. } => Some("renaming"),
            Cmd::Remove { .. } => Some("removing"),
            Cmd::Auto => Some("checking auto mode"),
            Cmd::Log { .. } => Some("reading the log"),
            Cmd::Run { .. } | Cmd::Cli(_) | Cmd::ShellInit { .. } => None,
            Cmd::Pin { name, .. } => name.as_ref().map(|_| "pinning"),
            Cmd::Serve { .. } => Some("serving"),
            Cmd::Import { .. }
            | Cmd::Login { .. }
            | Cmd::Open { .. }
            | Cmd::Completions { .. }
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

const SHIELDED: [libc::c_int; 4] = [libc::SIGINT, libc::SIGQUIT, libc::SIGHUP, libc::SIGTERM];

/// Waits for the CLI with the signals that end a terminal session ignored,
/// so remuda outlives it and syncs its tokens back; the CLI gets them as
/// usual.
fn shielded(cmd: &mut std::process::Command) -> io::Result<std::process::ExitStatus> {
    use std::os::unix::process::CommandExt;
    // SAFETY: signal() is async-signal-safe, and nothing else runs between
    // fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            for s in SHIELDED {
                libc::signal(s, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    // SAFETY: the previous handlers are put back once the CLI exits.
    let before: Vec<_> = SHIELDED
        .iter()
        .map(|s| unsafe { libc::signal(*s, libc::SIG_IGN) })
        .collect();
    let status = cmd.status();
    for (s, h) in SHIELDED.iter().zip(before) {
        unsafe { libc::signal(*s, h) };
    }
    status
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(1)
}

/// The CLI as it is, in place of remuda: nothing to sync back after.
fn exec(command: &str, args: &[String]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(command).args(args).exec();
    Err(err).with_context(|| format!("failed to start {command}"))
}

/// `remuda claude work -- x` and `remuda claude x`: a first word is the
/// login only when that CLI has one stored by that name, so the CLI's own
/// arguments pass through untouched.
fn shorthand(
    store: &Store,
    providers: &[&dyn Provider],
    words: &[String],
) -> Result<(String, Vec<String>)> {
    let (cli, rest) = words.split_first().context("no command")?;
    let p = providers
        .iter()
        .find(|p| p.id() == cli && p.run_spec().is_some())
        .with_context(|| format!("unrecognized subcommand '{cli}'; see `remuda --help`"))?;
    let named = rest.first().filter(|w| {
        !w.starts_with('-')
            && store::validate_name(w).is_ok()
            && store.get(p.id(), w).ok().flatten().is_some()
    });
    let (target, rest) = match named {
        Some(name) => (format!("{cli}/{name}"), &rest[1..]),
        None => (cli.clone(), rest),
    };
    let args = match rest.split_first() {
        Some((dashes, after)) if dashes == "--" => after.to_vec(),
        _ => rest.to_vec(),
    };
    Ok((target, args))
}

fn pin(
    store: &Store,
    providers: &[&dyn Provider],
    name: Option<&str>,
    remove: Option<&str>,
) -> Result<()> {
    let here = std::env::current_dir().context("cannot tell which directory this is")?;
    if let Some(cli) = remove {
        let p = commands::find(providers, cli)?;
        let file = pins::set(&here, p.id(), None)?;
        println!("unpinned {} in {}", p.name(), file.display());
        return Ok(());
    }
    if let Some(spec) = name {
        let (p, name) = commands::resolve(store, providers, spec)?;
        p.run_spec()
            .with_context(|| format!("remuda run cannot start {} yet", p.name()))?;
        let file = pins::set(&here, p.id(), Some(&name))?;
        println!("pinned {} to {name} in {}", p.name(), file.display());
        return Ok(());
    }
    let mut any = false;
    for p in providers.iter().filter(|p| p.run_spec().is_some()) {
        if let Some(pin) = pins::find(&here, p.id())? {
            println!("{}: {} ({})", p.id(), pin.name, pin.file.display());
            any = true;
        }
    }
    if !any {
        println!("nothing is pinned here");
    }
    Ok(())
}

fn run(store: &Store, providers: &[&dyn Provider], target: &str, args: &[String]) -> Result<()> {
    let (p, name) = match providers.iter().find(|p| p.id() == target) {
        Some(p) => {
            let here = std::env::current_dir().unwrap_or_default();
            (*p, pins::find(&here, p.id())?.map(|pin| pin.name))
        }
        None => {
            let (p, name) = commands::resolve(store, providers, target)?;
            (p, Some(name))
        }
    };
    let spec = p
        .run_spec()
        .with_context(|| format!("remuda run cannot start {} yet", p.name()))?;
    let Some(name) = name else {
        return exec(spec.command, args);
    };
    let started = {
        let _lock = store.lock()?;
        session::recover(store, &[p]);
        let live = ops::sync_live(store, p)?;
        if live.active_name() == Some(name.as_str()) {
            None
        } else {
            Some(session::start(store, p, &name)?)
        }
    };
    let Some((lease, spec, home)) = started else {
        return exec(spec.command, args);
    };
    eprintln!("remuda: {} on {name} for this session", p.name());
    let status = shielded(
        std::process::Command::new(spec.command)
            .args(args)
            .env(spec.var, &home),
    );
    let finished = store
        .lock()
        .and_then(|_lock| session::finish(store, p, &spec, &name));
    drop(lease);
    match &finished {
        Ok(()) => logbook::note(
            store,
            logbook::Event::new(logbook::Kind::Run, true, "the session ended")
                .about(p.id(), Some(&name)),
        ),
        Err(e) => eprintln!("warning: {e:#}"),
    }
    let status = status.with_context(|| format!("failed to start {}", spec.command))?;
    std::process::exit(exit_code(status));
}

/// The CLIs `remuda <cli> [NAME] [ARGS...]` starts.
const SHORTHAND: [&str; 2] = ["claude", "codex"];

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let command = match argv.get(1) {
        Some(first) if SHORTHAND.contains(&first.as_str()) => Cmd::Cli(argv[1..].to_vec()),
        _ => {
            let cli = Cli::parse();
            match cli.command {
                Some(command) => command,
                None => return update(cli.check_update),
            }
        }
    };
    if let Cmd::Open { no_browser } = command {
        return open(no_browser);
    }
    if let Cmd::SyncUnits = command {
        return sync_units();
    }
    if let Cmd::Completions { shell } = command {
        clap_complete::generate(shell, &mut Cli::command(), "remuda", &mut io::stdout());
        return Ok(());
    }
    let store = Store::open(&paths::store_root());
    let claude = Claude::from_env()?;
    let codex = Codex::from_env()?;
    let grok = Grok::from_env()?;
    let kimi = Kimi::from_env()?;
    let cursor = Cursor::from_env()?;
    let glm = ApiKey::glm();
    let opencode = ApiKey::opencode();
    let providers: [&dyn Provider; 7] = [&claude, &codex, &cursor, &glm, &grok, &kimi, &opencode];
    let seen = dirs::now(&providers);
    if let Some(doing) = command.needs_store()
        && !store.root().exists()
    {
        bail!(
            "not {doing}: there is no store at {}; `remuda import` or `remuda login` starts one, or set REMUDA_STORE in ~/.config/environment.d/60-remuda.conf if yours is elsewhere",
            store.root().display()
        );
    }
    let service = dirs::is_service(matches!(
        command,
        Cmd::Refresh {
            scheduled: true,
            ..
        }
    ));
    if !service {
        dirs::record(&store, &seen);
    }
    if let Cmd::Serve { port, no_browser } = command {
        if service {
            let plan = dirs::plan(dirs::recorded(&store).as_ref(), &seen, "serving");
            for line in &plan.skipped {
                eprintln!("{line}");
            }
            if !plan.skipped.is_empty() {
                bail!("not serving: the service would act on other logins than the shell's");
            }
        }
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(claude),
            Box::new(codex),
            Box::new(cursor),
            Box::new(glm),
            Box::new(grok),
            Box::new(kimi),
            Box::new(opencode),
        ];
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
        let app = std::sync::Arc::new(app);
        serve::watch(app.clone());
        app.start();
        http::serve(app, listener, http::Limits::default());
        return Ok(());
    }
    let out = &mut io::stdout();

    if let Cmd::Login {
        name,
        provider,
        force,
        no_browser,
    } = &command
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

    if let Cmd::Run { name, args } = &command {
        return run(&store, &providers, name, args);
    }
    if let Cmd::Cli(words) = &command {
        let (target, args) = shorthand(&store, &providers, words)?;
        return run(&store, &providers, &target, &args);
    }
    if let Cmd::ShellInit { shell } = &command {
        let commands: Vec<&str> = providers
            .iter()
            .filter_map(|p| p.run_spec().map(|s| s.command))
            .collect();
        print!("{}", pins::shell_init(shell, &commands)?);
        return Ok(());
    }
    if let Cmd::Pin { name, remove } = &command {
        return pin(&store, &providers, name.as_deref(), remove.as_deref());
    }

    let _lock = store.lock()?;
    match command {
        Cmd::Use { name, discard } => {
            let (p, name) = commands::resolve(&store, &providers, &name)?;
            println!("{}", ops::switch_by(&store, p, &name, discard, By::Cli)?);
            gauge::nudge(&paths::tokengauge_snapshot());
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
            if !report.refreshed.is_empty() {
                gauge::nudge(&paths::tokengauge_snapshot());
            }
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
        Cmd::Auto => {
            let lives = commands::sync_all(&store, &providers)?;
            commands::auto(
                &store,
                &lives,
                &paths::config_file(),
                &paths::tokengauge_snapshot(),
                out,
            )
        }
        Cmd::Log { name, lines, json } => commands::log(
            &store,
            name.as_deref(),
            usize::try_from(lines).unwrap_or(usize::MAX),
            json,
            &jiff::tz::TimeZone::system(),
            out,
        ),
        Cmd::Remove { name } => {
            let (p, name) = commands::resolve(&store, &providers, &name)?;
            let state = ops::sync_live(&store, p)?;
            commands::remove(&store, &paths::config_file(), p, &state, &name, out)
        }
        Cmd::Env { name } => {
            let (p, name) = commands::resolve(&store, &providers, &name)?;
            commands::print_env(&store, p, &name, out)
        }
        Cmd::Label { name, text } => {
            let (p, name) = commands::resolve(&store, &providers, &name)?;
            commands::label(&store, p, &name, text.as_deref(), out)
        }
        Cmd::Rename { name, new_name } => {
            let (p, name) = commands::resolve(&store, &providers, &name)?;
            commands::rename(&store, &paths::config_file(), p, &name, &new_name, out)
        }
        Cmd::Login { .. }
        | Cmd::Run { .. }
        | Cmd::Cli(_)
        | Cmd::ShellInit { .. }
        | Cmd::Pin { .. }
        | Cmd::Serve { .. }
        | Cmd::Open { .. }
        | Cmd::Completions { .. }
        | Cmd::SyncUnits => {
            unreachable!()
        }
    }
}
