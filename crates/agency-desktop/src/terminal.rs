use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use agency_mux::{
    Attachment, Controller, Event, Multiplexer, Program, Size as TerminalSize, Status,
};
use libghostty_vt::render::{CellIterator, RowIterator};
use libghostty_vt::{RenderState, Terminal, TerminalOptions};

const COLS: u16 = 100;
const ROWS: u16 = 30;

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
    Exited,
}

impl TerminalSession {
    pub fn spawn(
        multiplexer: &mut Multiplexer,
        program: Program,
        cwd: &Path,
    ) -> Result<Self, String> {
        let attachment = multiplexer.spawn(
            program,
            cwd,
            TerminalSize {
                cols: COLS,
                rows: ROWS,
                ..TerminalSize::default()
            },
        )?;
        let controller = attachment.controller();
        let (update_tx, update_rx) = mpsc::channel();

        thread::spawn(move || run_renderer(attachment, update_tx));

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
                TerminalUpdate::Exited => self.status = Status::Exited,
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

fn run_renderer(attachment: Attachment, updates: Sender<TerminalUpdate>) {
    let result = (|| -> Result<(), String> {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: COLS,
            rows: ROWS,
            max_scrollback: 10_000,
        })
        .map_err(|error| format!("Could not initialize Ghostty: {error}"))?;
        let mut render_state = RenderState::new()
            .map_err(|error| format!("Could not initialize renderer: {error}"))?;
        let mut rows =
            RowIterator::new().map_err(|error| format!("Could not initialize rows: {error}"))?;
        let mut cells =
            CellIterator::new().map_err(|error| format!("Could not initialize cells: {error}"))?;

        while let Ok(event) = attachment.recv() {
            match event {
                Event::Output(bytes) => {
                    terminal.vt_write(&bytes);
                    let screen =
                        render_screen(&terminal, &mut render_state, &mut rows, &mut cells)?;
                    let _ = updates.send(TerminalUpdate::Screen(screen));
                }
                Event::Exited => {
                    let _ = updates.send(TerminalUpdate::Exited);
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
