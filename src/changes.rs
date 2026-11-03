//! The change-tick idiom every change source shares: a `watch::Sender<u64>`
//! whose value only matters for being different.

use tokio::sync::watch;

/// Tells every receiver something changed; the counter wraps.
pub fn bump(tx: &watch::Sender<u64>) {
    tx.send_modify(|n| *n = n.wrapping_add(1));
}
