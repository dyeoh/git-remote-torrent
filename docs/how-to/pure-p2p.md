# Run as pure P2P

A `torrent://<key>[/<name>]` remote needs no server at all. Clone, fetch and push work
peer to peer, and your Ed25519 key, not a host, decides what the latest state is. This
guide sets up a repository that lives only in the swarm. To use the swarm alongside a forge
instead, see [Your first immortal repo](../tutorials/first-immortal-repo.md).

## 1. Publish the repository

```sh
git torrent key generate             # a dedicated key; see "Keys" below
git torrent sign --name project      # prints torrent://<key>/project
git remote add swarm torrent://<key>/project
git config remote.swarm.torrentPublish true
```

From now on, `git push swarm main` signs each push into the manifest. Pushes that would
rewind a published branch need `--force`, as on any git server.

## 2. Keep a peer online

Cloners need **one reachable peer** with your packs, and the manifest has to be stored on
the DHT again at least every two hours. `git torrent seed` does both, and it picks up each
push within two seconds. Run it as a service so it survives logouts and reboots:

```sh
git torrent seed --listen 0.0.0.0:6881 --install
```

`--install` turns the rest of the command into a launchd agent on macOS or a systemd user
service on Linux, which starts at login and restarts if it fails. Installing again replaces
it; `git torrent seed --uninstall` removes it, and `--print-service` shows the file without
installing anything. The seeder signs the current refs when it starts, with no one there
to type a passphrase, so `--install` refuses an encrypted signing key (see [Keys](#keys)).

On macOS, keep the repositories outside `~/Desktop`, `~/Documents` and `~/Downloads`:
launchd agents can't read those folders unless you grant `git-torrent` Full Disk Access.

To write the service file by hand instead, start from
[`packaging/systemd/git-torrent-seed.service`](../../packaging/systemd/git-torrent-seed.service)
or [`packaging/launchd/dev.git-torrent.seed.plist`](../../packaging/launchd/dev.git-torrent.seed.plist).

One process can serve several repositories, and also keep other people's repositories
findable without their keys:

```sh
git torrent seed --listen 0.0.0.0:6881 \
  --repo ~/src/project --repo ~/src/other \
  --republish torrent://<a friend's key>/their-project
```

With several `--repo`, each repository keeps the name it was signed with.

## 3. Let others in

- **Reachability.** Peers on the same network find each other automatically. Peers across
  the internet must reach your listen port. Forward it on your router, or let
  `seed --upnp` ask the router to do it. A fixed `--listen` port makes forwarding simple.
- **More copies.** Anyone can keep your manifest alive with
  `git torrent republish torrent://<key>/project` (no key needed), or with `seed --republish`.
  A [static mirror](http-mirrors.md) serves packs while no peer is online.
- **Cloning:** `git clone torrent://<key>/project`.

## Help keep someone else's repository available

Anyone who cloned a repository can serve it too, without its key:

```sh
git clone torrent://<key>/project && cd project
git torrent seed --follow origin --install
```

The follower seeds the packs of the newest manifest it knows for `origin`, the one it
fetched or a newer one from the DHT, and stores the manifest on the DHT again every 30
minutes. When the publisher signs a new manifest, the follower downloads its new packs,
checks both of their hashes against it, and serves them too. It asks the peers in
`torrent.peer` and the mirrors in `torrent.mirror`, as fetches do, and never signs
anything. `--follow` works with hybrid remotes as well, through their `torrentAddress`.

A follower downloads any packs it lacks when it starts. To skip that, set
`git config torrent.keepPacks true` before cloning or fetching (the clone keeps each pack,
at the cost of roughly twice the disk). After a fetch from the swarm, the helper suggests
`--follow` once per fetch; `git config torrent.seedHint false` hides that.

Like any seeder, a follower tells the DHT that your address holds this repository (see
the [threat model](../explanation/threat-model.md#l2-privacy)).

## What still depends on outside infrastructure

| Dependency | Why | How to reduce it |
|---|---|---|
| DHT bootstrap nodes | A new client starts from mainline's well-known nodes before it knows any peers | `torrent.dhtBootstrap` (cloners) and `seed --dht-bootstrap` point at nodes you run or trust |
| At least one online peer or mirror | Packs exist only where someone holds them | Several seeders, friends running `--republish`, a mirror |
| Manifest republishing | DHT entries expire about two hours after they were last stored | Any running `seed`, `republish` from cron, or the [CI action](publish-from-ci.md) |
| Reachability | NAT blocks incoming connections | Port forwarding, `--upnp`, or a peer with a public address |

## Keys

The always-on seeder signs only when it starts, and only if the refs changed since the last
manifest. Otherwise it publishes the manifests that `git push` or `git torrent sign` signed. To sign pushes unattended on the same machine, though, the
signing key can't have a passphrase. Use a dedicated key for the repository, not your
personal SSH key, so that losing the machine doesn't expose more than this address. Only one
machine can publish an address; to move it, run `git torrent adopt` on the new machine
(see [Set up keys and trust](keys-and-trust.md)).
