#!/usr/bin/env bash
# Every installer a release job is about to upload: named for its edition and
# version, and carrying that edition inside. Run by `.github/workflows/release.yml`
# after the bundler and before anything reaches the release, on all three
# platforms (bash 3.2 on macOS, Git Bash on Windows).
#
# The edition inside is what the packaged program itself says: `--version`
# prints `<program> <version>`, with ` (ISP edition)` when the engine it was
# built with registers the subscriber ledger's methods. That is read from the
# assembled engine, not from the name the build was given, so an ISP installer
# without billing — or a free one with it — fails here.
#
# Environment:
#   EDITION     free | isp
#   VERSION     the version the tag names, e.g. 0.1.0
#   ARTIFACTS   tauri-action's `artifactPaths` output (a JSON array of paths)
#   UPLOAD_DIR  where the checked files are put, named as GitHub will show them
#   UI_DIST     the interface that was built in (default: ui/dist; run from
#               the checkout's root)
#
# Exits 1 when any installer is wrong or missing, having said which and why;
# then nothing has been put in UPLOAD_DIR to be uploaded.

set -euo pipefail

case "${EDITION:-}" in
  free)
    product="Nocturne"; binary="nocturne"; package="nocturne"
    identifier="app.nocturne.desktop"; says=""; edition_name="a free"
    ;;
  isp)
    product="Nocturne ISP"; binary="nocturne-isp"; package="nocturne-isp"
    identifier="app.nocturne.desktop.isp"; says=" (ISP edition)"; edition_name="an ISP"
    ;;
  *) echo "EDITION must be free or isp, not '${EDITION:-}'" >&2; exit 2 ;;
esac
: "${VERSION:?the version the tag names}" "${ARTIFACTS:?tauri-action artifactPaths}" "${UPLOAD_DIR:?}"
expected="$binary $VERSION$says"

failed=0
fail() { echo "::error::$*"; failed=1; }
ok() { echo "  ok: $*"; }
# same <label> <what> <found> <wanted>
same() { if [ "$3" = "$4" ]; then ok "$1: $2 $3"; else fail "$1: $2 is '$3', not $4"; fi; }

# The bundler writes the version into the Linux packages in each format's own
# spelling of a pre-release.
rpm_version=$(printf '%s' "$VERSION" | sed 's/-/~/; s/-/./g')

# The interface every installer carries is `ui/dist` as it was when the
# program was compiled; the copy inside is compressed past reading. When that
# folder was missing, `src-tauri/build.rs` left a page there saying the
# interface was not built, and that page compiles and installs as well as the
# real one does.
interface="${UI_DIST:-ui/dist}/index.html"
if [ ! -f "$interface" ]; then
  fail "$interface is missing: there is no interface to have built in"
elif grep -q "was not built into this copy" "$interface"; then
  fail "$interface is the placeholder page: the installers carry no interface"
else
  ok "the interface built in is $interface, not the placeholder page"
fi

work=$(mktemp -d)
staged="$work/staged"
mkdir -p "$staged"

# 7-Zip is on the Windows runner, though not always on Git Bash's PATH.
if ! command -v 7z >/dev/null 2>&1 && [ -d "/c/Program Files/7-Zip" ]; then
  PATH="$PATH:/c/Program Files/7-Zip"
fi

windows_path() { if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s' "$1"; fi; }
unix_path() { if command -v cygpath >/dev/null 2>&1; then cygpath -u "$1"; else printf '%s' "$1"; fi; }

# What the packaged program says it is.
says_its_edition() {
  local label=$1 program=$2 said
  if [ ! -f "$program" ]; then
    fail "$label: there is no $(basename "$program") inside"
    return
  fi
  if ! said=$("$program" --version 2>&1); then
    fail "$label: \`$(basename "$program") --version\` failed: $said"
    return
  fi
  if [ "$said" = "$expected" ]; then
    ok "$label says \`$said\`"
  else
    fail "$label says \`$said\`, and $edition_name installer must say \`$expected\`"
  fi
}

# Package names in a Debian Depends field, one per line, without versions.
names_in() { printf '%s\n' "$1" | sed 's/[,|]/\n/g' | sed 's/^ *//; s/[ (].*//' | sed '/^$/d'; }

# The tray icon is drawn by libayatana-appindicator. Debian 12 and 13 have no
# libappindicator3-1 at all, so a package that asks for it does not install.
appindicator_is_ayatana() {
  local label=$1 wants=$2 refused=$3 list=$4
  if printf '%s\n' "$list" | grep -qxF "$wants"; then
    ok "$label needs $wants"
  else
    fail "$label does not ask for $wants, which draws the tray icon"
  fi
  if printf '%s\n' "$list" | grep -qxF "$refused"; then
    fail "$label asks for $refused, which Debian 12 and 13 do not have"
  fi
}

check_deb() {
  local file=$1 label dir field
  label=$(basename "$file")
  field=$(dpkg-deb -f "$file" Package 2>/dev/null || true)
  same "$label" package "$field" "$package"
  field=$(dpkg-deb -f "$file" Version 2>/dev/null || true)
  same "$label" version "$field" "$VERSION"
  appindicator_is_ayatana "$label" libayatana-appindicator3-1 libappindicator3-1 \
    "$(names_in "$(dpkg-deb -f "$file" Depends 2>/dev/null || true)")"
  dir="$work/deb"
  if ! dpkg-deb -x "$file" "$dir"; then
    fail "$label could not be unpacked"
    return
  fi
  if grep -qx "Exec=$binary" "$dir/usr/share/applications/$product.desktop" 2>/dev/null; then
    ok "$label's launcher starts $binary"
  else
    fail "$label has no launcher \`$product.desktop\` that starts $binary"
  fi
  says_its_edition "$label" "$dir/usr/bin/$binary"
}

check_rpm() {
  local file=$1 label dir field
  label=$(basename "$file")
  field=$(rpm -qp --qf '%{NAME}' "$file" 2>/dev/null || true)
  same "$label" package "$field" "$package"
  field=$(rpm -qp --qf '%{VERSION}' "$file" 2>/dev/null || true)
  same "$label" version "$field" "$rpm_version"
  appindicator_is_ayatana "$label" 'libayatana-appindicator3.so.1()(64bit)' 'libappindicator3.so.1()(64bit)' \
    "$(rpm -qp --requires "$file" 2>/dev/null | sed 's/ *$//' || true)"
  dir="$work/rpm"
  mkdir -p "$dir"
  if ! (cd "$dir" && rpm2cpio "$file" | cpio -idm --quiet); then
    fail "$label could not be unpacked"
    return
  fi
  says_its_edition "$label" "$dir/usr/bin/$binary"
}

check_appimage() {
  local file=$1 label dir
  label=$(basename "$file")
  dir="$work/appimage"
  mkdir -p "$dir"
  cp "$file" "$dir/image"
  chmod +x "$dir/image"
  # The runtime unpacks itself without FUSE, which the runner does not have.
  if (cd "$dir" && ./image --appimage-extract >/dev/null 2>&1); then
    says_its_edition "$label" "$dir/squashfs-root/usr/bin/$binary"
  else
    fail "$label could not be unpacked with --appimage-extract"
  fi
}

check_app() {
  local app=$1 label plist field program archs
  label=$2
  plist="$app/Contents/Info.plist"
  if [ ! -f "$plist" ]; then
    fail "$label has no $(basename "$app")"
    return
  fi
  field=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$plist" 2>/dev/null || true)
  same "$label" identifier "$field" "$identifier"
  field=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$plist" 2>/dev/null || true)
  same "$label" program "$field" "$binary"
  field=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist" 2>/dev/null || true)
  same "$label" version "$field" "$VERSION"
  program="$app/Contents/MacOS/$binary"
  archs=" $(lipo -archs "$program" 2>/dev/null || true) "
  case "$archs" in
    *" x86_64 "*" arm64 "* | *" arm64 "*" x86_64 "*) ok "$label runs on Intel and Apple silicon" ;;
    *) fail "$label is not universal: it holds only${archs% }" ;;
  esac
  # Not a signature anybody vouches for: the ad-hoc one that lets macOS 15 offer
  # "Open Anyway" instead of calling a downloaded copy damaged.
  if codesign --verify --deep --strict "$app" >/dev/null 2>&1; then
    ok "$label carries a valid (ad-hoc) signature"
  else
    fail "$label is not signed, not even ad hoc: macOS reports a downloaded copy as damaged"
  fi
  says_its_edition "$label" "$program"
}

check_dmg() {
  local file=$1 label mount
  label=$(basename "$file")
  mount="$work/dmg"
  mkdir -p "$mount"
  if ! hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mount" "$file" >/dev/null; then
    fail "$label could not be opened"
    return
  fi
  check_app "$mount/$product.app" "$label"
  hdiutil detach "$mount" -quiet || hdiutil detach "$mount" -force -quiet || true
}

# Both Windows installers are unpacked rather than installed, into paths
# without spaces, so that nothing on the runner changes and no quoting can
# go wrong between bash, PowerShell and msiexec.
check_nsis() {
  local file=$1 label dir program
  label=$(basename "$file")
  dir="$work/nsis"
  mkdir -p "$dir"
  cp "$file" "$dir/setup.exe"
  if ! 7z x -y -o"$(windows_path "$dir/out")" "$(windows_path "$dir/setup.exe")" >/dev/null; then
    fail "$label could not be unpacked"
    return
  fi
  program=$(find "$dir/out" -iname "$binary.exe" 2>/dev/null | head -n 1 || true)
  says_its_edition "$label" "${program:-$dir/out/$binary.exe}"
}

check_msi() {
  local file=$1 label dir program
  label=$(basename "$file")
  dir="$work/msi"
  mkdir -p "$dir/out"
  cp "$file" "$dir/package.msi"
  powershell -NoProfile -Command "exit (Start-Process msiexec -ArgumentList '/a $(windows_path "$dir/package.msi") /qn TARGETDIR=$(windows_path "$dir/out")' -Wait -PassThru).ExitCode" \
    || { fail "$label could not be unpacked (msiexec /a)"; return; }
  program=$(find "$dir/out" -iname "$binary.exe" 2>/dev/null | head -n 1 || true)
  says_its_edition "$label" "${program:-$dir/out/$binary.exe}"
}

kinds=""
artifacts=$(node -e 'for (const p of JSON.parse(process.env.ARTIFACTS)) console.log(p)')
while IFS= read -r path; do
  [ -n "$path" ] || continue
  path=$(unix_path "$path")
  name=$(basename "$path")
  echo "$name"
  # The name first: both editions' files share one release, and the only
  # thing telling them apart there is the product name at the front.
  case "$name" in
    "$product.app") ;;
    "${product}_${VERSION}_"* | "${product}-${VERSION}-"*) ok "named for $product $VERSION" ;;
    *) fail "$name is not named for $product $VERSION" ;;
  esac
  upload="$path"
  case "$name" in
    *.deb) kind=deb; check_deb "$path" ;;
    *.rpm) kind=rpm; check_rpm "$path" ;;
    *.AppImage) kind=appimage; check_appimage "$path" ;;
    *.dmg) kind=dmg; check_dmg "$path" ;;
    *.app)
      kind=app
      check_app "$path" "$name"
      # A folder cannot be uploaded; tauri-action packs it beside itself.
      [ -f "$path.tar.gz" ] || tar -czf "$path.tar.gz" -C "$(dirname "$path")" "$name"
      upload="$path.tar.gz"
      ;;
    *-setup.exe) kind=nsis; check_nsis "$path" ;;
    *.msi) kind=msi; check_msi "$path" ;;
    *) kind=""; fail "$name is a kind of file this check does not know, so it is not uploaded unchecked" ;;
  esac
  kinds="$kinds $kind"
  # As GitHub names an asset: a space becomes a dot. Doing it here keeps the
  # name the same on every re-run, so `--clobber` finds the file it replaces.
  cp -R "$upload" "$staged/$(basename "$upload" | tr ' ' '.')"
done <<EOF
$artifacts
EOF

# And nothing that should be there is missing.
case "$(uname -s)" in
  Linux) needed="deb rpm appimage" ;;
  Darwin) needed="dmg app" ;;
  *) needed="nsis msi" ;;
esac
for kind in $needed; do
  case " $kinds " in
    *" $kind "*) ;;
    *) fail "no $kind was built for $product" ;;
  esac
done

if [ "$failed" -ne 0 ]; then
  echo "Nothing is uploaded from this job."
  exit 1
fi
mkdir -p "$UPLOAD_DIR"
cp -R "$staged"/. "$UPLOAD_DIR"/
echo "Checked, and ready to upload:"
ls -1 "$UPLOAD_DIR"
