//! [`SingleInstance`]: one running copy of an app, behind the
//! `single-instance` feature. A second launch hands its arguments (files
//! to open, say) to the running copy and exits; the running copy gets
//! them as a message and may raise its window with
//! [`EventCx::focus_window`](crate::EventCx::focus_window), from
//! [`AppContext::event_cx`].
//!
//! The copies meet at a local socket only the user can reach: a Unix
//! domain socket in `$XDG_RUNTIME_DIR` (or a private folder in the
//! temporary folder where that is unset, as on macOS) readable by the user
//! alone, and on Windows a named pipe, which the system keeps to the
//! user's session. The arguments go over it length-prefixed and the
//! running copy answers with one byte once it has them.

#[cfg(unix)]
use std::env;
#[cfg(unix)]
use std::fs::{self, DirBuilder, Permissions};
use std::io::{self, ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use interprocess::local_socket::prelude::*;
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Name};
use thiserror::Error;

use super::application::AppContext;

// ========================================================================
// Constants
// ========================================================================

/// The longest app id, so the socket's path stays within what Unix allows.
const MAX_APP_ID_LEN: usize = 64;

/// The most bytes of arguments a launch may hand over, so a stray client
/// cannot make the running copy hold unbounded memory.
const MAX_HANDOFF_BYTES: usize = 1 << 20;

/// The byte the running copy answers with once it has the arguments.
const ACK: u8 = 1;

/// How long either side waits for the other before giving up.
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(2);

/// The name of the thread taking later launches' arguments.
const THREAD_NAME: &str = "misty single instance";

/// What the socket's file name ends with, after the app id.
#[cfg(unix)]
const SOCKET_SUFFIX: &str = ".instance";

/// The access the socket and the private folder give: the user alone.
#[cfg(unix)]
const OWNER_ONLY: u32 = 0o700;

/// The permission bits that would let anyone but the owner in.
#[cfg(unix)]
const GROUP_OTHER_BITS: u32 = 0o077;

/// What the private folder in the temporary folder is named after the
/// user, when `$XDG_RUNTIME_DIR` is unset.
#[cfg(unix)]
const PRIVATE_DIR_PREFIX: &str = "misty-";

/// The variables naming the user, for the private folder and the pipe.
const USER_VARIABLES: [&str; 3] = ["USER", "LOGNAME", "USERNAME"];

/// The user's name when no variable gives it.
const UNKNOWN_USER: &str = "user";

// ========================================================================
// Data Structures
// ========================================================================

/// What [`SingleInstance::claim`] found.
#[derive(Debug)]
pub enum Claim {
    /// This is the first copy running: keep the value, run the app, and
    /// [`SingleInstance::listen`] from a callback.
    First(SingleInstance),
    /// Another copy runs and now has the arguments: exit.
    Handed,
}

/// The claim of the first running copy of an app on its socket, which
/// later launches hand their arguments to.
///
/// ```no_run
/// use misty::{AppContext, Claim, SingleInstance};
///
/// enum Message {
///     Launched(Vec<String>),
/// }
///
/// fn main() {
///     let args = std::env::args().skip(1);
///     let instance = match SingleInstance::claim("org.example.reader", args) {
///         Ok(Claim::First(instance)) => Some(instance),
///         Ok(Claim::Handed) => return,
///         // Run anyway, as a copy of its own.
///         Err(error) => {
///             eprintln!("{error}");
///             None
///         }
///     };
///     // Keep `instance` in the app, then in `Application::init`:
///     # let cx: &AppContext<'_, Message> = todo!();
///     if let Some(instance) = instance {
///         instance.listen(cx, Message::Launched);
///     }
/// }
/// ```
#[derive(Debug)]
pub struct SingleInstance {
    /// The socket later launches connect to.
    listener: Listener,
}

/// Why an app could not claim its single instance or hand over to it.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SingleInstanceError {
    /// The app id has characters a socket name cannot hold.
    #[error(
        "`{0}` is not a usable app id: use 1 to 64 ASCII letters, digits, `.`, `_`, or `-`, \
         such as `org.example.reader`"
    )]
    InvalidAppId(String),
    /// The folder for the socket lets others in or is not a folder.
    #[error(
        "{0} is not a private folder, so other users could reach the app through it: set \
         XDG_RUNTIME_DIR, or remove it to have it made again"
    )]
    InsecureFolder(String),
    /// The socket could not be made or reached.
    #[error("the single instance socket failed ({0}): check the runtime folder is writable")]
    Io(#[from] io::Error),
    /// The running copy did not take the arguments in time.
    #[error(
        "the running copy did not take the arguments: it may be stuck; close it and launch \
         again"
    )]
    NoAnswer,
}

/// The socket the first copy listens on.
#[cfg(unix)]
#[derive(Debug)]
struct Listener {
    /// The socket.
    socket: UnixListener,
}

/// The socket the first copy listens on.
#[cfg(windows)]
struct Listener {
    /// The named pipe.
    socket: LocalSocketListener,
}

// ========================================================================
// SingleInstance Implementations
// ========================================================================

impl SingleInstance {
    /// Claims the single instance of the app named `app_id` (a reverse
    /// domain name such as `org.example.reader` keeps it from meeting
    /// another app's), or hands `args` to the copy that holds it.
    /// Call it first thing in `main`, before [`run`](crate::run).
    pub fn claim(
        app_id: &str,
        args: impl IntoIterator<Item = String>,
    ) -> Result<Claim, SingleInstanceError> {
        check_app_id(app_id)?;
        let args: Vec<String> = args.into_iter().collect();
        #[cfg(unix)]
        {
            let path = socket_folder()?.join(format!("{app_id}{SOCKET_SUFFIX}"));
            claim_at(&path, &args)
        }
        #[cfg(windows)]
        {
            claim_pipe(&pipe_name(app_id)?, &args)
        }
    }

    /// Hands each later launch's arguments to
    /// [`Application::message`](crate::Application::message) as
    /// `message(args)`, from a thread of its own, for as long as the app
    /// runs. Launches made before this was called wait their turn.
    pub fn listen<M: Send + 'static>(
        self,
        cx: &AppContext<'_, M>,
        message: impl Fn(Vec<String>) -> M + Send + 'static,
    ) {
        let waker = cx.waker();
        self.serve(move |args| waker.send(message(args)).is_ok());
    }

    /// Takes launches' arguments on a thread, handing each to `deliver`
    /// until it says the app is gone.
    fn serve(self, deliver: impl Fn(Vec<String>) -> bool + Send + 'static) {
        // A system that cannot start a thread only loses later launches,
        // which then report that nothing answered.
        let _ = thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                loop {
                    // A listener that fails to accept is broken for good.
                    let Ok(mut stream) = self.listener.accept() else {
                        return;
                    };
                    let Ok(args) = receive(&mut stream) else {
                        continue;
                    };
                    if !deliver(args) {
                        return;
                    }
                }
            });
    }
}

// ========================================================================
// Listener Implementations
// ========================================================================

#[cfg(unix)]
impl Listener {
    /// The next launch to connect, with time limits on its reads and
    /// writes.
    fn accept(&self) -> io::Result<UnixStream> {
        let (stream, _) = self.socket.accept()?;
        stream.set_read_timeout(Some(HANDOFF_TIMEOUT))?;
        stream.set_write_timeout(Some(HANDOFF_TIMEOUT))?;
        Ok(stream)
    }
}

#[cfg(windows)]
impl Listener {
    /// The next launch to connect, with a time limit on its reads.
    fn accept(&self) -> io::Result<LocalSocketStream> {
        let stream = self.socket.accept()?;
        stream.set_recv_timeout(Some(HANDOFF_TIMEOUT))?;
        Ok(stream)
    }
}

#[cfg(windows)]
impl std::fmt::Debug for Listener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listener").finish_non_exhaustive()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Refuses an app id a socket name cannot hold.
fn check_app_id(app_id: &str) -> Result<(), SingleInstanceError> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    let usable = !app_id.is_empty()
        && app_id.len() <= MAX_APP_ID_LEN
        && app_id.chars().all(allowed)
        && !app_id.starts_with('.');
    match usable {
        true => Ok(()),
        false => Err(SingleInstanceError::InvalidAppId(app_id.to_owned())),
    }
}

/// The user's name, from the environment.
fn user_name() -> String {
    USER_VARIABLES
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|user| !user.is_empty()))
        .unwrap_or_else(|| UNKNOWN_USER.to_owned())
}

/// Claims the socket at `path`, or hands `args` to the copy listening
/// there. A socket left by a copy that crashed is taken over.
#[cfg(unix)]
fn claim_at(path: &Path, args: &[String]) -> Result<Claim, SingleInstanceError> {
    match bind(path) {
        Ok(listener) => return Ok(Claim::First(SingleInstance { listener })),
        Err(error) if error.kind() == ErrorKind::AddrInUse => {}
        Err(error) => return Err(error.into()),
    }
    match UnixStream::connect(path) {
        Ok(mut stream) => {
            stream.set_read_timeout(Some(HANDOFF_TIMEOUT))?;
            stream.set_write_timeout(Some(HANDOFF_TIMEOUT))?;
            hand_over(&mut stream, args)?;
            Ok(Claim::Handed)
        }
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
            fs::remove_file(path)?;
            let listener = bind(path)?;
            Ok(Claim::First(SingleInstance { listener }))
        }
        Err(error) => Err(error.into()),
    }
}

/// A socket bound at `path` that only the user may connect to.
#[cfg(unix)]
fn bind(path: &Path) -> io::Result<Listener> {
    let socket = UnixListener::bind(path)?;
    fs::set_permissions(path, Permissions::from_mode(OWNER_ONLY))?;
    Ok(Listener { socket })
}

/// The folder the socket goes in: `$XDG_RUNTIME_DIR`, private to the user
/// by its specification, or else a folder of the user's own in the
/// temporary folder, made private.
#[cfg(unix)]
fn socket_folder() -> Result<PathBuf, SingleInstanceError> {
    if let Some(runtime) = env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(runtime));
    }
    let folder = env::temp_dir().join(format!("{PRIVATE_DIR_PREFIX}{}", user_name()));
    private_folder(&folder)?;
    Ok(folder)
}

/// Makes `folder` readable by the user alone, or checks that the one
/// there is: a folder, not a link, that lets no one else in.
#[cfg(unix)]
fn private_folder(folder: &Path) -> Result<(), SingleInstanceError> {
    match DirBuilder::new().mode(OWNER_ONLY).create(folder) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(folder)?;
    let private = metadata.is_dir() && metadata.permissions().mode() & GROUP_OTHER_BITS == 0;
    match private {
        true => Ok(()),
        false => Err(SingleInstanceError::InsecureFolder(
            folder.display().to_string(),
        )),
    }
}

/// The named pipe of the app `app_id` for this user.
#[cfg(windows)]
fn pipe_name(app_id: &str) -> io::Result<Name<'static>> {
    format!("{app_id}-{}", user_name()).to_ns_name::<GenericNamespaced>()
}

/// Claims the named pipe `name`, or hands `args` to the copy listening
/// on it.
#[cfg(windows)]
fn claim_pipe(name: &Name<'static>, args: &[String]) -> Result<Claim, SingleInstanceError> {
    match ListenerOptions::new().name(name.borrow()).create_sync() {
        Ok(socket) => {
            return Ok(Claim::First(SingleInstance {
                listener: Listener { socket },
            }));
        }
        Err(error) if error.kind() == ErrorKind::AddrInUse => {}
        Err(error) => return Err(error.into()),
    }
    let mut stream = LocalSocketStream::connect(name.borrow())?;
    stream.set_recv_timeout(Some(HANDOFF_TIMEOUT))?;
    hand_over(&mut stream, args)?;
    Ok(Claim::Handed)
}

/// Writes `args` to the running copy over `stream`, then waits for its
/// answer.
fn hand_over(stream: &mut (impl Read + Write), args: &[String]) -> Result<(), SingleInstanceError> {
    stream.write_all(&encode(args))?;
    stream.flush()?;
    let mut answer = [0; 1];
    match stream.read_exact(&mut answer) {
        Ok(()) if answer[0] == ACK => Ok(()),
        Ok(()) => Err(SingleInstanceError::NoAnswer),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            Err(SingleInstanceError::NoAnswer)
        }
        Err(error) => Err(error.into()),
    }
}

/// Reads a launch's arguments from `stream` and answers that they came.
fn receive(stream: &mut (impl Read + Write)) -> io::Result<Vec<String>> {
    let count = read_len(stream)?;
    let mut total = 0;
    let mut args = Vec::new();
    for _ in 0..count {
        let len = read_len(stream)?;
        total += len;
        if total > MAX_HANDOFF_BYTES {
            return Err(ErrorKind::InvalidData.into());
        }
        let mut bytes = vec![0; len];
        stream.read_exact(&mut bytes)?;
        args.push(String::from_utf8(bytes).map_err(|_| io::Error::from(ErrorKind::InvalidData))?);
    }
    stream.write_all(&[ACK])?;
    Ok(args)
}

/// `args` as they go over the socket: their count, then each one's
/// length and bytes, lengths as little-endian `u32`s.
fn encode(args: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&len_bytes(args.len()));
    for arg in args {
        bytes.extend_from_slice(&len_bytes(arg.len()));
        bytes.extend_from_slice(arg.as_bytes());
    }
    bytes
}

/// `len` as four little-endian bytes; no argument list comes near
/// `u32::MAX`, and one that did would be refused by the reader's limit.
fn len_bytes(len: usize) -> [u8; 4] {
    u32::try_from(len).unwrap_or(u32::MAX).to_le_bytes()
}

/// A length written by [`len_bytes`], read from `stream`.
fn read_len(stream: &mut impl Read) -> io::Result<usize> {
    let mut bytes = [0; 4];
    stream.read_exact(&mut bytes)?;
    usize::try_from(u32::from_le_bytes(bytes)).map_err(|_| ErrorKind::InvalidData.into())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, unix))]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// A fresh private folder for one test's socket.
    fn folder(name: &str) -> PathBuf {
        let folder = env::temp_dir().join(format!("misty-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        private_folder(&folder).expect("a private folder");
        folder
    }

    #[test]
    fn test_a_second_launch_hands_its_arguments_to_the_first() {
        let path = folder("handoff").join("app.instance");
        let Claim::First(instance) = claim_at(&path, &[]).expect("claims") else {
            panic!("the first launch claims the socket");
        };
        let mode = fs::metadata(&path).expect("a socket").permissions().mode();
        assert_eq!(mode & GROUP_OTHER_BITS, 0, "the user's alone");
        let (sent, got) = mpsc::channel();
        instance.serve(move |args| sent.send(args).is_ok());
        let args = vec!["open".to_owned(), "a b.pdf".to_owned(), "".to_owned()];
        assert!(matches!(claim_at(&path, &args), Ok(Claim::Handed)));
        assert_eq!(got.recv_timeout(HANDOFF_TIMEOUT).expect("handed"), args);
        assert!(matches!(
            claim_at(&path, &["é".to_owned()]),
            Ok(Claim::Handed)
        ));
        assert_eq!(got.recv_timeout(HANDOFF_TIMEOUT).expect("handed"), ["é"]);
        let _ = fs::remove_dir_all(path.parent().expect("a folder"));
    }

    #[test]
    fn test_a_socket_left_by_a_crashed_copy_is_taken_over() {
        let path = folder("stale").join("app.instance");
        drop(UnixListener::bind(&path).expect("binds"));
        assert!(path.exists(), "the socket file outlives its listener");
        assert!(matches!(claim_at(&path, &[]), Ok(Claim::First(_))));
        let _ = fs::remove_dir_all(path.parent().expect("a folder"));
    }

    #[test]
    fn test_an_oversized_handoff_is_refused_without_holding_it() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&len_bytes(1));
        bytes.extend_from_slice(&len_bytes(MAX_HANDOFF_BYTES + 1));
        let mut stream = io::Cursor::new(bytes);
        let error = receive(&mut stream).expect_err("too big");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn test_app_ids_and_folders_others_could_reach_are_refused() {
        for id in ["", "a/b", "..", ".hidden", &"x".repeat(MAX_APP_ID_LEN + 1)] {
            assert!(
                matches!(check_app_id(id), Err(SingleInstanceError::InvalidAppId(_))),
                "{id:?}"
            );
        }
        assert!(check_app_id("org.example.reader-2_b").is_ok());
        let open = folder("open");
        fs::set_permissions(&open, Permissions::from_mode(0o755)).expect("chmod");
        assert!(matches!(
            private_folder(&open),
            Err(SingleInstanceError::InsecureFolder(_))
        ));
        let _ = fs::remove_dir_all(open);
    }
}
