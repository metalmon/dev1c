# Contributing

This repository is source-available, not open source (see [LICENSE](LICENSE)). You are welcome to read it, to open issues, and to send changes — but know what that means before you spend an evening on it:

- **Issues are the most useful thing you can send.** A reproducible bug in the installer, the connector or the stand saves us more time than a patch does.
- **Pull requests are accepted by arrangement.** Open an issue first and say what you intend to change. We may decline a change that is correct simply because it does not fit where the product is going, and it is unfair to let you write it before saying so.
- **By sending a change you confirm it is yours to give**, and that we may use it under the current licence and under any later terms for this software. You keep the copyright in what you wrote. That is clause 3 of the licence, spelled out here so it is not a surprise.

## What a change has to pass

Everything below runs on a developer machine in minutes. Do not push and wait for CI to tell you something you could have learnt in seven seconds.

```sh
# Formatting — the pre-push hook runs exactly this
cargo fmt --all --manifest-path setup/Cargo.toml

# Lints, as CI runs them. The gui feature matters: without it four findings
# in dev1c-setup/src/gui are invisible
cargo clippy --workspace --all-targets --features dev1c-setup/gui \
    --manifest-path setup/Cargo.toml -- -D warnings

# Tests, all of them
cargo test --workspace --features dev1c-setup/gui --manifest-path setup/Cargo.toml
```

Enable the formatting gate once, and a bad format can never reach a push:

```sh
git config core.hooksPath .githooks
```

Rough costs, measured on a 16 GB workstation with a warm cache: `cargo check` 7–12 s, the whole test suite about six minutes, of which 210 s is `onec-connect`'s own 485 tests. A cold first build of the GPUI dependency tree is longer — allow a quarter of an hour and let it run in the foreground.

One module's tests can be run alone, which is the point of how the window is split:

```sh
cargo test -p onec-connect --lib gui::facts    # 25 tests, about 5 s
cargo test -p onec-connect --lib gui::save     # 18 tests
```

CI runs the lints and the tests on **Windows and Linux both**. This is not belt-and-braces: a good deal of the window and platform code sits behind `#[cfg(windows)]` or `#[cfg(unix)]`, so a lint clean on one system says nothing about the other. Expect to learn that the hard way at least once.

## What the code expects

- **Comments say why, not what.** The code says what it does; a comment exists to record the reason, the measurement or the incident behind a decision — including the ones that were tried and rejected. A comment that restates the line above it is noise and will be asked about in review.
- **Comments and commit messages are Russian; code, documentation and scripts are English.** The environment's users are Russian-speaking 1C developers; the engineering around it is not. Both rules are already followed throughout, so copy what you see.
- **A paragraph in a Markdown file is one line.** No hand-wrapping at 80 columns: the wrap helps whoever edits the file and hurts whoever reads the diff, because inserting a word reflows the paragraph and the diff then claims it all changed. Commit messages are the exception and stay wrapped — that is a git convention.
- **A new dependency needs a reason in the manifest.** Look at `setup/1c-connect/Cargo.toml`: every optional crate carries a comment saying why it is there and what it replaced. Something a few lines can do does not earn a crate.
- **Non-trivial logic leaves a test behind** — in the same module, next to the code, not in a separate tree. Look at any `src/gui/*/tests.rs`.
- **Deliberate shortcuts are marked.** A `ponytail:` comment names the ceiling that was accepted and the way out of it, so a known limit is never mistaken for an oversight.

## Commit messages

Subject in Russian, declarative, about the effect rather than the mechanics — `Проверка отличает «место занято снаружи» от поломки стенда`, not `Fix check`. The body explains why the change exists, what was measured and what was rejected; wrap it at about 76 columns. `git log` is the design record of this project and is worth reading before changing anything substantial.

## Where things are

| Path | What lives there |
|---|---|
| `setup/dev1c-setup/` | The installer: the one program a customer runs |
| `setup/1c-connect/` | The connector on the developer's machine: tunnel, bridge, window |
| `setup/common/` | What both share — the kit, checks, operations |
| `config/` | Everything baked into the image: XFCE, Zed, skills, agent rules, the stand |
| `docs/` | The documentation the README links to |
| `.githooks/` | This repository's own hooks. Not to be confused with `config/git-hooks/`, which ships inside the image and guards 1C modules going into Gitea |
