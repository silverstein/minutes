//! Portable interaction primitives. No audio, desktop, network, process or file I/O.
//!
//! These are host-side contracts, not model tools. In particular, local approval,
//! capture grants and source attestations must never be deserialized from a model
//! response. Platform adapters collect evidence; the host owns authority.

pub mod attention;
pub mod authority;
pub mod live;
pub mod work;

pub type Result<T> = std::result::Result<T, String>;
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_ITEMS: usize = 256;
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;

fn text(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES {
        return Err("text must be nonempty and at most 16384 bytes".into());
    }
    Ok(())
}

fn next(value: u64) -> Result<u64> {
    value
        .checked_add(1)
        .ok_or_else(|| "generation exhausted".into())
}

#[cfg(test)]
mod tests;
