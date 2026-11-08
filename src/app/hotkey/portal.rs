//! Global hotkeys bound through the desktop portal's GlobalShortcuts
//! interface over D-Bus, for Wayland, where no app may grab keys. The
//! portal answers each request later (it may ask the user first), so the
//! session is made, the keys bound, and their presses awaited on a thread
//! of their own.

use std::collections::HashMap;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use zbus::blocking::{Connection, Proxy};
use zbus::message::Message;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::{Binding, HotkeyError, HotkeyEvent, Sender};

// ========================================================================
// Constants
// ========================================================================

/// The bus name of the desktop portal.
const PORTAL_DESTINATION: &str = "org.freedesktop.portal.Desktop";

/// The object path of the desktop portal.
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

/// The portal's global shortcuts interface.
const SHORTCUTS_INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

/// The interface of a request the portal answers later.
const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";

/// The interface of a portal session.
const SESSION_INTERFACE: &str = "org.freedesktop.portal.Session";

/// Where the portal puts the request objects of each caller.
const REQUEST_PATH_PREFIX: &str = "/org/freedesktop/portal/desktop/request";

/// The method making a session.
const CREATE_SESSION: &str = "CreateSession";

/// The method binding the shortcuts of a session.
const BIND_SHORTCUTS: &str = "BindShortcuts";

/// The method ending a session.
const CLOSE: &str = "Close";

/// The signal answering a request.
const RESPONSE: &str = "Response";

/// The signal saying a shortcut was pressed.
const ACTIVATED: &str = "Activated";

/// The option naming a request's token.
const HANDLE_TOKEN: &str = "handle_token";

/// The option naming a session's token.
const SESSION_HANDLE_TOKEN: &str = "session_handle_token";

/// The result holding the session made.
const SESSION_HANDLE: &str = "session_handle";

/// A shortcut's description, in its properties.
const DESCRIPTION: &str = "description";

/// The key a shortcut asks for, in its properties.
const PREFERRED_TRIGGER: &str = "preferred_trigger";

/// The response code of a request the portal granted.
const RESPONSE_SUCCESS: u32 = 0;

/// The response code of a request the user declined.
const RESPONSE_CANCELLED: u32 = 1;

/// What every token misty makes starts with.
const TOKEN_PREFIX: &str = "misty";

/// The name of the thread talking to the portal.
const THREAD_NAME: &str = "misty global hotkeys";

/// Makes each token unique within the process.
static TOKENS: AtomicU64 = AtomicU64::new(0);

// ========================================================================
// Data Structures
// ========================================================================

/// Hotkeys bound through the portal; dropping it ends the session.
#[derive(Debug)]
pub(super) struct Portal {
    /// The session's connection and path, once the portal made it, taken
    /// by `Drop` to end it.
    session: Arc<Mutex<Option<Session>>>,
}

/// A portal session.
#[derive(Debug)]
struct Session {
    /// The connection the session lives on.
    connection: Connection,
    /// The session's object path.
    path: OwnedObjectPath,
}

/// A shortcut as `BindShortcuts` takes it: its id and properties.
type Shortcut<'a> = (&'a str, HashMap<&'a str, Value<'a>>);

/// The results of a request the portal answered.
type Results = HashMap<String, OwnedValue>;

// ========================================================================
// Portal Implementations
// ========================================================================

impl Portal {
    /// Binds `bindings` through the portal on a thread of its own, each
    /// press and any refusal going to `send`. Refused at once only when
    /// there is no session bus.
    pub(super) fn bind(bindings: Vec<Binding>, send: Sender) -> Result<Self, HotkeyError> {
        let connection =
            Connection::session().map_err(|error| HotkeyError::Portal(error.to_string()))?;
        let session = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&session);
        thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                if let Err(error) = listen(&connection, &bindings, &send, &slot) {
                    send(HotkeyEvent::Failed(error));
                }
            })
            .map_err(|error| HotkeyError::Unavailable(error.to_string()))?;
        Ok(Self { session })
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        let session = self
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        // The thread stays parked on the portal's signals, which stop with
        // the session, and ends at the next one it sees.
        if let Some(session) = session {
            let closed = Proxy::new(
                &session.connection,
                PORTAL_DESTINATION,
                session.path.as_ref(),
                SESSION_INTERFACE,
            )
            .and_then(|proxy| proxy.call_method(CLOSE, &()));
            // A session the portal already ended needs no closing.
            let _ = closed;
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Makes a session on `connection`, keeping it in `slot`, binds
/// `bindings` to it, and hands each press to `send` until the session
/// ends.
fn listen(
    connection: &Connection,
    bindings: &[Binding],
    send: &Sender,
    slot: &Mutex<Option<Session>>,
) -> Result<(), HotkeyError> {
    let portal = Proxy::new(
        connection,
        PORTAL_DESTINATION,
        PORTAL_PATH,
        SHORTCUTS_INTERFACE,
    )
    .map_err(portal_error)?;
    let request_token = token();
    let session_token = token();
    let options = HashMap::from([
        (HANDLE_TOKEN, Value::from(request_token.as_str())),
        (SESSION_HANDLE_TOKEN, Value::from(session_token.as_str())),
    ]);
    let results = request(connection, &request_token, || {
        portal.call_method(CREATE_SESSION, &(options,))
    })?;
    let path = session_path(&results)?;
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(Session {
        connection: connection.clone(),
        path: path.clone(),
    });
    let triggers: Vec<String> = bindings
        .iter()
        .map(|binding| binding.chord.trigger())
        .collect();
    let shortcuts: Vec<Shortcut<'_>> = bindings
        .iter()
        .zip(&triggers)
        .map(|(binding, trigger)| {
            let properties = HashMap::from([
                (DESCRIPTION, Value::from(binding.description.as_str())),
                (PREFERRED_TRIGGER, Value::from(trigger.as_str())),
            ]);
            (binding.id.as_str(), properties)
        })
        .collect();
    let bind_token = token();
    let options = HashMap::from([(HANDLE_TOKEN, Value::from(bind_token.as_str()))]);
    let activated = portal.receive_signal(ACTIVATED).map_err(portal_error)?;
    request(connection, &bind_token, || {
        portal.call_method(BIND_SHORTCUTS, &(&path, shortcuts, "", options))
    })?;
    for message in activated {
        let Ok((session, id, _, _)) = message
            .body()
            .deserialize::<(OwnedObjectPath, String, u64, Results)>()
        else {
            continue;
        };
        let open = slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        if !open {
            break;
        }
        if session == path {
            send(HotkeyEvent::Pressed(id));
        }
    }
    Ok(())
}

/// Makes a request with `call` under `token` and waits for the portal's
/// answer, listening before asking so the answer cannot be missed.
fn request(
    connection: &Connection,
    token: &str,
    call: impl FnOnce() -> zbus::Result<Message>,
) -> Result<Results, HotkeyError> {
    let sender = connection
        .unique_name()
        .ok_or_else(|| HotkeyError::Portal("the session bus gave misty no name".to_owned()))?;
    let path = request_path(sender.as_str(), token);
    let request = Proxy::new(
        connection,
        PORTAL_DESTINATION,
        path.as_str(),
        REQUEST_INTERFACE,
    )
    .map_err(portal_error)?;
    let mut responses = request.receive_signal(RESPONSE).map_err(portal_error)?;
    call().map_err(portal_error)?;
    let response = responses
        .next()
        .ok_or_else(|| HotkeyError::Portal("the portal never answered".to_owned()))?;
    let (code, results) = response
        .body()
        .deserialize::<(u32, Results)>()
        .map_err(portal_error)?;
    match code {
        RESPONSE_SUCCESS => Ok(results),
        RESPONSE_CANCELLED => Err(HotkeyError::Declined),
        other => Err(HotkeyError::Portal(format!(
            "the request ended with code {other}"
        ))),
    }
}

/// Where the portal puts the request `token` of the caller named
/// `sender` on the bus: its name without the colon, dots made
/// underscores.
fn request_path(sender: &str, token: &str) -> String {
    let caller = sender.trim_start_matches(':').replace('.', "_");
    format!("{REQUEST_PATH_PREFIX}/{caller}/{token}")
}

/// The session the portal made, from its answer; older portals give the
/// path as a string.
fn session_path(results: &Results) -> Result<OwnedObjectPath, HotkeyError> {
    let missing = || HotkeyError::Portal("the portal made no session".to_owned());
    let path = match results.get(SESSION_HANDLE).map(|value| &**value) {
        Some(Value::ObjectPath(path)) => path.clone(),
        Some(Value::Str(text)) => ObjectPath::try_from(text.as_str())
            .map_err(|_| missing())?
            .into_owned(),
        _ => return Err(missing()),
    };
    Ok(path.into())
}

/// A request or session token no other in this process has.
fn token() -> String {
    let count = TOKENS.fetch_add(1, Ordering::Relaxed);
    format!("{TOKEN_PREFIX}{}_{count}", process::id())
}

/// `error` from D-Bus as the reason the portal could not bind the keys.
fn portal_error(error: impl ToString) -> HotkeyError {
    HotkeyError::Portal(error.to_string())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_request_path_is_the_callers_bus_name_made_a_path() {
        assert_eq!(
            request_path(":1.42", "misty7_0"),
            "/org/freedesktop/portal/desktop/request/1_42/misty7_0"
        );
        assert_ne!(token(), token(), "each token is new");
    }

    #[test]
    fn test_the_session_path_is_read_whether_given_as_a_path_or_a_string() {
        let as_path = ObjectPath::try_from("/a/b").expect("a path");
        for value in [Value::from(as_path), Value::from("/a/b")] {
            let results = HashMap::from([(
                SESSION_HANDLE.to_owned(),
                OwnedValue::try_from(value).expect("owned"),
            )]);
            assert_eq!(session_path(&results).expect("a session").as_str(), "/a/b");
        }
        assert!(matches!(
            session_path(&Results::new()),
            Err(HotkeyError::Portal(_))
        ));
    }
}
