//! Diagnostics for a DLL that lives inside other people's processes.
//!
//! The DLL is loaded into whatever application the user types into, so nothing
//! here may take the host down and nothing can rely on a debugger being
//! attached. Everything it has to say lands in one log file, panics are caught
//! at every COM callback, and a "dump" can be requested from outside.
//!
//! Port of the logging/guard half of `tip/main.go`. The Go version dumped every
//! goroutine; Rust cannot walk other threads' stacks, so the dump instead lists
//! the COM callbacks that are *in flight* (a callback stuck on a lock shows up
//! with a large elapsed time) plus a short history of recent callbacks.
//!
//! Privacy: composition and committed text are redacted from the log unless
//! `PortRustWindowsIMELogText=1` is set. See [`text_for_log`].

use std::backtrace::Backtrace;
use std::collections::VecDeque;
use std::fmt::Arguments;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

pub const LOG_FILE_NAME: &str = "PortRustWindowsIME.log";
/// Overrides the log path, which is how a test or a second TIP instance is
/// kept out of the shared file.
pub const LOG_PATH_ENV: &str = "PortRustWindowsIMELogPath";
/// Set to `1` to log composition and committed text verbatim.
pub const LOG_TEXT_ENV: &str = "PortRustWindowsIMELogText";
/// Freezes the calling thread at a named point so a dump can be taken.
pub const PAUSE_AT_ENV: &str = "PortRustWindowsIMEPauseAt";
/// Creating this file in the temp directory requests a dump.
pub const DUMP_FILE_NAME: &str = "PortRustWindowsIME.dump";

/// How many finished callbacks the dump remembers.
const RECENT_CAPACITY: usize = 48;

/// `std::fmt` front end for [`log`]: `tip_log!("Activate clientId={}", id)`.
#[macro_export]
macro_rules! tip_log {
    ($($arg:tt)*) => { $crate::diag::log(::std::format_args!($($arg)*)) };
}

/// Metadata-only log for the candidate window (never candidate text).
#[macro_export]
macro_rules! candidate_log {
    ($($arg:tt)*) => { $crate::diag::candidate_debug_log(::std::format_args!($($arg)*)) };
}

// ---------------------------------------------------------------------------
// Time and thread identity
// ---------------------------------------------------------------------------

/// OS thread id on Windows (what a debugger and `tid=` in the log refer to).
pub fn thread_id() -> u64 {
    #[cfg(windows)]
    {
        unsafe { windows::Win32::System::Threading::GetCurrentThreadId() as u64 }
    }
    #[cfg(not(windows))]
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        thread_local! { static ID: u64 = NEXT.fetch_add(1, Ordering::Relaxed); }
        ID.with(|id| *id)
    }
}

/// Local wall-clock time on Windows, UTC elsewhere (only tests run elsewhere).
fn timestamp() -> String {
    #[cfg(windows)]
    {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
        )
    }
    #[cfg(not(windows))]
    {
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let (days, rem) = ((since.as_secs() / 86_400) as i64, since.as_secs() % 86_400);
        let (y, m, d) = civil_from_days(days);
        format!(
            "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}.{:03}",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60,
            since.subsec_millis()
        )
    }
}

/// Days since 1970-01-01 -> (year, month, day). Howard Hinnant's algorithm.
#[cfg(not(windows))]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

// ---------------------------------------------------------------------------
// Log files
// ---------------------------------------------------------------------------

pub fn log_path() -> PathBuf {
    match std::env::var_os(LOG_PATH_ENV) {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => std::env::temp_dir().join(LOG_FILE_NAME),
    }
}

fn append_line(path: &Path, line: &str) {
    // std opens with FILE_SHARE_READ|WRITE|DELETE, so the file can be tailed
    // while hosts keep appending to it. Failures are ignored on purpose: a log
    // that cannot be written must never break typing.
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Appends one line to the shared TIP log.
pub fn log(args: Arguments<'_>) {
    let line = format!("[{}][pid={} tid={}] {}\r\n", timestamp(), std::process::id(), thread_id(), args);
    append_line(&log_path(), &line);
}

/// Where the candidate-window log goes: `%LOCALAPPDATA%\PortRustWindowsIME`,
/// falling back to the temp directory.
pub fn candidate_debug_log_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("PortRustWindowsIME");
    if std::fs::create_dir_all(&dir).is_ok() {
        dir.join("candidate-debug.log")
    } else {
        std::env::temp_dir().join("candidate-debug.log")
    }
}

/// Records only candidate metadata, never candidate strings or composition text.
pub fn candidate_debug_log(args: Arguments<'_>) {
    let line = format!("[{}][pid={}][component=tip] {}\r\n", timestamp(), std::process::id(), args);
    append_line(&candidate_debug_log_path(), &line);
}

/// What to write in the log for user text (composition, committed text,
/// candidates).
///
/// The Go version logged all of it verbatim, which turns the log into a record
/// of everything typed, including into password fields (this TIP declares
/// secure-mode support). Here it is redacted to a length unless
/// `PortRustWindowsIMELogText=1`.
pub fn text_for_log(text: &str) -> String {
    if std::env::var_os(LOG_TEXT_ENV).is_some_and(|v| v == "1") {
        format!("{text:?}")
    } else {
        format!("<{} chars>", text.chars().count())
    }
}

// ---------------------------------------------------------------------------
// Callback tracking and the panic guard
// ---------------------------------------------------------------------------

struct InFlight {
    id: u64,
    name: &'static str,
    tid: u64,
    since: Instant,
}

struct Finished {
    at: Instant,
    name: &'static str,
    tid: u64,
    ran_for: Duration,
    panicked: bool,
}

struct Tracker {
    next_id: u64,
    in_flight: Vec<InFlight>,
    recent: VecDeque<Finished>,
}

static TRACKER: Mutex<Tracker> = Mutex::new(Tracker { next_id: 0, in_flight: Vec::new(), recent: VecDeque::new() });

fn tracker() -> MutexGuard<'static, Tracker> {
    TRACKER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Marks a callback as running until dropped.
struct CallbackScope {
    id: u64,
    panicked: bool,
}

impl CallbackScope {
    fn enter(name: &'static str) -> CallbackScope {
        let mut t = tracker();
        t.next_id += 1;
        let id = t.next_id;
        t.in_flight.push(InFlight { id, name, tid: thread_id(), since: Instant::now() });
        CallbackScope { id, panicked: false }
    }
}

impl Drop for CallbackScope {
    fn drop(&mut self) {
        let mut t = tracker();
        if let Some(pos) = t.in_flight.iter().position(|c| c.id == self.id) {
            let entry = t.in_flight.remove(pos);
            if t.recent.len() == RECENT_CAPACITY {
                t.recent.pop_front();
            }
            t.recent.push_back(Finished {
                at: Instant::now(),
                name: entry.name,
                tid: entry.tid,
                ran_for: entry.since.elapsed(),
                panicked: self.panicked,
            });
        }
    }
}

/// Number of callbacks currently running (used by tests and the dump).
pub fn callbacks_in_flight() -> usize {
    tracker().in_flight.len()
}

fn payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

/// Logs the panic site (with a backtrace) at the moment of the panic, since
/// the stack is gone by the time `catch_unwind` returns. The previous hook still
/// runs, so test output keeps its panic messages.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown>".into());
            log(format_args!(
                "panic at {location}: {}\r\n{}",
                payload_message(info.payload()),
                Backtrace::force_capture()
            ));
            previous(info);
        }));
    });
}

/// Runs one COM callback and keeps a fault inside it.
///
/// A panic that unwinds through a native frame takes the host process down (and
/// since Rust 1.81 a panic out of an `extern "system"` function aborts), so every
/// callback body runs under this. On a panic it is logged, a dump is written and
/// `Err(message)` is returned for the caller to turn into an HRESULT.
pub fn guarded<R>(name: &'static str, f: impl FnOnce() -> R) -> Result<R, String> {
    install_panic_hook();
    let mut scope = CallbackScope::enter(name);
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Ok(value),
        Err(payload) => {
            scope.panicked = true;
            let message = payload_message(&*payload);
            log(format_args!("PANIC {name}: {message}"));
            // Written while the callback is still marked in flight, so the dump
            // names the callback that failed.
            write_dump(&format!("panic in {name}"));
            Err(message)
        }
    }
}

/// Appends a dump: callbacks in flight, recent callbacks, and the calling
/// thread's stack. This is what makes a hung host inspectable without a debugger.
pub fn write_dump(reason: &str) {
    use std::fmt::Write as _;
    let now = Instant::now();
    let mut text = format!("dump ({reason}): pid={} tid={}\r\n", std::process::id(), thread_id());
    {
        let t = tracker();
        let _ = write!(text, "  callbacks in flight: {}\r\n", t.in_flight.len());
        for c in &t.in_flight {
            let _ =
                write!(text, "    {} on tid={} for {} ms\r\n", c.name, c.tid, now.duration_since(c.since).as_millis());
        }
        let _ = write!(text, "  recent callbacks, oldest first: {}\r\n", t.recent.len());
        for c in &t.recent {
            let _ = write!(
                text,
                "    {} on tid={} took {} ms, finished {} ms ago{}\r\n",
                c.name,
                c.tid,
                c.ran_for.as_millis(),
                now.duration_since(c.at).as_millis(),
                if c.panicked { " (panicked)" } else { "" }
            );
        }
    }
    let _ = write!(text, "  stack of the dumping thread:\r\n{}", Backtrace::force_capture());
    log(format_args!("{text}"));
}

// ---------------------------------------------------------------------------
// Out-of-band dump requests and pause points
// ---------------------------------------------------------------------------

pub fn dump_watch_path() -> PathBuf {
    std::env::temp_dir().join(DUMP_FILE_NAME)
}

/// Writes a dump and removes the request file if `path` exists. Removing it
/// re-arms the next request.
pub fn poll_dump_request(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    write_dump(&format!("dump requested by {}", path.display()));
    let _ = std::fs::remove_file(path);
    true
}

/// Starts (once) a thread that polls for the dump request file, so a stuck TIP
/// inside an application that cannot be restarted can still be inspected.
pub fn start_dump_watcher() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let spawned = std::thread::Builder::new().name("port-rust-ime-dump-watch".into()).spawn(|| {
            let path = dump_watch_path();
            loop {
                poll_dump_request(&path);
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        if let Err(error) = spawned {
            log(format_args!("could not start the dump watcher: {error}"));
        }
    });
}

/// Freezes the calling thread at a named point so a dump can be taken while the
/// object is still alive. Off unless the variable names exactly this point: a
/// blocked TSF callback hangs the host application, which is only ever wanted
/// deliberately.
pub fn pause_at(point: &str) {
    let wanted = std::env::var(PAUSE_AT_ENV).unwrap_or_default();
    if wanted.is_empty() || wanted != point {
        return;
    }
    log(format_args!("paused at {point} pid={} tid={}", std::process::id(), thread_id()));
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;

    /// Serializes tests that change process-wide environment variables.
    pub fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn unique_temp_dir(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "port-rust-ime-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Redirects the TIP log to a fresh file for the lifetime of the value.
    pub struct LogRedirect {
        _guard: MutexGuard<'static, ()>,
        pub path: PathBuf,
    }

    impl LogRedirect {
        pub fn new(tag: &str) -> LogRedirect {
            let guard = env_lock();
            let path = unique_temp_dir(tag).join("tip.log");
            std::env::set_var(LOG_PATH_ENV, &path);
            LogRedirect { _guard: guard, path }
        }
        pub fn contents(&self) -> String {
            std::fs::read_to_string(&self.path).unwrap_or_default()
        }
    }

    impl Drop for LogRedirect {
        fn drop(&mut self) {
            std::env::remove_var(LOG_PATH_ENV);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    // port of TestGuardedCallConvertsPanicToEFail
    #[test]
    fn guarded_turns_a_panic_into_an_error_and_logs_it() {
        let log = LogRedirect::new("panic");
        let result = guarded("unit.panic", || -> u32 { panic!("simulated failure inside a callback") });
        assert_eq!(result, Err("simulated failure inside a callback".to_owned()));
        let logged = log.contents();
        assert!(logged.contains("PANIC unit.panic"), "the log must name the callback that panicked:\n{logged}");
        // The dump is what makes a deadlock in a DLL debuggable at all.
        assert!(logged.contains("dump (panic in unit.panic)"), "missing dump:\n{logged}");
        assert!(logged.contains("unit.panic on tid="), "the failing callback must be listed as in flight:\n{logged}");
        assert!(logged.contains("simulated failure"), "the panic site must be logged:\n{logged}");
    }

    // port of TestGuardedCallPassesThroughSuccess
    #[test]
    fn guarded_passes_a_result_through() {
        assert_eq!(guarded("unit.ok", || 2u32), Ok(2));
    }

    #[test]
    fn guarded_handles_string_and_opaque_payloads() {
        let _log = LogRedirect::new("payload");
        assert_eq!(guarded("unit.string", || -> () { panic!("{}", String::from("owned")) }), Err("owned".into()));
        let opaque = guarded("unit.opaque", || -> () { std::panic::panic_any(42u8) });
        assert_eq!(opaque, Err("<non-string panic payload>".into()));
    }

    #[test]
    fn a_finished_callback_is_no_longer_in_flight() {
        let _log = LogRedirect::new("inflight");
        let before = callbacks_in_flight();
        let seen_inside = guarded("unit.inside", callbacks_in_flight).unwrap();
        assert!(seen_inside > before, "the running callback must be counted");
        let _ = guarded("unit.panics", || -> () { panic!("x") });
        // other tests may run callbacks concurrently, so only compare ours
        let mut recent = String::new();
        write_dump("check");
        recent.push_str(&std::fs::read_to_string(log_path()).unwrap_or_default());
        assert!(recent.contains("unit.inside on tid="), "history must remember finished callbacks");
        assert!(recent.contains("(panicked)"), "history must flag a callback that panicked");
    }

    #[test]
    fn log_lines_carry_pid_tid_and_a_timestamp() {
        let log = LogRedirect::new("format");
        crate::tip_log!("hello {}", 7);
        let logged = log.contents();
        let line = logged.lines().next().unwrap();
        assert!(line.starts_with('['), "{line}");
        assert!(line.contains(&format!("pid={}", std::process::id())), "{line}");
        assert!(line.contains(" tid="), "{line}");
        assert!(line.ends_with("] hello 7"), "{line}");
        assert!(logged.ends_with("\r\n"), "lines end in CRLF like the Go log");
    }

    #[test]
    fn user_text_is_redacted_unless_opted_in() {
        let _guard = env_lock();
        std::env::remove_var(LOG_TEXT_ENV);
        assert_eq!(text_for_log("你好"), "<2 chars>");
        assert_eq!(text_for_log(""), "<0 chars>");
        std::env::set_var(LOG_TEXT_ENV, "1");
        assert_eq!(text_for_log("你好"), "\"你好\"");
        std::env::set_var(LOG_TEXT_ENV, "0");
        assert_eq!(text_for_log("你好"), "<2 chars>");
        std::env::remove_var(LOG_TEXT_ENV);
    }

    #[test]
    fn candidate_log_goes_under_localappdata() {
        let _guard = env_lock();
        let base = unique_temp_dir("localappdata");
        std::env::set_var("LOCALAPPDATA", &base);
        crate::candidate_log!("candidate.update.begin candidate_count={}", 3);
        let path = base.join("PortRustWindowsIME").join("candidate-debug.log");
        let text = std::fs::read_to_string(&path).expect("candidate log must exist");
        assert!(text.contains("[component=tip] candidate.update.begin candidate_count=3"), "{text}");
        std::env::remove_var("LOCALAPPDATA");
    }

    #[test]
    fn a_dump_request_file_is_served_and_removed() {
        let log = LogRedirect::new("dumpreq");
        let request = unique_temp_dir("dumpreq-file").join("req.dump");
        assert!(!poll_dump_request(&request), "no request, no dump");
        std::fs::write(&request, b"").unwrap();
        assert!(poll_dump_request(&request));
        assert!(!request.exists(), "the request file must be removed to re-arm");
        assert!(log.contents().contains("dump (dump requested by"), "{}", log.contents());
    }

    #[test]
    fn pause_only_triggers_on_the_exact_point_name() {
        let _guard = env_lock();
        std::env::remove_var(PAUSE_AT_ENV);
        pause_at("Activate"); // unset: returns immediately
        std::env::set_var(PAUSE_AT_ENV, "Deactivate");
        pause_at("Activate"); // different point: returns immediately
        std::env::remove_var(PAUSE_AT_ENV);
    }

    #[cfg(not(windows))]
    #[test]
    fn civil_date_conversion_is_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
