# sak

*Rustic... but in reverse!*

Sak uses [rustic-core](https://github.com/rustic-rs/rustic_core) through a small [fork](https://github.com/brege/rustic_core) patched to enable backing up remote source trees into a local Restic-format repository.

## Topology

[Restic](https://github.com/restic/restic) defines the storage engine and repository format, using
chunking, deduplication, encryption, and snapshots for efficient backups. [Rustic](https://github.com/rustic-rs/rustic) builds on that with config-driven execution via TOML manifest, execution sequencing, and better telemetry. **Sak** flips the direction: remote sources are backed up into local repositories by **pulling**, not pushing.

### Restic / Rustic

![restic](docs/img/restic.svg)

### Sak

![sak](docs/img/sak.svg)


## Install

```bash
cargo install --git https://github.com/brege/sak --bin sak
```

## Usage

These commands and config snippets reflect the topology in the above diagram.

### Rustic

The **laptop ⇒ remote/backend** mirroring runs from the source machine and backs up to a remote repository. This is how Rustic works today with no modifications. This config snippet is provided for familiarity.

<details>
<summary><b>Show Laptop's Rustic TOML</b></summary>

### `~/.config/rustic/laptop.toml`

```toml
[repository]
repository = "sftp:user@host:/backup/Laptop"
password-file = "/home/user/.config/rustic/.pass"

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

</details>

### Sak

But what if you have multiple machines whose configs and databases need to be backed up periodically? This is time-hard data that's difficult to reproduce. Headless Linux installs are typically SSH server enabled at genesis, while portable desktop devices like laptops with Linux installs often do not have an SSH *server* enabled. Plus, a laptop is not "always on", so even if you did setup an SSH server on your laptop for your servers to Rustic-backup to, they cannot rely on your laptop being alive to perform the backup.

Sak reuses Restic's backend with Rustic's TOML upgrades, but enables your laptop to pull sources at will to a local repository on its own internal schedules.

#### laptop ⇐ {Unraid, MiniPC}

You can keep one local repository per remote host.

```bash
mkdir -p ~/Backups/MiniPC ~/Backups/Unraid
```

<details>
<summary><b>Show Unraid's sak.toml</b></summary>

### `~/Backups/Unraid/sak.toml`

```toml
[repository]
repository = "/home/user/Backups/Unraid"
password-file = "/home/user/Backups/Unraid/.sak-pass"

[[backup.snapshots]]
sources = [
    "Unraid:/.config",
    "Unraid:/db.sqlite3",
    "Unraid:/etc/nginx",
    "Unraid:/LinuxISOs",
]
globs = [
    "!.cache",
    "!*.tmp",
    "!*.log",
]
```

</details>

<details>
<summary><b>Show MiniPC's sak.toml</b></summary>

### `~/Backups/MiniPC/sak.toml`

```toml
[repository]
repository = "/home/user/Backups/MiniPC"
password-file = "/home/user/Backups/MiniPC/.sak-pass"

[[backup.snapshots]]
sources = [
    "MiniPC:/mnt/user/appdata",
    "MiniPC:/var/lib/plexmediaserver",
    "MiniPC:/boot/config",
    "MiniPC:/docker",
]
globs = [
    "!**/cache",
    "!**/logs",
    "!/var/lib/plexmediaserver/Library/Application Support/Plex Media Server/Cache",
    "!**/*.tmp",
]
```

</details>

Then one backup at a time:

```bash
sak backup --config /home/user/Backups/Unraid/sak.toml
sak backup --config /home/user/Backups/MiniPC/sak.toml
```

### Inspect

List snapshots while in the laptop-local `MiniPC` repo:

```bash
cd ~/Backups/MiniPC
restic --repo . --password-file .sak-pass snapshots
```

List the latest snapshot in the laptop-local `Unraid` repo while you're somewhere else:

```bash
restic --repo ~/Backups/Unraid \
    --password-file ~/Backups/Unraid/.sak-pass \
    ls latest
```

Also, `rustic` can be used interchangeably with `restic` for these checks.

## Resources

These pages have been an enormous source of knowledge for me with respect to Restic and Rustic.

- [Comparison of Rustic vs. Restic](https://rustic.cli.rs/docs/comparison-restic.html): Rustic is not a port of Restic and this page is invaluable in understanding the feature set differences between the two.
- [Rustic Configuration File](https://rustic.cli.rs/docs/commands/init/configuration_file.html)
  is one of Rustic's key feature-adds over Restic, besides being written in Rust instead of Go.
- [Rustic Config TOML Reference](https://github.com/rustic-rs/rustic/blob/main/config/README.md)
  canonical key-by-key TOML reference; where `globs` and `glob-file` are formally defined.
- [Discussion: backup using config and glob](https://github.com/rustic-rs/rustic/discussions/1194)
  has worked examples of the `!` negation pattern with glob files.

### Related Projects

Restic is incredibly efficient, easy to retrieve oops-deleted files, and idiomatic for those who work a lot with Git. Previously, before switching my thinking over to the restic paradigm, I did a lot of my backing up with [rsync](https://rsync.samba.org/). This sync from my remotes was noisy and required building up finer and finer filters so my portable backups were slim.

My related projects may be of some historical value or present need.

1. I made [**ilma**](https://github.com/brege/ilma), an over-engineered Bash script system for multi-machine encrypted backups using monolithic, portable archives (zst, tar, rsync, gpg). There's no de-duplication effort here, but its spiritual predecessor to Sak.

2. Then [**dil**](https://github.com/brege/dil), ported from ilma's now-deprecated codebase pruner, detects and cleans project build litter: `node_modules`, `__pycache__`, LaTeX cruft (`*.aux`, `*.synctex.gz`), and equivalents across many languages and frameworks. Useful to run before a backup to avoid archiving forgettable, reproducible junk.

## License

Licensed under either of:

- [Apache License, Version 2.0](./LICENSE-APACHE)
- [MIT license](./LICENSE-MIT)
