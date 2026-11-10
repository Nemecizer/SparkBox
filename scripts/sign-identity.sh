#!/bin/sh
# Prints the code-signing identity SparkBox builds use, or nothing.
# Set SPARKBOX_SIGN_IDENTITY to choose one; otherwise the first Apple Development
# (or Developer ID Application) certificate in your keychain is used.
#
# Why: macOS remembers "Always Allow" for SparkBox's Keychain item per signing
# team. Unsigned builds have no team, so every rebuild looks like a new app and
# asks for your login password again. Builds signed with the same Apple
# certificate keep the permission across rebuilds and updates.
if [ -n "$SPARKBOX_SIGN_IDENTITY" ]; then
  echo "$SPARKBOX_SIGN_IDENTITY"
  exit 0
fi
security find-identity -v -p codesigning 2>/dev/null \
  | sed -nE 's/^ *[0-9]+\) [0-9A-F]+ "((Apple Development|Developer ID Application): [^"]*)".*/\1/p' \
  | head -1
