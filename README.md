# grit

A native Rust Git client inspired by GittyUp's focus on understanding repository history. Built with [egui](https://github.com/emilk/egui) and [libgit2](https://libgit2.org/), with one application codebase for Windows, Linux, and macOS.

## Features

- Open a local Git repository
- Initialize a repository or clone one from HTTPS/SSH
- Browse local branches and check them out
- Create and safely delete local branches
- Create lightweight tags
- Configure remotes, fetch, pull (fast-forward only), and push
- Review working tree changes and unified diffs
- Stage and unstage files
- Save and restore stashes
- Create commits
- Amend the latest commit
- Browse a commit graph and inspect commit diffs
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

Remote authentication uses Git's configured credential helpers and SSH agent through the system `git` executable. Pull uses fast-forward-only mode; local branch merges are available and can be completed by resolving conflicted files in an editor and committing the resolved changes. Interactive rebase, an in-app conflict editor, partial-line staging, and repository tabs are not implemented yet.
