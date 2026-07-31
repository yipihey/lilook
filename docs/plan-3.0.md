# lilook 3.0 — packaging and distribution

`docs/plan.md` §1 (the settled ADRs) and §5 (standalone) still hold.
`plan-1.0.md` and `plan-2.0.md` are shipped. This plan ships nothing new to the
program: it is about how the program reaches people, and about doing that in a
way that still works in a year.

**The constraint is sustainability, and it is a real constraint.** The failure
mode for a project this size is not a missing package format — it is a
distribution matrix that needs babysitting. Every channel is a standing tax paid
on every release, forever, by whoever is least interested in paying it. So the
question for each one is not "would this be nice" but **"what does it cost per
release, and who pays it".**

---

## 1. What is true today

| | |
| --- | --- |
| crates | 8, all `version = "0.1.0"`, declared independently |
| `[workspace.package]` | **absent** — no shared version, repository, or MSRV |
| `repository`, `keywords`, `categories` | **absent from every crate** |
| `license` | present on 7 crates, **missing on `lilook-data`** |
| `LICENSE` | MIT, at the root |
| CI | `ci.yml` (the gate) and `pages.yml` (the browser build) |
| release automation | **none** |
| browser bundle | ~11 MB, about 10.5 MB gzipped over the wire |
| `.lil` registration | `packaging/` has the desktop entry, MIME type and `Info.plist` fragment; nothing installs them |

None of that is hard to fix. It is listed because "publish to crates.io" is four
hours of metadata away, not four minutes, and a plan that pretends otherwise
produces a half-finished release.

---

## 2. The principle

**One action produces everything: pushing a tag.**

Anything that cannot be driven from a tag is a liability, because it is a step
someone has to remember while doing something else. A channel earns its place by
costing *zero* per release, or by being one command that a workflow runs.

Two corollaries, both load-bearing:

- **The browser is the front door, and it already costs nothing.** `pages.yml`
  deploys on every push to `main`. Someone can use lilook without installing
  anything, which means every other channel is a convenience rather than a
  prerequisite. That is a rare position and the plan should lean on it hard.
- **Version in lockstep.** Eight crates with independent versions is eight
  decisions per release. One workspace version is none. The cost is publishing a
  crate that did not change, which is free.

---

## 3. Tiers

### Tier 1 — zero per-release cost. Do these.

**The browser.** Already done. `pages.yml` builds and deploys on push. The only
change: publish the *tag* as well, so a released version is reachable at a stable
URL and not only "whatever main is".

**GitHub Releases with prebuilt binaries.** A `release.yml` triggered by `v*`
tags, building `lilook-app` for macOS (arm64 + x86_64), Linux (x86_64 gnu) and
Windows (x86_64), attaching them with the `packaging/` files. This is where most
people will actually get lilook, and after the workflow exists it costs nothing.

*Exit:* pushing `v0.2.0` produces four archives and a release page without anyone
touching a keyboard afterwards.

### Tier 2 — one command, driven by the tag. Do these.

**crates.io, all eight crates, in lockstep.** The libraries are the reusable part
and `lilook-data` in particular — a dependency-light pure-Rust reader for npz,
FITS, HDF5 and Veusz ASCII — is worth having on its own. `cargo install
lilook-app` falls out for free, and it is the lowest-friction install for the
audience most likely to have a Rust toolchain already.

Needs first: `[workspace.package]` with version, repository, licence, keywords,
categories and `rust-version`; a licence on `lilook-data`; and publish order
(`core` → `data` → `compile` → `ui` → `editor` → `app`/`web`/`ffi`).

Use `cargo-release` rather than a bot: one command, no third party in the loop,
and it fails loudly rather than opening a pull request nobody reads.

**Homebrew, as a formula that builds from source.** A tap
(`yipihey/homebrew-lilook`) with a formula whose only per-release change is a
version and a tarball sha256 — which the release workflow can compute and commit.

Deliberately a *formula*, not a cask: building from source sidesteps macOS
notarisation entirely. A cask would mean an Apple Developer account and a
notarisation step in CI, and unsigned casks give users a Gatekeeper dialogue that
looks like a virus warning.

### Tier 3 — only when something else lands.

**`lilook-lsp` on crates.io.** A language server is a natural `cargo install`,
and it is how editor users expect to get one. Blocked on the binary existing;
worth doing the day it does.

**A VS Code extension.** Only once `lilook-lsp` exists, and then it is mostly
`package.json`. Not before: an extension whose only feature is syntax
highlighting duplicates tinymist.

### Tier 4 — explicitly not doing. Say so, so it is not re-litigated.

**Linux distribution packages (deb, rpm, AUR).** Each is a separate packaging
system with its own review, conventions and update cadence. Three of them is more
per-release work than everything else in this document combined. A tarball plus
`packaging/` is enough for anyone who wants to package it, and distro maintainers
are better at it than we would be.

**PyPI.** There is no Python here. Wrapping a GUI binary in platform wheels to
get `uv tool install` means building and shipping wheels for something that is
not a Python package. If the Python audience matters, the honest target is
`bindings/python` — the existing ctypes wrapper, published as a real package so
lilook can be driven from a notebook.

**Windows and macOS code signing.** Deferred, not refused. It costs money
annually and a keychain in CI, and the audience most likely to hit Gatekeeper is
also the one most likely to use Homebrew, which avoids it. Revisit if unsigned
downloads turn out to be a real barrier — that is a fact to observe, not to
predict.

**Snap, Flatpak, winget, Chocolatey, Nix.** Same reasoning as distro packages.
A Nix flake is the most tempting because it is one file, but it is one file that
breaks silently when a dependency moves, and nobody here runs NixOS to notice.

---

## 4. Milestones

### R1 — make the workspace publishable

`[workspace.package]` with `version`, `edition`, `license = "MIT"`,
`repository`, `rust-version`, `authors`; every crate inheriting it; a licence on
`lilook-data`; `description` on the three that lack one; and `keywords` and
`categories` per crate.

Also: `publish = false` on `lilook-web` if the wasm shell has no meaning outside
this repository, so a release cannot accidentally push something meaningless.

*Exit:* `cargo publish --dry-run` succeeds for every crate that should be
published, in dependency order.

### R2 — the release workflow

`release.yml` on `v*` tags: run the gate, build the four binaries, build the
browser bundle, create the release, attach the archives and the `packaging/`
files.

*Exit:* a `v0.2.0-rc1` tag produces a complete draft release, and the gate
failing stops it.

### R3 — first publish

`cargo-release` configuration, then a real `0.2.0` to crates.io. `0.2.0` rather
than `0.1.0` because the version has been sitting at `0.1.0` through everything
in `plan-1.0` and `plan-2.0`, and a first published version that is honest about
being mature is better than one that looks abandoned.

*Exit:* `cargo install lilook-app` works on a machine that has never seen this
repository.

### R4 — Homebrew tap

A formula built from source, and a workflow step that bumps it.

*Exit:* `brew install yipihey/lilook/lilook` works on both Apple Silicon and
Intel.

---

## 5. What each release costs, once this is done

| step | who |
| --- | --- |
| decide the version, write the changelog entry | a person |
| `cargo release <version>` | one command |
| everything else | CI |

That is the test the plan has to pass. If a release ever needs more than a
decision and a command, a channel has become a liability and should be dropped
rather than maintained.

---

## 6. Risks

**Version lockstep is wrong if a library stabilises before the app.** It is the
right default now, when nothing outside this repository depends on any of it. The
day someone does, `lilook-data` probably wants its own version, and that is a
pleasant problem to have rather than a design flaw to prevent.

**The browser bundle is 10.5 MB gzipped.** Fine on a laptop, noticeable on a
phone over cellular. It is already measured and already cached after the first
visit; worth watching rather than fixing, and worth *not* regressing.

**A release workflow rots quietly.** It runs on tags, which is rarely, so it will
break between uses. Mitigation: run it on release-candidate tags before real
ones, and never hand-fix a release — fix the workflow and re-tag.

**crates.io is forever.** A name published is a name taken and a version
published cannot be replaced. `--dry-run` for every crate is R1's exit criterion
for exactly this reason.
