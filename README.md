# grit

A native Rust Git client inspired by GittyUp's focus on understanding repository history. Built with [egui](https://github.com/emilk/egui) and [libgit2](https://libgit2.org/), with one application codebase for Windows, Linux, and macOS.

## Workspace

The Rust/egui interface uses repository tabs, a resizable reference sidebar, and a history table above a resizable commit detail area. Select a branch to filter history; double-click it to check it out. Choose **All branches** above the table to restore the complete history.

The Changes workspace separates unstaged and staged files, with a commit editor below and a diff alongside. Selecting a file shows only the changes from that side of the index. Diffs include old/new line numbers, addition/deletion colors, hunk staging, and an editable patch for partial staging. Right-click a file for stage/unstage and discard actions; right-click a stash for apply/pop/drop actions.

Use **Repository** to open, clone, initialize, or revisit a repository. `Ctrl+O` opens a folder and `Ctrl+R` refreshes it (`Cmd` on macOS). A repository path can also be passed directly:

```sh
cargo run --release -- /path/to/repository
```

The detailed comparison against GittyUp, source references, implementation packages, and remaining gaps are in [the feature analysis](docs/GITTYUP_FEATURE_ANALYSIS.md).

New workflows include selecting changed diff lines for stage/unstage (`S` / `U`), starring commits with their context menu, and structured history queries such as `author:"Alex Morgan" after:2026-01-01 path:src/main.rs`. The History **View** menu contains first-parent, stars, sorting, and column options. Commit templates and author/sign-off settings are available under the commit editor's **More** menu.

**Repository tools** provides branch rename/merge, annotated tags, submodule updates, and literal or pattern-based ignore entries. **LFS / external tools** provides explicit repository-local Git LFS actions when `git-lfs` is installed, an external editor, a terminal, and the file manager. File history and byte-exact version exports are available from file actions. `Ctrl/Cmd+M` focuses the active history/diff pane; `Ctrl/Cmd+Q` closes the app. Commit stars, templates and display preferences survive restarts. Diff controls offer 0–50 context lines and whitespace filtering. Line staging uses the displayed context, and is disabled while whitespace changes are hidden; file staging continues to include every change.

Use **List / Tree** to browse tracked files and staged/unstaged changes as nested folders. The commit file list has its own mode button. Tree selections retain file history/copy actions and use **Stage selected file** / **Unstage selected file** controls. Blame is interactive for the working copy and historical revisions; attribution loads in the background and clicking a commit opens its history details.

## Features

- Open a local Git repository
- Switch among open repositories with tabs and revisit recent repositories from the Repository menu
- Initialize a repository or clone one from HTTPS/SSH
- Browse local branches and check them out
- Rebase the current branch onto another local branch and continue or abort after conflicts
- Edit an interactive rebase plan to reorder, squash, fix up, or drop commits
- Create and safely delete local branches
- Create lightweight tags
- Configure remotes, fetch, pull (fast-forward only), push, and publish new branches with upstream tracking
- Review working tree changes and unified diffs
- Stage and unstage files
- Stage individual diff hunks
- Edit a diff patch for partial line staging
- Resolve merge conflicts by choosing either side or marking an edited file resolved
- Save and restore stashes
- Create commits
- Amend the latest commit
- Browse the multi-branch commit graph with selected commit details, a unified diff and adjacent changed-file list; cherry-pick or revert commits, check out a commit, and create a branch at a commit
- Inspect interactive line attribution for the working copy or a historical revision, with author/commit navigation
- Filter commits by message, author, email, SHA, date and exact changed-file path
- Star commits and filter by first-parent ancestry
- Select individual changed lines to stage or unstage
- Manage named commit templates, author overrides and sign-off
- Rename and merge branches, create annotated tags, and update submodules
- Browse and export historical file revisions
- Configure ignore patterns and explicit Git LFS / external tool actions

## Run

For development, install Rust and Git, then run:

```sh
cargo run --release
```

On Linux, install the native windowing system development libraries required by eframe (X11/Wayland and OpenGL). Cargo can build the application on Windows, Linux, and macOS. The packaged app also requires Git to be installed and available on `PATH` for repository operations. Native hosting API actions additionally use `curl` on `PATH` for HTTPS transport.

### Linux AppImage

Build the release binary and package a portable AppImage with:

```sh
cargo build --locked --release
./scripts/build-appimage.sh
```

The finished file is `dist/grit-x86_64.AppImage`. Make it executable and open it directly. The packaging script fetches the official linuxdeploy AppImage and AppImage runtime when they are not already cached under `target/tools/`.

For broad distro compatibility, build the AppImage on Ubuntu 22.04 or use the Linux AppImage artifact from CI. A local build inherits the host's glibc baseline.

## Platform builds

The GitHub Actions workflow builds on Windows, Linux, Apple Silicon macOS, and Intel macOS for each pull request, main branch update, and version tag. Its downloadable artifacts include a portable Windows ZIP, a macOS `.app` bundle ZIP for each architecture, and the `grit-linux-AppImage-x86_64` AppImage. The CI macOS bundle is not code signed, so macOS may ask you to confirm the first launch.

A successful push build on `main` publishes the four desktop packages under [GitHub Releases](https://github.com/trashcoder/grit/releases) as a prerelease tagged `dev.<workflow-run-number>`. A successful version-tag build such as `v0.1.0` publishes a stable release marked **Latest**. Version tags with a hyphen, such as `v0.1.0-rc.1`, also publish as prereleases.

## Current scope

Remote authentication uses Git's configured credential helpers and SSH agent through the system `git` executable. Pull uses fast-forward-only mode. Conflict files can be resolved from the Changes view by choosing either side or editing the file in grit. Interactive rebase plans require a clean working tree and currently exclude merge commits.

**Hosting accounts** manages GitHub, GitLab, Gitea, Bitbucket Cloud and Beanstalk accounts, repository lists and local-copy mappings. Tokens stay in memory; only account metadata and local paths are saved. Repository and review pages load on explicit actions. Review creation requires confirmation; selecting a clone URL opens the normal clone dialog so the destination remains an explicit choice.
