# Mirror packs over HTTP

Seeders come and go. A static HTTP mirror keeps your repository cloneable even when no
peer is online and the upstream is gone. A cloner only needs the DHT to find your signed
manifest, and the manifest tells them where the mirror is.

Mirrors need no trust. Every pack is checked against both infohashes in your signed
manifest (SHA-1 and SHA-256) before git sees it, so a mirror that serves other bytes is
ignored. Clients older than 0.4 can't verify packs signed by 0.4 or later from mirrors
(they still fetch them from peers), so mirror users should upgrade.

## 1. Export the pack chain

```sh
git torrent sign                      # or `git torrent seed`
git torrent export ./mirror
```

This copies every pack of the signed manifest's chain into `./mirror` as
`<infohash>.pack`. Packs never change once written, so you can upload the directory
again after each push: existing files stay the same.

## 2. Upload the directory

Use any host that serves files at a stable URL:

- GitHub Pages or another static site host
- an object store bucket with public reads (S3, R2, GCS)
- a forge's generic package registry or release assets
- a plain web server

The files must be reachable as `<base>/<infohash>.pack`.

## 3. Sign the mirror into the manifest

```sh
git torrent seed --mirror https://you.github.io/project-packs
```

`--mirror` can be repeated. Later `sign` and `seed` runs keep the mirrors from the
previous manifest unless you pass `--mirror` again. Each URL is up to 200 bytes and comes
out of the manifest's 1000-byte budget.

## For cloners

Mirrors listed in the manifest are used automatically. A cloner can add mirrors of their
own, for example an internal cache:

```sh
git config torrent.mirror https://cache.internal/packs
```

Peers are tried first. When mirrors are available, the helper waits at most 30 seconds
for peers per pack, then tries each mirror in turn.

## Keep the manifest findable

A mirror keeps the packs available, but cloners find the manifest through the DHT, where
entries expire about two hours after they were last stored. Without a running seeder,
republish on a schedule. This needs no key, so any machine can do it:

```sh
git torrent republish torrent://<key>[/<name>]
```
