use std::fs;
use std::path::{Path, PathBuf};

use iced::widget::markdown;

#[derive(Default)]
pub struct State {
    pub visible: bool,
    pub path: Option<PathBuf>,
    pub source: String,
    pub blocks: Vec<Block>,
    pub mode: Mode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Rendered,
    Raw,
}

pub enum Block {
    Markdown(Box<markdown::Content>),
    Mermaid(MermaidDiagram),
}

#[derive(Debug, Clone)]
pub struct MermaidDiagram {
    pub kind: String,
    pub lines: Vec<String>,
}

impl State {
    pub fn toggle(&mut self, root: &Path, preferred: Option<&Path>) -> Result<(), String> {
        self.visible = !self.visible;
        if !self.visible {
            return Ok(());
        }

        let path = preferred
            .filter(|path| path.is_file())
            .map(Path::to_path_buf)
            .or_else(|| {
                ["README.md", "README.markdown"]
                    .into_iter()
                    .map(|name| root.join(name))
                    .find(|path| path.is_file())
            })
            .or_else(|| first_markdown(root));
        let Some(path) = path else {
            self.path = None;
            self.source.clear();
            self.blocks.clear();
            return Err("No viewable file selected and no Markdown file found".to_owned());
        };
        self.open(path)
    }

    pub fn open(&mut self, path: PathBuf) -> Result<(), String> {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        self.blocks = parse_blocks(&source);
        self.source = source;
        self.path = Some(path);
        self.mode = if self.is_markdown() {
            Mode::Rendered
        } else {
            Mode::Raw
        };
        Ok(())
    }

    pub fn toggle_mode(&mut self) {
        if self.is_markdown() {
            self.mode = match self.mode {
                Mode::Rendered => Mode::Raw,
                Mode::Raw => Mode::Rendered,
            };
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if self.is_markdown() {
            self.mode = mode;
        }
    }

    pub fn is_markdown(&self) -> bool {
        self.path.as_deref().is_some_and(is_markdown)
    }

    /// Rendered Markdown lays itself out against the width it is given, so the
    /// viewer must scroll vertically only. Offering horizontal scrolling gives
    /// the blocks an unbounded width, which collapses the code blocks nested
    /// scrollables and leaves the whole document blank.
    pub fn wraps_content(&self) -> bool {
        self.is_markdown() && self.mode == Mode::Rendered
    }
}

pub fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

fn first_markdown(root: &Path) -> Option<PathBuf> {
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let mut entries = fs::read_dir(directory).ok()?.flatten().collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                let hidden = entry.file_name().to_string_lossy().starts_with('.');
                if !hidden && entry.file_name() != "target" && entry.file_name() != "node_modules" {
                    directories.push(path);
                }
            } else if is_markdown(&path) {
                return Some(path);
            }
        }
    }
    None
}

fn parse_blocks(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut markdown_source = String::new();
    let mut mermaid_source = Vec::new();
    let mut in_mermaid = false;

    for line in source.lines() {
        if !in_mermaid && line.trim_start().starts_with("```mermaid") {
            if !markdown_source.is_empty() {
                blocks.push(Block::Markdown(Box::new(markdown::Content::parse(
                    &markdown_source,
                ))));
                markdown_source.clear();
            }
            in_mermaid = true;
        } else if in_mermaid && line.trim() == "```" {
            blocks.push(Block::Mermaid(parse_mermaid(&mermaid_source)));
            mermaid_source.clear();
            in_mermaid = false;
        } else if in_mermaid {
            mermaid_source.push(line.to_owned());
        } else {
            markdown_source.push_str(line);
            markdown_source.push('\n');
        }
    }
    if in_mermaid {
        markdown_source.push_str("```mermaid\n");
        markdown_source.push_str(&mermaid_source.join("\n"));
    }
    if !markdown_source.is_empty() {
        blocks.push(Block::Markdown(Box::new(markdown::Content::parse(
            &markdown_source,
        ))));
    }
    blocks
}

fn parse_mermaid(source: &[String]) -> MermaidDiagram {
    let mut lines = source
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty());
    let kind = lines.next().unwrap_or("diagram").to_owned();
    let lines = lines.map(render_mermaid_line).collect();
    MermaidDiagram { kind, lines }
}

fn render_mermaid_line(line: &str) -> String {
    for (syntax, arrow) in [("-->", "→"), ("---", "—"), ("-.->", "⇢"), ("==>", "⇒")] {
        if let Some((left, right)) = line.split_once(syntax) {
            return format!("{}  {arrow}  {}", node_label(left), node_label(right));
        }
    }
    node_label(line)
}

fn node_label(node: &str) -> String {
    let node = node.trim().trim_end_matches(';');
    for delimiters in [("[", "]"), ("(", ")"), ("{", "}")] {
        if let Some(start) = node.find(delimiters.0)
            && let Some(end) = node.rfind(delimiters.1)
            && end > start
        {
            return node[start + 1..end].trim_matches(['\"', '\'']).to_owned();
        }
    }
    node.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_inline_mermaid_between_markdown_blocks() {
        let blocks = parse_blocks("# Before\n```mermaid\ngraph LR\nA[One] --> B[Two]\n```\nAfter");
        assert_eq!(blocks.len(), 3);
        let Block::Mermaid(diagram) = &blocks[1] else {
            panic!("expected diagram")
        };
        assert_eq!(diagram.kind, "graph LR");
        assert_eq!(diagram.lines, ["One  →  Two"]);
    }

    #[test]
    fn markdown_defaults_to_rendered_and_can_switch_to_raw() {
        let mut state = State {
            path: Some(PathBuf::from("notes.md")),
            ..State::default()
        };
        assert_eq!(state.mode, Mode::Rendered);
        state.toggle_mode();
        assert_eq!(state.mode, Mode::Raw);
    }

    #[test]
    fn only_rendered_markdown_wraps_to_the_viewer_width() {
        let mut state = State {
            path: Some(PathBuf::from("README.md")),
            ..State::default()
        };
        assert!(state.wraps_content());
        state.toggle_mode();
        assert!(!state.wraps_content());

        let source = State {
            path: Some(PathBuf::from("main.rs")),
            mode: Mode::Raw,
            ..State::default()
        };
        assert!(!source.wraps_content());
    }

    #[test]
    fn non_markdown_files_remain_raw() {
        let mut state = State {
            path: Some(PathBuf::from("main.rs")),
            mode: Mode::Raw,
            ..State::default()
        };
        state.toggle_mode();
        assert_eq!(state.mode, Mode::Raw);
    }
}
