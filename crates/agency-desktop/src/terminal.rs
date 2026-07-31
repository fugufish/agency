use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use agency_mux::{
    Attachment, CommandSpec, Controller, Event, Exit, HeadlessSession, Multiplexer, Program,
    Size as TerminalSize, Status,
};
use libghostty_vt::render::{CellIterator, RowIterator};
use libghostty_vt::{RenderState, Terminal, TerminalOptions};

const COLS: u16 = 100;
const ROWS: u16 = 30;
/// Headless commands have no visible surface, so they use a tall screen that
/// keeps a whole command's output addressable instead of a viewport-sized one.
const HEADLESS_COLS: u16 = 120;
const HEADLESS_ROWS: u16 = 200;
/// A process is reaped before its final bytes are guaranteed to have been
/// broadcast, so the renderer drains briefly before reporting the exit.
const TRAILING_OUTPUT: Duration = Duration::from_millis(100);

pub struct TerminalSession {
    controller: Controller,
    updates: Receiver<TerminalUpdate>,
    cwd: PathBuf,
    program: Program,
    screen: String,
    status: Status,
}

enum TerminalUpdate {
    Screen(String),
    Failed(String),
    Exited(Exit),
}

/// A terminal that renders off screen. Its output is streamed to whichever
/// surface asked for the command instead of to a terminal pane.
pub struct HeadlessTerminal {
    controller: Controller,
    updates: Receiver<TerminalUpdate>,
    screen: String,
    outcome: Option<HeadlessOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadlessOutcome {
    Exited(Exit),
    Failed(String),
}

impl HeadlessTerminal {
    pub fn spawn(command: CommandSpec, cwd: &Path) -> Result<Self, String> {
        let size = TerminalSize {
            cols: HEADLESS_COLS,
            rows: HEADLESS_ROWS,
            ..TerminalSize::default()
        };
        let session = HeadlessSession::spawn(command, cwd, size)?;
        let controller = session.controller();
        let (update_tx, update_rx) = mpsc::channel();

        thread::spawn(move || run_renderer(session, update_tx, size));

        Ok(Self {
            controller,
            updates: update_rx,
            screen: String::new(),
            outcome: None,
        })
    }

    /// Drains pending renderer updates into the rendered screen and outcome.
    pub fn poll(&mut self) {
        for update in self.updates.try_iter() {
            match update {
                TerminalUpdate::Screen(screen) => self.screen = screen.trim_end().to_owned(),
                TerminalUpdate::Failed(error) => {
                    self.outcome.get_or_insert(HeadlessOutcome::Failed(error));
                }
                TerminalUpdate::Exited(exit) => {
                    self.outcome.get_or_insert(HeadlessOutcome::Exited(exit));
                }
            }
        }
    }

    pub fn output(&self) -> &str {
        &self.screen
    }

    pub fn outcome(&self) -> Option<&HeadlessOutcome> {
        self.outcome.as_ref()
    }
}

impl Drop for HeadlessTerminal {
    fn drop(&mut self) {
        if self.outcome.is_none() {
            self.controller.terminate();
        }
    }
}

/// Lets the renderer read from an attached session or a headless command.
trait EventSource {
    fn recv(&self) -> Result<Event, mpsc::RecvError>;
    fn recv_timeout(&self, timeout: Duration) -> Result<Event, mpsc::RecvTimeoutError>;
}

impl EventSource for Attachment {
    fn recv(&self) -> Result<Event, mpsc::RecvError> {
        Attachment::recv(self)
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<Event, mpsc::RecvTimeoutError> {
        Attachment::recv_timeout(self, timeout)
    }
}

impl EventSource for HeadlessSession {
    fn recv(&self) -> Result<Event, mpsc::RecvError> {
        HeadlessSession::recv(self)
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<Event, mpsc::RecvTimeoutError> {
        HeadlessSession::recv_timeout(self, timeout)
    }
}

impl TerminalSession {
    pub fn spawn(
        multiplexer: &mut Multiplexer,
        program: Program,
        cwd: &Path,
    ) -> Result<Self, String> {
        let size = TerminalSize {
            cols: COLS,
            rows: ROWS,
            ..TerminalSize::default()
        };
        let attachment = multiplexer.spawn(program, cwd, size)?;
        let controller = attachment.controller();
        let (update_tx, update_rx) = mpsc::channel();

        thread::spawn(move || run_renderer(attachment, update_tx, size));

        Ok(Self {
            controller,
            updates: update_rx,
            cwd: cwd.to_path_buf(),
            program,
            screen: String::new(),
            status: Status::Running,
        })
    }

    pub fn send(&self, bytes: Vec<u8>) {
        self.controller.write(bytes);
    }

    pub fn poll(&mut self) {
        for update in self.updates.try_iter() {
            match update {
                TerminalUpdate::Screen(screen) => self.screen = screen,
                TerminalUpdate::Failed(error) => {
                    self.screen.push_str("\n\n");
                    self.screen.push_str(&error);
                    self.status = Status::Failed;
                }
                TerminalUpdate::Exited(_) => self.status = Status::Exited,
            }
        }
    }

    pub fn screen(&self) -> &str {
        &self.screen
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn program(&self) -> Program {
        self.program
    }

    pub fn status(&self) -> &'static str {
        match self.status {
            Status::Running => "running",
            Status::Exited => "exited",
            Status::Failed => "failed",
        }
    }
}

fn run_renderer(source: impl EventSource, updates: Sender<TerminalUpdate>, size: TerminalSize) {
    let result = (|| -> Result<(), String> {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: size.cols,
            rows: size.rows,
            max_scrollback: 10_000,
        })
        .map_err(|error| format!("Could not initialize Ghostty: {error}"))?;
        let mut render_state = RenderState::new()
            .map_err(|error| format!("Could not initialize renderer: {error}"))?;
        let mut rows =
            RowIterator::new().map_err(|error| format!("Could not initialize rows: {error}"))?;
        let mut cells =
            CellIterator::new().map_err(|error| format!("Could not initialize cells: {error}"))?;

        while let Ok(event) = source.recv() {
            match event {
                Event::Output(bytes) => {
                    terminal.vt_write(&bytes);
                    let screen =
                        render_screen(&terminal, &mut render_state, &mut rows, &mut cells)?;
                    let _ = updates.send(TerminalUpdate::Screen(screen));
                }
                Event::Exited(exit) => {
                    while let Ok(Event::Output(bytes)) = source.recv_timeout(TRAILING_OUTPUT) {
                        terminal.vt_write(&bytes);
                    }
                    let screen =
                        render_screen(&terminal, &mut render_state, &mut rows, &mut cells)?;
                    let _ = updates.send(TerminalUpdate::Screen(screen));
                    let _ = updates.send(TerminalUpdate::Exited(exit));
                    break;
                }
                Event::Failed(error) => {
                    let _ = updates.send(TerminalUpdate::Failed(error));
                    break;
                }
            }
        }

        Ok(())
    })();

    if let Err(error) = result {
        let _ = updates.send(TerminalUpdate::Failed(error));
    }
}

fn render_screen<'alloc, 'callbacks>(
    terminal: &Terminal<'alloc, 'callbacks>,
    render_state: &mut RenderState<'alloc>,
    rows: &mut RowIterator<'alloc>,
    cells: &mut CellIterator<'alloc>,
) -> Result<String, String> {
    let snapshot = render_state
        .update(terminal)
        .map_err(|error| format!("Could not update terminal screen: {error}"))?;
    let mut row_iterator = rows
        .update(&snapshot)
        .map_err(|error| format!("Could not read terminal rows: {error}"))?;
    let mut screen = String::new();

    while let Some(row) = row_iterator.next() {
        let mut cell_iterator = cells
            .update(row)
            .map_err(|error| format!("Could not read terminal cells: {error}"))?;
        let mut line = String::new();

        while let Some(cell) = cell_iterator.next() {
            let graphemes = cell
                .graphemes()
                .map_err(|error| format!("Could not read terminal text: {error}"))?;

            if graphemes.is_empty() {
                line.push(' ');
            } else {
                line.extend(graphemes);
            }
        }

        screen.push_str(line.trim_end());
        screen.push('\n');
    }

    Ok(screen)
}
