# Composer Command Routing and Multiline Editing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a submitted slash command reach the agent that owns it instead of being rejected locally, and make a multiline prompt render with its cursor in the right place.

**Architecture:** Both defects live in `crates/agency-desktop`. The prompt model is already correct — a `String` with a byte cursor whose motion helpers split on `'\n'` — so the fixes are a new pure resolver consulted at submit time, a pure line-layout function the composer view renders from, and a newline normalization at the one point where text enters the model. Every new decision lands in a pure function with unit tests; the view and the submit path stay thin wrappers over them.

**Tech Stack:** Rust 2024, iced 0.14 (wgpu renderer), `cargo test` with in-file `#[cfg(test)] mod tests`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-03-composer-command-routing-and-multiline-design.md`. Read it before starting.
- The harness must not hardcode any provider's surface syntax. Sigil and invocation grammar reach the harness only as data on catalog entries. Comparisons against an entry's `insertion` are plain string comparisons, never a match against a known set of sigils.
- No new colors. Composer styling comes from the existing `ui_theme::block_cursor()` and `ui_theme::text_selection()`.
- Field-name trap: the spec says `invocation`, which is the field on `agency_translator_api::commands::AgentCommand`. `merge_catalog` (`crates/agency-desktop/src/slash_commands.rs:104`) copies it onto `SlashCommandCompletion` under the name **`insertion`**. Desktop code uses `insertion`.
- Verification for every task: `cargo test -p agency-desktop`, then `cargo clippy -p agency-desktop --all-targets`, then `cargo fmt --check`.
- Commit after every task. Branch is `composer-command-routing-and-multiline`.

## File Structure

- `crates/agency-desktop/src/keybindings.rs` — modified. Gains one match arm so Shift+Enter yields a newline action.
- `crates/agency-desktop/src/main.rs` — modified. Gains `normalize_newlines`, `PromptSpan`, `composer_lines`, and `route_agent_command`; `composer_prompt` and `submit_agent_input` are rewritten to delegate to them.
- `crates/agency-desktop/src/slash_commands.rs` — modified. Gains `Submission`, `resolve_submission`, and the private helpers `resolve_entry` and `names`; `parse_slash_command`'s unknown-command arm changes from `Err` to `Ok(None)`.
- `AGENTS.md` — modified. Gains a "Provider-neutral resolution" section. `CLAUDE.md` is a symlink to it and needs no edit.

Tasks 1 and 2 fix the multiline defect and are independent of tasks 3 and 4, which fix the routing defect. Within each pair, order matters.

---

### Task 1: Newline input

Shift+Enter currently falls through to `printable_text` (`keybindings.rs:1178`), which returns the platform's text for the Enter key — `"\r"`. Every motion helper in `composer_motion_target` (`main.rs:5664`) splits on `'\n'`, so that break is invisible to `Up`, `Down`, `LineStart`, and `LineEnd`. Paste has the same problem via `\r\n`.

`AgentView` cannot be constructed in a unit test — it requires a spawned `AgentSession` — so the normalization goes in a free function that `insert_prompt_text` calls, and the test targets the free function.

**Files:**
- Modify: `crates/agency-desktop/src/keybindings.rs:987` (add an arm), `crates/agency-desktop/src/keybindings.rs` tests module (starts line 1277)
- Modify: `crates/agency-desktop/src/main.rs:12` (import), `main.rs:5870` (`insert_prompt_text`), `main.rs` tests module (starts line 6544)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `fn normalize_newlines(text: &str) -> Cow<'_, str>` in `main.rs`. Task 2 relies on the invariant it establishes — the prompt model never holds a `'\r'`.

- [ ] **Step 1: Write the failing normalization test**

In `crates/agency-desktop/src/main.rs`, inside `mod tests` (starts line 6544), add:

```rust
/// Every motion helper splits lines on `'\n'`, so a `'\r'` that reaches the
/// model is a line break nothing can see. Normalizing at the single point
/// where text enters the prompt is what makes that invariant hold.
#[test]
fn text_entering_the_prompt_normalizes_every_line_break_to_a_newline() {
    assert_eq!(normalize_newlines("a\r\nb\rc"), "a\nb\nc");
    assert_eq!(normalize_newlines("already\nfine"), "already\nfine");
    assert_eq!(normalize_newlines("no breaks"), "no breaks");
}
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p agency-desktop text_entering_the_prompt_normalizes`
Expected: FAIL to compile with `cannot find function \`normalize_newlines\` in this scope`.

- [ ] **Step 3: Implement `normalize_newlines`**

In `crates/agency-desktop/src/main.rs`, add to the import block at line 12:

```rust
use std::borrow::Cow;
```

Then add the function next to the other prompt helpers, immediately above `fn clamped_prompt_cursor` (line 5642):

```rust
/// The prompt model treats `'\n'` as the only line break, because every motion
/// helper splits on it. Platforms hand us `"\r\n"` from a paste and `"\r"` from
/// the Enter key, so text is normalized at the one point where it enters the
/// model rather than at each of the places that read it.
fn normalize_newlines(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}
```

- [ ] **Step 4: Run it to make sure it passes**

Run: `cargo test -p agency-desktop text_entering_the_prompt_normalizes`
Expected: PASS.

- [ ] **Step 5: Route paste through it**

In `crates/agency-desktop/src/main.rs`, replace `insert_prompt_text` (line 5870):

```rust
    fn insert_prompt_text(&mut self, text: &str) {
        let text = normalize_newlines(text);
        self.delete_prompt_selection();
        let cursor = clamped_prompt_cursor(&self.prompt, self.prompt_cursor);
        self.prompt.insert_str(cursor, &text);
        self.prompt_cursor = cursor + text.len();
    }
```

- [ ] **Step 6: Write the failing Shift+Enter test**

In `crates/agency-desktop/src/keybindings.rs`, inside `mod tests` (starts line 1277), add next to `agent_composer_accepts_input_while_explorer_is_open` (line 2069):

```rust
/// Shift+Enter must produce a newline the motion helpers recognize. Left to
/// the printable-text fallthrough it produces the platform's text for the
/// key — `"\r"` — which nothing downstream treats as a line break.
#[test]
fn shift_enter_inserts_a_newline_rather_than_submitting() {
    let mut bindings = Keybindings {
        mode: Mode::Insert,
        ..Keybindings::default()
    };

    let action = bindings.handle_in_context(
        &Key::Named(Named::Enter),
        Physical::Code(Code::Enter),
        Modifiers::SHIFT,
        Some("\r"),
        DispatchContext::focused(KeybindingContext::Composer),
    );

    assert_eq!(action, Action::AgentAppend("\n".to_owned()));
    assert_eq!(bindings.mode_label(), "INSERT");
}
```

- [ ] **Step 7: Run it to make sure it fails**

Run: `cargo test -p agency-desktop shift_enter_inserts_a_newline`
Expected: FAIL with a left/right mismatch showing `AgentAppend("\r")`.

If it instead fails with `AgentSubmit`, an earlier handler is swallowing the modifier — read `handle_in_context` from line 960 down to the `Mode::Insert` match at line 982 to find which, and report it rather than working around it.

- [ ] **Step 8: Add the binding**

In `crates/agency-desktop/src/keybindings.rs`, in the `Mode::Insert` match (line 982), directly below the existing `AgentSubmit` arm:

```rust
                Key::Named(Named::Enter) if !modifiers.shift() => Action::AgentSubmit,
                // Shift+Enter would otherwise fall through to `printable_text`,
                // which returns the platform's text for the key — `"\r"` — and
                // no motion helper recognizes that as a line break.
                Key::Named(Named::Enter) => Action::AgentAppend("\n".to_owned()),
```

- [ ] **Step 9: Run the full suite**

Run: `cargo test -p agency-desktop`
Expected: PASS, no regressions.

- [ ] **Step 10: Lint and format**

Run: `cargo clippy -p agency-desktop --all-targets` then `cargo fmt --check`
Expected: no warnings, no diff.

- [ ] **Step 11: Commit**

```bash
git add crates/agency-desktop/src/keybindings.rs crates/agency-desktop/src/main.rs
git commit -m "fix(desktop): give the composer a newline it can see

Shift+Enter fell through to the printable-text path and inserted the
platform's \"\\r\", which no motion helper recognizes as a line break.
Paste had the same problem via \"\\r\\n\". Normalize at the single point
where text enters the prompt model."
```

---

### Task 2: Multiline composer rendering

`composer_prompt` (`main.rs:5777`) builds one horizontal `Row` of text segments split at the cursor and selection boundaries. Newlines inside a segment never stack into lines, so a multiline prompt lays out sideways and the block cursor lands nowhere near the character it addresses.

Split the function: a pure layout half that is tested, and a thin view half that is not.

**Files:**
- Modify: `crates/agency-desktop/src/main.rs:12` (import), `main.rs:5777` (`composer_prompt`), `main.rs` tests module (starts line 6544)

**Interfaces:**
- Consumes: the invariant from Task 1 that the prompt holds no `'\r'`.
- Produces:
  - `enum PromptSpan { Cursor, Text { range: Range<usize>, selected: bool } }`
  - `fn composer_lines(prompt: &str, cursor: usize, selection: Option<(usize, usize)>) -> Vec<Vec<PromptSpan>>`
  - `const COMPOSER_LINE_HEIGHT: f32 = 17.0;`

- [ ] **Step 1: Write the failing layout tests**

In `crates/agency-desktop/src/main.rs`, inside `mod tests`, add:

```rust
/// The cursor has to sit on the line and column the byte offset names. Drawn
/// from a single row, as it was, it landed at a horizontal offset that ignored
/// every line break before it.
#[test]
fn the_composer_cursor_lands_on_its_own_line_and_column() {
    assert_eq!(
        composer_lines("abc\ndef", 5, None),
        vec![
            vec![PromptSpan::Text {
                range: 0..3,
                selected: false
            }],
            vec![
                PromptSpan::Text {
                    range: 4..5,
                    selected: false
                },
                PromptSpan::Cursor,
                PromptSpan::Text {
                    range: 5..7,
                    selected: false
                },
            ],
        ]
    );
}

/// `str::lines` drops a trailing empty line, which would leave the cursor
/// after a final newline with nowhere to draw.
#[test]
fn a_trailing_newline_keeps_a_final_line_for_the_cursor() {
    assert_eq!(
        composer_lines("abc\n", 4, None),
        vec![
            vec![PromptSpan::Text {
                range: 0..3,
                selected: false
            }],
            vec![PromptSpan::Cursor],
        ]
    );
}

#[test]
fn a_blank_interior_line_still_occupies_a_row() {
    let lines = composer_lines("a\n\nb", 0, None);

    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines[0],
        vec![
            PromptSpan::Cursor,
            PromptSpan::Text {
                range: 0..1,
                selected: false
            }
        ]
    );
    assert!(lines[1].is_empty());
}

/// A selection crossing line breaks has to mark a partial first line, a whole
/// middle line, and a partial last line, with the newline itself drawing
/// nothing.
#[test]
fn a_selection_spanning_lines_marks_each_line_it_covers() {
    assert_eq!(
        composer_lines("one\ntwo\nthree", 9, Some((2, 9))),
        vec![
            vec![
                PromptSpan::Text {
                    range: 0..2,
                    selected: false
                },
                PromptSpan::Text {
                    range: 2..3,
                    selected: true
                },
            ],
            vec![PromptSpan::Text {
                range: 4..7,
                selected: true
            }],
            vec![
                PromptSpan::Text {
                    range: 8..9,
                    selected: true
                },
                PromptSpan::Cursor,
                PromptSpan::Text {
                    range: 9..13,
                    selected: false
                },
            ],
        ]
    );
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p agency-desktop composer_lines`
Expected: FAIL to compile — `composer_lines` and `PromptSpan` do not exist.

- [ ] **Step 3: Implement the pure layout**

In `crates/agency-desktop/src/main.rs`, add to the import block at line 12:

```rust
use std::ops::Range;
```

Then add directly above `fn composer_prompt` (line 5777):

```rust
/// The height of one composer line, matched to the block cursor so a blank
/// line keeps its vertical space instead of collapsing.
const COMPOSER_LINE_HEIGHT: f32 = 17.0;

/// One drawn piece of a composer line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptSpan {
    Cursor,
    Text { range: Range<usize>, selected: bool },
}

/// The composer laid out one row per line.
///
/// Splitting uses `split('\n')` rather than `lines()`, which drops a trailing
/// empty line and would leave a cursor after a final newline with nowhere to
/// draw. Line boundaries stay unambiguous because the `'\n'` occupies a byte:
/// `cursor == line_end` is the end of one line, and `cursor == line_start` is
/// the start of the next, so exactly one line claims the cursor.
fn composer_lines(
    prompt: &str,
    cursor: usize,
    selection: Option<(usize, usize)>,
) -> Vec<Vec<PromptSpan>> {
    let mut lines = Vec::new();
    let mut line_start = 0;
    for line in prompt.split('\n') {
        let line_end = line_start + line.len();
        let span = line_start..=line_end;
        let mut boundaries = vec![line_start, line_end];
        if span.contains(&cursor) {
            boundaries.push(cursor);
        }
        if let Some((start, end)) = selection {
            boundaries.extend([start, end].into_iter().filter(|bound| span.contains(bound)));
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        let mut spans = Vec::new();
        for window in boundaries.windows(2) {
            let (start, end) = (window[0], window[1]);
            if start == cursor {
                spans.push(PromptSpan::Cursor);
            }
            let selected = selection.is_some_and(|(selection_start, selection_end)| {
                start >= selection_start && end <= selection_end
            });
            spans.push(PromptSpan::Text {
                range: start..end,
                selected,
            });
        }
        // A cursor at the very end of a line closes no window, so it is pushed
        // here rather than by the loop above.
        if cursor == line_end {
            spans.push(PromptSpan::Cursor);
        }
        lines.push(spans);
        line_start = line_end + 1;
    }
    lines
}
```

- [ ] **Step 4: Run them to make sure they pass**

Run: `cargo test -p agency-desktop composer_lines`
Expected: PASS, all four.

- [ ] **Step 5: Render from the layout**

In `crates/agency-desktop/src/main.rs`, replace the body of `composer_prompt` from its `let selection = agent.prompt_selection();` line (5803) through `prompt.into()` (5841) with:

```rust
    let mut prompt = iced::widget::Column::new();
    for spans in composer_lines(&agent.prompt, agent.prompt_cursor, agent.prompt_selection()) {
        let mut line = iced::widget::Row::new()
            .spacing(0)
            .height(Length::Fixed(COMPOSER_LINE_HEIGHT));
        for span in spans {
            line = line.push(match span {
                PromptSpan::Cursor => cursor(),
                PromptSpan::Text { range, selected } => {
                    container(text(&agent.prompt[range]).font(Font::MONOSPACE).size(14))
                        .style(move |_theme: &Theme| {
                            if selected {
                                ui_theme::text_selection()
                            } else {
                                container::Style::default()
                            }
                        })
                        .into()
                }
            });
        }
        prompt = prompt.push(line);
    }
    prompt.into()
```

The empty-prompt placeholder branch above it (lines 5795-5802) stays exactly as it is.

Also change the cursor closure's height (line 5781) to use the new constant:

```rust
            .height(Length::Fixed(COMPOSER_LINE_HEIGHT))
```

- [ ] **Step 6: Run the full suite and build**

Run: `cargo test -p agency-desktop`
Expected: PASS.

- [ ] **Step 7: Lint and format**

Run: `cargo clippy -p agency-desktop --all-targets` then `cargo fmt --check`
Expected: no warnings, no diff.

- [ ] **Step 8: See it in the real app**

Run: `cargo run -p agency-desktop`
Type `one`, Shift+Enter, `two`, Shift+Enter, `three`. Confirm three stacked lines, the block cursor after `three`, and that `k` in NORMAL mode moves it up a line while keeping its column. Paste a multiline block and confirm it stacks too. Do not mark this step done on a clean build alone.

- [ ] **Step 9: Commit**

```bash
git add crates/agency-desktop/src/main.rs
git commit -m "fix(desktop): draw the composer one row per line

The prompt was drawn as a single horizontal row, so a multiline prompt
laid out sideways and the block cursor ignored every line break before
it. Layout is now a pure function the view renders from."
```

---

### Task 3: Submit-time command resolution

`parse_slash_command` (`slash_commands.rs:325`) ends with `[command, ..] => Err(format!("Unknown Agency command: {command}"))`. That arm is the whole defect: every submitted `/command` Agency does not own is rejected locally instead of reaching the agent that owns it.

This task adds the resolver and the repository rule that keeps it provider-neutral. It does not touch the submit path — that is Task 4 — so this task's deliverable is a tested pure function with no caller yet.

**Files:**
- Modify: `crates/agency-desktop/src/slash_commands.rs:339` (the unknown arm), and add `Submission`/`resolve_submission`/`resolve_entry`/`names` above `pub fn parse_slash_command` (line 325)
- Modify: `crates/agency-desktop/src/slash_commands.rs:759` (replace the obsolete test) and its tests module (starts line 466)
- Modify: `AGENTS.md` (insert a section after the "Agency harness" section, before `## UI color and theming` at line 18)

**Interfaces:**
- Consumes: `SlashCommandCompletion { command, description, insertion, provider, built_in }` and `merge_catalog`, both already in this file.
- Produces:
  - `pub enum Submission { Agency(SlashCommand), Agent { provider: Provider, prompt: String }, Verbatim }`
  - `pub fn resolve_submission(catalog: &[SlashCommandCompletion], prompt: &str, active: Option<Provider>) -> Result<Submission, String>`

  Task 4 calls exactly this signature.

- [ ] **Step 1: Write the failing resolver tests**

In `crates/agency-desktop/src/slash_commands.rs`, inside `mod tests` (starts line 466), add this helper next to `provider_completion` (line 799):

```rust
    /// `provider_completion` derives its insertion from the command, which is
    /// right for Claude-shaped entries and wrong for any agent whose sigil is
    /// not `/`. Resolution has to be tested against both.
    fn invoked_completion(
        command: &str,
        insertion: &str,
        provider: Provider,
    ) -> SlashCommandCompletion {
        SlashCommandCompletion {
            command: command.to_owned(),
            description: String::new(),
            insertion: insertion.to_owned(),
            provider: Some(provider),
            built_in: false,
        }
    }
```

Then add these tests:

```rust
    #[test]
    fn a_typed_command_resolves_to_the_agent_that_owns_it() {
        let catalog = vec![provider_completion(
            "/superpowers:brainstorming",
            Provider::Claude,
        )];

        assert_eq!(
            resolve_submission(
                &catalog,
                "/superpowers:brainstorming",
                Some(Provider::Codex)
            ),
            Ok(Submission::Agent {
                provider: Provider::Claude,
                prompt: "/superpowers:brainstorming".to_owned(),
            })
        );
    }

    /// Indexing exists so the short name a user remembers finds the command.
    /// The namespace and the entry's own invocation are filled in on the way
    /// out, and everything typed after the token is left alone.
    #[test]
    fn a_bare_namespace_segment_resolves_and_arguments_survive() {
        let catalog = vec![provider_completion(
            "/superpowers:brainstorming",
            Provider::Claude,
        )];

        assert_eq!(
            resolve_submission(&catalog, "/brainstorming an idea", Some(Provider::Claude)),
            Ok(Submission::Agent {
                provider: Provider::Claude,
                prompt: "/superpowers:brainstorming an idea".to_owned(),
            })
        );
    }

    /// The harness must not know any provider's sigil. `$` is Codex's today;
    /// `^` belongs to no shipped agent, and resolving it is what proves the
    /// comparison is against catalog data rather than a hardcoded set.
    #[test]
    fn a_provider_sigil_resolves_without_the_harness_knowing_it() {
        let catalog = vec![
            invoked_completion("/letterhead", "$letterhead ", Provider::Codex),
            invoked_completion("/blueprint", "^blueprint ", Provider::Claude),
        ];

        assert_eq!(
            resolve_submission(&catalog, "$letterhead", Some(Provider::Codex)),
            Ok(Submission::Agent {
                provider: Provider::Codex,
                prompt: "$letterhead".to_owned(),
            })
        );
        assert_eq!(
            resolve_submission(&catalog, "^blueprint", Some(Provider::Claude)),
            Ok(Submission::Agent {
                provider: Provider::Claude,
                prompt: "^blueprint".to_owned(),
            })
        );
    }

    /// The defect this replaces. An unresolvable name is the focused agent's
    /// to report, not Agency's.
    #[test]
    fn an_unresolvable_command_reaches_the_agent_instead_of_a_notice() {
        assert_eq!(
            resolve_submission(&[], "/wat", Some(Provider::Claude)),
            Ok(Submission::Verbatim)
        );
    }

    #[test]
    fn the_focused_agent_wins_a_tie_and_a_tie_within_one_agent_falls_through() {
        let across_agents = vec![
            provider_completion("/review", Provider::Claude),
            provider_completion("/review", Provider::Codex),
        ];
        assert_eq!(
            resolve_submission(&across_agents, "/review", Some(Provider::Codex)),
            Ok(Submission::Agent {
                provider: Provider::Codex,
                prompt: "/review".to_owned(),
            })
        );

        // Two plugins under one agent: guessing between them would send the
        // wrong command, so the agent gets the text and reports it itself.
        let within_one_agent = vec![
            provider_completion("/one:review", Provider::Claude),
            provider_completion("/two:review", Provider::Claude),
        ];
        assert_eq!(
            resolve_submission(&within_one_agent, "/review", Some(Provider::Claude)),
            Ok(Submission::Verbatim)
        );
    }

    #[test]
    fn agency_keeps_its_own_commands_and_its_usage_errors() {
        let catalog = agency_commands();

        assert_eq!(
            resolve_submission(&catalog, "/init", Some(Provider::Claude)),
            Ok(Submission::Agency(SlashCommand::Init))
        );
        assert_eq!(
            resolve_submission(&catalog, "/mcp", Some(Provider::Claude)),
            Err("Usage: /mcp add <name>".to_owned())
        );
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p agency-desktop resolve_submission`
Expected: FAIL to compile — `resolve_submission` and `Submission` do not exist.

- [ ] **Step 3: Implement the resolver**

In `crates/agency-desktop/src/slash_commands.rs`, add directly above `pub fn parse_slash_command` (line 325):

```rust
/// What a submitted prompt turns out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Submission {
    /// One of Agency's own commands, for the harness to run.
    Agency(SlashCommand),
    /// A command the catalog attributes to an agent. `prompt` carries the
    /// entry's own insertion in place of the token that was typed, so an agent
    /// receives the command in the form it expects regardless of how the user
    /// reached it.
    Agent { provider: Provider, prompt: String },
    /// Nothing Agency recognizes. The focused agent gets the text as typed and
    /// reports on it itself.
    Verbatim,
}

/// What to do with `prompt` when it is submitted.
///
/// Agency's own commands come first, then the catalog. Anything left over goes
/// to the focused agent untouched: Agency rejecting a name it does not know
/// would make every command an agent gained after the last catalog load
/// unreachable, which is the failure this exists to remove.
pub fn resolve_submission(
    catalog: &[SlashCommandCompletion],
    prompt: &str,
    active: Option<Provider>,
) -> Result<Submission, String> {
    let prompt = prompt.trim();
    if let Some(command) = parse_slash_command(prompt)? {
        return Ok(Submission::Agency(command));
    }
    let Some(token) = prompt.split_whitespace().next() else {
        return Ok(Submission::Verbatim);
    };
    let Some(entry) = resolve_entry(catalog, token, active) else {
        return Ok(Submission::Verbatim);
    };
    // Agency's own rows carry no provider and were handled above; one reaching
    // here has nobody to route to.
    let Some(provider) = entry.provider else {
        return Ok(Submission::Verbatim);
    };

    let arguments = prompt[token.len()..].trim_start();
    let invocation = entry.insertion.trim_end();
    let prompt = if arguments.is_empty() {
        invocation.to_owned()
    } else {
        format!("{invocation} {arguments}")
    };
    Ok(Submission::Agent { provider, prompt })
}

/// The one catalog entry `token` names, if exactly one survives.
///
/// Matching is exact, unlike the overlay's `matches`. A prefix match is right
/// for a live-filtered list and wrong for deciding what a submitted line means:
/// under it a submitted `/b` would silently fire whichever command sorted
/// first.
fn resolve_entry<'a>(
    catalog: &'a [SlashCommandCompletion],
    token: &str,
    active: Option<Provider>,
) -> Option<&'a SlashCommandCompletion> {
    let mut candidates = catalog
        .iter()
        .filter(|entry| names(entry, token))
        .collect::<Vec<_>>();
    // A name both agents offer belongs to the one being talked to; routing it
    // elsewhere would switch agents behind a command that needed no switch.
    if let Some(active) = active
        && candidates.iter().any(|entry| entry.provider == Some(active))
    {
        candidates.retain(|entry| entry.provider == Some(active));
    }
    match candidates.as_slice() {
        [entry] => Some(entry),
        _ => None,
    }
}

/// Whether `token` names `entry` exactly.
///
/// Three forms count: the catalog's own `/namespace:name`, the entry's
/// insertion as the overlay itself would have typed it, and the bare trailing
/// segment under Agency's `/` sigil. The insertion comparison is a plain string
/// comparison against whatever the entry carries, never a match on a known set
/// of sigils, so a translator that invokes with `^` resolves with no change
/// here.
fn names(entry: &SlashCommandCompletion, token: &str) -> bool {
    if entry.command == token || entry.insertion.trim_end() == token {
        return true;
    }
    let Some(bare) = token.strip_prefix('/') else {
        return false;
    };
    entry
        .command
        .rsplit(':')
        .next()
        .is_some_and(|segment| segment == bare)
}
```

- [ ] **Step 4: Stop rejecting unknown commands**

In the same file, change the unknown arm of `parse_slash_command` (line 339) from:

```rust
        [command, ..] => Err(format!("Unknown Agency command: {command}")),
```

to:

```rust
        // Not Agency's. `resolve_submission` takes it from here: the catalog
        // may own it, and if nothing does, the focused agent gets it verbatim.
        _ => Ok(None),
```

Delete the now-unreachable `[] => Ok(None),` arm below it.

- [ ] **Step 5: Replace the test that pinned the old behavior**

Replace `unknown_commands_are_rejected_locally` (line 759) with:

```rust
    /// The inverse of the behavior this replaces: Agency parsing a command it
    /// does not own must yield to the catalog rather than reject it.
    #[test]
    fn an_unknown_command_is_left_for_the_catalog_to_resolve() {
        assert_eq!(parse_slash_command("/wat"), Ok(None));
    }
```

- [ ] **Step 6: Run the suite**

Run: `cargo test -p agency-desktop`
Expected: PASS. `submit_agent_input` still has no caller for the resolver, which is Task 4; if the compiler warns that `resolve_submission` is unused, that is expected until then.

- [ ] **Step 7: Write the repository rule**

In `AGENTS.md`, insert this section after the "Agency harness" section and before `## UI color and theming` (line 18):

```markdown
## Provider-neutral resolution

- The harness must not hardcode any provider's surface syntax. Command sigils,
  invocation grammar, prompt file formats, and naming conventions belong to
  that provider's translator and reach the harness only as data on translator
  API types, such as `AgentCommand.invocation`.
- Agency owns one neutral surface the user types against. `/` is Agency's
  command sigil no matter what an agent uses natively; the translator maps it
  to and from the provider's native form.
- Any code that resolves a user action to an agent — commands, skills, prompts,
  MCP entries, sessions, worktrees — must decide from translator-supplied data,
  never from a literal sigil or a `match` on which provider it is. Adding an
  agent should mean adding a translator, not editing resolution logic.
- Cover each resolution path with a test using a fabricated provider whose
  syntax matches no shipped agent, so a hardcoded assumption fails a test
  instead of the next integration.

This is scoped to resolution and syntax, not to the existence of the `Provider`
enum, which the harness legitimately matches on elsewhere — mapping a provider
to its translator ID, or wiring each agent's process at startup.
```

Do not edit `CLAUDE.md`; it is a symlink to `AGENTS.md`.

- [ ] **Step 8: Lint and format**

Run: `cargo clippy -p agency-desktop --all-targets` then `cargo fmt --check`
Expected: no warnings other than the unused-function one noted in Step 6, no diff.

- [ ] **Step 9: Commit**

```bash
git add crates/agency-desktop/src/slash_commands.rs AGENTS.md
git commit -m "feat(desktop): resolve submitted commands against the catalog

Agency rejected every submitted slash command it did not own, so plugin
commands and skills were unreachable unless picked from the overlay and
left unedited. Resolution is exact, prefers the focused agent, and falls
through to the agent rather than guessing. Record the provider-neutrality
rule the resolver depends on."
```

---

### Task 4: Wire the submit path to the resolver

`submit_agent_input` (`main.rs:2036`) routes to an agent only when `agent.command_provider` is set, which happens in one place — `AppEvent::CompleteSlashCommand` (`main.rs:1285`) — and is cleared by most subsequent edits (`main.rs:1919`, `1956`, `1983`, `1999`). This task makes the resolver the general path and keeps `command_provider` as an exact record of a user choice that beats inference.

The switch-and-submit body currently inlined in the `command_provider` branch becomes a method both callers share, because an accepted completion and a resolved submission must route identically.

**Files:**
- Modify: `crates/agency-desktop/src/main.rs:56` (import), `main.rs:2036-2092` (`submit_agent_input`), and add `route_agent_command` beside it
- Modify: `crates/agency-desktop/src/main.rs` tests module (starts line 6544)

**Interfaces:**
- Consumes: `slash_commands::resolve_submission` and `slash_commands::Submission` from Task 3, and the existing `command_needs_agent_switch(active: Provider, command: Provider) -> bool` (`main.rs:5611`).
- Produces: `fn route_agent_command(&mut self, provider: Provider, prompt: String)` on `Agency`.

- [ ] **Step 1: Write the failing routing test**

`Agency` cannot be constructed in a unit test — it owns spawned agent sessions — so the test pins the contract between the two pure pieces the submit path composes, in the manner of the existing `the_catalog_stamps_the_provider_that_routing_depends_on` (line 7179).

In `crates/agency-desktop/src/main.rs`, inside `mod tests`, add next to that test:

```rust
/// Resolution and routing have to agree. The provider a resolved submission
/// names is the one `command_needs_agent_switch` is asked about, so if they
/// ever disagreed a command would either strand on the wrong agent or churn
/// between two. This also pins that the rewritten prompt is what gets sent.
#[test]
fn a_resolved_submission_names_the_agent_that_routing_will_switch_to() {
    let catalog = slash_commands::merge_catalog(vec![(
        Provider::Claude,
        agent_command("superpowers:brainstorming"),
    )]);

    let resolved = slash_commands::resolve_submission(
        &catalog,
        "/brainstorming an idea",
        Some(Provider::Codex),
    )
    .expect("a catalog command is not an Agency usage error");

    let slash_commands::Submission::Agent { provider, prompt } = resolved else {
        panic!("a catalog command must resolve to the agent that owns it");
    };
    assert_eq!(provider, Provider::Claude);
    assert_eq!(prompt, "/superpowers:brainstorming an idea");
    assert!(command_needs_agent_switch(Provider::Codex, provider));
    assert!(!command_needs_agent_switch(Provider::Claude, provider));
}
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p agency-desktop a_resolved_submission_names_the_agent`
Expected: FAIL to compile — `slash_commands::Submission` is not imported into the test scope, or `resolve_submission` is not yet reachable by that path.

If Task 3 is already merged and the module path resolves, this test may pass immediately. That is fine — it is a contract test across two tasks. Confirm it passes for the right reason by reading its assertions, then continue to Step 3, whose behavior has no test of its own.

- [ ] **Step 3: Extract the shared routing method**

In `crates/agency-desktop/src/main.rs`, add directly above `fn submit_agent_input` (line 2036):

```rust
    /// Sends `prompt` to `provider`, switching agents first when it belongs to
    /// the one that is not focused. An accepted completion and a resolved
    /// submission share this, because they must route identically: the only
    /// difference between them is how the provider was learned.
    fn route_agent_command(&mut self, provider: Provider, prompt: String) {
        if self
            .active_agent()
            .is_some_and(|agent| command_needs_agent_switch(agent.session.provider(), provider))
        {
            self.start_agent(provider);
            if !self
                .active_agent()
                .is_some_and(|agent| agent.session.provider() == provider)
            {
                return;
            }
        }
        // Set unconditionally: a resolved submission rewrites the token to the
        // entry's own insertion, so the composer's text is not what should be
        // sent even when no switch happened.
        if let Some(agent) = self.active_agent_mut() {
            agent.prompt = prompt;
            agent.prompt_cursor = agent.prompt.len();
            agent.prompt_selection_anchor = None;
            agent.command_provider = Some(provider);
        }
        let submitted = self.active_agent_mut().and_then(AgentView::submit);
        if let Some((provider, id, name)) = submitted
            && let Err(error) = self.sessions.name_if_missing(provider, &id, name)
        {
            self.notice = Some(error);
        }
    }
```

- [ ] **Step 4: Rewrite the submit path**

Replace the body of `submit_agent_input` from `if let Some(provider) = command_provider {` (line 2043) through the closing `}` of the `match parse_slash_command(&prompt)` block (line 2092) with:

```rust
        if let Some(provider) = command_provider {
            if has_images {
                self.notice = Some(
                    "Agent slash commands and skills cannot include image attachments".to_owned(),
                );
                return;
            }
            self.route_agent_command(provider, prompt);
            return;
        }

        let active = self.active_agent().map(|agent| agent.session.provider());
        match resolve_submission(&self.slash_command_catalog, &prompt, active) {
            Err(error) => self.notice = Some(error),
            Ok(Submission::Agency(_)) if has_images => {
                self.notice = Some("Slash commands cannot include image attachments".to_owned());
            }
            Ok(Submission::Agency(command)) => {
                if let Err(error) = self.run_slash_command(command) {
                    self.notice = Some(error);
                    return;
                }
                if let Some(agent) = self.active_agent_mut() {
                    agent.clear_prompt();
                }
            }
            Ok(Submission::Agent { provider, prompt }) => {
                if has_images {
                    self.notice = Some(
                        "Agent slash commands and skills cannot include image attachments"
                            .to_owned(),
                    );
                    return;
                }
                self.route_agent_command(provider, prompt);
            }
            Ok(Submission::Verbatim) => {
                let submitted = self.active_agent_mut().and_then(AgentView::submit);
                if let Some((provider, id, name)) = submitted
                    && let Err(error) = self.sessions.name_if_missing(provider, &id, name)
                {
                    self.notice = Some(error);
                }
            }
        }
```

Then replace the `slash_commands` import block (lines 56-61) with this. `parse_slash_command` goes: line 2073 was its only caller in `main.rs`, and the resolver calls it internally now.

```rust
use slash_commands::{
    ComposerState, INIT_AGENT_PROMPT, SlashCommand, SlashCommandCompletion, SlashCompletionState,
    Submission, TabCompletion, agency_commands, completion_count, discover_agent_commands,
    initialize_workspace, load_codex_mcp, merge_catalog, resolve_submission,
    slash_command_completions, tab_completion,
};
```

- [ ] **Step 5: Run the full suite**

Run: `cargo test -p agency-desktop`
Expected: PASS, no regressions.

- [ ] **Step 6: Lint and format**

Run: `cargo clippy -p agency-desktop --all-targets` then `cargo fmt --check`
Expected: no warnings, no diff. The unused-function warning from Task 3 is gone now that the resolver has a caller.

- [ ] **Step 7: See it in the real app**

Run: `cargo run -p agency-desktop`

Confirm each of these, which is the whole point of the change:

1. Type `/superpowers:brainstorming` by hand — no completion accepted — and submit. It reaches the agent instead of showing "Unknown Agency command".
2. Type `/brainstorming` and submit. Same, with the namespace filled in.
3. Accept a completion from the overlay, edit the arguments after it, and submit. It still routes rather than erroring — this is the path that broke.
4. Submit `/wat`. The agent responds; Agency shows no notice.
5. Submit `/init` and `/mcp` with no arguments. The first runs, the second shows its usage error.

- [ ] **Step 8: Commit**

```bash
git add crates/agency-desktop/src/main.rs
git commit -m "fix(desktop): route submitted commands through the resolver

An edited or hand-typed slash command was rejected locally because
routing depended on a provider stamped only when a completion was
accepted. Submission now resolves against the catalog, and an accepted
completion and a resolved one share one routing path."
```

---

## Verification

After Task 4, from a clean tree:

```bash
cargo test -p agency-desktop
cargo clippy -p agency-desktop --all-targets
cargo fmt --check
```

All three must be clean before the branch is considered done. The manual checks in Task 2 Step 8 and Task 4 Step 7 are not optional — both defects are user-visible behaviors that no unit test in this plan observes end to end.

## Out of scope

Carried from the spec, not to be implemented here:

- Capping the composer's height with an internal scroll. The composer now grows with the prompt, and a long pasted prompt will push the transcript off screen.
- Soft wrapping lines longer than the composer width.
- Replacing the composer with iced's `text_editor`.
- Opening the `Provider` enum into a registry.
