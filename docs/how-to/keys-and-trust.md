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

## Sign and check

```sh
git torrent sign                 # signs refs/heads/* and refs/tags/*, records HEAD and upstream
git torrent sign --name project  # publish as torrent://<key>/project
git torrent status               # shows the manifest, whether its key is trusted, and differences from your local refs
```

The signed manifest is cached at `.git/torrent/manifest` and is re-verified every time it
is read. Each `sign` with the same key and name increments the sequence number.
Publishing to the swarm arrives in 0.2.
