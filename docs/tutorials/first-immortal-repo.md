# Tutorial: your first immortal repo

In this tutorial you publish a repository to the swarm, delete its upstream, and clone it
anyway. It takes about ten minutes on one machine, over the real public Mainline DHT.

The "hosted" upstream is a bare repository on your disk, so you can delete it without
consequences. Nothing about the flow changes with GitHub or any other host; step 6
shows what to swap in.

## 1. Install

```sh
cargo install --git https://github.com/dyeoh/git-remote-torrent grt-helper grt-cli
```

This installs `git-remote-torrent`, which git runs for `torrent::` and `torrent://`
URLs, and `git-torrent`, which git runs as `git torrent`.

## 2. Create a project and its upstream

```sh
mkdir immortal-demo && cd immortal-demo
git init -q --bare upstream.git          # stands in for the hosted repository
git init -q project && cd project
echo "hello, swarm" > README
git add README && git commit -qm "first"
git remote add origin "torrent::file://$PWD/../upstream.git"
git push -q origin main
```

The `torrent::` prefix makes the remote go through the helper. While the upstream is
up, pushes and fetches go to it exactly as before.

## 3. Get a publisher key

```sh
git torrent key generate
```

```text
Public key: ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHDl… git-torrent publisher
Address:    torrent://70e55308d2ff296f0da9924440d36509ec230ca1bcb48929703d6f76257b3acb
```

The address is your repository's permanent name in the swarm. It stays the same across
pushes. If you already sign commits or log in to a forge with an Ed25519 SSH key,
`git torrent key use ~/.ssh/id_ed25519` uses that instead. See
[Set up keys and trust](../how-to/keys-and-trust.md).

## 4. Seed

```sh
git torrent seed
```

```text
Added pack c7831e48… (3 objects) to the chain.
Signed manifest #1 with 1 ref(s) and 1 pack(s), HEAD -> refs/heads/main.
Address: torrent://70e55308…
Seeding 1 pack(s) on 0.0.0.0:60490.
Published manifest #1 to the DHT. Press Ctrl-C to stop.
```

Your refs are now signed and published to the DHT, and your objects are packed and
seeded. Leave this running and open a second terminal for the rest.

## 5. Pull the kill switch

In the second terminal, from `immortal-demo/`:

```sh
mv upstream.git taken-down.git
```

The upstream is gone, as if the host deleted the repository, suspended the account, or
went offline.

## 6. Clone it anyway

Clone through the upstream URL, and tell git which swarm address backs it:

```sh
git clone -c remote.origin.torrentAddress=torrent://<your key> \
  "torrent::file://$PWD/upstream.git" rescued
```

```text
Cloning into 'rescued'...
warning: the upstream is unavailable (`git ls-remote` failed (exit status: 128))
warning: serving manifest #1 from the swarm
Fetched 1 pack(s) of manifest #1 from the swarm.
```

```sh
cat rescued/README     # hello, swarm
```

Or skip the upstream entirely and clone by address:

```sh
git clone torrent://<your key> p2p
```

With a real host, the first form is
`git clone -c remote.origin.torrentAddress=torrent://<key> torrent::https://github.com/you/project.git`.
While the host is up, refs come from it and objects come from the swarm when the
manifest matches. When it's gone, the swarm serves both.

## What just happened

1. `git torrent seed` packed your objects, signed a manifest of your refs with your key,
   and stored it on the DHT under that key ([BEP 44](https://www.bittorrent.org/beps/bep_0044.html)).
2. The clone asked the upstream for refs. That failed, so the helper looked up your key
   on the DHT and verified the manifest's signature.
3. The helper downloaded the packs the manifest lists by infohash from the peers it
   found (here, the seeder in your other terminal), checked them, and indexed them with
   `git index-pack --strict`.

A peer can't forge refs without your key, and can't alter objects without changing
their infohash.

## Keeping it alive for real

- **Reachable seeders.** Peers on the same machine or network find each other
  automatically. Peers across the internet need to reach your listen port, and
  `git torrent seed --upnp` asks your router to forward it.
- **A mirror for when no peer is online.** [Mirror packs over HTTP](../how-to/http-mirrors.md)
  puts the packs on any static host and signs its URL into the manifest.
- **The manifest has to stay published.** DHT entries expire about two hours after the
  last publish, and `seed` republishes every 30 minutes while it runs. When nothing is
  seeding, anyone can keep the repository findable without its key:

  ```sh
  git torrent republish torrent://<key>     # e.g. hourly from cron or a CI schedule
  ```

  This stores the same signed manifest again, after checking its signature. Once a
  manifest has expired everywhere, only its publisher can publish it again. Existing
  clones keep their last verified copy and can still fetch from peers and mirrors.
- **Not there yet:** helping seed someone else's repository's *packs* (running `seed` in
  a clone publishes under *your* key, as a different repository). The background daemon
  in the [roadmap](../explanation/roadmap.md) covers that.
