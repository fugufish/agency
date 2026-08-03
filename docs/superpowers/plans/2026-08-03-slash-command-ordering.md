# Slash Command Ordering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rank the slash command completion list so Agency's own commands lead, the focused agent's commands come next, and the other agent's sink below them.

**Architecture:** Ordering is applied where the list is derived, not stored on the app. `slash_command_completions` gains an `active: Option<Provider>` parameter and stable-sorts its matches by a three-way rank. Every consumer already funnels through that function, so the rendered order, the selected index, and the match count cannot drift apart. The functions that only count or fold over matches are order-independent and keep their current signatures.

**Tech Stack:** Rust, iced. Crate `agency-desktop` (binary `agency`). Tests are inline `#[cfg(test)] mod tests` blocks; run with `cargo test -p agency-desktop`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-03-slash-command-ordering-design.md`
- The sort must be **stable** — that is what preserves each translator's discovery order inside a provider's block.
- Commands from the unfocused agent stay in the list. Never filter by provider.
- `merge_catalog`, the stored `Agency::slash_command_catalog`, and `matches` are not modified.
- Tests must never construct an `AgentSession` or call `Agency::default()`. Every `Session` constructor in `agency-agents` spawns a real `codex`/`claude` child process. Reducer tests use `Agency::for_testing()`, which has no agents at all, so `active_agent()` returns `None` in tests by construction.
- Run `cargo fmt --all` before each commit.

---

### Task 1: Rank completions by the focused agent

**Files:**
- Modify: `crates/agency-desktop/src/slash_commands.rs:130-138` (`slash_command_completions`)
- Modify: `crates/agency-desktop/src/slash_commands.rs:203-220` (`SlashCompletionState::refresh`)
- Modify: `crates/agency-desktop/src/slash_commands.rs:236-238` (`completion_count`)
- Modify: `crates/agency-desktop/src/slash_commands.rs:250-262` (`tab_completion`)
- Modify: `crates/agency-desktop/src/slash_commands.rs:267-283` (`shared_completion_prefix`)
- Modify: `crates/agency-desktop/src/main.rs:1302-1309`, `main.rs:1586-1591`, `main.rs:3699-3700`
- Test: `crates/agency-desktop/src/slash_commands.rs`, inside the existing `#[cfg(test)] mod tests` (starts at line 428)

**Interfaces:**
- Produces: `slash_command_completions(catalog: &[SlashCommandCompletion], input: &str, active: Option<Provider>) -> Vec<&SlashCommandCompletion>` and `tab_completion(catalog: &[SlashCommandCompletion], input: &str, selected: usize, active: Option<Provider>) -> Option<TabCompletion>`. Both are used by Task 2.
- Unchanged: `completion_count(catalog, prompt) -> usize`, `shared_completion_prefix(catalog, input) -> Option<String>`, `SlashCompletionState::refresh(&mut self, catalog, prompt, composer)`. These only count or fold over the matches, so ordering cannot change their results and they do not take `active`.

- [ ] **Step 1: Write the failing tests**

Add a test helper and four tests to the `mod tests` block in `crates/agency-desktop/src/slash_commands.rs`. Put the helper next to the existing `completion` helper (line 758) and the tests at the end of the module, before its closing brace at line 1018.

```rust
    /// The existing `completion` helper builds Agency-owned rows
    /// (`provider: None`). Ordering needs rows that belong to an agent.
    fn provider_completion(command: &str, provider: Provider) -> SlashCommandCompletion {
        SlashCommandCompletion {
            command: command.to_owned(),
            description: String::new(),
            insertion: format!("{command} "),
            provider: Some(provider),
            built_in: false,
        }
    }

    fn ordered_commands(
        catalog: &[SlashCommandCompletion],
        input: &str,
        active: Option<Provider>,
    ) -> Vec<String> {
        slash_command_completions(catalog, input, active)
            .into_iter()
            .map(|completion| completion.command.clone())
            .collect()
    }

    /// Picking a command routes it to the agent that owns it, so the other
    /// agent's commands stay listed — they just sink below the ones the
    /// composer is already pointed at.
    #[test]
    fn the_focused_agents_commands_are_offered_before_the_other_agents() {
        let catalog = vec![
            provider_completion("/review-codex", Provider::Codex),
            completion("/init"),
            provider_completion("/review-claude", Provider::Claude),
        ];

        // Asserted both ways, so a ranking that hardcodes one provider fails.
        assert_eq!(
            ordered_commands(&catalog, "/", Some(Provider::Claude)),
            vec!["/init", "/review-claude", "/review-codex"]
        );
        assert_eq!(
            ordered_commands(&catalog, "/", Some(Provider::Codex)),
            vec!["/init", "/review-codex", "/review-claude"]
        );
    }

    /// The sort is stable, so a provider's block keeps the order its
    /// translator discovered — built-ins, then personal, project, and plugin
    /// entries. The names here are deliberately not alphabetical, so a sort
    /// by name would fail this.
    #[test]
    fn commands_from_one_agent_keep_their_catalog_order() {
        let catalog = vec![
            provider_completion("/second", Provider::Claude),
            provider_completion("/first", Provider::Claude),
        ];

        assert_eq!(
            ordered_commands(&catalog, "/", Some(Provider::Claude)),
            vec!["/second", "/first"]
        );
    }

    /// Before any session exists there is no agent to rank against. Every
    /// agent command ties, so a stable sort leaves them where the catalog put
    /// them, and only Agency's own rows lead.
    #[test]
    fn without_a_focused_agent_the_agents_keep_their_catalog_order() {
        let catalog = vec![
            provider_completion("/review-codex", Provider::Codex),
            completion("/init"),
            provider_completion("/review-claude", Provider::Claude),
        ];

        assert_eq!(
            ordered_commands(&catalog, "/", None),
            vec!["/init", "/review-codex", "/review-claude"]
        );
    }

    /// Tab commits the highlighted row, and the highlighted row is now the
    /// focused agent's. Both agents offer `/review`, so an ordering that
    /// ignored the focused agent would hand Tab the wrong one.
    #[test]
    fn tab_accepts_the_focused_agents_row() {
        let catalog = vec![
            provider_completion("/review", Provider::Codex),
            provider_completion("/review", Provider::Claude),
        ];

        let Some(TabCompletion::Accept(accepted)) =
            tab_completion(&catalog, "/review", 0, Some(Provider::Claude))
        else {
            panic!("a fully typed command should be accepted");
        };
        assert_eq!(accepted.provider, Some(Provider::Claude));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agency-desktop slash_commands`

Expected: FAIL to compile, with `error[E0061]: this function takes 2 arguments but 3 arguments were supplied` at `slash_command_completions` and `this function takes 3 arguments but 4 arguments were supplied` at `tab_completion`. A compile failure is the red state here — the parameter does not exist yet.

- [ ] **Step 3: Add the rank and sort**

In `crates/agency-desktop/src/slash_commands.rs`, replace `slash_command_completions` (lines 130-138) with:

```rust
/// Where a completion sits relative to the agent the composer is pointed at.
///
/// Agency's own commands lead: they are a small fixed set, are present before
/// any catalog has loaded, and never route to an agent, so a stable position
/// means their rows do not jump when an agent switch or a load lands. The
/// focused agent's come next. The other agent's stay listed, because picking
/// one still routes it to its owner, but they sink below the ones that need no
/// switch.
fn completion_rank(completion: &SlashCommandCompletion, active: Option<Provider>) -> u8 {
    match completion.provider {
        None => 0,
        Some(provider) if Some(provider) == active => 1,
        Some(_) => 2,
    }
}

pub fn slash_command_completions<'a>(
    catalog: &'a [SlashCommandCompletion],
    input: &'a str,
    active: Option<Provider>,
) -> Vec<&'a SlashCommandCompletion> {
    let input = input.trim_start();
    let mut ordered = catalog
        .iter()
        .filter(|completion| matches(&completion.command, input))
        .collect::<Vec<_>>();
    // Stable, which is what keeps each translator's discovery order intact
    // inside a provider's block: this moves whole blocks, nothing within them.
    ordered.sort_by_key(|completion| completion_rank(completion, active));
    ordered
}
```

Replace `completion_count` (lines 236-238) with:

```rust
/// How many catalog entries `prompt` currently matches. Counting is
/// order-independent, so this needs no focused agent to rank against.
pub fn completion_count(catalog: &[SlashCommandCompletion], prompt: &str) -> usize {
    slash_command_completions(catalog, prompt, None).len()
}
```

In `SlashCompletionState::refresh`, replace line 209 with:

```rust
        let matches = completion_count(catalog, prompt);
```

Replace the body of `tab_completion` (lines 250-262) with:

```rust
pub fn tab_completion(
    catalog: &[SlashCommandCompletion],
    input: &str,
    selected: usize,
    active: Option<Provider>,
) -> Option<TabCompletion> {
    match shared_completion_prefix(catalog, input) {
        Some(prefix) => Some(TabCompletion::Fill(prefix)),
        None => slash_command_completions(catalog, input, active)
            .into_iter()
            .nth(selected)
            .cloned()
            .map(TabCompletion::Accept),
    }
}
```

In `shared_completion_prefix`, replace lines 269-270 with:

```rust
    // The shared prefix folds over every match, so ordering cannot change it
    // and Tab's fill behaves exactly as it did before ranking existed.
    let ordered = slash_command_completions(catalog, input, None);
    let mut matches = ordered.iter().map(|completion| completion.command.as_str());
```

- [ ] **Step 4: Update the existing in-file test call sites**

These are mechanical. In `slash_prefixes_offer_matching_completions` (lines 738-755), the function now returns a `Vec`, so the `.collect()` and `.next()` calls change:

```rust
        assert_eq!(
            slash_command_completions(&completions, "/", None),
            completions.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            slash_command_completions(&completions, "/mcp a", None),
            completions.iter().collect::<Vec<_>>()
        );
        assert!(slash_command_completions(&completions, "hello", None).is_empty());
        assert!(slash_command_completions(&completions, "/wat", None).is_empty());
```

Then add `, None` as the fourth argument to every existing `tab_completion` call in the test module — lines 820, 825, 828, 841, 852, and the two inside `tab_fills_a_unique_segment_match_and_accepts_an_ambiguous_one` (999, 1003). For example:

```rust
        assert_eq!(
            tab_completion(&catalog, "/p", 1, None),
            Some(TabCompletion::Fill("/plugin ".to_owned()))
        );
```

No `completion_count` or `state.refresh` call site changes — those signatures are unchanged.

- [ ] **Step 5: Update the three call sites in `main.rs`**

`TabCompleteSlashCommand` (around line 1302) currently pulls only the prompt out of the focused agent. Pull the provider alongside it:

```rust
            AppEvent::TabCompleteSlashCommand => {
                let Some((prompt, active)) = self
                    .active_agent()
                    .map(|agent| (agent.prompt.clone(), agent.session.provider()))
                else {
                    return Task::none();
                };
                match tab_completion(
                    &self.slash_command_catalog,
                    &prompt,
                    self.overlays.slash.selected(),
                    Some(active),
                ) {
```

The Enter handler (around line 1586):

```rust
                        let completion = self.active_agent().and_then(|agent| {
                            slash_command_completions(
                                &self.slash_command_catalog,
                                &agent.prompt,
                                Some(agent.session.provider()),
                            )
                            .into_iter()
                            .nth(self.overlays.slash.selected())
                            .filter(|completion| completion.insertion != agent.prompt)
                            .cloned()
                        });
```

The completion list in the agent view (around line 3699):

```rust
            let completions = slash_command_completions(
                &self.slash_command_catalog,
                &agent.prompt,
                Some(agent.session.provider()),
            )
            .into_iter()
            .enumerate()
            .fold(column![].spacing(4), |completions, (index, completion)| {
```

`AgentView` has no `provider` field — the focused agent's provider is `agent.session.provider()`, which is the same expression the submit path already feeds to `command_needs_agent_switch`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p agency-desktop`

Expected: PASS. The four new tests pass and every pre-existing test in `slash_commands` and `main` still passes.

- [ ] **Step 7: Format and commit**

```bash
cargo fmt --all
git diff --stat
git add crates/agency-desktop/src/slash_commands.rs crates/agency-desktop/src/main.rs
git commit -m "feat(desktop): rank slash commands by the focused agent"
```

Check `git diff --stat` before staging: it should touch only those two files. If `cargo fmt` reformatted anything unrelated, drop those hunks.

---

### Task 2: Pin the ordering to the routing it promises

**Files:**
- Test: `crates/agency-desktop/src/main.rs`, inside the existing `#[cfg(test)] mod tests`, next to `the_catalog_stamps_the_provider_that_routing_depends_on`

**Interfaces:**
- Consumes: `slash_command_completions(catalog, input, active)` from Task 1; the existing `command_needs_agent_switch(active: Provider, command: Provider) -> bool` and the `agent_command(name: &str) -> AgentCommand` test helper, both already in `main.rs`.

This task adds no production code. Ordering is only meaningful if the rows on top are the ones that will *not* be rerouted; that invariant spans two functions that know nothing about each other, so it needs its own test.

An end-to-end test driving `SelectAgent` is not possible here: `select_agent` on an agency with no session calls `start_agent`, which spawns a real `codex`/`claude` process, and `Agency::for_testing()` deliberately has no agents. This test pins the same guarantee without a process.

- [ ] **Step 1: Write the failing test**

```rust
    /// The rows offered first must be exactly the ones that will not be
    /// rerouted. Ranking and `command_needs_agent_switch` are separate
    /// decisions; if they ever disagree, the top of the list stops meaning
    /// "the agent you are talking to" and Enter starts committing a row that
    /// silently switches agents.
    #[test]
    fn the_commands_offered_first_are_the_ones_that_need_no_switch() {
        let catalog = slash_commands::merge_catalog(vec![
            (Provider::Claude, agent_command("superpowers:brainstorming")),
            (Provider::Codex, agent_command("review")),
        ]);

        for active in [Provider::Codex, Provider::Claude] {
            let mut seen_a_switch = false;
            for completion in slash_command_completions(&catalog, "/", Some(active)) {
                let Some(owner) = completion.provider else {
                    assert!(
                        !seen_a_switch,
                        "Agency's own commands must lead, but {} came after an agent's",
                        completion.command
                    );
                    continue;
                };
                if command_needs_agent_switch(active, owner) {
                    seen_a_switch = true;
                } else {
                    assert!(
                        !seen_a_switch,
                        "{} needs no switch but was listed below one that does",
                        completion.command
                    );
                }
            }
        }
    }
```

- [ ] **Step 2: Run the test to verify it passes, then break the ranking to verify it fails**

Run: `cargo test -p agency-desktop the_commands_offered_first`
Expected: PASS.

Then confirm the test can actually fail. In `slash_commands.rs`, temporarily flip `completion_rank`'s middle arm to `Some(provider) if Some(provider) != active => 1`, and re-run:

Run: `cargo test -p agency-desktop the_commands_offered_first`
Expected: FAIL with "needs no switch but was listed below one that does".

Revert the flip and re-run to confirm PASS. Do not commit the flip.

- [ ] **Step 3: Run the whole suite**

Run: `cargo test -p agency-desktop`
Expected: PASS.

- [ ] **Step 4: Format and commit**

```bash
cargo fmt --all
git diff --stat
git add crates/agency-desktop/src/main.rs
git commit -m "test(desktop): tie slash command ordering to command routing"
```

---

## Verification

After both tasks:

```bash
cargo test -p agency-desktop
cargo fmt --all --check
cargo clippy -p agency-desktop --all-targets
git log --oneline -2
```

All must be clean. Then check the behavior in the app: with both agents configured, open the composer, type `/`, and confirm Agency's three commands lead, the focused agent's commands follow, and switching agents moves the other agent's block to the top.
