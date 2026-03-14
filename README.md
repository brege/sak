# sak

*Restic... but in reverse!*

## About

Restic is designed to backup data on the Restic's runtime machine to a remote repository, typically through SSH using Zstd compression, AES-256 encryption, de-duplicating backups and snapshotting for fast and efficient storage retrieval.  It's an impressive stack.

Sak works on the back of [`rustic\_core`](https://github.com/rustic-rs/rustic_core) (my minimal [fork](https://github.com/brege/rustic_core)) but in the other direction.

## Restic vs. Sak

![paradigms](img/paradigms.svg)

## Usage

This example uses the device topology of the diagram.

Install `sak` from GitHub.

```bash
cargo install --git https://github.com/brege/sak --bin sak
```

Pull `appdata` from `MiniPC` into the local `Unraid` backup repository.

```bash
sak import \
    --repo ~/Backups/Unraid \
    --source MiniPC:/mnt/user/appdata \
    --snapshot-path appdata \
    --host MiniPC \
    --password-file ~/Backups/Unraid/.sak-pass
```

Pull `music` from `Unraid` into the local `MiniPC` backup repository.

```bash
sak import \
    --repo ~/Backups/MiniPC \
    --source Unraid:/srv/music \
    --snapshot-path music \
    --host Unraid \
    --password-file ~/Backups/MiniPC/.sak-pass
```

List snapshots in the local `Unraid` backup repository.

```bash
restic --repo ~/Backups/Unraid snapshots
```

Inspect the latest snapshot in the local `MiniPC` backup repository.

```bash
restic --repo ~/Backups/MiniPC ls latest
```

`rustic` can be used interchangeably with `restic` for these checks.

## References

- [Restic Source](https://github.com/restic/restic)
- [Rustic Source](https://github.com/rustic-rs/rustic)
- [Rustic Core](https://github.com/rustic-rs/rustic_core)
- [Comparison of Rustic vs. Restic](https://rustic.cli.rs/docs/comparison-restic.html)

## Related

Related projects of mine:

- [**dil**](https://github.com/brege/dil)
  · find and clean project litter like node\_modules, \_\_pycache\_\_, \*.aux, etc.
- [ilma](https://github.com/brege/ilma)
  · multi-Linux machine encrypted snapshot manager in Bash (zst, tar, rsync, gpg)

## License

Licensed under either of:

- [Apache License, Version 2.0](./LICENSE-APACHE)
- [MIT license](./LICENSE-MIT)
