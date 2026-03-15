# sak

*Rustic... but in reverse!*

## About

Rustic (and Restic) is designed to backup data on the local machine to a remote repository, typically through SSH, using Zstd compression, AES-256 encryption, de-duplicating backups and snapshotting for fast and efficient storage retrieval. It's an impressive stack.

Sak uses [`rustic_core`](https://github.com/rustic-rs/rustic_core) through a small [fork](https://github.com/brege/rustic_core) patched to enable backing up remote source trees into a local restic repository.

## Topology

![paradigms](img/paradigms.svg)

## Install

Install `sak` from GitHub.

```bash
cargo install --git https://github.com/brege/sak --bin sak
```

## Usage

### Rustic Pattern

This example uses the device topology of the diagram above. We are backing up two machines, Unraid and MiniPC, and the Laptop is the destination for the backups from which you run Sak.

Rustic keeps sources and filter patterns in TOML: 

- `sources` is a list of paths to back up
- `globs` is a separate list of filter patterns applied during traversal

Backing up the Laptop's data to some remote using a config file, `~/.config/rustic/laptop.toml`:

```toml
[repository]
repository = "sftp:user@host:/backup/Laptop"
password-file = "~/.config/rustic/.pass"

[[backup.snapshots]]
sources = [
    "/home/user/Documents",
    "/home/user/Pictures",
    "/home/user/.config",
    "/home/user/.mozilla",
]
globs = [
    "!**/.cache",
    "!**/.local/share/Trash",
    "!**/node_modules",
    "!**/__pycache__",
    "!**/*.pyc",
    "!**/.mozilla/firefox/*/Cache",
]
```

Lines in `globs` without a `!` prefix are explicit includes; lines with `!` are excludes. Once a directory is excluded, files inside it cannot be re-included.

### Sak Pattern

`sak` uses the same TOML shape, but the source lives on a remote host and the repository lives on the laptop.

Keep one local repository per remote host.

```bash
mkdir --parents ~/Backups/MiniPC ~/Backups/Unraid
```

Keep the source set beside each local repository in the same shape.

1. `~/Backups/Unraid/sak.toml`

```toml
[repository]
repository = "~/Backups/Unraid"
password-file = "~/Backups/Unraid/.sak-pass"

[[backup.snapshots]]
sources = [
    "Unraid:/.config",
    "Unraid:/db.sqlite3",
    "Unraid:/etc/nginx",
    "Unraid:/LinuxISOs",
]
globs = [
    "!**/.cache",
    "!**/*.tmp",
    "!**/*.log",
]
```

2. `~/Backups/MiniPC/sak.toml`

```toml
[repository]
repository = "~/Backups/MiniPC"
password-file = "~/Backups/MiniPC/.sak-pass"

[[backup.snapshots]]
sources = [
    "MiniPC:/mnt/user/appdata",
    "MiniPC:/var/lib/plexmediaserver",
    "MiniPC:/boot/config",
    "MiniPC:/docker",
]
globs = [
    "!/mnt/user/appdata/**/cache",
    "!/mnt/user/appdata/**/Cache",
    "!/mnt/user/appdata/**/logs",
    "!/var/lib/plexmediaserver/Library/Application Support/Plex Media Server/Cache",
    "!**/*.tmp",
]
```

The host prefix on each source path (`MiniPC:`, `Unraid:`) is the only
structural difference from a standard rustic config. The `globs` key behaves
identically in both tools.

### Inspect

List snapshots in the laptop-local `MiniPC` repository.

```bash
restic --repo ~/Backups/MiniPC \
    --password-file ~/Backups/MiniPC/.sak-pass \
    snapshots
```

Inspect the latest snapshot in the laptop-local `Unraid` repository.

```bash
restic --repo ~/Backups/Unraid \
    --password-file ~/Backups/Unraid/.sak-pass \
    ls latest
```

`rustic` can be used interchangeably with `restic` for these checks.

## References

### Repositories

- [Restic](https://github.com/restic/restic)
- [Rustic](https://github.com/rustic-rs/rustic)
- [Rustic Core](https://github.com/rustic-rs/rustic_core)
- [Fork of Rustic Core used by Sak](https://github.com/brege/rustic_core)

Rustic and Restic write the exact same repository format and can be used interchangeably against the same repo. Do not run `prune` from both tools simultaneously; read-only commands like `snapshots` and `ls` are fine from either.

### Documentation

- [Comparison of Rustic vs. Restic](https://rustic.cli.rs/docs/comparison-restic.html): Rustic is not a port of Restic and this page is invaluable in understanding the feature set differences betwee the two.
- [Rustic Configuration File](https://rustic.cli.rs/docs/commands/init/configuration_file.html)
  covers `[[backup.snapshots]]`, `glob-file`, and profile usage.
- [Rustic Config TOML Reference](https://github.com/rustic-rs/rustic/blob/main/config/README.md)
  canonical key-by-key TOML reference; where `globs` and `glob-file` are formally defined.
- [Discussion: backup using config and glob](https://github.com/rustic-rs/rustic/discussions/1194)
  has worked examples of the `!` negation pattern with glob files.

## Related Projects

### [dil](https://github.com/brege/dil)

Finds and cleans build litter: `node_modules`, `__pycache__`, LaTeX cruft (`*.aux`, `*.synctex.gz`), and equivalents across many languages and frameworks. Useful to run before a backup to avoid archiving garbage.

### [ilma](https://github.com/brege/ilma)

An over-engineered Bash script for multi-machine encrypted backups using monolithic archives (zst, tar, rsync, gpg). No de-duplication, but the spiritual predecessor to Sak.

## License

Licensed under either of:

- [Apache License, Version 2.0](./LICENSE-APACHE)
- [MIT license](./LICENSE-MIT)
