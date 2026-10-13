use std::sync::mpsc;

use midi_file::midly::{self, MidiMessage, live::LiveEvent};
use winit::event_loop::EventLoopProxy;

use crate::NeothesiaEvent;

type Connection = (midi_io::MidiInputPort, midi_io::MidiInputConnection);

pub struct InputManager {
    input: midi_io::MidiInputManager,
    tx: EventLoopProxy<NeothesiaEvent>,
    current_connection: Option<Connection>,
    /// A connection being opened on a background thread. Opening a Bluetooth MIDI
    /// port can block for a long time (or forever, if the piano is connected to
    /// another device), so it must never run on the UI thread.
    pending: Option<(midi_io::MidiInputPort, mpsc::Receiver<Option<Connection>>)>,
}

impl InputManager {
    pub fn new(tx: EventLoopProxy<NeothesiaEvent>) -> Self {
        let input = midi_io::MidiInputManager::new().unwrap();
        Self {
            input,
            tx,
            current_connection: None,
            pending: None,
        }
    }

    /// Finish a connection started by [`Self::connect_input`]; call once per frame.
    pub fn poll(&mut self) {
        let Some((port, rx)) = self.pending.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(Some(conn)) => {
                log::info!("MIDI input connected: {port}");
                self.current_connection = Some(conn);
                self.pending = None;
            }
            Ok(None) => {
                log::warn!("MIDI input connection failed: {port}");
                self.pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
        }
    }

    /// The port that is connected or currently being connected.
    pub fn connected_port(&self) -> Option<&midi_io::MidiInputPort> {
        self.current_connection
            .as_ref()
            .map(|(p, _)| p)
            .or_else(|| self.pending.as_ref().map(|(p, _)| p))
    }

    pub fn inputs(&self) -> Vec<midi_io::MidiInputPort> {
        self.input.inputs()
    }

    pub fn connect_input(&mut self, port: midi_io::MidiInputPort) {
        let tx = self.tx.clone();

        if self.connected_port() == Some(&port) {
            return;
        }

        // Close the connection first, as Windows does not like it when we hold 2 connections
        self.current_connection = None;
        self.pending = None;

        let (result_tx, result_rx) = mpsc::channel();
        self.pending = Some((port.clone(), result_rx));

        std::thread::spawn(move || {
            let conn = midi_io::MidiInputManager::connect_input(port, move |message| {
                let event = LiveEvent::parse(message).unwrap();

                if let LiveEvent::Midi { channel, message } = event {
                    match message {
                        // Some keyboards send NoteOn event with vel 0 instead of NoteOff
                        midly::MidiMessage::NoteOn { key, vel } if vel == 0 => {
                            tx.send_event(NeothesiaEvent::MidiInput {
                                channel: channel.as_int(),
                                message: MidiMessage::NoteOff { key, vel },
                            })
                            .ok();
                        }
                        message => {
                            tx.send_event(NeothesiaEvent::MidiInput {
                                channel: channel.as_int(),
                                message,
                            })
                            .ok();
                        }
                    }
                }
            });
            result_tx.send(conn).ok();
        });
    }
}
