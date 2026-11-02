# grit

A native Rust Git client inspired by GittyUp's focus on understanding repository history. Built with [egui](https://github.com/emilk/egui) and [libgit2](https://libgit2.org/), with one application codebase for Windows, Linux, and macOS.

## Features

- Open a local Git repository
- Keep multiple repositories open in persistent tabs
- Initialize a repository or clone one from HTTPS/SSH
- Browse local branches and check them out
- Rebase the current branch onto another local branch and continue or abort after conflicts
- Edit an interactive rebase plan to reorder, squash, fix up, or drop commits
- Create and safely delete local branches
- Create lightweight tags
- Configure remotes, fetch, pull (fast-forward only), and push
- Review working tree changes and unified diffs
- Stage and unstage files
- Stage individual diff hunks
- Edit a diff patch for partial line staging
- Resolve merge conflicts by choosing either side or marking an edited file resolved
- Save and restore stashes
- Create commits
- Amend the latest commit
- Browse the multi-branch commit graph, inspect diffs, cherry-pick or revert commits, check out a commit, and create a branch at a commit
- View line attribution with Git blame
- Filter commits by message or author

## Run

Install Rust and Git, then run:

```sh
cargo run --release
```

On Linux, install the native windowing system development libraries required by eframe (X11/Wayland and OpenGL). Cargo can build the application on Windows, Linux, and macOS.

### Linux AppImage

Build the release binary and package a portable AppImage with:

```sh
cargo build --locked --release
./scripts/build-appimage.sh
```

The finished file is `dist/grit-x86_64.AppImage`. Make it executable and open it directly. The packaging script fetches the official linuxdeploy AppImage and AppImage runtime when they are not already cached under `target/tools/`.

For broad distro compatibility, build the AppImage on Ubuntu 22.04 or use the Linux AppImage artifact from CI. A local build inherits the host's glibc baseline.

## Platform builds

The GitHub Actions workflow builds a release binary on Windows, Linux, Apple Silicon macOS, and Intel macOS for each pull request, main branch update, and version tag. Linux runs also produce the `grit-linux-AppImage-x86_64` artifact.

## Current scope

Remote authentication uses Git's configured credential helpers and SSH agent through the system `git` executable. Pull uses fast-forward-only mode. Conflict files can be resolved from the Changes view by choosing either side or editing the file in grit. Interactive rebase plans require a clean working tree and currently exclude merge commits.
