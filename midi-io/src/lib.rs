use std::{
    error::Error,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// How long a cached port list is served before a background refresh is started.
const PORT_CACHE_TTL: Duration = Duration::from_secs(1);

/// Port list that is enumerated on a background thread.
///
/// On Windows with the WinRT backend every enumeration waits for the system, which
/// takes several milliseconds and, for Bluetooth devices, can take much longer.
/// The UI asks for the list every frame, so the last result is cached and refreshed
/// at most once per [`PORT_CACHE_TTL`] without blocking the caller.
struct PortCache<T> {
    ports: Arc<Mutex<Vec<T>>>,
    refreshing: Arc<AtomicBool>,
    last_refresh: Mutex<Option<Instant>>,
}

impl<T: Clone + Send + 'static> PortCache<T> {
    fn new() -> Self {
        Self {
            ports: Arc::new(Mutex::new(Vec::new())),
            refreshing: Arc::new(AtomicBool::new(false)),
            last_refresh: Mutex::new(None),
        }
    }

    fn get(&self, enumerate: fn() -> Vec<T>) -> Vec<T> {
        let mut last = self.last_refresh.lock().unwrap();
        match *last {
            // First call: enumerate synchronously so the caller gets a real list
            // (the settings auto-select the first port on startup).
            None => {
                let ports = enumerate();
                *self.ports.lock().unwrap() = ports.clone();
                *last = Some(Instant::now());
                ports
            }
            Some(t) => {
                if t.elapsed() > PORT_CACHE_TTL
                    && self
                        .refreshing
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    *last = Some(Instant::now());
                    let ports = self.ports.clone();
                    let refreshing = self.refreshing.clone();
                    std::thread::spawn(move || {
                        let fresh = enumerate();
                        *ports.lock().unwrap() = fresh;
                        refreshing.store(false, Ordering::Release);
                    });
                }
                self.ports.lock().unwrap().clone()
            }
        }
    }
}

/// An error that can occur during initialization (i.e., while
/// creating a `MidiInput` or `MidiOutput` object).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitError;

impl Error for InitError {}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "MIDI support could not be initialized".fmt(f)
    }
}

impl From<midir::InitError> for InitError {
    fn from(_: midir::InitError) -> Self {
        Self
    }
}

pub struct MidiOutputManager {
    cache: PortCache<MidiOutputPort>,
}

impl MidiOutputManager {
    pub fn new() -> Result<Self, InitError> {
        // Fail early if MIDI cannot be initialized at all
        midir::MidiOutput::new("MidiIo-out-manager")?;

        Ok(Self {
            cache: PortCache::new(),
        })
    }

    fn enumerate() -> Vec<MidiOutputPort> {
        let Ok(output) = midir::MidiOutput::new("MidiIo-out-enum") else {
            return Vec::new();
        };
        output
            .ports()
            .iter()
            .filter_map(|p| output.port_name(p).ok())
            .map(MidiOutputPort)
            .collect()
    }

    /// Known output ports; served from a cache that is refreshed in the background.
    pub fn outputs(&self) -> Vec<MidiOutputPort> {
        self.cache.get(Self::enumerate)
    }

    pub fn connect_output(port: MidiOutputPort) -> Option<MidiOutputConnection> {
        let output = midir::MidiOutput::new("MidiIo-out").unwrap();

        let port = output.ports().into_iter().find(|info| {
            output
                .port_name(info)
                .ok()
                .map(|name| name == port.0)
                .unwrap_or(false)
        });

        port.and_then(move |port| output.connect(&port, "MidiIo-in-conn").ok())
            .map(MidiOutputConnection)
    }
}

pub struct MidiInputManager {
    cache: PortCache<MidiInputPort>,
}

impl MidiInputManager {
    pub fn new() -> Result<Self, InitError> {
        // Fail early if MIDI cannot be initialized at all
        midir::MidiInput::new("MidiIo-in-manager")?;

        Ok(Self {
            cache: PortCache::new(),
        })
    }

    fn enumerate() -> Vec<MidiInputPort> {
        let Ok(input) = midir::MidiInput::new("MidiIo-in-enum") else {
            return Vec::new();
        };
        input
            .ports()
            .iter()
            .filter_map(|p| input.port_name(p).ok())
            .map(MidiInputPort)
            .collect()
    }

    /// Known input ports; served from a cache that is refreshed in the background.
    pub fn inputs(&self) -> Vec<MidiInputPort> {
        self.cache.get(Self::enumerate)
    }

    pub fn connect_input<F>(
        port: MidiInputPort,
        mut callback: F,
    ) -> Option<(MidiInputPort, MidiInputConnection)>
    where
        F: FnMut(&[u8]) + Send + 'static,
    {
        let input = midir::MidiInput::new("MidiIo-in").unwrap();

        let midir_port = input.ports().into_iter().find(|info| {
            input
                .port_name(info)
                .ok()
                .map(|name| name == port.0)
                .unwrap_or(false)
        })?;

        Some((
            port.clone(),
            MidiInputConnection(
                input
                    .connect(
                        &midir_port,
                        "MidiIo-in-conn",
                        move |_, data, _| {
                            callback(data);
                            //
                        },
                        (),
                    )
                    .inspect_err(|err| log::error!("MIDI-in connection fail: {err}"))
                    .ok()?,
            ),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MidiOutputPort(String);

impl std::fmt::Display for MidiOutputPort {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MidiInputPort(String);

impl std::fmt::Display for MidiInputPort {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[allow(unused)]
pub struct MidiInputConnection(midir::MidiInputConnection<()>);
pub struct MidiOutputConnection(midir::MidiOutputConnection);

impl MidiOutputConnection {
    /// Send a message to the port that this output connection is connected to.
    /// The message must be a valid MIDI message (see https://www.midi.org/specifications-old/item/table-1-summary-of-midi-message).
    pub fn send(&mut self, message: &[u8]) -> Result<(), SendError> {
        self.0.send(message)?;
        Ok(())
    }
}

/// An error that can occur when sending MIDI messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    InvalidData(&'static str),
    Other(&'static str),
}

impl Error for SendError {}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            SendError::InvalidData(msg) | SendError::Other(msg) => msg.fmt(f),
        }
    }
}

impl From<midir::SendError> for SendError {
    fn from(err: midir::SendError) -> Self {
        match err {
            midir::SendError::InvalidData(e) => Self::InvalidData(e),
            midir::SendError::Other(e) => Self::Other(e),
        }
    }
}
