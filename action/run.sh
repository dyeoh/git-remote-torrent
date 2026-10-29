#!/usr/bin/env bash
# The action's `publish` and `republish` commands; see action.yml for inputs.
set -euo pipefail

fail() {
  echo "::error::$*"
  exit 1
}

# bash 3.2 (macOS) treats an empty array as unset, so arrays expand through
# ${array[@]+"${array[@]}"}.
bootstrap=()
for node in $DHT_BOOTSTRAP; do
  bootstrap+=(--dht-bootstrap "$node")
done

work="$RUNNER_TEMP/git-torrent"
rm -rf "$work"
mkdir -p "$work/packs" "$work/copy"
copy="$work/copy/manifest"

if [[ -n $MIRROR ]]; then
  mirror=${MIRROR%/}
  [[ -n $UPLOAD_COMMAND || $COMMAND == republish ]] || fail "\`mirror\` needs an \`upload-command\`"
else
  [[ -n $RELEASE ]] || fail "set \`release\`, or \`mirror\` and \`upload-command\`"
  mirror="$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/releases/download/$RELEASE"
  if gh release list --repo "$GITHUB_REPOSITORY" --limit 1000 --json tagName --jq '.[].tagName' |
    grep -qxF "$RELEASE"; then
    has_release=true
    assets=$(gh release view "$RELEASE" --repo "$GITHUB_REPOSITORY" --json assets --jq '.assets[].name')
  else
    has_release=false
    assets=
  fi
fi

# Downloads the copy of the last published manifest into $copy. Only a
# missing copy is tolerated: starting a new chain because a download failed
# would publish an older sequence number than cloners already have.
fetch_copy() {
  if [[ -n $MIRROR ]]; then
    local status
    status=$(curl --silent --show-error --location --output "$copy" --write-out '%{http_code}' \
      "$mirror/manifest") || fail "cannot download $mirror/manifest"
    case $status in
      200) ;;
      404 | 410) rm -f "$copy" ;;
      *) fail "$mirror/manifest answered HTTP $status" ;;
    esac
  elif grep -qxF manifest <<< "$assets"; then
    gh release download "$RELEASE" --repo "$GITHUB_REPOSITORY" --pattern manifest --dir "$work/copy"
  fi
}

case $COMMAND in
  republish)
    fetch_copy
    [[ -f $copy ]] || fail "no manifest copy at $mirror/manifest; run the action with \`command: publish\` first"
    git torrent republish --manifest "$copy" ${bootstrap[@]+"${bootstrap[@]}"}
    exit 0
    ;;
  publish) ;;
  *) fail "unknown command \`$COMMAND\`; expected publish or republish" ;;
esac

[[ -n $SIGNING_KEY ]] || fail "\`publish\` needs \`signing-key\`"
[[ $(git rev-parse --is-shallow-repository) == false ]] ||
  fail "the checkout is shallow; use actions/checkout with \`fetch-depth: 0\`"

# Patterns as `git for-each-ref` takes them: `refs/heads/main`, `refs/tags`.
set -f
read -r -d '' -a patterns <<< "$REFS" || true
set +f
updates=()
# The release that holds the packs has a tag of its own; it is not a release
# of the project, so it is never published, and removed if it was before.
packs_tag=
if [[ -z $MIRROR ]]; then
  packs_tag="refs/tags/$RELEASE"
  updates+=(--delete "$packs_tag")
fi
while read -r ref oid; do
  [[ $ref == "$packs_tag" ]] && continue
  case $ref in
    refs/heads/* | refs/tags/*) updates+=(--update "$ref=$oid") ;;
    *) fail "$ref is not a branch or tag; run this action on push" ;;
  esac
done < <(git for-each-ref --format='%(refname) %(objectname)' -- ${patterns[@]+"${patterns[@]}"})
[[ " ${updates[*]} " == *" --update "* ]] || fail "no ref in this checkout matches \`refs\` ($REFS)"

key="$work/signing-key"
trap 'rm -f "$key"' EXIT
(
  umask 077
  printf '%s\n' "$SIGNING_KEY" > "$key"
)
git torrent key use "$key"

# Continue the chain from the newest of the DHT's manifest and the copy.
fetch_copy
adopt=(git torrent adopt --allow-new --name "$NAME")
[[ -f $copy ]] && adopt+=(--manifest "$copy")
"${adopt[@]}" ${bootstrap[@]+"${bootstrap[@]}"}

git torrent sign --name "$NAME" --mirror "$mirror" "${updates[@]}" | tee "$work/sign.log"
address=$(sed -n 's/^Address: //p' "$work/sign.log")
seq=$(sed -n -E 's/^(Signed manifest|Manifest) #([0-9]+).*/\2/p' "$work/sign.log")

git torrent export "$work/packs"
cp "$(git rev-parse --git-dir)/torrent/manifest" "$work/manifest"

if [[ -n $MIRROR ]]; then
  cp "$work/manifest" "$work/packs/manifest"
  GIT_TORRENT_PACKS="$work/packs" bash -c "$UPLOAD_COMMAND"
else
  if [[ $has_release == false ]]; then
    gh release create "$RELEASE" --repo "$GITHUB_REPOSITORY" --target "$GITHUB_SHA" \
      --latest=false --title "Torrent packs" \
      --notes "Packs and the signed manifest of \`$address\`, served as a git-remote-torrent mirror."
  fi
  # Packs are named by their content, so ones already uploaded are the same.
  new=()
  for pack in "$work/packs"/*.pack; do
    grep -qxF "$(basename "$pack")" <<< "$assets" || new+=("$pack")
  done
  if [[ ${#new[@]} -gt 0 ]]; then
    gh release upload "$RELEASE" --repo "$GITHUB_REPOSITORY" "${new[@]}"
  fi
fi

git torrent verify-mirror "$mirror"
git torrent publish ${bootstrap[@]+"${bootstrap[@]}"}

if [[ -z $MIRROR ]]; then
  # Only once published, so the copy never runs ahead of the DHT.
  cp "$work/manifest" "$work/copy/manifest"
  gh release upload "$RELEASE" --repo "$GITHUB_REPOSITORY" --clobber "$work/copy/manifest"
fi

{
  echo "address=$address"
  echo "seq=$seq"
} >> "$GITHUB_OUTPUT"
echo "### Published manifest #$seq" >> "$GITHUB_STEP_SUMMARY"
echo "Clone with \`git clone $address\`; packs are mirrored at $mirror." >> "$GITHUB_STEP_SUMMARY"
