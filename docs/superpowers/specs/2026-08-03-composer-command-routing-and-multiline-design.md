# Composer command routing and multiline editing

Two defects share the composer and are fixed together.

1. Submitting a slash command that Agency does not own is rejected locally with
   "Unknown Agency command" instead of reaching the agent that owns it, so
   plugin commands and skills are unreachable unless picked from the completion
   overlay and then left unedited.
2. A multiline prompt renders sideways and the block cursor lands nowhere near
   the character it addresses, because the composer draws the prompt as a single
   horizontal row. Shift+Enter compounds this by inserting `\r`, which no motion
   helper recognizes as a line break.

## Current behavior

`submit_agent_input` (`crates/agency-desktop/src/main.rs:2036`) routes to an
agent only when `agent.command_provider` is set. That field is stamped in one
place, `AppEvent::CompleteSlashCommand` (`main.rs:1285`), when a completion is
accepted from the overlay, and is cleared by most subsequent edits
(`main.rs:1919`, `1956`, `1983`, `1999`). Every other prompt beginning with `/`
falls to `parse_slash_command` (`slash_commands.rs:325`), whose final arm is
`[command, ..] => Err(format!("Unknown Agency command: {command}"))`.

The prompt model is sound. `composer_motion_target` (`main.rs:5664`) implements
vertical motions, line motions, and word motions over a `String` with a byte
cursor, splitting lines on `'\n'`. Only the view and the newline character are
wrong.

## Part 1 — Submit-time command resolution

A new pure function in `crates/agency-desktop/src/slash_commands.rs`:

```rust
resolve_submission(
    catalog: &[SlashCommandCompletion],
    prompt: &str,
    active: Option<Provider>,
) -> Result<Submission, String>
```

It reads the first whitespace-delimited token of the trimmed prompt and returns:

- `Err(message)` — the token is one of Agency's own but its arguments are
  malformed. The caller shows this as a notice, exactly as it shows
  `parse_slash_command`'s errors today.
- `Ok(Submission::Agency(SlashCommand))` — the token is one of Agency's own and
  parsed. `parse_slash_command` still owns this case, including its usage errors
  for malformed `/mcp` and `/plugin install`, which surface as `Err` above. Its
  unknown-command arm changes from `Err` to `Ok(None)`, so an unrecognized token
  falls through to the catalog lookup below instead of being rejected. That arm
  is the whole defect.
- `Ok(Submission::Agent { provider, prompt })` — the token resolved to a catalog
  entry. `prompt` is the submitted text with the token rewritten to that entry's
  `invocation`, with arguments after the token preserved verbatim.
- `Ok(Submission::Verbatim)` — nothing resolved. The prompt goes to the focused
  agent exactly as typed.

### Matching

Matching is exact. The overlay's `matches()` stays a prefix match, which is
right for a live-filtered list and wrong for deciding what a submitted line
means: under prefix matching a submitted `/b` would silently fire whichever
command sorted first.

A token matches an entry when any of these hold:

- it equals the entry's `command` — `/superpowers:brainstorming`
- it equals the entry's `invocation` with trailing whitespace trimmed —
  `$letterhead`, which is how a Codex prompt stays reachable when typed in the
  form the overlay itself inserted, and which is not gated on a leading `/`
- it equals the entry's final `:`-delimited segment prefixed with `/` —
  `/brainstorming` resolves to `/superpowers:brainstorming`

Candidates are then narrowed: if any candidate belongs to the focused agent,
only those survive. Exactly one survivor wins. Zero survivors and more than one
survivor both yield `Verbatim` — an unresolvable or ambiguous name is the
focused agent's to report, not Agency's, which is the failure this spec exists
to remove.

### Provider neutrality

The `invocation` comparison is a plain string comparison against whatever the
entry carries. It must not become a match against a hardcoded set of sigils. A
future translator emitting `^skill ` then resolves with no harness change. See
Part 3.

### Submit path

`submit_agent_input` becomes:

- `command_provider` set — keep today's behavior. It is an exact record of a
  user choice and beats any inference.
- otherwise call `resolve_submission` and act on the three cases. The `Agent`
  case reuses the existing switch-and-resubmit path that `command_provider`
  already drives, including the image-attachment guard and
  `command_needs_agent_switch` (`main.rs:5611`).

### Tests

- `/superpowers:brainstorming`, `/brainstorming`, and `$letterhead` each resolve
  to the right entry and provider.
- The token is rewritten to the entry's `invocation` and trailing arguments
  survive: `/brainstorming an idea` sends `/superpowers:brainstorming an idea`.
- An unknown `/wat` yields `Verbatim` and produces no notice.
- Two providers offering the same name resolve to the focused agent's entry;
  two entries within one agent yield `Verbatim`.
- Agency's own commands still resolve to `Agency`, and `/mcp` with no arguments
  still returns its usage error.
- A fabricated catalog entry whose `invocation` uses a `^` sigil resolves, so a
  hardcoded sigil assumption fails a test rather than the next integration.

## Part 2 — Newline input

`crates/agency-desktop/src/keybindings.rs` gains an arm ahead of the
printable-text fallthrough:

```rust
Key::Named(Named::Enter) if modifiers.shift() => Action::AgentAppend("\n".to_owned()),
```

Today Shift+Enter falls through to `printable_text` (`keybindings.rs:1178`),
which returns the platform's text for the key — `"\r"` — which no motion helper
recognizes.

`insert_prompt_text` (`main.rs:5870`) normalizes `\r\n` and bare `\r` to `\n`.
This covers paste, the common way a multiline prompt actually arrives, and
establishes the invariant that the prompt model never holds a `\r`, enforced at
the single point where text enters it.

### Tests

- Shift+Enter in insert mode produces `Action::AgentAppend("\n")`.
- `insert_prompt_text("a\r\nb\rc")` leaves the model holding `"a\nb\nc"` with
  the cursor at the end.

## Part 3 — A provider-neutral resolution rule in `AGENTS.md`

`AGENTS.md` mentions provider neutrality once, at line 7, and only about
instructions and work products. Nothing binds the code. `CLAUDE.md` is a symlink
to `AGENTS.md`, so one edit covers both. Add a section in the voice of the
existing ones:

> ## Provider-neutral resolution
>
> - The harness must not hardcode any provider's surface syntax. Command sigils,
>   invocation grammar, prompt file formats, and naming conventions belong to
>   that provider's translator and reach the harness only as data on translator
>   API types, such as `AgentCommand.invocation`.
> - Agency owns one neutral surface the user types against. `/` is Agency's
>   command sigil no matter what an agent uses natively; the translator maps it
>   to and from the provider's native form.
> - Any code that resolves a user action to an agent — commands, skills,
>   prompts, MCP entries, sessions, worktrees — must decide from
>   translator-supplied data, never from a literal sigil or a `match` on which
>   provider it is. Adding an agent should mean adding a translator, not editing
>   resolution logic.
> - Cover each resolution path with a test using a fabricated provider whose
>   syntax matches no shipped agent, so a hardcoded assumption fails a test
>   instead of the next integration.

The third bullet is scoped to resolution and syntax, not to the existence of the
`Provider` enum. The harness legitimately matches on `Provider` outside
resolution — `client_id()` (`slash_commands.rs:123`) maps a provider to its
translator ID, and startup wires each agent's process. Opening the enum into a
registry is a separate change and belongs in its own spec.

## Part 4 — Multiline composer rendering

`composer_prompt` (`main.rs:5777`) splits into two functions so the interesting
half is testable.

### `composer_lines`

```rust
fn composer_lines(
    prompt: &str,
    cursor: usize,
    selection: Option<(usize, usize)>,
) -> Vec<Vec<PromptSpan>>
```

`PromptSpan` is `Cursor` or `Text { range: Range<usize>, selected: bool }`.

The prompt splits on `'\n'` with `split('\n')`, not `lines()`: `lines()` drops a
trailing empty line, so `"abc\n"` with the cursor at the end would have nowhere
to draw it. Within each line the function performs the boundary-window walk the
current code performs, clipped to that line's byte range, emitting the cursor
span on the one line whose range contains it. Line boundaries are unambiguous
because the `'\n'` occupies a byte: `cursor == line_end` is the end of line *n*,
`cursor == line_start` is the start of line *n+1*.

### `composer_prompt`

Maps each line to a `Row` of widgets as today and collects them into a `column`.
Each line `Row` carries a fixed height matching the block cursor's `17.0`, so a
blank interior line keeps its vertical space instead of collapsing. The
empty-prompt placeholder path is unchanged. Styling comes from the existing
`ui_theme::block_cursor()` and `ui_theme::text_selection()`; no new colors.

A selection spanning lines highlights the text on each line inside the range and
draws nothing for the newline itself, so a selection ending at a line break
stops at the last character.

### Tests

Against `composer_lines`, which is pure:

- The cursor lands on the correct line and column for a mid-document position.
- A trailing `'\n'` yields a final empty line carrying the cursor.
- A blank interior line yields an empty span list and still occupies a row.
- A selection spanning three lines marks a partial first line, a whole middle
  line, and a partial last line.

## Out of scope

- **Composer height cap.** The composer now grows vertically with the prompt,
  and nothing bounds it, so a long pasted prompt pushes the transcript off
  screen. Capping the height with an internal scroll carries its own
  scroll-follows-cursor question and is filed separately.
- **Soft wrapping of long lines.** A single line longer than the composer width
  still runs off the edge, as it does today. Wrapping needs either a custom
  widget or character-count measurement, and is a feature rather than part of
  this fix.
- **Replacing the composer with iced's `text_editor`.** It would supply
  multiline, wrapping, and mouse selection, but it owns its own cursor and edit
  actions, which conflicts with the requirement in `AGENTS.md` that composer
  state reduce from typed application events.
- **Opening the `Provider` enum into a registry.**
