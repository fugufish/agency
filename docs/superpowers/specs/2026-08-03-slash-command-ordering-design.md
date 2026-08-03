# Ordering slash commands by the focused agent

## Problem

`merge_catalog` in `crates/agency-desktop/src/slash_commands.rs` builds one flat
catalog: Agency's own commands, then every configured provider's commands in the
order `discover_agent_commands` walked them. `slash_command_completions` filters
that catalog and preserves its order, so the completion list looks the same no
matter which agent pane has focus.

That puts the commands you cannot use first as often as not. Typing `/re` into a
Claude pane can surface Codex's entries above Claude's, and the highlighted row —
the one Enter and Tab commit — belongs to the agent you are not talking to.

## Approach

Ordering is a projection of state for display, not state itself, so it is applied
where the list is derived rather than stored on the app. Every consumer of the
completion list already funnels through `slash_command_completions`: the view,
the Enter handler, `TabCompleteSlashCommand`, `slash_completion_count`, and
`SlashCompletionState::refresh`. Sorting inside that one function means the
rendered order, the selected index, and the match count cannot drift apart —
they are the same list by construction.

The transition that drives the ordering, `SelectAgent`, is already a typed event.
Nothing new is needed for the order to follow the focused agent.

### The seam

```rust
pub fn slash_command_completions<'a>(
    catalog: &'a [SlashCommandCompletion],
    input: &'a str,
    active: Option<Provider>,
) -> Vec<&'a SlashCommandCompletion>
```

It filters exactly as it does today, then stable-sorts the matches by rank:

| Rank | Entry |
|---|---|
| 0 | `provider: None` — Agency's own commands |
| 1 | The focused agent's provider |
| 2 | Any other provider |

Agency's commands are pinned on top because they are a small fixed set, are
present synchronously before any catalog load, and never reroute to an agent.
A stable position means their rows never jump when an agent switch or a catalog
load lands.

The sort must be stable. That is what preserves each translator's discovery
order — built-ins, then personal, project, and plugin entries — inside a block,
so this change moves whole blocks and nothing within them.

`active` is `Option<Provider>` rather than a bare `Provider` because the app
genuinely runs without an agent: `Agency::for_testing` has no session at all,
and the real app has none until the first one starts. `None` ranks every agent
command equally, so a stable sort leaves the agents in their catalog order
rather than in a special case; Agency's own rows still lead, which is already
where `merge_catalog` puts them.

The return type becomes `Vec<&SlashCommandCompletion>` because sorting cannot be
lazy. Callers use `.iter()`, `.len()`, and `.get(selected)` in place of the
iterator methods they use now.

### Call sites

Only the order-sensitive callers need the parameter. `tab_completion` takes it
and forwards it, because it accepts a row *by index*. `completion_count`,
`shared_completion_prefix`, and `SlashCompletionState::refresh` only count or
fold over the whole match set, so ordering cannot change their results: they
keep their current signatures and pass `None` internally. That keeps roughly
twenty existing test call sites untouched and says something true about those
functions rather than threading a parameter they ignore.

Three call sites in `main.rs`, all mechanical, all already holding the focused
agent:

- the Enter handler, inside its `active_agent()` closure
- the completion list in the agent view, inside `if let Some(agent) =
  self.active_agent()`
- `TabCompleteSlashCommand`, which currently pulls only the prompt out of
  `active_agent()` and pulls the provider alongside it

`AgentView` has no `provider` field; the focused agent's provider is
`agent.session.provider()` — the same expression the submit path already feeds
to `command_needs_agent_switch`.

### What does not change

- Commands from the unfocused agent stay in the list. Picking one still routes
  to its owning agent through the provider that `CompleteSlashCommand` already
  carries, so hiding them would waste working behavior.
- `matches` and its segment rule are untouched.
- `merge_catalog` and the stored `slash_command_catalog` keep their current
  contents and order. The catalog is not re-sorted on agent switch, so no
  reducer gains derived state that could go stale.
- `shared_completion_prefix` folds over every match, so Tab's fill is
  order-independent and behaves as it does today. Only the row Tab *accepts*
  moves, and it moves to the row now at the top of the list.

## Testing

**`slash_commands.rs`**, against a catalog holding an Agency command plus one
Codex and one Claude command that all match the same input:

- Agency's command ranks first whichever provider is active
- the focused provider's command precedes the other provider's, asserted both
  ways — Codex active, then Claude active — so a hardcoded provider order
  cannot pass
- two commands from the same provider keep their catalog order, pinning the
  stability guarantee
- `active: None` leaves the agents in catalog order
- `tab_completion` with `selected: 0` accepts the focused agent's row rather
  than the other agent's

**`main.rs`**:

- for a catalog spanning both providers and either agent focused, every row
  offered above the first one that would reroute is a row that needs no
  switch — the ranking and `command_needs_agent_switch` agree

Ranking and routing are separate decisions that know nothing about each other.
If they disagree, the top of the list stops meaning "the agent you are talking
to" and Enter starts committing a row that silently switches agents, so that
agreement is what needs pinning.

A test driving `SelectAgent` end to end is not possible: `select_agent` with no
session calls `start_agent`, which spawns a real `codex`/`claude` process, and
every `Session` constructor in `agency-agents` does the same, so no test can
hold an `AgentView`. `Agency::for_testing` exists precisely to avoid that.

Existing `SlashCompletionState` tests are unaffected beyond the added parameter;
the state machine itself does not change.

## Out of scope

- Filtering the unfocused agent's commands out of the list
- Re-ordering within a provider's block, whether alphabetically or by origin
- Ranking by anything other than the focused pane — running-versus-configured
  agents, recency of use, or frequency
- Re-sorting the stored catalog on agent switch
