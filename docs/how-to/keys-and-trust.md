# How to set up keys and trust

Every manifest is signed by a **publisher key**. Peers accept a manifest only if
its key is in their repository's **trust store**.

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
git torrent key generate     # writes ~/.config/git-torrent/publisher_ed25519, mode 0600
```

A generated key is stored **unencrypted**. Either way, the key's path is saved in your
global git config as `torrent.signingKey`.

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

## Sign, seed and check

```sh
git torrent sign                 # sign refs/heads/* and refs/tags/*, with HEAD and the upstream
git torrent sign --name project  # publish as torrent://<key>/project instead
git torrent seed                 # sign, publish to the DHT, and seed the packs until Ctrl-C
git torrent status               # the manifest, whether its key is trusted, and drift from local refs
```

**The pack chain.** Each `sign` checks whether your refs reach objects that the previous
manifest's packs don't contain. If they do, it writes one new pack holding just those
objects and appends it to the manifest's `packs` list. Packs are stored at
`.git/torrent/packs/<infohash>/pack`. Signing with no new commits adds no pack.

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

The signed manifest is cached at `.git/torrent/manifest` and is re-verified every time it
is read. Each `sign` or `seed` with the same key and name increments the sequence number.
