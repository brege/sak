# sak

*Restic... but in reverse!*

## About

Restic is designed to backup data on the Restic's runtime machine to a remote repository, typically through SSH using Zstd compression, AES-256 encryption, de-duplicating backups and snapshotting for fast and efficient storage retrieval.  It's an impressive stack.

Sak uses [`rustic_core`](https://github.com/rustic-rs/rustic_core) through a small [fork](https://github.com/brege/rustic_core) patched to enable backing up remote source trees into a local restic repository.

## Topology

![paradigms](img/paradigms.svg)

## Usage

This example uses the device topology of the diagram.

The normal `restic` pattern is a fixed include list and a fixed exclude list.

`includes.txt`

```text
/home/user/Documents
/home/user/Pictures
/home/user/.config
/home/user/.mozilla
```

`excludes.txt`

```text
# picture thumbnails
/home/user/Pictures/**/Thumbs.db

# .mozilla
/home/user/.mozilla/firefox/*/cache2/**
/home/user/.mozilla/firefox/*/thumbnails/**
/home/user/.mozilla/firefox/crashreports/**
```

Then `restic` backs up that local source set into a repository.

```bash
restic --repo /path/to/repo \
    --password-file /path/to/repo/.restic-pass \
    backup \
    --files-from includes.txt \
    --exclude-file excludes.txt
```

`sak` follows the same idea, but the source set lives on the remote machine
and the repository lives on the laptop.

Install `sak` from GitHub.

```bash
cargo install --git https://github.com/brege/sak --bin sak
```

Pull `appdata` from `MiniPC` into the local `Unraid` backup repository.

```bash
sak import \
    --repo ~/Backups/Unraid \
    --source MiniPC:/mnt/user/appdata \
    --as-path appdata \
    --host MiniPC \
    --password-file ~/Backups/Unraid/.sak-pass
```

Pull `music` from `Unraid` into the local `MiniPC` backup repository.

```bash
sak import \
    --repo ~/Backups/MiniPC \
    --source Unraid:/srv/music \
    --as-path music \
    --host Unraid \
    --password-file ~/Backups/MiniPC/.sak-pass
```

List snapshots in the local `Unraid` backup repository.

```bash
restic --repo ~/Backups/Unraid \
    --password-file ~/Backups/Unraid/.sak-pass \
    snapshots
```

Inspect the latest snapshot in the local `MiniPC` backup repository.

```bash
restic --repo ~/Backups/MiniPC \
    --password-file ~/Backups/MiniPC/.sak-pass \
    ls latest
```

`rustic` can be used interchangeably with `restic` for these checks.

## References

- [Restic Source](https://github.com/restic/restic)
- [Rustic Source](https://github.com/rustic-rs/rustic)
- [Rustic Core](https://github.com/rustic-rs/rustic_core)
- [Comparison of Rustic vs. Restic](https://rustic.cli.rs/docs/comparison-restic.html)

## Related Projects

###  [**dil**](https://github.com/brege/dil)

[dil](https://github.com/brege/dil) finds and cleans project litter leftover from coding builds like `node_modules`, `__pycache__`, the LaTeX cruft `*.aux`, `.synctex.gz`, etc, and many other languages and frameworks. 

### [**ilma**](https://github.com/brege/ilma)

[ilma](https://github.com/brege/ilma) is/was an over-engineered Bash script for multi-Linux machine encrypted backups using monolithic archives (zst, tar, rsync, gpg). No de-duplication, but the spiritual pre-cursor to building sak.

## License

Licensed under either of:

- [Apache License, Version 2.0](./LICENSE-APACHE)
- [MIT license](./LICENSE-MIT)
