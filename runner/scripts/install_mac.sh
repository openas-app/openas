#!/bin/bash
# install.sh FOLDER "Name" ICON.png [DEST_DIR]
# Install = copy the prebuilt Runner, give it its own name, id and icon, ad-hoc sign.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
folder="$(cd "$1" && pwd)"; name="$2"; icon="$3"; dest="${4:-$HOME/Applications}"
id="app.openas.$(echo "$name" | tr 'A-Z' 'a-z' | tr -cs 'a-z0-9' '-' | sed 's/-$//')"
app="$dest/$name.app"; P="$app/Contents/Info.plist"
mkdir -p "$dest"; rm -rf "$app"
cp -R "${RUNNER:-$here/out/Runner.app}" "$app"
/usr/libexec/PlistBuddy -c "Set :CFBundleName $name" -c "Set :CFBundleIdentifier $id" \
  -c "Set :OAAppFolder $folder" -c "Add :CFBundleDisplayName string $name" \
  -c "Set :CFBundleExecutable $name" "$P"
# The process carries the app's own name too (Activity Monitor, the Force Quit list).
mv "$app/Contents/MacOS/Runner" "$app/Contents/MacOS/$name"
set_icon() {
  local set; set="$(mktemp -d)/AppIcon.iconset"; mkdir -p "$set"
  for s in 16 32 128 256 512; do
    sips -z $s $s "$icon" --out "$set/icon_${s}x${s}.png" >/dev/null
    sips -z $((s*2)) $((s*2)) "$icon" --out "$set/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$set" -o "$app/Contents/Resources/AppIcon.icns"
}
set_icon
# A copy of a downloaded Runner inherits its quarantine flag; this copy was made
# here, by the user's own action, so it is not a download.
xattr -dr com.apple.quarantine "$app"
codesign --force --sign - "$app"
touch "$app"   # nudge Finder and the Dock to pick up the new icon
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app"
echo "$app ($id)"
