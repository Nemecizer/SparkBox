#!/bin/sh
# Cargo runner (src-tauri/.cargo/config.toml): signs the SparkBox dev binary with
# your Apple certificate, when there is one, then runs it. Test binaries and
# machines without a certificate just run as they are.
bin="$1"
shift
if [ "$(basename "$bin")" = "sparkbox" ]; then
  identity="$("$(dirname "$0")/sign-identity.sh")"
  if [ -n "$identity" ]; then
    codesign --force --sign "$identity" --identifier com.nemecj.sparkbox "$bin" 2>/dev/null \
      || echo "SparkBox: couldn't sign the dev build with \"$identity\"; Keychain may ask again." >&2
  fi
fi
exec "$bin" "$@"
