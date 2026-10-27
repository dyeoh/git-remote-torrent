# Publish from CI

The repository's GitHub Action publishes every push with no machine left running. It
signs the pushed refs, uploads the packs to a release of the repository, checks that the
release serves them intact, and puts the signed manifest on the DHT. A scheduled run of
the same action keeps the manifest from expiring.

## 1. Give CI a key

The action signs with an unencrypted OpenSSH Ed25519 key kept in a secret. Use a key for
this address alone, not your SSH login key:

```sh
ssh-keygen -t ed25519 -N '' -C 'git-torrent CI' -f ci_ed25519
gh secret set TORRENT_SIGNING_KEY < ci_ed25519
```

Keep `ci_ed25519` somewhere safe offline, then delete the working copy. Its address
(`torrent://<key>`) is printed by the first run. Anyone holding the key can publish at that
address, so treat the secret like a deploy key: only workflows on your default branch and
tags should get it (see [Security](#security)).

To move an address you already publish from a laptop, store that key instead. The first
run adopts your chain.

## 2. Add the workflow

```yaml
name: Torrent
on:
  push:
    branches: [main]
    tags: ["v*"]
  schedule:
    - cron: "23 * * * *" # DHT entries expire after about two hours

# One run at a time: each continues the chain the previous one published.
concurrency:
  group: torrent
  cancel-in-progress: false

jobs:
  publish:
    if: github.event_name != 'schedule'
    runs-on: ubuntu-latest
    permissions:
      contents: write # the torrent-packs release
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0 # packs need the full history
      - run: git update-ref refs/heads/main refs/remotes/origin/main
      - uses: dyeoh/git-remote-torrent@v0.4.0
        with:
          signing-key: ${{ secrets.TORRENT_SIGNING_KEY }}
          refs: refs/heads/main refs/tags

  republish:
    if: github.event_name == 'schedule'
    runs-on: ubuntu-latest
    steps:
      - uses: dyeoh/git-remote-torrent@v0.4.0
        with:
          command: republish
```

Each `publish` run does the following:

1. **Installs the release** named by the action's tag, after checking it against the
   release's `SHA256SUMS` and its build provenance (`gh attestation verify`, signed by
   this repository's release workflow).
2. **Adopts the chain** (`git torrent adopt`) from the DHT, or from the copy of the
   manifest kept in the release, whichever is newer. A runner starts with nothing, and
   the copy still works after the DHT entry has expired. On the first run there is
   nothing to adopt.
3. **Signs** `refs` at their checked-out values (`git torrent sign --update`). Other refs
   keep the values of the previous manifest. The release is the manifest's only mirror.
4. **Uploads** new packs to the `torrent-packs` release. It creates the release if it is
   missing, and never marks it as latest.
5. **Verifies** that the release serves every pack with both infohashes
   (`git torrent verify-mirror`), then **publishes** (`git torrent publish`).
6. **Saves the copy** of the published manifest in the release, for the next run.

`refs` takes patterns, like `git for-each-ref`. Publishing `main` and every tag on each run
means a run cancelled by the concurrency group loses nothing. Deleting a branch or tag
does not remove it from the manifest; sign the deletion from a machine with the key
(`git torrent sign --delete <ref>`).

The `republish` job needs no key and no checkout: it downloads the copy and stores the
newest manifest for its address on the DHT again.

The job summary and the `address` and `seq` outputs give the published address:

```sh
git clone torrent://<key>
```

## Your own mirror

A release asset URL works only for public repositories. To use another host, upload with
your own command. It runs with `GIT_TORRENT_PACKS` set to a directory of `<infohash>.pack`
files and the `manifest` copy, and must upload them keeping their names:

```yaml
      - uses: dyeoh/git-remote-torrent@v0.4.0
        with:
          signing-key: ${{ secrets.TORRENT_SIGNING_KEY }}
          mirror: https://packs.example.com/project
          upload-command: aws s3 sync "$GIT_TORRENT_PACKS" s3://packs-example/project
```

The `republish` job then needs the same `mirror`, to find the copy at `<mirror>/manifest`.

## Inputs

| Input | Default | Meaning |
|---|---|---|
| `command` | `publish` | `publish`, or `republish` on a schedule |
| `signing-key` | | The key, from a secret; `publish` only |
| `name` | | Publish at `torrent://<key>/<name>` |
| `refs` | `github.ref` | Ref patterns to publish |
| `release` | `torrent-packs` | Release that serves the packs and the manifest copy |
| `mirror`, `upload-command` | | Your own mirror instead of the release |
| `version` | the action's tag | Release to install; `source` builds the action's checkout |
| `dht-bootstrap` | | Private DHT nodes, space-separated |

## Security

- **The key is the address.** A workflow that can read the secret can publish any refs
  there. Store it as an environment secret limited to the default branch and tags, and
  never give it to `pull_request` workflows from forks.
- **The binary is checked before it gets the key:** its checksum against the release's
  `SHA256SUMS`, and its provenance against this repository's release workflow. Pin the
  action to a tag you have reviewed, or to a commit SHA.
- **The key touches the disk only during the run**, in `$RUNNER_TEMP` with mode 0600, and
  is deleted when the step ends. On a self-hosted runner, `git torrent key use` also writes
  the path to the runner user's global git config.
- **Mirrors are untrusted.** A tampered release asset fails `verify-mirror` before
  anything is published, and cloners check every pack against the signed manifest anyway.
  The manifest copy is checked against your key before it is adopted or republished.
- **One publisher per address.** If a laptop and CI both publish with the same key and
  name, `publish` refuses whichever falls behind. Publish from CI only, or give each
  machine its own `--name`.
