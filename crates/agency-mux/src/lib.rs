use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

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

        prepare(&mut command, cwd);
        command
    }
}

/// An explicit terminal command, run without an attached terminal surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// The command as a user would type it, for transcripts and errors.
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn command(&self, cwd: &Path) -> CommandBuilder {
        let mut command = CommandBuilder::new(&self.program);
        command.args(&self.args);
        prepare(&mut command, cwd);
        command
    }
}

fn prepare(command: &mut CommandBuilder, cwd: &Path) {
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
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
    Exited(Exit),
    Failed(String),
}

/// How a terminal process finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Exit {
    pub success: bool,
    pub code: Option<u32>,
}

impl Exit {
    fn from_code(code: u32) -> Self {
        Self {
            success: code == 0,
            code: Some(code),
        }
    }
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

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, mpsc::RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    pub fn try_events(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter()
    }
}

/// A terminal process that runs without a terminal surface. Headless commands
/// are owned by their caller instead of the multiplexer, so they never appear
/// among the attachable sessions.
pub struct HeadlessSession {
    command: CommandSpec,
    control: mpsc::Sender<Control>,
    events: mpsc::Receiver<Event>,
}

impl HeadlessSession {
    pub fn spawn(command: CommandSpec, cwd: impl AsRef<Path>, size: Size) -> Result<Self, String> {
        let pty = Pty::open(command.command(cwd.as_ref()), &command.display(), size)?;
        let (events_tx, events_rx) = mpsc::channel();
        pty.subscribe(events_tx);
        let pty = pty.start();

        Ok(Self {
            command,
            control: pty.control,
            events: events_rx,
        })
    }

    pub fn command(&self) -> &CommandSpec {
        &self.command
    }

    pub fn controller(&self) -> Controller {
        Controller {
            control: self.control.clone(),
        }
    }

    pub fn recv(&self) -> Result<Event, mpsc::RecvError> {
        self.events.recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, mpsc::RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    pub fn terminate(&self) {
        let _ = self.control.send(Control::Terminate);
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
        let pty = Pty::open(program.command(&cwd), program.label(), size)?;
        let (events_tx, events_rx) = mpsc::channel();
        pty.subscribe(events_tx);
        let attachment = Attachment {
            id,
            program,
            cwd: cwd.clone(),
            control: pty.control.clone(),
            events: events_rx,
        };
        let pty = pty.start();

        self.sessions.insert(
            id,
            Session {
                id,
                program,
                cwd,
                control: pty.control,
                subscribers: pty.subscribers,
                status: pty.status,
            },
        );
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

/// The process, reader, control, and exit plumbing shared by attachable
/// sessions and headless commands. Subscribers attach before [`Pty::start`]
/// spawns the worker threads, so no output is lost between the two calls.
struct Pty {
    control: mpsc::Sender<Control>,
    subscribers: Subscribers,
    status: Arc<Mutex<Status>>,
    resources: PtyResources,
}

struct PtyResources {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    control: mpsc::Receiver<Control>,
}

impl Pty {
    fn open(command: CommandBuilder, label: &str, size: Size) -> Result<Self, String> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: size.pixel_width,
                pixel_height: size.pixel_height,
            })
            .map_err(|error| format!("Could not open terminal: {error}"))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("Could not start {label}: {error}"))?;
        let killer = child.clone_killer();

        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("Could not read terminal: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("Could not write terminal: {error}"))?;
        let master = pair.master;
        let (control_tx, control_rx) = mpsc::channel();

        Ok(Self {
            control: control_tx,
            subscribers: Arc::new(Mutex::new(Vec::new())),
            status: Arc::new(Mutex::new(Status::Running)),
            resources: PtyResources {
                child,
                killer,
                reader,
                writer,
                master,
                control: control_rx,
            },
        })
    }

    fn subscribe(&self, events: mpsc::Sender<Event>) {
        self.subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(events);
    }

    fn start(self) -> RunningPty {
        let PtyResources {
            mut child,
            mut killer,
            mut reader,
            mut writer,
            master,
            control: control_rx,
        } = self.resources;
        let control = self.control;
        let subscribers = self.subscribers;
        let status = self.status;

        let output_subscribers = Arc::clone(&subscribers);
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

        let control_subscribers = Arc::clone(&subscribers);
        let control_status = Arc::clone(&status);
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

        let exit_subscribers = Arc::clone(&subscribers);
        let exit_status = Arc::clone(&status);
        thread::spawn(move || {
            let exit = child.wait().map_or_else(
                |_| Exit::default(),
                |status| Exit::from_code(status.exit_code()),
            );
            set_status(&exit_status, Status::Exited);
            broadcast(&exit_subscribers, Event::Exited(exit));
        });

        RunningPty {
            control,
            subscribers,
            status,
        }
    }
}

/// A [`Pty`] whose reader, control, and exit threads are running.
struct RunningPty {
    control: mpsc::Sender<Control>,
    subscribers: Subscribers,
    status: Arc<Mutex<Status>>,
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
    fn command_specs_render_as_typed_commands() {
        let command = CommandSpec::new("claude", ["plugin", "marketplace", "add", "./source"]);

        assert_eq!(command.display(), "claude plugin marketplace add ./source");
        assert_eq!(
            CommandSpec::new("codex", Vec::<String>::new()).display(),
            "codex"
        );
    }

    #[cfg(unix)]
    #[test]
    fn headless_commands_stream_output_and_report_their_exit() {
        let session = HeadlessSession::spawn(
            CommandSpec::new("sh", ["-c", "printf hello; exit 3"]),
            std::env::temp_dir(),
            Size::default(),
        )
        .unwrap();

        let mut output = Vec::new();
        let exit = loop {
            match session.recv_timeout(Duration::from_secs(10)).unwrap() {
                Event::Output(bytes) => output.extend(bytes),
                Event::Exited(exit) => break exit,
                Event::Failed(error) => panic!("{error}"),
            }
        };

        assert!(String::from_utf8_lossy(&output).contains("hello"));
        assert_eq!(
            exit,
            Exit {
                success: false,
                code: Some(3)
            }
        );
    }

    #[test]
    fn session_ids_are_unique() {
        let first = SessionId(NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed));
        let second = SessionId(NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed));

        assert_ne!(first, second);
    }
}
