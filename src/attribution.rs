//! What remuda writes to the ledger TokenGauge attributes spend with
//! (`selvedge::usage::ledger`): which login each CLI's files hold, and from
//! when. A failed write costs that line and nothing else: no command fails
//! for it.

use std::path::Path;
use std::time::UNIX_EPOCH;

use selvedge::usage::ledger::{self, Entry as Line};

use crate::fsx::now_ms;
use crate::ops::LiveState;
use crate::provider::Provider;
use crate::store::{Entry, Store};

/// A switch put `entry` in the CLI's files just now.
pub fn switched(store: &Store, p: &dyn Provider, entry: &Entry) {
    let _ = ledger::append(
        store.root(),
        &Line::Live {
            at: now_ms(),
            provider: p.id().to_owned(),
            account_id: Some(entry.meta.account_id.clone()),
            name: Some(entry.name.clone()),
        },
    );
}

/// The CLI's files were read and hold `live`. When that is not the login the
/// ledger last wrote down, something outside remuda changed it, at the
/// latest when the file was last written.
pub fn noticed(store: &Store, p: &dyn Provider, live: &LiveState) {
    let (account_id, name) = match live {
        LiveState::Stored { name, .. } => match store.get(p.id(), name) {
            Ok(Some(entry)) if entry.verified => (Some(entry.meta.account_id), Some(entry.name)),
            Ok(_) => (None, None),
            Err(_) => return,
        },
        LiveState::SignedOut | LiveState::Unstored { .. } | LiveState::Foreign { .. } => {
            (None, None)
        }
        LiveState::Unreadable { .. } => return,
    };
    let Ok(lines) = ledger::read(store.root()) else {
        return;
    };
    let last = lines.iter().rev().find_map(|line| match line {
        Line::Live {
            at,
            provider,
            account_id,
            ..
        } if provider == p.id() => Some((*at, account_id)),
        _ => None,
    });
    let at = match last {
        Some((_, last)) if *last == account_id => return,
        None if account_id.is_none() => return,
        None => now_ms(),
        Some((then, _)) => p
            .live_file()
            .as_deref()
            .and_then(modified_ms)
            .map_or_else(now_ms, |at| at.min(now_ms()))
            .max(then + 1),
    };
    let _ = ledger::append(
        store.root(),
        &Line::Live {
            at,
            provider: p.id().to_owned(),
            account_id,
            name,
        },
    );
}

fn modified_ms(path: &Path) -> Option<i64> {
    let at = std::fs::metadata(path).ok()?.modified().ok()?;
    i64::try_from(at.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}
