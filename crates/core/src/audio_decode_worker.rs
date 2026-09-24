//! Bounded compressed-audio decode worker.
//!
//! ffmpeg remains the preferred decoder: its resampler and AAC decoder produce
//! samples that whisper transcribes correctly across languages, while
//! Symphonia's AAC decoder produces subtly different samples that trigger
//! hallucination loops on non-English audio (issue #21). This worker exists so
//! that a user who has never installed ffmpeg keeps the behaviour they had
//! before the conversation-trust work: iPhone/iCloud voice memos are `.m4a`,
//! and the folder watcher is a headline input mode, so requiring ffmpeg would
//! be a default-user regression.
//!
//! Symphonia is never linked into a decode that runs in this process. Container
//! probing can allocate attacker-declared tables, and the objection to
//! in-process use was one of ordering: the allocation happens before any
//! resource limit applies. Confining it to a child inverts that ordering, and
//! the ceiling binds before Symphonia reads a single attacker-controlled byte.
//!
//! How that ceiling is configured is platform-specific:
//!
//! - Unix other than macOS configures it from the parent's `pre_exec`, before
//!   `exec`, and the child refuses to parse unless it sees the expected values.
//! - Windows configures process and job memory limits on an initially suspended
//!   process.
//! - macOS attempts to install a measured baseline-plus-budget ceiling in the
//!   child at [`maybe_run_audio_decode_worker`], before constructing a decoder.
//!
//! Verification scope for this lane: Linux and macOS were exercised through the
//! real child path. On macOS, successful decode and duration-probe children prove
//! that ceiling installation returned success before decoder construction, and a
//! dedicated subprocess test proves Darwin refuses a mapping larger than the
//! installed growth budget. No mutation-sensitive test proves that the production
//! entry point retains its call to the installer. The Windows command builder was
//! inspected and tested, but Job Object attachment, ordering, and effective
//! enforcement were not executed here.
//!
//! The child additionally gets a wall-clock ceiling, capped stdout streamed
//! into a private file it has no pathname for, and a cleared environment.
//!
//! Ambient descriptors are marked `FD_CLOEXEC` on Unix and therefore closed BY
//! `exec`, not before it: closing them outright would destroy the spawn-error
//! pipe std relies on.
//!
//! **Windows has no equivalent sweep here, and the child can inherit ambient
//! HANDLEs.** `CreateProcessW` is invoked with `bInheritHandles: TRUE`.
//! `graph_worker` closes them; this worker does not, and the reasoning for that
//! is at the point in [`maybe_run_audio_decode_worker`] where the call would go.
//!
//! The worker emits the same bytes ffmpeg is asked for, raw 16 kHz mono
//! `s16le` PCM on stdout, so both decoders share one downstream path.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Set on the child so a re-executed Minutes binary knows to decode instead of
/// running its ordinary command line. Mirrors the policy graph worker's marker
/// contract.
const WORKER_MARKER: &str = "MINUTES_AUDIO_DECODE_WORKER_V1";

/// Address-space budget for the decode child: configured as an absolute ceiling
/// on non-macOS Unix and Windows, and as a growth allowance on macOS.
///
/// A four-hour input holds roughly 921 MB of `f32` output alongside 461 MB of
/// `s16le` bytes, so the budget must clear ~1.4 GB plus allocator slack while
/// still failing an attacker-declared allocation inside the child rather than
/// exhausting the machine.
///
/// WHAT THIS NUMBER MEANS DIFFERS BY PLATFORM, and this doc used to say it was
/// "a growth allowance over the process baseline, never an absolute ceiling",
/// which is true on macOS only. Anyone sizing the budget from that sentence on
/// Linux would be out by the whole image:
///
/// - non-macOS Unix: an ABSOLUTE ceiling on VIRTUAL ADDRESS SPACE. The parent
///   sets `rlim_cur = rlim_max` to this figure, so the mapped executable and
///   every shared library come out of it. The debug worker binary alone is
///   ~250 MB.
/// - Windows: `ProcessMemoryLimit` and `JobMemoryLimit`, which bound COMMITTED
///   memory, not reserved address space. File-backed image pages are not charged
///   the way they are under `RLIMIT_AS`, so the sizing above does not transfer.
///   An earlier version of this doc grouped Windows with Unix and implied it
///   did. What the two share is only that the figure is absolute rather than a
///   growth allowance.
/// - macOS: a growth allowance. The child measures its own virtual size and
///   installs baseline plus this budget, because Darwin rejects an absolute
///   `RLIMIT_AS` below its pre-`main()` shared-cache baseline.
const WORKER_ADDRESS_SPACE_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// Install the child's own address-space ceiling, measured against this
/// process's baseline.
///
/// The macOS path adds the budget to the measured process baseline rather than
/// configuring an absolute three-GiB limit from the parent.
///
/// Ordering is the security property and is preserved either way: this runs
/// before the decoder is constructed and therefore before Symphonia reads a
/// single attacker-controlled byte. Only dyld and Rust runtime startup precede
/// it, and neither touches the input.
///
/// EXECUTED AND EFFECTIVELY ENFORCED under test. Native macOS end-to-end decode
/// and duration-probe tests launch the real child and prove that this function
/// returns success, because failure exits 71 before decoder construction.
/// `the_macos_child_ceiling_refuses_an_over_budget_mapping` installs the same
/// ceiling in an isolated subprocess and requires Darwin to refuse a mapping
/// larger than the entire growth budget.
///
/// That is worth stating loudly, because the non-macOS sibling
/// [`verify_parent_bound_address_space`] carries a per-branch TESTED BY list, so
/// the least-verified path was reading as the best-covered one. One gap remains:
/// deleting the production entry point's call to this function leaves the suite
/// green because the enforcement test calls the installer directly. Closing that
/// caller-relationship gap needs a real child test that deliberately omits the
/// install and requires refusal, the way the non-macOS Unix tests do.
#[cfg(target_os = "macos")]
fn install_child_address_space_ceiling() -> Result<(), String> {
    let baseline = process_virtual_size()?;
    let limit = baseline
        .checked_add(WORKER_ADDRESS_SPACE_BYTES)
        .ok_or_else(|| "decode worker address-space ceiling overflowed".to_string())?;
    let rlimit = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &rlimit) } != 0 {
        return Err("decode worker could not install its address-space ceiling".into());
    }
    Ok(())
}

/// Measure this process's current virtual size so the ceiling can be expressed
/// as baseline plus budget.
#[cfg(target_os = "macos")]
fn process_virtual_size() -> Result<u64, String> {
    use mach2::kern_return::KERN_SUCCESS;
    use mach2::task::task_info;
    use mach2::task_info::{
        task_basic_info_64, task_info_t, TASK_BASIC_INFO_64, TASK_BASIC_INFO_64_COUNT,
    };
    use mach2::traps::mach_task_self;

    let mut info = task_basic_info_64::default();
    let mut count = TASK_BASIC_INFO_64_COUNT;
    let status = unsafe {
        task_info(
            mach_task_self(),
            TASK_BASIC_INFO_64,
            (&mut info as *mut task_basic_info_64).cast::<libc::c_int>() as task_info_t,
            &mut count,
        )
    };
    if status != KERN_SUCCESS || count != TASK_BASIC_INFO_64_COUNT {
        return Err("decode worker could not measure its address space".into());
    }
    Ok(info.virtual_size)
}

/// Refuse to parse anything unless both `RLIMIT_AS` values equal the worker
/// budget.
///
/// On non-macOS Unix the ceiling is installed by the parent, before `exec`, so
/// nothing inside the child would otherwise notice it missing: an unbounded
/// worker decodes attacker-controlled bytes exactly as a bounded one does, right
/// up until it exhausts the machine. Checking it here makes the ceiling an
/// observable property of launches ON THOSE PLATFORMS, which is what lets the
/// real entry points be tested for containment instead of a command builder a
/// test called itself and then asserted.
///
/// Not "every launch", which is what this said first. macOS installs its own
/// ceiling in the child and verifies nothing afterwards; Windows installs Job
/// Object limits in the parent and has no child-side check at all.
///
/// WHAT THIS ESTABLISHES, stated narrowly because an earlier version of this
/// comment did not. It compares two numbers to a constant. Both `rlim_cur` and
/// `rlim_max` must equal the worker budget exactly. That is a check on the
/// VALUE, not on who installed it: a foreign launcher that set both limits to
/// exactly this constant would be accepted, and nothing here could tell the
/// difference. Calling it a provenance check, as this comment once did, claimed
/// an authentication property the code does not have.
///
/// What exact equality does buy over "finite and no looser than the budget",
/// which is what this checked first: an ambient `ulimit -v` under the budget no
/// longer satisfies it. The parent sets `rlim_cur` and `rlim_max` to this
/// constant, so equality holds for every real launch. If a lower ambient hard
/// limit were in force, the parent's own `setrlimit` would fail and no child
/// would exist to run this.
///
/// Requiring `rlim_max` too is what stops a child from raising its own soft
/// limit back up.
///
/// The consequence, stated rather than left implicit: a decode child contained
/// by some OTHER mechanism, a cgroup or an outer sandbox, while presenting an
/// unbounded `RLIMIT_AS`, is refused here. That is deliberate. This worker's
/// containment argument is the ceiling its own parent installs, and on the
/// platforms this function is compiled for, no production launch path omits it.
/// That scope matters: the macOS production path omits the parent ceiling
/// deliberately, which is the whole reason for the split. Three tests below
/// build launch paths that omit it on purpose, which is how they observe this
/// function at all.
///
/// TESTED BY, one test per branch, because a reviewer proved the `rlim_max` half
/// could be deleted with the whole suite still green:
/// - `the_production_probe_child_runs_under_an_address_space_ceiling`: no
///   ceiling at all, and the production probe succeeding is what shows the
///   parent installs one;
/// - `a_ceiling_that_is_not_the_worker_budget_is_refused`: a ceiling of a
///   different value;
/// - `a_soft_ceiling_the_child_could_raise_is_refused`: the `rlim_max` half
///   specifically, which needs a shell because
///   `BoundedCommand::address_space_limit` always sets both limits together.
///
/// Untested: the `getrlimit` failure branch, which needs the syscall to fail.
#[cfg(all(unix, not(target_os = "macos")))]
fn verify_parent_bound_address_space() -> Result<(), String> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut limit) } != 0 {
        return Err("decode worker could not read its address-space ceiling".into());
    }
    // `try_from` rather than `u64::from`: `rlim_t` is u64 on our shipped Unix
    // targets but is `i64` on the FreeBSD family and `uintptr_t` on Haiku, both
    // of which this cfg selects and neither of which has a `From` impl. The
    // parent uses `try_into` at the matching `setrlimit` for the same reason.
    // A value that cannot be represented becomes `u64::MAX` and therefore fails
    // the comparison below: unreadable means refused, never accepted.
    // The conversion is an identity on our shipped targets, which is what the
    // allow is for; it is not one on the targets named above.
    #[allow(clippy::useless_conversion)]
    let current = u64::try_from(limit.rlim_cur).unwrap_or(u64::MAX);
    #[allow(clippy::useless_conversion)]
    let maximum = u64::try_from(limit.rlim_max).unwrap_or(u64::MAX);
    if current != WORKER_ADDRESS_SPACE_BYTES || maximum != WORKER_ADDRESS_SPACE_BYTES {
        return Err(
            "decode worker refuses to parse input: its address-space ceiling values do not equal \
             the configured worker budget"
                .into(),
        );
    }
    Ok(())
}

/// Child exit code used when the input could not be decoded at all, as opposed
/// to a resource or plumbing failure.
const EXIT_UNDECODABLE: i32 = 65;

/// Environment the decode child is allowed to inherit.
///
/// The decoder needs no configuration, no `HOME`, and no `PATH`: it is handed
/// one already-opened input path and writes to stdout.
fn retain_safe_environment(command: &mut crate::bounded_child::BoundedCommand) {
    command.env_clear();
    for name in ["LANG", "LC_ALL", "LC_CTYPE"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

/// Executables known to dispatch [`maybe_run_audio_decode_worker`] before they
/// parse a command line.
///
/// Self-exec is only safe against a binary that actually honours the marker.
/// Re-executing an arbitrary host, notably a test harness, would run that
/// host's real work with the marker set and could recurse, so anything not on
/// this list fails closed instead.
const WORKER_CAPABLE_EXECUTABLES: [&str; 2] = ["minutes", "minutes-app"];

fn executable_handles_worker_protocol(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| WORKER_CAPABLE_EXECUTABLES.contains(&stem))
}

/// Resolve the executable that will run the decode.
///
/// Self-exec is tried FIRST, because re-running our own already-running image
/// is the strongest identity available without a signature check: it is by
/// definition the same code the user already trusted to run. Only when the
/// current host does not implement the worker protocol does this fall back to
/// an adjacent Minutes binary, and then only one that sits in the very same
/// directory as the current executable.
///
/// The previous order searched the parent directory too and exec'd any regular
/// file named `minutes` found there, with no identity check. For a
/// `~/.local/bin/minutes` install that second candidate was `~/.local/minutes`,
/// so anyone able to create a file in an adjacent directory could obtain
/// execution with the user's full authority at a moment of their choosing.
///
/// Self-exec also avoids introducing a separate helper executable.
fn resolve_worker_executable() -> Result<crate::bounded_child::BoundExecutable, String> {
    let current = std::env::current_exe()
        .map_err(|_| "compressed audio decode worker host was unavailable".to_string())?;
    if executable_handles_worker_protocol(&current) {
        if let Ok(executable) = crate::bounded_child::BoundExecutable::current() {
            return Ok(executable);
        }
    }
    let helper_name = format!("minutes{}", std::env::consts::EXE_SUFFIX);
    // Production searches only the current executable's own directory for the
    // adjacent fallback.
    #[allow(unused_mut)]
    let mut candidates = vec![current.parent().map(|parent| parent.join(&helper_name))];
    // The unit-test harness runs from target/debug/deps, one level below the
    // built CLI, so it needs the wider search to exercise the real child. This
    // widening exists only under cfg(test) and is never compiled into a
    // shipped binary.
    #[cfg(test)]
    candidates.push(
        current
            .parent()
            .and_then(|parent| parent.parent())
            .map(|grandparent| grandparent.join(&helper_name)),
    );
    let adjacent = candidates
        .into_iter()
        .flatten()
        .find(|candidate| candidate.is_file() && candidate != &current);
    // Binding copies the whole executable into an immutable snapshot, so it can
    // fail for reasons that have nothing to do with the binary being absent:
    // memory pressure, a full temp filesystem, a snapshot budget. Reporting
    // those as "no Minutes binary was found" is what made an intermittent bind
    // failure look like a missing build for a whole gate round.
    //
    // Scope, precisely: this preserves the cause for the ADJACENT-HELPER branch
    // only. A failure inside `BoundExecutable::current()` below is still
    // flattened into one generic string. There is no test for this branch either
    // - injecting a bind failure means planting a file in the harness's own
    // target directory, which is not hermetic and races other tests.
    let mut bind_failure = None;
    if let Some(helper) = adjacent {
        match crate::bounded_child::BoundExecutable::bind(&helper) {
            Ok(executable) => return Ok(executable),
            Err(error) => bind_failure = Some(error),
        }
    }
    if !executable_handles_worker_protocol(&current) {
        return Err(match bind_failure {
            Some(error) => format!(
                "compressed audio decode worker is unavailable because the Minutes binary \
                 beside this process could not be bound: {error}"
            ),
            None => "compressed audio decode worker is unavailable because no Minutes binary \
                     was found next to this process"
                .to_string(),
        });
    }
    crate::bounded_child::BoundExecutable::current()
        .map_err(|_| "compressed audio decode worker executable could not be resolved".to_string())
}

/// Whether a compressed decode may fall back to the bounded Symphonia worker.
///
/// Default-on by design. Shipping this opt-in would leave the very user it
/// exists for, someone who never installed ffmpeg, still regressed; the flag is
/// for an operator who wants to refuse the extra decoder, not something a
/// default user must discover.
pub fn bounded_decode_fallback_enabled(config: &crate::config::Config) -> bool {
    config.transcription.compressed_decode_fallback
}

/// Whether the bounded fallback is both permitted and actually usable here.
///
/// Preflight surfaces must ask this rather than the config flag alone: a build
/// with no resolvable worker executable would otherwise advertise a decoder it
/// cannot run, and refuse the user at decode time instead of at admission.
pub fn bounded_decode_fallback_available(config: &crate::config::Config) -> bool {
    bounded_decode_fallback_enabled(config) && resolve_worker_executable().is_ok()
}

/// Which job the child is being launched for.
///
/// Both jobs run Symphonia over attacker-controlled bytes and therefore need
/// identical containment; keeping them in one builder is what stops a second,
/// unasserted command drifting out of sync with the first.
#[derive(Clone, Copy)]
enum WorkerMode {
    Decode,
    ProbeDuration,
}

/// Build the decode child's command exactly as production launches it.
///
/// Extracted so tests can assert the real configuration instead of rebuilding
/// an equivalent-looking command of their own, which cannot catch a setting
/// being dropped from this chain.
fn build_decode_command(
    path: &Path,
    mode: WorkerMode,
) -> Result<crate::bounded_child::BoundedCommand, String> {
    let executable = resolve_worker_executable()?;
    let mut command = crate::bounded_child::BoundedCommand::from_bound_executable(executable)
        .map_err(|_| "compressed audio decode worker authority could not be bound".to_string())?;
    retain_safe_environment(&mut command);
    command.env(WORKER_MARKER, "1");
    if matches!(mode, WorkerMode::ProbeDuration) {
        command.arg(PROBE_DURATION_ARG);
    }
    command
        .arg("--")
        .arg(path)
        .single_process()
        .close_extra_descriptors();
    // Ordering is the security property: the ceiling must bind before Symphonia
    // reads an attacker-controlled byte. Every non-macOS Unix binds it from
    // `pre_exec`, before `exec` itself, not Linux alone. Windows installs a Job
    // Object memory ceiling from this same setting at CREATE_SUSPENDED, so it
    // binds before the child runs its first instruction. An earlier version
    // called that "the strongest ordering of any platform", an unmeasured
    // superlative of the kind this file has been corrected for twice: on Linux
    // the rlimit binds inside pre_exec, so the child image never executes
    // unbounded either.
    // Only Darwin rejects an absolute RLIMIT_AS below its pre-main()
    // shared-cache baseline, so only Darwin defers to the measured in-child
    // install at `maybe_run_audio_decode_worker`; setting it here as well would
    // fail, since a process cannot raise its own hard limit.
    #[cfg(not(target_os = "macos"))]
    command.address_space_limit(WORKER_ADDRESS_SPACE_BYTES);
    Ok(command)
}

/// Decode a compressed file to 16 kHz mono `s16le` PCM inside a bounded child.
///
/// Writes the raw PCM the child produced into the caller's private file. The
/// caller reads it back with the same reader used for ffmpeg output.
///
/// This paragraph used to sit above `WorkerMode`, so rustdoc attached it to the
/// enum and this function had no docs at all.
pub(crate) fn decode_to_private_pcm(
    path: &Path,
    destination: &mut crate::pipeline::PrivateAudioTempFile,
    max_output_bytes: u64,
    wall_clock: Duration,
) -> Result<(), String> {
    let mut command = build_decode_command(path, WorkerMode::Decode)?;

    let output = crate::pipeline::output_with_authorized_audio_stdin_to_private_file_with_budget(
        &mut command,
        None,
        destination,
        max_output_bytes,
        wall_clock,
    )
    .map_err(|error| {
        if crate::bounded_child::is_spawn_failure(&error) {
            format!("compressed audio decode worker could not be started: {error}")
        } else {
            format!("compressed audio decode worker failed: {error}")
        }
    })?;

    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.lines().last().unwrap_or("unknown error").to_string();
    if output.status.code() == Some(EXIT_UNDECODABLE) {
        Err(format!("the audio could not be decoded: {detail}"))
    } else {
        Err(format!(
            "compressed audio decode worker failed closed: {detail}"
        ))
    }
}

/// Child entry point. Returns `None` in an ordinary process.
///
/// Called before any argument parsing so a decode child never runs a user
/// command. The marker is removed immediately so it cannot be inherited any
/// further, matching the policy graph worker.
pub fn maybe_run_audio_decode_worker() -> Option<i32> {
    let marker = std::env::var_os(WORKER_MARKER)?;
    std::env::remove_var(WORKER_MARKER);
    if marker != "1" {
        // A stale or hostile marker must never silently swallow an ordinary
        // command such as `minutes record`, so say why the process is exiting.
        eprintln!(
            "{WORKER_MARKER} was set to an unrecognized value; refusing to run as a decode worker"
        );
        return Some(EXIT_UNDECODABLE);
    }
    // Ordering: the ceiling must be in force before anything parses input.
    // Genuinely three-way, and BOTH earlier versions of this comment collapsed
    // it to two. It is not "Linux, then everywhere else", and it is not "install
    // on macOS, verify everywhere else" either:
    //
    //   macOS          the child installs it, just below. Real child tests prove
    //                  success and a subprocess proves effective enforcement,
    //                  but the caller relationship is not mutation-sensitive.
    //   non-macOS Unix the parent installed it in pre_exec; the child verifies.
    //   Windows        the parent installed Job Object limits before the child
    //                  ran at all. NOTHING VERIFIES IT HERE: the check below is
    //                  cfg(all(unix, not(macos))), so Windows has no equivalent.
    #[cfg(target_os = "macos")]
    if let Err(error) = install_child_address_space_ceiling() {
        eprintln!("{error}");
        return Some(71);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Err(error) = verify_parent_bound_address_space() {
        eprintln!("{error}");
        return Some(71);
    }
    // NOT SWEPT ON WINDOWS, and that is a decision rather than an oversight.
    //
    // The exposure is real and was checked rather than assumed, in the pinned
    // toolchain's own source: `sys::process::windows` defaults
    // `inherit_handles: true` and passes it straight to `CreateProcessW`, so
    // this child can inherit ambient inheritable HANDLEs. `graph_worker` closes
    // them with `close_inherited_windows_handles_before_authority`, and calling
    // that here was tried.
    //
    // It was backed out because the first execution would be on Windows CI,
    // which no one working on this can watch. `ci.yml` runs the `minutes-core`
    // lib tests on `windows-latest` with no guard, and several ungated tests
    // there spawn this child, so the sweep would immediately become load-bearing
    // on a runner with no local reproduction. The two children are not
    // equivalent afterwards: the graph child reads stdin, computes, and writes
    // stdout, while this one opens files and runs Symphonia container probing,
    // which can pull delay-loaded imports. The sweep retains only the three
    // std handles. Whether that leaves this child able to finish is untested and
    // untestable from here, and the failure mode is losing compressed import on
    // Windows entirely, which is the regression this whole track exists to
    // close.
    //
    // The fix is the sweep plus a decode-worker canary plus a CI invocation that
    // actually runs it, landed by someone who can watch it go green. Until then
    // the honest statement is that Windows has no sweep here.
    let probe_only = std::env::args_os().any(|argument| argument == PROBE_DURATION_ARG);
    let path = std::env::args_os()
        .skip_while(|argument| argument != "--")
        .nth(1)
        .map(PathBuf::from);
    let Some(path) = path else {
        eprintln!("compressed audio decode worker requires exactly one input path");
        return Some(EXIT_UNDECODABLE);
    };
    Some(if probe_only {
        run_probe(&path)
    } else {
        run_worker(&path)
    })
}

/// Argument that switches the child into container-duration probe mode.
const PROBE_DURATION_ARG: &str = "--probe-duration";

/// Probe a compressed container's duration inside the bounded child.
///
/// `None` means "no duration is available", which the caller cannot distinguish
/// from any of the ways getting one can fail, and its response is to fall back to
/// `config.watch.type`: long calls filed as voice memos with no diarization,
/// which is the regression this probe exists to prevent.
///
/// So every `None` is logged with its cause. An earlier version documented only
/// the benign case, "the container does not declare a frame count", while five
/// branches returned `None` and one of them threw away the bind error that
/// `resolve_worker_executable` goes to some trouble to preserve. That branch
/// matters most: the caller binds the worker once to answer
/// `bounded_decode_fallback_available` and this function binds it again
/// immediately after, so a transient bind failure between the two lands here.
pub(crate) fn probe_compressed_duration(
    path: &Path,
    wall_clock: Duration,
) -> Option<std::time::Duration> {
    let label = crate::pipeline::private_audio_diagnostic_label(path);
    let mut command = match build_decode_command(path, WorkerMode::ProbeDuration) {
        Ok(command) => command,
        Err(error) => {
            // The bind error specifically. Track-1 item 10 exists because a
            // transient bind failure was being reported as a missing build.
            tracing::warn!(
                path = %label,
                %error,
                "compressed duration probe could not be built; content-type routing falls back to config"
            );
            return None;
        }
    };

    let run = match crate::bounded_child::run(
        &mut command,
        None,
        crate::bounded_child::StdoutTarget::Capture { max_bytes: 128 },
        crate::bounded_child::ChildBudget {
            wall_clock,
            stderr_tail: 4 * 1024,
        },
    ) {
        Ok(run) => run,
        Err(error) => {
            tracing::warn!(
                path = %label,
                %error,
                "compressed duration probe could not be launched; content-type routing falls back to config"
            );
            return None;
        }
    };
    if run.timed_out || !run.output.status.success() {
        tracing::warn!(
            path = %label,
            timed_out = run.timed_out,
            exit = ?run.output.status.code(),
            detail = %String::from_utf8_lossy(&run.output.stderr)
                .lines()
                .last()
                .unwrap_or("no detail")
                .chars()
                .take(200)
                .collect::<String>(),
            "compressed duration probe failed; content-type routing falls back to config"
        );
        return None;
    }
    let reported = String::from_utf8_lossy(&run.output.stdout)
        .trim()
        .to_string();
    let Ok(seconds) = reported.parse::<f64>() else {
        tracing::warn!(
            path = %label,
            "compressed duration probe returned no parseable duration; content-type routing falls back to config"
        );
        return None;
    };
    if !(seconds.is_finite() && seconds > 0.0) {
        tracing::warn!(
            path = %label,
            seconds,
            "compressed duration probe reported an unusable duration; content-type routing falls back to config"
        );
        return None;
    }
    Some(std::time::Duration::from_secs_f64(seconds))
}

/// Read a container's declared duration without decoding its packets.
fn probe_duration_seconds(path: &Path) -> Result<f64, String> {
    use symphonia::core::codecs::CODEC_TYPE_NULL;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|error| format!("input unavailable: {error}"))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| format!("probe failed: {error}"))?;
    let track = probed
        .format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "no audio track found".to_string())?;
    let rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| "container declared no sample rate".to_string())?;
    let frames = track
        .codec_params
        .n_frames
        .ok_or_else(|| "container declared no frame count".to_string())?;
    if rate == 0 {
        return Err("container declared a zero sample rate".into());
    }
    // `n_frames` is only a sample count when the container's time base IS the
    // sample rate. Matroska and WebM express it in the segment's own time base,
    // which is milliseconds, so dividing by the sample rate read a 150 s
    // recording as 3.1 s: about 48x short, and short enough that every browser
    // and Meet recording was filed as a voice memo and never diarized. Prefer
    // the declared time base, which is what makes the unit explicit, and keep
    // the sample-rate division only for containers that declare none.
    if let Some(time_base) = track.codec_params.time_base {
        let time = time_base.calc_time(frames);
        return Ok(time.seconds as f64 + time.frac);
    }
    Ok(frames as f64 / f64::from(rate))
}

fn run_probe(path: &Path) -> i32 {
    match probe_duration_seconds(path) {
        Ok(seconds) => {
            println!("{seconds}");
            0
        }
        Err(error) => {
            eprintln!("{error}");
            EXIT_UNDECODABLE
        }
    }
}

fn run_worker(path: &Path) -> i32 {
    match decode_compressed_to_s16le(path) {
        Ok(pcm) => {
            let mut stdout = std::io::stdout().lock();
            if stdout.write_all(&pcm).is_err() || stdout.flush().is_err() {
                return 74;
            }
            0
        }
        Err(error) => {
            eprintln!("{error}");
            EXIT_UNDECODABLE
        }
    }
}

/// Decode `path` with Symphonia into 16 kHz mono `s16le` bytes.
///
/// Runs only inside the bounded child. The address-space ceiling is already in
/// force here, so an attacker-declared table allocation fails this process
/// rather than the caller's.
fn decode_compressed_to_s16le(path: &Path) -> Result<Vec<u8>, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|error| format!("input unavailable: {error}"))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| format!("probe failed: {error}"))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "no audio track found".to_string())?;
    let track_id = track.id;
    let source_rate = track.codec_params.sample_rate.unwrap_or(44_100);
    let channels = track
        .codec_params
        .channels
        .map(|value| value.count())
        .unwrap_or(1)
        .max(1);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| format!("decoder unavailable: {error}"))?;

    // Resample and bound in one streaming pass so a hostile declared duration
    // cannot force an unbounded intermediate buffer even under the ceiling.
    //
    // The resampler is built from the first decoded frame's actual rate rather
    // than the container's declaration, so it is created lazily below.
    let budget = crate::audio_budget::AudioWorkBudget::new();
    budget
        .validate_stream(source_rate, channels)
        .map_err(|error| error.to_string())?;
    let mut resampler: Option<crate::audio_budget::StreamingMonoResampler> = None;

    let mut decoded_any = false;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            // A reset from the demuxer is the same event as one from the
            // decoder, one layer up: a chained stream changes parameters and
            // everything after it belongs to a different logical stream.
            // Ending the loop here would return the leading fragment as a
            // complete transcript.
            Err(symphonia::core::errors::Error::ResetRequired) => {
                return Err("stream reset mid-file; this container needs ffmpeg".into())
            }
            // Any other packet-level error ends the stream: a truncated or
            // hostile container yields whatever decoded cleanly so far.
            Err(_) => break,
        };
        // The deadline is normally polled inside push_mono_sample, but a
        // container whose packets decode to zero frames never reaches it and
        // would spin until the parent's wall clock. Charge the budget per
        // packet so a hostile container cannot burn CPU for the full deadline.
        budget
            .check_deadline()
            .map_err(|error| format!("decode exceeded its resource budget: {error}"))?;
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A reset invalidates every later packet. Returning what decoded
            // so far would be a truncated transcript presented as complete, so
            // fail instead: a wrong result is worse than none.
            Err(symphonia::core::errors::Error::ResetRequired) => {
                return Err("decoder reset mid-stream; this container needs ffmpeg".into())
            }
            Err(_) => continue,
        };
        let spec = *decoded.spec();
        // Trust the decoded rate over the container's declaration, and build
        // the resampler from it. An HE-AAC/SBR stream declares 44100 in the
        // AudioSampleEntry while the decoder reports the 22050 core rate;
        // resampling by the declared rate returns time- and pitch-scaled audio
        // as success, and refusing the file would deny it to exactly the
        // no-ffmpeg users this exists for.
        let resampler = match resampler.as_mut() {
            Some(resampler) => resampler,
            None => {
                budget
                    .validate_stream(spec.rate, spec.channels.count().max(1))
                    .map_err(|error| error.to_string())?;
                resampler.insert(
                    crate::audio_budget::StreamingMonoResampler::new(
                        spec.rate,
                        crate::audio_budget::CANONICAL_SAMPLE_RATE,
                        budget,
                        crate::audio_budget::MAX_CANONICAL_SAMPLES,
                    )
                    .map_err(|error| error.to_string())?,
                )
            }
        };
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        let frame_channels = spec.channels.count().max(1);
        for frame in buffer.samples().chunks(frame_channels) {
            if frame.len() < frame_channels {
                continue;
            }
            let mono = frame.iter().copied().sum::<f32>() / frame_channels as f32;
            if !mono.is_finite() {
                return Err("decoded audio contains a non-finite sample".into());
            }
            resampler
                .push_mono_sample(mono)
                .map_err(|error| format!("decode exceeded its resource budget: {error}"))?;
            decoded_any = true;
        }
    }
    if !decoded_any {
        return Err("no decodable audio was found".into());
    }
    let samples = resampler
        .ok_or_else(|| "no decodable audio was found".to_string())?
        .finish()
        .map_err(|error| format!("decode exceeded its resource budget: {error}"))?;
    if samples.is_empty() {
        return Err("no decodable audio was found".into());
    }

    let mut pcm = Vec::with_capacity(samples.len() * std::mem::size_of::<i16>());
    for sample in samples {
        let clamped = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        pcm.extend_from_slice(&clamped.to_le_bytes());
    }
    Ok(pcm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_marker_is_absent_in_an_ordinary_process() {
        // The ordinary path must never be mistaken for a decode child.
        std::env::remove_var(WORKER_MARKER);
        assert!(maybe_run_audio_decode_worker().is_none());
    }

    #[test]
    fn the_fallback_is_on_by_default_and_can_be_refused() {
        // Shipping this opt-in would leave the user it exists for, someone who
        // never installed ffmpeg, still regressed. The flag is for an operator
        // who wants to refuse the extra decoder.
        let mut config = crate::config::Config::default();
        assert!(
            bounded_decode_fallback_enabled(&config),
            "compressed-import fallback must default to on"
        );
        config.transcription.compressed_decode_fallback = false;
        assert!(!bounded_decode_fallback_enabled(&config));
    }

    #[test]
    fn an_existing_config_without_the_field_keeps_the_fallback() {
        // A user upgrading into this build has no such key in their
        // config.toml. They must not silently lose compressed imports.
        let existing: crate::config::TranscriptionConfig =
            toml::from_str("engine = \"whisper\"\n").unwrap();
        assert!(existing.compressed_decode_fallback);
    }

    /// The ceiling is the containment argument, so assert the PRODUCTION
    /// builder installs it rather than a command the test built itself.
    ///
    /// The previous shape called `.address_space_limit(...)` in the test and
    /// then asserted the getter, so deleting the production call left it green.
    /// This one reads the production command builder's configuration. It does
    /// NOT prove any caller still uses that builder, and the two earlier
    /// versions of this comment both implied otherwise: one claimed it drove
    /// `decode_to_private_pcm`'s own path, the next called it "the builder every
    /// launch goes through", which reads as coverage of the caller relationship
    /// in the same breath as denying it.
    /// On non-macOS Unix,
    /// `the_production_probe_child_runs_under_an_address_space_ceiling` covers
    /// the caller. Windows has builder-configuration coverage for both decode
    /// and probe modes, but no mutation-sensitive caller test and no runtime
    /// assertion that the Job Object limits were attached or effective.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_production_decode_command_carries_the_address_space_ceiling() {
        let nonexistent_input = std::env::temp_dir()
            .join("minutes-nonexistent-audio")
            .join("input.m4a");
        let command = build_decode_command(&nonexistent_input, WorkerMode::Decode)
            .expect("the decode command must be constructible in the test tree");
        assert_eq!(
            command.configured_address_space_limit(),
            Some(WORKER_ADDRESS_SPACE_BYTES),
            "the decode child must be launched under an address-space ceiling"
        );
    }

    /// macOS installs the ceiling in the child instead, so the parent command
    /// deliberately carries none. Assert that too, so the split stays honest.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_macos_decode_command_defers_its_ceiling_to_the_child() {
        let command = build_decode_command(Path::new("/nonexistent/input.m4a"), WorkerMode::Decode)
            .expect("the decode command must be constructible in the test tree");
        assert_eq!(command.configured_address_space_limit(), None);
    }

    /// Exercise Darwin's enforcement, not merely the `setrlimit` success path.
    ///
    /// This must run in a subprocess because an address-space hard limit cannot
    /// be raised again safely inside the shared test harness. The child installs
    /// the same measured baseline-plus-budget ceiling as a production decode
    /// child, then asks for one page more virtual address space than the whole
    /// growth budget. Removing the install makes that mapping succeed and this
    /// test fail.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_macos_child_ceiling_refuses_an_over_budget_mapping() {
        const CHILD_ENV: &str = "MINUTES_AUDIO_DECODE_CEILING_TEST_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            install_child_address_space_ceiling()
                .expect("the decode child ceiling must install on macOS");
            let requested = usize::try_from(WORKER_ADDRESS_SPACE_BYTES)
                .unwrap()
                .checked_add(16 * 1024)
                .unwrap();
            // SAFETY: this anonymous PROT_NONE mapping has no backing authority
            // and is never dereferenced. A surprising success is unmapped below.
            let mapping = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    requested,
                    libc::PROT_NONE,
                    libc::MAP_PRIVATE | libc::MAP_ANON,
                    -1,
                    0,
                )
            };
            if mapping != libc::MAP_FAILED {
                // SAFETY: `mapping` came from the successful mmap immediately
                // above and `requested` is the exact length passed to it.
                unsafe {
                    libc::munmap(mapping, requested);
                }
                panic!("Darwin permitted a mapping larger than the decode child's growth budget");
            }
            return;
        }

        let mut command = crate::engine_process::command(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg(
                "audio_decode_worker::tests::the_macos_child_ceiling_refuses_an_over_budget_mapping",
            )
            .arg("--nocapture")
            .env(CHILD_ENV, "1");
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "macOS ceiling child failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    /// The probe child runs Symphonia over the same attacker-controlled bytes
    /// as the decode child, so it needs the same ceiling. It previously had a
    /// second, inline command builder whose ceiling nothing asserted.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_probe_command_carries_the_same_ceiling_as_the_decode_command() {
        let nonexistent_input = std::env::temp_dir()
            .join("minutes-nonexistent-audio")
            .join("input.m4a");
        let decode = build_decode_command(&nonexistent_input, WorkerMode::Decode)
            .expect("decode command must be constructible in the test tree");
        let probe = build_decode_command(&nonexistent_input, WorkerMode::ProbeDuration)
            .expect("probe command must be constructible in the test tree");
        assert_eq!(
            probe.configured_address_space_limit(),
            Some(WORKER_ADDRESS_SPACE_BYTES)
        );
        assert_eq!(
            probe.configured_address_space_limit(),
            decode.configured_address_space_limit(),
            "both children parse hostile containers and must be bounded identically"
        );
    }

    /// A stream reset invalidates everything after it, so the decode must fail
    /// rather than return the leading fragment as a complete transcript.
    ///
    /// The committed fixture is two complete Vorbis streams at different sample
    /// rates concatenated, which is what a chained container looks like in the
    /// wild: 3 s at 44.1 kHz followed by 2 s at 48 kHz. Both the demuxer and the
    /// decoder can raise the reset; either must fail closed.
    ///
    /// Item 2 of the track-1 remediation list. The previous fixture was 0.25 s
    /// and delivered no packets before the reset, so the pre-fix code failed
    /// closed too and this test could tell the two apart only by the word
    /// "reset" in a message. It now asserts the VERDICT as well as the wording:
    /// with the reset arms restored to `break`, the decode hands back the leading
    /// three seconds of a five-second file as a success, which is exactly the
    /// truncated-transcript outcome that has to fail here. The substring check
    /// remains, deliberately, on the error branch only - it is secondary, and no
    /// longer the thing standing between a truncated decode and a green suite.
    ///
    /// Fixture provenance, so it can be rebuilt rather than treated as opaque:
    ///
    /// ```text
    /// ffmpeg -f lavfi -i "sine=frequency=440:duration=3:sample_rate=44100" -ac 1 -c:a libvorbis a.ogg
    /// ffmpeg -f lavfi -i "sine=frequency=880:duration=2:sample_rate=48000" -ac 1 -c:a libvorbis b.ogg
    /// cat a.ogg b.ogg > decode-fixture-chained.ogg
    /// ```
    #[test]
    fn a_mid_stream_reset_fails_instead_of_truncating() {
        const CHAINED: &[u8] = include_bytes!("../resources/decode-fixture-chained.ogg");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chained.ogg");
        std::fs::write(&path, CHAINED).unwrap();

        // Assert the fixture's own premise before asserting anything about the
        // decode. Everything that makes this test meaningful - that the first
        // logical stream is real and delivers packets before the reset - lived
        // only in the comment above and in an opaque binary, so a fixture
        // accidentally replaced by one that resets immediately would leave this
        // test green while its name became false. A first stream that probes as
        // about three seconds is the cheapest available check that it is there.
        //
        // What this does NOT prove: that packets are DECODED before the reset.
        // Symphonia derives an OGG duration from the first stream's final page
        // granule, which is metadata. The packet-delivery premise rests on the
        // recorded mutation result in the docstring, not on this assertion.
        let first_stream = probe_duration_seconds(&path)
            .expect("the chained fixture's first logical stream must be probeable");
        assert!(
            (2.5..=3.5).contains(&first_stream),
            "the fixture's first stream must be the ~3 s one this test reasons about, got \
             {first_stream}; if it was regenerated, re-read the provenance commands above"
        );

        match decode_compressed_to_s16le(&path) {
            Ok(pcm) => panic!(
                "a chained stream must not be reported as a complete decode: got {} samples, \
                 {:.2} s at 16 kHz, from a 5 s file",
                pcm.len() / 2,
                (pcm.len() / 2) as f64 / 16_000.0
            ),
            Err(error) => assert!(
                error.contains("reset"),
                "expected the reset to be named so the user knows why: {error}"
            ),
        }
    }

    #[test]
    fn undecodable_input_fails_closed_rather_than_returning_silence() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("not-audio.m4a");
        std::fs::write(&path, b"this is not a media container").unwrap();
        let error = decode_compressed_to_s16le(&path).unwrap_err();
        // Assert the failure class, not merely that some string came back:
        // an empty or wrong-class error would satisfy a non-empty check.
        assert!(
            error.contains("probe failed") || error.contains("no audio track found"),
            "unexpected failure for a non-container input: {error}"
        );
    }

    #[test]
    fn missing_input_is_reported_rather_than_panicking() {
        let directory = tempfile::tempdir().unwrap();
        let error = decode_compressed_to_s16le(&directory.path().join("absent.mp3")).unwrap_err();
        assert!(error.contains("input unavailable"));
    }

    #[test]
    fn a_non_minutes_host_refuses_to_self_exec() {
        // Re-executing a test harness would run the suite again with the
        // marker set. Resolution must fail closed instead of recursing.
        assert!(!executable_handles_worker_protocol(Path::new(
            "/tmp/minutes_core-0123456789abcdef"
        )));
        assert!(!executable_handles_worker_protocol(Path::new(
            "/usr/bin/env"
        )));
        assert!(executable_handles_worker_protocol(Path::new(
            "/usr/local/bin/minutes"
        )));
        assert!(executable_handles_worker_protocol(Path::new(
            "/Applications/Minutes.app/Contents/MacOS/minutes-app"
        )));
    }

    /// Build a WAV that Symphonia decodes through the same path as a
    /// compressed container, so the resample and PCM emission are covered
    /// without depending on an external encoder.
    fn write_test_wav(path: &Path, sample_rate: u32, frames: usize) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for index in 0..frames {
            let phase = index as f32 / sample_rate as f32 * 440.0 * std::f32::consts::TAU;
            writer
                .write_sample((phase.sin() * 16_000.0) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn decode_resamples_to_canonical_sixteen_khz_mono_pcm() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tone.wav");
        write_test_wav(&path, 44_100, 44_100);

        let pcm = decode_compressed_to_s16le(&path).unwrap();
        assert_eq!(pcm.len() % 2, 0, "s16le output must be whole samples");
        let samples = pcm.len() / 2;
        // One second at 44.1 kHz resamples to about one second at 16 kHz.
        assert!(
            (15_500..=16_500).contains(&samples),
            "expected ~16000 samples, got {samples}"
        );
        assert!(
            pcm.chunks_exact(2)
                .any(|pair| i16::from_le_bytes([pair[0], pair[1]]).abs() > 1_000),
            "decoded tone must carry real signal"
        );
    }

    #[test]
    fn decode_preserves_already_canonical_audio_length() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("canonical.wav");
        write_test_wav(&path, 16_000, 16_000);

        let pcm = decode_compressed_to_s16le(&path).unwrap();
        assert_eq!(pcm.len() / 2, 16_000);
    }

    /// Full parent-to-child path: spawn the bounded worker and read back PCM
    /// through the private file that the child never has a pathname for.
    /// Requires a built `minutes` binary next to the test harness.
    #[test]
    fn bounded_worker_child_round_trips_pcm_into_a_private_file() {
        // Deliberately not a silent skip. Reporting `ok` when the precondition
        // is absent is the defect class an earlier block in this epic was
        // rejected for: mutating the function under test still looked green
        // anywhere the worker could not resolve.
        resolve_worker_executable()
            .expect("a worker-capable executable must resolve for the end-to-end decode test");
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("tone.wav");
        write_test_wav(&source, 44_100, 44_100);

        let mut destination =
            crate::pipeline::PrivateAudioTempFile::new("minutes-decode-test-", ".s16le").unwrap();
        decode_to_private_pcm(
            &source,
            &mut destination,
            crate::audio_budget::AudioWorkBudget::max_pcm_s16le_bytes(),
            Duration::from_secs(120),
        )
        .unwrap();

        let mut reader = destination.try_clone_reader().unwrap();
        let mut pcm = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut pcm).unwrap();
        let samples = pcm.len() / 2;
        assert!(
            (15_500..=16_500).contains(&samples),
            "expected ~16000 samples through the child, got {samples}"
        );
    }

    /// A committed one-second mono AAC/m4a fixture: the container an iPhone
    /// voice memo actually uses.
    ///
    /// Committed rather than encoded on demand so this test cannot skip in the
    /// exact environment the feature exists for, a machine with no ffmpeg.
    const M4A_FIXTURE: &[u8] = include_bytes!("../resources/decode-fixture-tone.m4a");
    const WEBM_FIXTURE: &[u8] = include_bytes!("../resources/decode-fixture-tone.webm");

    /// Matroska and WebM express `n_frames` in the segment's own time base,
    /// which is milliseconds, not in samples. Dividing it by the sample rate
    /// read this 8 s fixture as 0.167 s, and a real 150 s Meet recording as
    /// 3.1 s. `webm` is a default watch extension, so with no ffmpeg installed
    /// every browser recording was short enough to be filed as a voice memo and
    /// never diarized, while the same file WITH ffmpeg routed correctly.
    ///
    /// The tolerance is deliberately tight. A 48x error has to fail here; a
    /// loose band would let the unit bug back in.
    #[test]
    fn webm_duration_is_read_in_seconds_not_container_ticks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("meet-recording.webm");
        std::fs::write(&path, WEBM_FIXTURE).unwrap();

        let seconds = probe_duration_seconds(&path).expect("webm fixture must probe");
        assert!(
            (7.5..=8.5).contains(&seconds),
            "8 s webm must probe as about 8 s, got {seconds}"
        );
    }

    /// Item 1 of the track-1 remediation list, the half that must be asserted
    /// through `probe_compressed_duration` itself.
    ///
    /// `the_probe_command_carries_the_same_ceiling_as_the_decode_command` calls
    /// `build_decode_command` and never asserts the production probe uses it, so
    /// restoring the inline builder that had no ceiling at all leaves it green.
    /// This launches the real probe child and reads its answer, and the child
    /// now refuses to parse input unless the parent bound its address space, so
    /// a probe launched without a ceiling returns `None` here.
    ///
    /// The first half is a precondition, not decoration. The child's behaviour
    /// comes from a SEPARATELY built `minutes` binary, so a stale one beside the
    /// harness would let the second half pass whatever the parent did. Proving
    /// the refusal first is what rules that out.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_production_probe_child_runs_under_an_address_space_ceiling() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memo.m4a");
        std::fs::write(&path, M4A_FIXTURE).unwrap();

        let executable = resolve_worker_executable()
            .expect("a worker-capable executable must resolve for the end-to-end probe test");
        let mut unbounded = crate::bounded_child::BoundedCommand::from_bound_executable(executable)
            .expect("the worker authority must bind");
        retain_safe_environment(&mut unbounded);
        unbounded.env(WORKER_MARKER, "1");
        unbounded.arg(PROBE_DURATION_ARG).arg("--").arg(&path);
        let refused = crate::bounded_child::run(
            &mut unbounded,
            None,
            crate::bounded_child::StdoutTarget::Capture { max_bytes: 128 },
            crate::bounded_child::ChildBudget {
                wall_clock: Duration::from_secs(30),
                stderr_tail: 4 * 1024,
            },
        )
        .expect("an unbounded probe child must at least launch");
        // Assert the SPECIFIC refusal, not merely a nonzero exit. Any argument,
        // loader or decoder failure would satisfy `!success()`, and then this
        // precondition would pass for a reason unrelated to containment and the
        // second half would prove nothing.
        let diagnostic = String::from_utf8_lossy(&refused.output.stderr).into_owned();
        assert_eq!(
            refused.output.status.code(),
            Some(71),
            "an unbounded probe child must exit with the containment refusal code; got \
             {:?} with stderr {diagnostic:?}. If this is not 71, the `minutes` binary beside \
             this harness predates the worker's self-check and this test cannot observe the \
             ceiling: rebuild it with `cargo build -p minutes-cli --no-default-features`",
            refused.output.status.code()
        );
        assert!(
            diagnostic.contains("address-space ceiling"),
            "the refusal must name the ceiling so it cannot be confused with another \
             fail-closed exit: {diagnostic:?}"
        );

        let seconds = probe_compressed_duration(&path, Duration::from_secs(30))
            .expect("the production probe must report the fixture duration");
        assert!(
            (0.75..=1.25).contains(&seconds.as_secs_f64()),
            "1 s m4a must probe as about 1 s, got {seconds:?}"
        );
    }

    /// A ceiling that is not the worker budget is refused even when it is
    /// TIGHTER than the budget.
    ///
    /// The name is deliberately about the VALUE rather than about whose ceiling
    /// it is. An earlier name said "someone else's ceiling is refused", which
    /// overclaimed: a foreign launcher setting exactly the worker budget is
    /// accepted, and this check cannot tell that apart from the parent's own.
    /// What it does buy is that the first version of the check, which accepted
    /// anything finite and no looser than the budget, is now observable: under
    /// that version this child exits 0.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_ceiling_that_is_not_the_worker_budget_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memo.m4a");
        std::fs::write(&path, M4A_FIXTURE).unwrap();

        let executable = resolve_worker_executable()
            .expect("a worker-capable executable must resolve for the ceiling-value test");
        let mut foreign = crate::bounded_child::BoundedCommand::from_bound_executable(executable)
            .expect("the worker authority must bind");
        retain_safe_environment(&mut foreign);
        foreign.env(WORKER_MARKER, "1");
        foreign.arg(PROBE_DURATION_ARG).arg("--").arg(&path);
        // Tighter than the worker budget and still not the worker's own.
        foreign.address_space_limit(2 * 1024 * 1024 * 1024);
        let run = crate::bounded_child::run(
            &mut foreign,
            None,
            crate::bounded_child::StdoutTarget::Capture { max_bytes: 128 },
            crate::bounded_child::ChildBudget {
                wall_clock: Duration::from_secs(30),
                stderr_tail: 4 * 1024,
            },
        )
        .expect("a child under a foreign ceiling must still launch");
        let diagnostic = String::from_utf8_lossy(&run.output.stderr).into_owned();
        assert_eq!(
            run.output.status.code(),
            Some(71),
            "a ceiling that is not the worker budget must be refused; got {:?} with stderr \
             {diagnostic:?}",
            run.output.status.code()
        );
        assert!(
            diagnostic.contains("address-space ceiling"),
            "{diagnostic:?}"
        );
    }

    /// A soft limit at the worker budget with a DIFFERENT hard limit is refused,
    /// because that child could raise its own ceiling back up.
    ///
    /// The name says "could raise" rather than "unbounded hard limit", which is
    /// what it said first, because the shell cannot guarantee unbounded:
    /// `ulimit -H -v unlimited` fails silently for an unprivileged process that
    /// inherited a finite hard limit. What the assertions DO establish is
    /// enough: the soft set succeeded (or the wrapper exits 70), so soft equals
    /// the budget; a hard limit equal to the budget would make the child exit 0
    /// and fail this test; and soft can never exceed hard. So reaching exit 71
    /// means hard is strictly greater than the budget, which is exactly the
    /// "child could raise it back" case.
    ///
    /// This half went untested for a whole gate round and a reviewer proved it:
    /// deleting the `rlim_max` comparison left the entire suite green, while the
    /// docstring called it load-bearing. Neither sibling ceiling test can see it,
    /// because `BoundedCommand::address_space_limit` sets `rlim_cur` and
    /// `rlim_max` together, so no command it can build varies them apart.
    ///
    /// Hence the shell: `ulimit -S -v` lowers the soft limit alone, leaving the
    /// hard limit as inherited. Nothing here can RAISE a hard limit, which is
    /// exactly why the case is reachable at all.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_soft_ceiling_the_child_could_raise_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memo.m4a");
        std::fs::write(&path, M4A_FIXTURE).unwrap();

        // Resolve the adjacent binary directly. The production resolver hands
        // back a sealed snapshot reachable only through /proc/self/fd, which a
        // separate `sh` process cannot exec.
        let current = std::env::current_exe().unwrap();
        let helper = current
            .parent()
            .and_then(|parent| parent.parent())
            .map(|grandparent| grandparent.join("minutes"))
            .filter(|candidate| candidate.is_file())
            .expect(
                "this test needs a worker-capable binary beside the harness; build one with \
                 `cargo build -p minutes-cli --no-default-features`",
            );

        let budget_kib = (WORKER_ADDRESS_SPACE_BYTES / 1024).to_string();
        let script = format!(
            "ulimit -H -v unlimited 2>/dev/null; ulimit -S -v {budget_kib} || exit 70; \
             exec \"$1\" {PROBE_DURATION_ARG} -- \"$2\""
        );
        let output = crate::engine_process::command("/bin/sh")
            .arg("-c")
            .arg(&script)
            .arg("minutes-soft-ceiling-probe")
            .arg(&helper)
            .arg(&path)
            .env_clear()
            .env(WORKER_MARKER, "1")
            .output()
            .expect("the shell wrapper must launch");

        let diagnostic = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_ne!(
            output.status.code(),
            Some(70),
            "the shell could not set a soft-only ceiling, so this test proves nothing: \
             {diagnostic:?}"
        );
        assert_eq!(
            output.status.code(),
            Some(71),
            "a soft ceiling the child could raise must be refused; got {:?} with stderr \
             {diagnostic:?}",
            output.status.code()
        );
        assert!(
            diagnostic.contains("address-space ceiling"),
            "{diagnostic:?}"
        );
    }

    /// The container that already worked must keep working: the fix reads the
    /// declared time base, so a format whose time base IS the sample rate has
    /// to come out unchanged.
    #[test]
    fn m4a_duration_is_unchanged_by_the_time_base_reading() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memo.m4a");
        std::fs::write(&path, M4A_FIXTURE).unwrap();

        let seconds = probe_duration_seconds(&path).expect("m4a fixture must probe");
        assert!(
            (0.75..=1.25).contains(&seconds),
            "1 s m4a must probe as about 1 s, got {seconds}"
        );
    }

    /// The actual regression: an m4a voice memo must decode with no ffmpeg
    /// involved at decode time.
    #[test]
    fn compressed_m4a_decodes_without_ffmpeg_at_decode_time() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memo.m4a");
        std::fs::write(&path, M4A_FIXTURE).unwrap();

        // Decoded entirely by Symphonia, which is what a user without ffmpeg
        // installed would get.
        let pcm = decode_compressed_to_s16le(&path).unwrap();
        let samples = pcm.len() / 2;
        assert!(
            (14_000..=18_000).contains(&samples),
            "expected roughly one second of 16 kHz audio, got {samples}"
        );
        assert!(
            pcm.chunks_exact(2)
                .any(|pair| i16::from_le_bytes([pair[0], pair[1]]).abs() > 1_000),
            "decoded memo must carry real signal"
        );
    }

    /// The end-to-end regression through the public decode entry point: with
    /// ffmpeg unavailable, a compressed import must still produce samples.
    #[test]
    fn compressed_import_survives_an_unavailable_ffmpeg() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memo.m4a");
        std::fs::write(&path, M4A_FIXTURE).unwrap();

        let guard = crate::test_home_env_lock();
        let previous = std::env::var_os("MINUTES_FFMPEG");
        std::env::set_var("MINUTES_FFMPEG", directory.path().join("absent-ffmpeg"));
        let decoded =
            crate::transcribe::decode_compressed_for_test(&path, &crate::config::Config::default());
        match previous {
            Some(value) => std::env::set_var("MINUTES_FFMPEG", value),
            None => std::env::remove_var("MINUTES_FFMPEG"),
        }
        drop(guard);

        let samples = decoded.expect("a compressed import must decode without ffmpeg");
        assert!(
            (14_000..=18_000).contains(&samples.len()),
            "expected roughly one second at 16 kHz, got {}",
            samples.len()
        );
    }
}
