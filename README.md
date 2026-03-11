# sak

> *Restic... but in reverse!*

## Scope

From `rustic`, extract what we can to achieve the goal of having an efficient indexer and snapshotter from remote data sources to be pulled to the local, sak-running machine. 

Take advantage of the fact very little of the wheel needs to be re-invented. Rustic is awesome and understands the transport and storage of the data structure.

- figure out how to efficiently index the remote's file tree and store it locally in the same restic/rustic native format

### Rustic vs. Restic

https://rustic.cli.rs/docs/comparison-restic.html
