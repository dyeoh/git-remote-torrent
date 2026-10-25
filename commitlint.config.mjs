export default {
  extends: ["@commitlint/config-conventional"],
  // Bot commits (Dependabot, release-plz) use their own message format.
  ignores: [(message) => /^Signed-off-by: (dependabot|github-actions)\[bot\]/m.test(message)],
  rules: {
    // Bodies often carry URLs and changelog excerpts.
    "body-max-line-length": [0],
  },
};
