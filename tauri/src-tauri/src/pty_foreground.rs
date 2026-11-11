//! Terminal foreground identity shared with a failure-bounded integration test.
pub(crate) fn shell_is_foreground(
    master: &dyn portable_pty::MasterPty,
    child: &dyn portable_pty::Child,
) -> bool {
    #[cfg(unix)]
    {
        master
            .process_group_leader()
            .and_then(|pid| u32::try_from(pid).ok())
            .zip(child.process_id())
            .is_some_and(|(foreground, shell)| foreground == shell)
    }
    #[cfg(not(unix))]
    {
        let _ = (master, child);
        false
    }
}
