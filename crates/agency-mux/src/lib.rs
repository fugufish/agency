use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl SessionId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    Shell,
}

impl Program {
    pub fn label(self) -> &'static str {
        match self {
            Self::Shell => "Shell",
        }
    }

    fn command(self, cwd: &Path) -> CommandBuilder {
        let mut command = match self {
            Self::Shell => CommandBuilder::new_default_prog(),
        };

        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    Exited,
    Failed,
}

#[derive(Debug, Clone)]
pub enum Event {
    Output(Vec<u8>),
    Exited,
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl Default for Size {
    fn default() -> Self {
        Self {
            cols: 100,
            rows: 30,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

pub struct Attachment {
    id: SessionId,
    program: Program,
    cwd: PathBuf,
    control: mpsc::Sender<Control>,
    events: mpsc::Receiver<Event>,
}

#[derive(Clone)]
pub struct Controller {
    control: mpsc::Sender<Control>,
}

impl Controller {
    pub fn write(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.control.send(Control::Input(bytes));
        }
    }

    pub fn resize(&self, size: Size) {
        let _ = self.control.send(Control::Resize(size));
    }

    pub fn terminate(&self) {
        let _ = self.control.send(Control::Terminate);
    }
}

impl Attachment {
    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn program(&self) -> Program {
        self.program
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn write(&self, bytes: Vec<u8>) {
        self.controller().write(bytes);
    }

    pub fn resize(&self, size: Size) {
        self.controller().resize(size);
    }

    pub fn terminate(&self) {
        self.controller().terminate();
    }

    pub fn controller(&self) -> Controller {
        Controller {
            control: self.control.clone(),
        }
    }

    pub fn recv(&self) -> Result<Event, mpsc::RecvError> {
        self.events.recv()
    }

    pub fn try_events(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter()
    }
}

struct Session {
    id: SessionId,
    program: Program,
    cwd: PathBuf,
    control: mpsc::Sender<Control>,
    subscribers: Subscribers,
    status: Arc<Mutex<Status>>,
}

type Subscribers = Arc<Mutex<Vec<mpsc::Sender<Event>>>>;

enum Control {
    Input(Vec<u8>),
    Resize(Size),
    Terminate,
}

#[derive(Default)]
pub struct Multiplexer {
    sessions: HashMap<SessionId, Session>,
}

impl Multiplexer {
    pub fn spawn(
        &mut self,
        program: Program,
        cwd: impl AsRef<Path>,
        size: Size,
    ) -> Result<Attachment, String> {
        let cwd = cwd.as_ref().to_path_buf();
        let id = SessionId(NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed));
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: size.pixel_width,
                pixel_height: size.pixel_height,
            })
            .map_err(|error| format!("Could not open terminal: {error}"))?;
        let mut child = pair
            .slave
            .spawn_command(program.command(&cwd))
            .map_err(|error| format!("Could not start {}: {error}", program.label()))?;
        let mut killer = child.clone_killer();

        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("Could not read terminal: {error}"))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("Could not write terminal: {error}"))?;
        let master = pair.master;
        let (control_tx, control_rx) = mpsc::channel();
        let subscribers = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(Mutex::new(Status::Running));

        let session = Session {
            id,
            program,
            cwd,
            control: control_tx,
            subscribers,
            status,
        };
        let attachment = session.attach();

        let output_subscribers = Arc::clone(&session.subscribers);
        thread::spawn(move || {
            let mut buffer = [0_u8; 8 * 1024];

            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        broadcast(&output_subscribers, Event::Output(buffer[..count].to_vec()));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        broadcast(
                            &output_subscribers,
                            Event::Failed(format!("Could not read process output: {error}")),
                        );
                        break;
                    }
                }
            }
        });

        let control_subscribers = Arc::clone(&session.subscribers);
        let control_status = Arc::clone(&session.status);
        thread::spawn(move || {
            for control in control_rx {
                let result = match control {
                    Control::Input(bytes) => writer.write_all(&bytes).and_then(|_| writer.flush()),
                    Control::Resize(size) => master
                        .resize(PtySize {
                            rows: size.rows,
                            cols: size.cols,
                            pixel_width: size.pixel_width,
                            pixel_height: size.pixel_height,
                        })
                        .map_err(std::io::Error::other),
                    Control::Terminate => {
                        let _ = killer.kill();
                        break;
                    }
                };

                if let Err(error) = result {
                    set_status(&control_status, Status::Failed);
                    broadcast(
                        &control_subscribers,
                        Event::Failed(format!("Terminal operation failed: {error}")),
                    );
                    break;
                }
            }
        });

        let exit_subscribers = Arc::clone(&session.subscribers);
        let exit_status = Arc::clone(&session.status);
        thread::spawn(move || {
            let _ = child.wait();
            set_status(&exit_status, Status::Exited);
            broadcast(&exit_subscribers, Event::Exited);
        });

        self.sessions.insert(id, session);
        Ok(attachment)
    }

    pub fn attach(&self, id: SessionId) -> Option<Attachment> {
        self.sessions.get(&id).map(Session::attach)
    }

    pub fn sessions(&self) -> impl Iterator<Item = (SessionId, Program, &Path, Status)> {
        self.sessions.values().map(|session| {
            (
                session.id,
                session.program,
                session.cwd.as_path(),
                *session
                    .status
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
            )
        })
    }
}

impl Session {
    fn attach(&self) -> Attachment {
        let (events_tx, events_rx) = mpsc::channel();
        self.subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(events_tx);

        Attachment {
            id: self.id,
            program: self.program,
            cwd: self.cwd.clone(),
            control: self.control.clone(),
            events: events_rx,
        }
    }
}

fn set_status(status: &Mutex<Status>, value: Status) {
    *status.lock().unwrap_or_else(|error| error.into_inner()) = value;
}

fn broadcast(subscribers: &Mutex<Vec<mpsc::Sender<Event>>>, event: Event) {
    subscribers
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|subscriber| subscriber.send(event.clone()).is_ok());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_have_stable_labels() {
        assert_eq!(Program::Shell.label(), "Shell");
    }

    #[test]
    fn session_ids_are_unique() {
        let first = SessionId(NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed));
        let second = SessionId(NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed));

        assert_ne!(first, second);
    }
}
