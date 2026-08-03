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

`active` is `Option<Provider>` rather than a bare `Provider` because one caller
genuinely has no agent: `refresh_slash_completions` defaults the prompt when no
pane is focused. `None` ranks every agent command equally, so a stable sort
leaves the list in exactly its current order rather than in a special case.

The return type becomes `Vec<&SlashCommandCompletion>` because sorting cannot be
lazy. Callers use `.iter()`, `.len()`, and `.get(selected)` in place of the
iterator methods they use now.

### Call sites

Five, all mechanical. Four already hold the focused agent and pass
`Some(agent.provider)`:

- the Enter handler in `main.rs`, inside its `active_agent()` closure
- the completion list in the agent view, inside `if let Some(agent) =
  self.active_agent()`
- `slash_completion_count`
- `TabCompleteSlashCommand`, which currently pulls only the prompt out of
  `active_agent()` and pulls the provider alongside it

The fifth is `SlashCompletionState::refresh`, which gains the parameter and
receives it from `refresh_slash_completions`. That caller already does
`self.active_agent().map(...).unwrap_or_default()` for the prompt and extends
the same expression to carry `Option<Provider>`.

`completion_count` and `tab_completion` forward the parameter through to
`slash_command_completions`.

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
- `active: None` yields the pre-change order
- `tab_completion` with `selected: 0` accepts the focused agent's row rather
  than the other agent's

**`main.rs`**, at the reducer level:

- with a catalog spanning both providers, `SelectAgent` flips which command the
  Enter handler resolves at index 0

The ordering is only useful if it follows a live agent switch, so that last test
is what pins the requirement end to end.

Existing `SlashCompletionState` tests are unaffected beyond the added parameter;
the state machine itself does not change.

## Out of scope

- Filtering the unfocused agent's commands out of the list
- Re-ordering within a provider's block, whether alphabetically or by origin
- Ranking by anything other than the focused pane — running-versus-configured
  agents, recency of use, or frequency
- Re-sorting the stored catalog on agent switch
