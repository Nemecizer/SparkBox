#!/bin/sh
# Builds SparkBox.app, signed with your Apple certificate when there is one
# (see sign-identity.sh), so the Keychain permission carries over to new builds.
cd "$(dirname "$0")/.." || exit 1
# No release with Help unfinished: a picture still to be taken, or one with no dark version, fails here.
VITE_HELP_RELEASE=1 npx vitest run src/help/help.test.ts || exit 1
identity="$(scripts/sign-identity.sh)"
if [ -n "$identity" ]; then
  echo "Signing with: $identity"
  export APPLE_SIGNING_IDENTITY="$identity"
else
  echo "No Apple Development certificate found; building unsigned (Keychain will ask after each update)."
fi
exec npx tauri build --bundles app "$@"
