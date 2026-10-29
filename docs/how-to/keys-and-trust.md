# How to set up keys and trust

Every manifest is signed by a **publisher key**. When refs come from a manifest rather
than the upstream, a cloner accepts the key only if it is in their repository's **trust
store** or pinned for the upstream (see [When trust is checked](#when-trust-is-checked)).

## Choose a signing key

The best option is the SSH key you already use with your forge:

```sh
git torrent key use ~/.ssh/id_ed25519
```

If the key has a passphrase, `git torrent sign` asks for it each time. The key never
leaves its file. Only Ed25519 keys are supported. If you don't have one, create it with
`ssh-keygen -t ed25519`.

You can also generate a dedicated key:

```sh
git torrent key generate --passphrase  # writes ~/.config/git-torrent/publisher_ed25519, mode 0600
```

`--passphrase` encrypts the key with a passphrase you type twice, and `sign` asks for it
each time. Without it, the key is stored **unencrypted**, which suits only a key kept in a
CI secret ([Publish from CI](publish-from-ci.md)). When stdin is not a terminal, the
passphrase is read from its first line. Either way, the key's path is saved in your
global git config as `torrent.signingKey`.

Whoever holds the key controls the address for good: there is no revocation yet. Prefer
a key per address, encrypted, over one key reused everywhere.

```sh
git torrent key show         # your public key and torrent:// address
```

## Trust publishers

The trust store is per repository, at `.git/torrent/trusted-keys`. It holds one OpenSSH
public key line per publisher.

```sh
git torrent trust import https://github.com/alice   # every Ed25519 key alice publishes
git torrent trust add "ssh-ed25519 AAAAC3Nz... bob" # a single key line
git torrent trust add 3b6a27bc...                   # or a 64-digit hex key
git torrent trust add --self                        # your own signing key
git torrent trust list
git torrent trust remove 3b6a27bc...
```

`trust import` fetches `<profile>.keys`, which GitHub, GitLab, Gitea and Forgejo all
serve. It only accepts HTTPS, and it skips RSA and ECDSA keys. Once imported, the keys
stay in your trust store, so manifests can still be verified after the forge account or
the forge itself is gone.

### When trust is checked

While the upstream of a hybrid remote answers, its refs are canonical, and git's object
ids keep anything fetched from the swarm honest, so the publisher's key doesn't matter.
When refs come from a manifest instead (a `torrent://` remote, or a hybrid remote whose
upstream is gone), the key decides what code you get:

1. The helper always prints the key: `note: refs are signed by ssh-ed25519 AAAA…
   (torrent://<hex>)`. Compare it with the keys the publisher lists on their forge.
2. If the repository's trust store has any entries, only those keys are accepted.
3. Otherwise, a hybrid remote accepts the key **pinned** for its upstream. The first
   fetch that succeeds while the upstream answers records the remote's key in
   `.git/torrent/pins`, like SSH's `known_hosts`. If `torrentAddress` later points at
   another key, that key is refused once the upstream is gone.

If a publisher really moves to a new key, `git torrent trust add <new key>` accepts it
and moves the pin. Pinning can't protect a clone made with a wrong `torrentAddress` in the
first place, so check the printed key against the publisher's forge keys, or import them
with `trust import` before you need them.

## Sign, seed and check

```sh
git torrent sign                 # sign your shared branches and tags, with HEAD and the upstream
git torrent sign --all           # every local branch and tag
git torrent sign --name project  # publish as torrent://<key>/project instead
git torrent seed                 # sign, publish to the DHT, and seed the packs until Ctrl-C
git torrent status               # the manifest, whether its key is trusted, and drift from local refs
```

**What gets published.** A published pack can't be recalled, so a plain `sign` publishes
only refs that are already shared:

- the refs in the previous manifest;
- the current branch;
- branches a remote has (`refs/remotes/<remote>/<branch>`);
- tags on any of those branches.

It lists what it publishes and names the local branches and tags it leaves out. `--all`
publishes every local branch and tag. Pushes publish exactly the refs pushed.

**The pack chain.** Each `sign` checks whether your refs reach objects that the previous
manifest's packs don't contain. If they do, it writes one new pack holding just those
objects and appends it to the manifest's `packs` list. Packs are stored at
`.git/torrent/packs/<infohash>/pack`. Signing when nothing has changed keeps the current
manifest and its sequence number, so restarting a seeder doesn't churn the address.

**Seeding.** `git torrent seed` signs and then seeds every pack in the chain. It
publishes the manifest to the Mainline DHT under your key (and `--name`, if given), and
republishes it every 30 minutes while it runs, because DHT items expire after about two
hours. If a newer manifest is already published under the same key and name (for
example, from another machine), it stops rather than overwrite it.

| Option | Use |
|---|---|
| `--listen 0.0.0.0:6881` | Fixed address for peers to connect to; by default a random port |
| `--dht-bootstrap HOST:PORT` | Join a private DHT instead of the public one (repeatable) |
| `--no-peer-dht` | Don't announce packs on the BitTorrent DHT; peers must be given the listen address |
| `--no-local-discovery` | Don't announce packs to the local network by multicast (BEP 14) |
| `--upnp` | Ask your router to forward the listen port, so peers outside your network can connect |
| `--repo PATH` | Serve this repository; repeat to serve several from one process (see [Run as pure P2P](pure-p2p.md)) |
| `--republish ADDRESS` | Also keep another address's manifest published; needs no key |
| `--follow REMOTE` | Seed the packs of a cloned remote's manifest and keep it published, picking up new ones; needs no key (see [Run as pure P2P](pure-p2p.md#help-keep-someone-elses-repository-available)) |
| `--install` | Run this seed command as a service that starts at login (launchd on macOS, systemd on Linux), replacing any installed before |
| `--uninstall` | Stop and remove that service |
| `--print-service` | Print the service file `--install` would write, and change nothing |
| `--mirror URL` | Sign an HTTP mirror of the packs into the manifest (repeatable); see [Mirror packs over HTTP](http-mirrors.md) |

The signed manifest is cached at `.git/torrent/manifest` and is re-verified every time it
is read. Each `sign` or `seed` with the same key and name increments the sequence number.

## Publish on push

Instead of running `git torrent sign` by hand, let `git push` sign for you:

```sh
git config remote.origin.torrentAddress torrent://<your key>   # hybrid remotes only
git config remote.origin.torrentPublish true
git torrent seed                                               # in another terminal
git push origin main
```

Each push signs a new manifest. It holds the previously published refs, plus the ones you
pushed, minus the ones you deleted; your other local branches stay private. The manifest
is signed with `git torrent sign --update <ref>=<oid>` (and `--delete <ref>`), which you
can also run yourself.

A running `git torrent seed` notices the new manifest within two seconds, seeds its new
pack and publishes it, using the previous sequence number as compare-and-swap. If no
seeder is running, the push says so, and the next `git torrent seed` publishes it.

- **Hybrid remotes:** the upstream decides. Only the refs it accepts are signed.
- **Pure `torrent://` remotes:** the manifest is the only copy of the refs, so updates
  that would rewind a published ref are rejected unless you force them (`git push
  --force` or `+ref`).

### When the upstream is down

A push to a hybrid remote that can't reach its upstream goes to the swarm only, if the
remote has `torrentPublish` set. Because the manifest is then the only up-to-date copy,
the same fast-forward rule as for pure remotes applies. The pushed refs are recorded in
`.git/torrent/unsynced`.

Once the upstream is back:

```sh
git torrent reconcile            # or: git torrent reconcile <remote>
```

This pushes each recorded ref's value from the manifest to the upstream, without force.
Refs the upstream accepts are cleared. If someone pushed to the upstream in the meantime,
it rejects the ref, and reconcile leaves it pending: fetch from the upstream, merge, push
again, and rerun `git torrent reconcile`.

## Compact the pack chain

Each push or sign that reaches new objects adds a pack, and every pack id takes 20 of the
manifest's 1000 bytes. When the next pack would not fit, `sign` (and so `git push`)
replaces the whole chain with one full pack by itself. To do it earlier:

```sh
git torrent compact            # one full pack; same refs; next sequence number
git torrent compact --prune    # also delete every stored pack not in the new chain
```

A running `git torrent seed` seeds and publishes the compacted manifest like any other.
Clones that applied the old chain download the full pack once on their next fetch.
Without `--prune`, the old packs stay in `.git/torrent/packs`, so a seeder started before
the compaction can keep serving peers that are part-way through the old chain.

## One machine per address

Publish each address (key and name) from one machine. Its pack chain lives only there,
so another machine can't extend it, even with a copy of the key.

- A push from another repository whose newest manifest wasn't signed there is rejected
  with `fetch first: manifest #N of this address was signed on another machine`.
  Nothing is signed.
- If another machine publishes while `git torrent seed` runs, the seeder's next publish
  loses the compare-and-swap. The seeder then stops with an error instead of fighting
  over the address.

To move publishing to another machine, such as a new laptop or a CI runner, give it the
key and run `git torrent adopt` (with `--name` if you used one) in a **full** clone:

1. It fetches the newest manifest for your key from the DHT.
2. It downloads the pack chain from the manifest's mirrors (`--mirror` adds more), or
   from peers (`--peer`).
3. It checks both infohashes of every pack, and proves that the chain rebuilds every ref
   in a scratch repository.
4. Only then does it save the manifest, so `sign` and `git push` continue the chain.

`--manifest <file>` adopts from a copy of the signed manifest when the DHT has nothing
newer, for example after it expired. `--allow-new` succeeds without adopting when nothing
is published yet; [the GitHub Action](publish-from-ci.md) uses both.

Afterwards, publish only from the new machine. `sign` refuses shallow clones, because
their packs would be missing history. To publish the same code from several machines, give each its own name
(`--name`) or key.
