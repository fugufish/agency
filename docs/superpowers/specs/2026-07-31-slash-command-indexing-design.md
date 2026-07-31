# Slash command indexing across providers

## Problem

Agency's slash command catalog is built by `slash_command_catalog` in
`crates/agency-desktop/src/slash_commands.rs`. It scans six fixed directories:
`~/.codex/skills`, `<workspace>/.codex/skills`, `~/.claude/skills`,
`<workspace>/.claude/skills`, `~/.claude/commands`, and
`<workspace>/.claude/commands`. It also carries three Agency commands and two
hardcoded Claude built-ins.

That misses most of what the agents can actually run:

- **Plugin-provided skills and commands are invisible.** Claude Code installs
  plugins under `~/.claude/plugins/cache/<marketplace>/<plugin>/<version>/`,
  each with its own `skills/` and `commands/` directories. None are indexed.
- **Enablement and version state are ignored.** `~/.claude/settings.json`
  records `enabledPlugins`, and `installed_plugins.json` records which version
  of each plugin is live. A plugin can have several versions on disk at once.
- **Namespacing is absent.** Plugin entries are invoked as
  `/superpowers:brainstorming`, not `/brainstorming`.
- **Codex plugins are invisible.** Codex installs under
  `~/.codex/plugins/cache/<marketplace>/<plugin>/<version>/` and declares its
  skills path in `.codex-plugin/plugin.json`. Codex prompts in
  `~/.codex/prompts` are also unindexed.
- **Descriptions are wrong.** `push_agent_completion` takes the first line that
  is neither empty nor `---`. Every real skill and command file opens with YAML
  frontmatter, so this yields `name: brainstorming` instead of the description
  that sits two lines below it.
- **Provider knowledge lives in the desktop crate.** Path layouts, settings
  precedence, and the `$name` versus `/name` invocation difference are all
  hardcoded in a UI module.

## Approach

Discovery moves into the agent-level translators, which already own
provider-specific knowledge and are registered per `ClientId` in
`crates/agency-translators/src/lib.rs`. Discovery stays filesystem-based rather
than querying a running agent: it is fast, works before any agent starts, and
is testable against fixture directories.

### Layers

**`agency-translator-api`** defines the neutral vocabulary and a trait:

```rust
pub trait CommandCatalog: Send + Sync {
    fn commands(&self, workspace: &Path) -> Vec<AgentCommand>;
}

pub struct AgentCommand {
    /// Fully qualified, without the leading slash: "superpowers:brainstorming".
    pub name: String,
    pub description: String,
    /// Exactly what gets typed at the agent, including its sigil.
    pub invocation: String,
    pub argument_hint: Option<String>,
    pub origin: CommandOrigin,
}

pub enum CommandOrigin {
    BuiltIn,
    Personal,
    Project,
    Plugin { plugin: String, marketplace: String },
}
```

It also ships a `discovery` module of mechanical helpers that both translators
call: YAML frontmatter parsing, walking a `commands/` tree into namespaced
entries, and walking a `skills/` tree of `SKILL.md` directories.

**`agency-translators`** implements `CommandCatalog` per provider.
`ClaudeTranslator` owns Claude's directory layout, plugin cache, settings
precedence, and built-in list. `CodexTranslator` owns Codex's layout, prompt
directory, plugin manifests, and `$name` invocation form. Both are registered
alongside the existing translators in `built_in()`, so a third provider means
implementing one trait rather than editing the desktop crate.

Claude discovery lives in a sibling module, `claude/commands.rs`, because
`claude.rs` is already 1063 lines.

**`agency-desktop`** keeps Agency's own commands (`/init`, `/mcp add`,
`/plugin install`), the `SlashCompletionState` machine, matching, Tab
completion, and rendering. `slash_command_catalog` becomes a merge of Agency's
commands with each configured provider's catalog, mapped into the existing
`SlashCommandCompletion` shape: `AgentCommand::name` becomes `command`,
`invocation` becomes `insertion`, and `origin` supplies `built_in` for the
existing badge. Names that collide across two providers stay as separate rows,
as they do today, because their insertions differ.

## Discovery rules

### Claude

Sources in precedence order; a later source shadows an earlier one holding an
identical fully qualified name.

| Source | Location | Name |
|---|---|---|
| Built-ins | hardcoded list | `/review`, `/security-review`, … |
| Plugin skills | `<installPath>/skills/<name>/SKILL.md` | `/<plugin>:<name>` |
| Plugin commands | `<installPath>/commands/**/<name>.md` | `/<plugin>:<name>`, nested directories add segments |
| Personal | `~/.claude/{skills,commands}` | `/<name>`, nested directories give `/<dir>:<name>` |
| Project | `<workspace>/.claude/{skills,commands}` | as above |

`<installPath>` is read from `~/.claude/plugins/installed_plugins.json`, which
records the resolved install path per plugin. Reading it avoids guessing which
of several on-disk versions is live.

Enablement comes from `enabledPlugins` across the settings chain — user
`settings.json`, user `settings.local.json`, project `.claude/settings.json`,
project `.claude/settings.local.json` — with later files overriding earlier
ones. A plugin absent from every file counts as enabled. An explicit `false`
drops all of that plugin's entries.

Claude Code's documented settings precedence is authoritative here. Confirm it
against the current documentation during implementation rather than inferring
it from a single local file.

### Codex

- `~/.codex/skills` and `<workspace>/.codex/skills`
- `~/.codex/prompts/*.md`
- Plugin skills from `~/.codex/plugins/cache/<marketplace>/<plugin>/<version>/`,
  reading each `.codex-plugin/plugin.json` and following its declared `skills`
  path rather than assuming a fixed directory

Codex invocations keep the existing `$name` form.

Local ground truth for Codex is thin: `~/.codex/prompts` and `~/.codex/skills`
are both empty, and the one installed plugin ships only skills. Nested prompt
naming and whether Codex namespaces plugin skills are unverified. Confirm both
against Codex's documentation before implementing that half; if namespacing
cannot be confirmed, index Codex plugin skills unnamespaced and note the
limitation.

### Descriptions and arguments

Descriptions come from the frontmatter `description` key. When it is missing,
fall back to the first prose line after the frontmatter block; when that is
also missing, use a generic label. The `argument-hint` key is captured into
`AgentCommand::argument_hint` so completion rows can show expected arguments.

### Failure handling

Failures are contained per source. An unreadable directory, malformed JSON in
`installed_plugins.json`, or a skill file with broken frontmatter drops that one
entry or that one root and leaves the rest of the catalog intact. A corrupt
plugin must never empty the command list.

## Matching

An entry matches when the typed input prefixes either the whole command or any
of its segments, where segments are delimited by `:`. So
`/superpowers:brainstorming` is found by `/super`, `/superpowers:b`, and
`/brain`.

Tab completion needs no new logic. `shared_completion_prefix` returns `None`
when the common prefix is not longer than the input, so a unique segment match
fills in full and divergent segment matches fall through to accepting the
highlighted row.

## Refresh

Filesystem access is an effect, and walking hundreds of plugin directories on
the UI thread would be perceptible. Discovery runs as an effect behind three
typed events:

- `SlashCatalogRequested` — published on startup, on worktree switch, and after
  `/plugin install` or `/mcp add` completes
- `SlashCatalogLoaded { catalog }` — replaces the agent-provided half of the
  catalog
- `SlashCatalogFailed { error }` — sets a notice and leaves the previous catalog
  in place

Agency's own commands are always present synchronously, so the completion list
is useful before the first load lands and never empties on a failure.

A filesystem watcher is out of scope. The effect is driven by
`SlashCatalogRequested`, so a watcher can publish that same event later without
restructuring anything.

## Testing

**`agency-translator-api`**

- Frontmatter parsing: description present, absent, malformed; `argument-hint`
  captured; a file with no frontmatter at all
- Segment name construction from nested paths

**`agency-translators`**

Fixture trees in a temporary directory covering:

- An enabled plugin, indexed with its namespace
- A disabled plugin, absent from the catalog
- A plugin with three versions on disk, indexed only at the path
  `installed_plugins.json` names
- Nested command directories producing multi-segment names
- A project-level entry shadowing a personal entry of the same name
- An unreadable entry that does not sink the rest of the catalog

`claude_built_ins_are_available_and_can_be_overridden` moves here from the
desktop crate, since built-in shadowing is now a within-provider concern.

**`agency-desktop`**

- Segment matching
- Tab fill-then-accept against namespaced names
- Two providers offering the same name still produce two rows with different
  insertions — the surviving half of
  `duplicate_names_are_kept_between_agents_and_replaced_within_one_agent`,
  rewritten against the merge rather than `push_agent_completion`
- Reducers for `SlashCatalogRequested → SlashCatalogLoaded` and
  `SlashCatalogRequested → SlashCatalogFailed`, asserting the previous catalog
  survives a failure and a notice is set

The existing `SlashCompletionState` tests stay unchanged; that state machine is
not affected.

## Out of scope

- Querying running agents for their command lists
- A filesystem watcher over the discovery roots
- Fuzzy or subsequence matching
- Surfacing installed-but-disabled plugins in the completion list
